//! Preview frame transport + control: a `tauri::ipc::Channel` streaming raw RGBA
//! frames from Rust to the webview `<canvas>` (Architecture §E/§F, Decision Log
//! #15), plus the commands that drive what gets rendered.
//!
//! The transport ([`pump_frames`]) is decoupled from the producer behind
//! [`preview_engine::FrameSource`]. Phase 0 drove it with a dummy gradient;
//! Phase 1 spawns a render thread owning a [`preview_engine::RenderSource`] (the
//! offscreen wgpu renderer) and feeds it live [`RenderCommand`]s over an mpsc
//! channel. The Tauri commands [`load_source`], [`load_shader`], and
//! [`set_viewport`] do the file IO / slang compile here, then hand the decoded
//! image / compiled shader to the render thread — so they return quickly and
//! frames keep flowing asynchronously over the channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use preview_engine::{
    FrameSource, PositionUpdate, RenderCommand, RenderSource, SourceSpec, TestPattern,
};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, State};

/// Default preview resolution. Small and fixed: transfer cost is bounded by the
/// pane size, not the simulated viewport (Architecture §F).
const DEFAULT_PREVIEW_WIDTH: u32 = 512;
const DEFAULT_PREVIEW_HEIGHT: u32 = 384;
/// Frame period for ~60 fps.
const FRAME_PERIOD: Duration = Duration::from_micros(16_667);

/// The `source-position` event payload (#31): the pump's current frame index and
/// the sequence length, forwarded to the webview whenever the source position
/// changes (play advance / step / seek / load). A still image or a static pattern
/// reports `len == 1`.
#[derive(Clone, Copy, serde::Serialize)]
pub struct PositionPayload {
    /// The current frame index in `0..len`.
    pub index: usize,
    /// The number of frames in the pump (`>= 1`).
    pub len: usize,
}

/// The Tauri event name carrying [`PositionPayload`] (#31).
const SOURCE_POSITION_EVENT: &str = "source-position";

/// The single active preview stream: a caller-supplied id, its stop flag, the
/// command channel to its render thread, and the current pane size (so a
/// `load_source` with no path can size the built-in test pattern to the pane).
struct ActiveStream {
    id: String,
    running: Arc<AtomicBool>,
    commands: Sender<RenderCommand>,
    viewport: (u32, u32),
}

/// Managed state holding the single active preview stream, if any.
#[derive(Default)]
pub struct PreviewState {
    active: Mutex<Option<ActiveStream>>,
}

impl PreviewState {
    /// Stop whatever stream is active (used when a new one starts).
    fn stop_any(&self) {
        if let Some(stream) = self.active.lock().unwrap().take() {
            stream.running.store(false, Ordering::Relaxed);
        }
    }

    /// Stop the active stream only if its id matches `id`. A stale `stop` for a
    /// superseded stream is then a no-op — which is what makes start/stop robust
    /// to out-of-order IPC (e.g. React StrictMode's mount→unmount→mount, where
    /// the first unmount's stop can arrive after the second mount's start).
    fn stop_matching(&self, id: &str) {
        let mut guard = self.active.lock().unwrap();
        if guard.as_ref().is_some_and(|s| s.id == id) {
            guard
                .take()
                .unwrap()
                .running
                .store(false, Ordering::Relaxed);
        }
    }

    /// Send a render command to the active stream's render thread.
    fn send(&self, command: RenderCommand) -> Result<(), String> {
        let guard = self.active.lock().unwrap();
        let stream = guard.as_ref().ok_or("no active preview stream")?;
        stream
            .commands
            .send(command)
            .map_err(|_| "preview render thread is gone".to_string())
    }

    /// The active stream's current pane size, if any.
    fn viewport(&self) -> Option<(u32, u32)> {
        self.active.lock().unwrap().as_ref().map(|s| s.viewport)
    }

    /// Record a new pane size on the active stream (so later `load_source`
    /// defaults track it).
    fn set_viewport_size(&self, width: u32, height: u32) {
        if let Some(stream) = self.active.lock().unwrap().as_mut() {
            stream.viewport = (width, height);
        }
    }
}

/// Start streaming preview frames over `channel` at ~60 fps. Any previously
/// running stream is stopped first, so at most one producer runs at a time.
/// `stream_id` correlates this stream with its later `stop_preview_stream` call.
///
/// The render thread is spawned immediately and returns; the wgpu device is
/// created on that thread (not here), so this command never blocks the UI. Until
/// a `load_source` + `load_shader` arrive, the stream emits a solid "waiting"
/// frame. If no wgpu adapter is available the thread logs and exits — no frames,
/// but the app stays up.
///
/// Frames are sent as raw binary ([`InvokeResponseBody::Raw`]), not JSON; the
/// frontend parses the documented header and blits to a `<canvas>`.
#[tauri::command]
pub fn start_preview_stream(
    app: AppHandle,
    state: State<'_, PreviewState>,
    channel: Channel<InvokeResponseBody>,
    stream_id: String,
    width: Option<u32>,
    height: Option<u32>,
) {
    state.stop_any();

    let width = width.unwrap_or(DEFAULT_PREVIEW_WIDTH).max(1);
    let height = height.unwrap_or(DEFAULT_PREVIEW_HEIGHT).max(1);

    let running = Arc::new(AtomicBool::new(true));
    let (commands, rx) = mpsc::channel();
    *state.active.lock().unwrap() = Some(ActiveStream {
        id: stream_id,
        running: running.clone(),
        commands,
        viewport: (width, height),
    });

    // Source-position event forwarding (#31): the render thread does NOT hold the
    // `AppHandle`, so it reports `PositionUpdate`s on a channel and a small
    // forwarder thread (which does hold the handle) emits the `source-position`
    // event. The render loop is never blocked on emission. The forwarder exits
    // when the render thread drops its sender (stream stopped).
    let (pos_tx, pos_rx) = mpsc::channel::<PositionUpdate>();
    let forward_running = running.clone();
    std::thread::spawn(move || {
        for update in pos_rx {
            // A dropped/closed webview makes emit fail — keep forwarding; the loop
            // ends when the render thread's sender is dropped.
            let _ = app.emit(
                SOURCE_POSITION_EVENT,
                PositionPayload {
                    index: update.index,
                    len: update.len,
                },
            );
        }
        forward_running.store(false, Ordering::Relaxed);
    });

    std::thread::spawn(move || {
        match RenderSource::with_position_sink(width, height, rx, Some(pos_tx)) {
            Ok(source) => pump_frames(&channel, source, &running, FRAME_PERIOD),
            Err(err) => {
                eprintln!("preview: renderer unavailable, no frames will stream: {err}");
                running.store(false, Ordering::Relaxed);
            }
        }
    });
}

/// Load the preview **source image** as a still-image pump (#31). With a path,
/// decodes that file; with `None`, uses the built-in checkerboard test pattern
/// sized to the pane. The decoded image is handed to the render thread as a
/// 1-frame [`SourceSpec`]; rendering continues asynchronously. File IO happens
/// here (in the command), off the render loop.
#[tauri::command]
pub fn load_source(
    state: State<'_, PreviewState>,
    source_path: Option<String>,
) -> Result<(), String> {
    let frame = match source_path {
        Some(path) => source::load_image(&path).map_err(|e| e.to_string())?,
        None => {
            let (width, height) = state.viewport().ok_or("no active preview stream")?;
            source::test_pattern(width, height)
        }
    };
    state.send(RenderCommand::LoadSourcePump(SourceSpec::StillImage(frame)))
}

/// Load a **built-in test pattern** as the source pump (#31). `pattern` is one of
/// `smpte_bars` / `checkerboard` / `gradient` / `motion_sweep`; it is rendered at
/// the current pane size. `motion_sweep` is animated (a multi-frame pump that
/// `play`/`step`/`seek` drive); the others are static 1-frame pumps.
#[tauri::command]
pub fn load_test_pattern(state: State<'_, PreviewState>, pattern: String) -> Result<(), String> {
    let pattern = parse_test_pattern(&pattern)?;
    let (width, height) = state.viewport().ok_or("no active preview stream")?;
    state.send(RenderCommand::LoadSourcePump(SourceSpec::TestPattern {
        pattern,
        width,
        height,
    }))
}

/// Load a **PNG sequence** from a numbered directory as the source pump (#31).
/// The directory is enumerated and every numbered PNG is **decoded here, in the
/// command** (off the render loop), then the decoded `Vec<Frame>` is shipped to
/// the render thread — so the render loop never touches the disk. The sequence
/// starts paused at frame 0; drive it with `play`/`pause`/`step`/`seek`/`set_fps`.
#[tauri::command]
pub fn load_source_sequence(state: State<'_, PreviewState>, dir: String) -> Result<(), String> {
    // Decode the whole sequence up front (acceptable for v1, see PngSequencePump),
    // then ship the decoded `Vec<Frame>` over IPC so the render thread never
    // touches the disk.
    let frames = source::PngSequencePump::load(std::path::Path::new(&dir))
        .map_err(|e| e.to_string())?
        .into_frames();
    state.send(RenderCommand::LoadSourcePump(SourceSpec::PngSequence(
        frames,
    )))
}

/// Parse the JS-facing test-pattern name into the engine enum (#31).
fn parse_test_pattern(name: &str) -> Result<TestPattern, String> {
    match name {
        "smpte_bars" | "smpte" => Ok(TestPattern::SmpteBars),
        "checkerboard" | "checker" => Ok(TestPattern::Checkerboard),
        "gradient" => Ok(TestPattern::Gradient),
        "motion_sweep" | "motion" => Ok(TestPattern::MotionSweep),
        other => Err(format!("unknown test pattern {other:?}")),
    }
}

/// Start advancing the source pump at its fps (#31).
#[tauri::command]
pub fn play(state: State<'_, PreviewState>) -> Result<(), String> {
    state.send(RenderCommand::Play)
}

/// Pause the source pump, holding the current frame (#31).
#[tauri::command]
pub fn pause(state: State<'_, PreviewState>) -> Result<(), String> {
    state.send(RenderCommand::Pause)
}

/// Advance the source pump exactly one frame, even when paused (#31).
#[tauri::command]
pub fn step(state: State<'_, PreviewState>) -> Result<(), String> {
    state.send(RenderCommand::Step)
}

/// Seek the source pump to a frame index (#31): jumps + resets history & feedback.
#[tauri::command]
pub fn seek(state: State<'_, PreviewState>, index: usize) -> Result<(), String> {
    state.send(RenderCommand::Seek(index))
}

/// Set the source pump's advance rate in frames per second (#31).
#[tauri::command]
pub fn set_fps(state: State<'_, PreviewState>, fps: f32) -> Result<(), String> {
    state.send(RenderCommand::SetFps(fps))
}

/// Load the preview **shader**. With a path, reads and compiles that `.slang`
/// file (resolving `#include`s from its directory); with `None`, compiles the
/// built-in passthrough ([`preview_engine::DEFAULT_SHADER`]). A compile error is
/// returned to the caller and the previous shader keeps running.
#[tauri::command]
pub fn load_shader(
    state: State<'_, PreviewState>,
    shader_path: Option<String>,
) -> Result<(), String> {
    let (source, base_dir) = match shader_path {
        Some(path) => {
            let loaded = preset_io::load_slang_file(&path).map_err(|e| e.to_string())?;
            (loaded.source, loaded.base_dir)
        }
        None => (preview_engine::DEFAULT_SHADER.to_string(), None),
    };
    let compiled =
        slang_compile::compile_slang(&source, base_dir.as_deref()).map_err(|e| e.to_string())?;
    state.send(RenderCommand::SetShader(compiled))
}

/// Load a multi-pass **`.slangp` preset** (#22). Parses the preset, compiles
/// each pass's `.slang` (resolving `#include`s from its directory), maps the
/// parsed scale config to the engine's, and sends the chain to the render
/// thread. A parse/compile error is returned to the caller and the previous
/// chain keeps running.
#[tauri::command]
pub fn load_preset(state: State<'_, PreviewState>, preset_path: String) -> Result<(), String> {
    let preset = preset_io::parse_slangp(&preset_path).map_err(|e| e.to_string())?;
    let passes = compile_preset_chain(&preset)?;
    // Decode the preset's LUTs (#27) and replace the engine's set (always sent, so
    // switching to a preset with fewer/no LUTs clears stale ones). Decoded here so
    // file IO stays off the render loop.
    let luts = compile_preset_luts(&preset)?;
    state.send(RenderCommand::SetLuts(luts))?;
    state.send(RenderCommand::SetChain(passes))?;
    // Apply the preset's `parameter_overrides` (§8) AFTER the chain so they land
    // on the freshly-collected `#pragma parameter` defaults (#29). Skipped when
    // the preset declares none.
    if !preset.parameter_overrides.is_empty() {
        state.send(RenderCommand::ApplyParameterOverrides(
            preset.parameter_overrides,
        ))?;
    }
    Ok(())
}

/// Compile a single **in-memory** slang source string and set it as the live
/// shader (#54). This is the live-preview twin of [`load_shader`]: where that
/// reads a `.slang` file then compiles, this takes the source the editor just
/// generated (via `compile_graph`) directly — closing the edit → compile →
/// preview loop for a SINGLE-PASS graph project without writing to disk. A
/// compile error is returned to the caller and the previous shader keeps running.
#[tauri::command]
pub fn load_shader_source(state: State<'_, PreviewState>, source: String) -> Result<(), String> {
    // No base dir: generated graph source is self-contained (no `#include`s).
    let compiled = slang_compile::compile_slang(&source, None).map_err(|e| e.to_string())?;
    state.send(RenderCommand::SetShader(compiled))
}

/// One pass of an in-memory live-preview chain (#54): the generated slang `source`
/// plus the editor-owned [`core_model::PassSettings`] that size/format it. Mirrors
/// a `.slangp` `shaderN` line + its scale/format keys, but with the source carried
/// inline instead of as a file path.
#[derive(serde::Deserialize)]
pub struct ChainPassInput {
    /// The pass's generated (or verbatim whole-pass) slang source.
    source: String,
    /// The pass's RetroArch render settings (scale/filter/wrap/format/alias).
    settings: core_model::PassSettings,
}

/// Build a multi-pass live-preview **chain** from in-memory generated sources
/// (#54). This is the live-preview twin of [`load_preset`]: where that parses a
/// `.slangp`, loads each `shaderN` file, and compiles it, this takes the per-pass
/// generated slang the editor just produced (a graph pass via `compile_graph`, or
/// a whole-pass pass verbatim) plus its editor [`core_model::PassSettings`], and
/// builds the engine chain — reusing the SAME scale/format/wrap mapping helpers as
/// the file path. A compile error is returned to the caller and the previous chain
/// keeps running.
#[tauri::command]
pub fn load_chain_sources(
    state: State<'_, PreviewState>,
    passes: Vec<ChainPassInput>,
) -> Result<(), String> {
    let mut chain = Vec::with_capacity(passes.len());
    for p in &passes {
        let compiled = slang_compile::compile_slang(&p.source, None).map_err(|e| e.to_string())?;
        chain.push(pass_from_settings(compiled, &p.settings));
    }
    state.send(RenderCommand::SetChain(chain))
}

/// Build one engine [`preview_engine::Pass`] from a compiled shader + the editor's
/// [`core_model::PassSettings`] (#54). Reuses the same per-axis scale resolution
/// and wrap-mode mapping as the `.slangp` path ([`compile_preset_chain`]); a
/// `None` settings key leaves the engine default (so the §2/§3 position defaults
/// apply).
fn pass_from_settings(
    compiled: slang_compile::CompiledShader,
    settings: &core_model::PassSettings,
) -> preview_engine::Pass {
    let mut pass = preview_engine::Pass::new(compiled);
    if let Some(scale) = model_scale_config(settings) {
        pass = pass.with_scale(scale);
    }
    pass.alias = settings.alias.clone();
    if let Some(v) = settings.srgb_framebuffer {
        pass.srgb_framebuffer = v;
    }
    if let Some(v) = settings.float_framebuffer {
        pass.float_framebuffer = v;
    }
    if let Some(v) = settings.filter_linear {
        pass.filter_linear = v;
    }
    if let Some(v) = settings.wrap_mode {
        pass.wrap_mode = map_model_wrap_mode(v);
    }
    if let Some(v) = settings.mipmap_input {
        pass.mipmap_input = v;
    }
    if let Some(v) = settings.frame_count_mod {
        pass.frame_count_mod = v;
    }
    pass
}

/// Map the editor's [`core_model::PassSettings`] per-axis scale to the engine's
/// [`preview_engine::ScaleConfig`], or `None` when the pass declares no scale keys
/// (engine applies the §2 default). Mirrors [`scale_config`] for the file path.
fn model_scale_config(settings: &core_model::PassSettings) -> Option<preview_engine::ScaleConfig> {
    let x = &settings.scale_x;
    let y = &settings.scale_y;
    if x.scale_type.is_none() && x.scale.is_none() && y.scale_type.is_none() && y.scale.is_none() {
        return None;
    }
    Some(preview_engine::ScaleConfig {
        x: model_axis_scale(x),
        y: model_axis_scale(y),
    })
}

/// Build one engine axis-scale from an editor [`core_model::ScaleAxis`]. A missing
/// type or factor defaults the missing half (factor 1.0, or type source) exactly
/// as [`axis_scale`] does for the parsed preset.
fn model_axis_scale(axis: &core_model::ScaleAxis) -> preview_engine::AxisScale {
    match (axis.scale_type, axis.scale) {
        (Some(ty), Some(factor)) => preview_engine::AxisScale {
            ty: map_model_scale_type(ty),
            factor,
        },
        (Some(ty), None) => preview_engine::AxisScale {
            ty: map_model_scale_type(ty),
            factor: 1.0,
        },
        (None, Some(factor)) => preview_engine::AxisScale {
            ty: preview_engine::ScaleType::Source,
            factor,
        },
        (None, None) => preview_engine::AxisScale::SOURCE_1X,
    }
}

/// Convert the editor model's scale-type enum to the engine's.
fn map_model_scale_type(ty: core_model::ScaleType) -> preview_engine::ScaleType {
    match ty {
        core_model::ScaleType::Source => preview_engine::ScaleType::Source,
        core_model::ScaleType::Viewport => preview_engine::ScaleType::Viewport,
        core_model::ScaleType::Absolute => preview_engine::ScaleType::Absolute,
    }
}

/// Convert the editor model's wrap-mode enum to the engine's (#54).
fn map_model_wrap_mode(wrap: core_model::WrapMode) -> preview_engine::WrapMode {
    match wrap {
        core_model::WrapMode::ClampToBorder => preview_engine::WrapMode::ClampToBorder,
        core_model::WrapMode::ClampToEdge => preview_engine::WrapMode::ClampToEdge,
        core_model::WrapMode::Repeat => preview_engine::WrapMode::Repeat,
        core_model::WrapMode::MirroredRepeat => preview_engine::WrapMode::MirroredRepeat,
    }
}

/// Decode a parsed preset's LUTs (#27, §7) into engine [`preview_engine::LutSpec`]s:
/// load each PNG (paths already resolved relative to the preset dir by the parser)
/// and carry its per-LUT sampler settings, defaulting an absent key to the §7 LUT
/// defaults (nearest filter, `clamp_to_border` wrap, no mipmap).
fn compile_preset_luts(preset: &preset_io::Preset) -> Result<Vec<preview_engine::LutSpec>, String> {
    let mut luts = Vec::with_capacity(preset.luts.len());
    for entry in &preset.luts {
        let image = source::load_image(&entry.path)
            .map_err(|e| format!("LUT {:?} ({}): {e}", entry.name, entry.path.display()))?;
        luts.push(preview_engine::LutSpec {
            name: entry.name.clone(),
            image,
            filter_linear: entry.linear.unwrap_or(false),
            wrap_mode: entry.wrap_mode.map(map_wrap_mode).unwrap_or_default(),
            mipmap: entry.mipmap.unwrap_or(false),
        });
    }
    Ok(luts)
}

/// Set a `#pragma parameter`'s current value live (#29), driving the slider UI.
/// Clamped to the parameter's `[min, max]` by the engine; an unknown name is a
/// no-op. No shader recompile or pipeline rebuild — the next frame re-packs the
/// value into the param UBO.
#[tauri::command]
pub fn set_parameter(
    state: State<'_, PreviewState>,
    name: String,
    value: f32,
) -> Result<(), String> {
    state.send(RenderCommand::SetParameter { name, value })
}

/// Compile every pass of a parsed preset into an engine [`preview_engine::Pass`],
/// mapping the parsed scale config (per axis, with the §2 combined/override
/// resolution) to the engine's. `None` scale keys are left as `None` so the
/// engine applies the position-dependent default.
fn compile_preset_chain(preset: &preset_io::Preset) -> Result<Vec<preview_engine::Pass>, String> {
    let mut passes = Vec::with_capacity(preset.passes.len());
    for p in &preset.passes {
        let loaded = preset_io::load_slang_file(&p.shader).map_err(|e| e.to_string())?;
        let compiled = slang_compile::compile_slang(&loaded.source, loaded.base_dir.as_deref())
            .map_err(|e| e.to_string())?;
        let mut pass = preview_engine::Pass::new(compiled);
        if let Some(scale) = scale_config(p) {
            pass = pass.with_scale(scale);
        }
        // Carry the preset `aliasN` onto the engine descriptor (#26) so a later
        // pass sampling `<alias>` / reading `<alias>Size` binds this pass's output.
        pass.alias = p.alias.clone();
        // Carry the parsed format/sampler hints onto the engine descriptor (#23).
        // An absent key keeps the engine default (linear filter, clamp_to_border
        // wrap, no srgb/float/mipmap — §3 v1 choices).
        if let Some(v) = p.srgb_framebuffer {
            pass.srgb_framebuffer = v;
        }
        if let Some(v) = p.float_framebuffer {
            pass.float_framebuffer = v;
        }
        if let Some(v) = p.filter_linear {
            pass.filter_linear = v;
        }
        if let Some(v) = p.wrap_mode {
            pass.wrap_mode = map_wrap_mode(v);
        }
        if let Some(v) = p.mipmap_input {
            pass.mipmap_input = v;
        }
        // `frame_count_modN` (#28): the FrameCount this pass sees is pre-wrapped
        // mod this value. Absent -> 0 (no wrap).
        if let Some(v) = p.frame_count_mod {
            pass.frame_count_mod = v;
        }
        passes.push(pass);
    }
    Ok(passes)
}

/// Map a parsed pass's per-axis scale to an engine [`preview_engine::ScaleConfig`],
/// or `None` if the pass declares no scale keys (engine applies the §2 default).
fn scale_config(p: &preset_io::Pass) -> Option<preview_engine::ScaleConfig> {
    if !p.has_scale() {
        return None;
    }
    Some(preview_engine::ScaleConfig {
        x: axis_scale(p.scale_type_x(), p.scale_factor_x()),
        y: axis_scale(p.scale_type_y(), p.scale_factor_y()),
    })
}

/// Build one engine axis-scale from a parsed (type, factor) pair. A missing type
/// or factor for one axis defaults to `source × 1.0` (§2: an axis with no keys
/// falls back to `source` × `1.0`).
fn axis_scale(ty: Option<preset_io::ScaleType>, factor: Option<f32>) -> preview_engine::AxisScale {
    match (ty, factor) {
        (Some(ty), Some(factor)) => preview_engine::AxisScale {
            ty: map_scale_type(ty),
            factor,
        },
        // An axis with a type but no factor, or vice versa, defaults the missing
        // half: factor 1.0, or type source (§2).
        (Some(ty), None) => preview_engine::AxisScale {
            ty: map_scale_type(ty),
            factor: 1.0,
        },
        (None, Some(factor)) => preview_engine::AxisScale {
            ty: preview_engine::ScaleType::Source,
            factor,
        },
        (None, None) => preview_engine::AxisScale::SOURCE_1X,
    }
}

/// Convert the preset parser's scale-type enum to the engine's.
fn map_scale_type(ty: preset_io::ScaleType) -> preview_engine::ScaleType {
    match ty {
        preset_io::ScaleType::Source => preview_engine::ScaleType::Source,
        preset_io::ScaleType::Viewport => preview_engine::ScaleType::Viewport,
        preset_io::ScaleType::Absolute => preview_engine::ScaleType::Absolute,
    }
}

/// Convert the preset parser's wrap-mode enum to the engine's (#23).
fn map_wrap_mode(wrap: preset_io::WrapMode) -> preview_engine::WrapMode {
    match wrap {
        preset_io::WrapMode::ClampToBorder => preview_engine::WrapMode::ClampToBorder,
        preset_io::WrapMode::ClampToEdge => preview_engine::WrapMode::ClampToEdge,
        preset_io::WrapMode::Repeat => preview_engine::WrapMode::Repeat,
        preset_io::WrapMode::MirroredRepeat => preview_engine::WrapMode::MirroredRepeat,
    }
}

/// Resize the preview pane (and thus the offscreen target frames downsample to).
#[tauri::command]
pub fn set_viewport(state: State<'_, PreviewState>, width: u32, height: u32) -> Result<(), String> {
    let width = width.max(1);
    let height = height.max(1);
    state.set_viewport_size(width, height);
    state.send(RenderCommand::SetViewport(width, height))
}

/// Configure the **simulated viewport** (#30, Architecture §D/§E): the output
/// resolution + integer-scale the final pass renders at, distinct from the pane
/// (which [`set_viewport`] controls). `enabled = false` clears it so the viewport
/// tracks the pane (the default); `enabled = true` sets `width × height` with the
/// `integer_scale` toggle. The change takes effect on the next frame: `viewport`-
/// scaled FBOs, the final pass's `OutputSize`, and `FinalViewportSize` recompute to
/// the §9 content rect, which is composited (with black letterbox bars) into the
/// pane. Dimensions are clamped to at least 1.
#[tauri::command]
pub fn set_simulated_viewport(
    state: State<'_, PreviewState>,
    enabled: bool,
    width: u32,
    height: u32,
    integer_scale: bool,
) -> Result<(), String> {
    let config = if enabled {
        Some(preview_engine::ViewportConfig {
            width: width.max(1),
            height: height.max(1),
            integer_scale,
        })
    } else {
        None
    };
    state.send(RenderCommand::SetSimulatedViewport(config))
}

/// Drive a [`FrameSource`], sending each rendered frame over `channel` as raw
/// binary, paced to `period`, until `running` is cleared or the channel closes.
///
/// Extracted from [`start_preview_stream`] so the transport can be unit-tested
/// headlessly (no Tauri runtime) — see the tests below.
fn pump_frames<S: FrameSource>(
    channel: &Channel<InvokeResponseBody>,
    mut source: S,
    running: &AtomicBool,
    period: Duration,
) {
    let mut frame_index: u64 = 0;
    let mut next = Instant::now();
    let mut buf = Vec::new();

    while running.load(Ordering::Relaxed) {
        source.render_into(frame_index, &mut buf);
        // `send` takes ownership; hand over this frame and start a fresh buffer.
        if channel
            .send(InvokeResponseBody::Raw(std::mem::take(&mut buf)))
            .is_err()
        {
            // The webview/channel went away — stop cleanly.
            break;
        }
        frame_index = frame_index.wrapping_add(1);

        // Frame pacing: sleep until the next deadline; if we've fallen behind,
        // resync rather than accumulating drift.
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    running.store(false, Ordering::Relaxed);
}

/// Stop the preview stream with the given id (idempotent; a non-matching or
/// already-stopped id is a no-op).
#[tauri::command]
pub fn stop_preview_stream(state: State<'_, PreviewState>, stream_id: String) {
    state.stop_matching(&stream_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use preview_engine::{
        GradientSource, ScaleType as EngineScaleType, FRAME_HEADER_LEN, FRAME_MAGIC,
    };

    // ---- Preset → engine scale-config mapping (#22; no GPU needed). ----

    /// Parse a one-pass preset body and map pass 0's scale to the engine config.
    fn scale_of(extra_keys: &str) -> Option<preview_engine::ScaleConfig> {
        let body = format!("shaders = 1\nshader0 = a.slang\n{extra_keys}");
        let preset =
            preset_io::parse_slangp_str(&body, std::path::Path::new("/p")).expect("preset parses");
        scale_config(&preset.passes[0])
    }

    #[test]
    fn no_scale_keys_map_to_none() {
        // The engine then applies the §2 position default.
        assert!(scale_of("").is_none());
    }

    #[test]
    fn combined_scale_maps_both_axes() {
        let s = scale_of("scale_type0 = source\nscale0 = 2.0\n").expect("has scale");
        assert_eq!(s.x.ty, EngineScaleType::Source);
        assert_eq!(s.x.factor, 2.0);
        assert_eq!(s.y.ty, EngineScaleType::Source);
        assert_eq!(s.y.factor, 2.0);
    }

    #[test]
    fn per_axis_scale_maps_independently() {
        let s = scale_of(
            "scale_type_x0 = absolute\nscale_x0 = 320\nscale_type_y0 = viewport\nscale_y0 = 1.0\n",
        )
        .expect("has scale");
        assert_eq!(s.x.ty, EngineScaleType::Absolute);
        assert_eq!(s.x.factor, 320.0);
        assert_eq!(s.y.ty, EngineScaleType::Viewport);
        assert_eq!(s.y.factor, 1.0);
    }

    #[test]
    fn one_axis_only_defaults_the_other_to_source_1x() {
        // Only scale_x given -> Y defaults to source × 1.0 (§2).
        let s = scale_of("scale_type_x0 = viewport\nscale_x0 = 1.0\n").expect("has scale");
        assert_eq!(s.x.ty, EngineScaleType::Viewport);
        assert_eq!(s.y.ty, EngineScaleType::Source);
        assert_eq!(s.y.factor, 1.0);
    }

    // ---- Editor PassSettings → engine scale/format mapping (#54; no GPU). ----

    #[test]
    fn model_settings_no_scale_keys_map_to_none() {
        // An all-`None` ScaleAxis pair means "no scale keys" → engine §2 default.
        let settings = core_model::PassSettings::default();
        assert!(model_scale_config(&settings).is_none());
    }

    #[test]
    fn model_settings_per_axis_scale_maps_independently() {
        let settings = core_model::PassSettings {
            scale_x: core_model::ScaleAxis {
                scale_type: Some(core_model::ScaleType::Absolute),
                scale: Some(320.0),
            },
            scale_y: core_model::ScaleAxis {
                scale_type: Some(core_model::ScaleType::Viewport),
                scale: Some(1.0),
            },
            ..Default::default()
        };
        let s = model_scale_config(&settings).expect("has scale");
        assert_eq!(s.x.ty, EngineScaleType::Absolute);
        assert_eq!(s.x.factor, 320.0);
        assert_eq!(s.y.ty, EngineScaleType::Viewport);
        assert_eq!(s.y.factor, 1.0);
    }

    #[test]
    fn model_settings_one_axis_defaults_the_other_to_source_1x() {
        // Only X has scale keys → Y defaults to source × 1.0 (§2), as the file path.
        let settings = core_model::PassSettings {
            scale_x: core_model::ScaleAxis {
                scale_type: Some(core_model::ScaleType::Viewport),
                scale: Some(1.0),
            },
            ..Default::default()
        };
        let s = model_scale_config(&settings).expect("has scale");
        assert_eq!(s.x.ty, EngineScaleType::Viewport);
        assert_eq!(s.y.ty, EngineScaleType::Source);
        assert_eq!(s.y.factor, 1.0);
    }

    #[test]
    fn model_settings_carry_format_and_alias_onto_pass() {
        // The format/filter/wrap/alias keys land verbatim on the engine descriptor.
        let shader = slang_compile::compile_slang(preview_engine::DEFAULT_SHADER, None)
            .expect("compile default shader");
        let settings = core_model::PassSettings {
            alias: Some("crtPass".to_owned()),
            float_framebuffer: Some(true),
            filter_linear: Some(false),
            wrap_mode: Some(core_model::WrapMode::Repeat),
            mipmap_input: Some(true),
            frame_count_mod: Some(60),
            ..Default::default()
        };
        let pass = pass_from_settings(shader, &settings);
        assert_eq!(pass.alias.as_deref(), Some("crtPass"));
        assert!(pass.float_framebuffer);
        assert!(!pass.filter_linear);
        assert_eq!(pass.wrap_mode, preview_engine::WrapMode::Repeat);
        assert!(pass.mipmap_input);
        assert_eq!(pass.frame_count_mod, 60);
        // No scale keys → the §2 position default applies (scale stays None).
        assert!(pass.scale.is_none());
    }

    /// End-to-end transport check, no Tauri runtime: a `Channel` built from a
    /// collecting closure receives exactly the raw frames the producer sends.
    #[test]
    fn pump_sends_raw_frames_until_stopped() {
        let frames: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicBool::new(true));

        let sink = frames.clone();
        let stop = running.clone();
        let channel = Channel::new(move |body: InvokeResponseBody| {
            // The transport must carry *binary*, never JSON.
            let InvokeResponseBody::Raw(bytes) = body else {
                panic!("expected a raw binary frame, got JSON");
            };
            let mut got = sink.lock().unwrap();
            got.push(bytes);
            if got.len() >= 3 {
                stop.store(false, Ordering::Relaxed); // stop after a few frames
            }
            Ok(())
        });

        // period = ZERO so the test doesn't actually sleep.
        pump_frames(
            &channel,
            GradientSource::new(4, 4),
            &running,
            Duration::ZERO,
        );

        // The producer stopped once the consumer cleared the flag.
        assert!(
            !running.load(Ordering::Relaxed),
            "pump must exit when stopped"
        );

        let frames = frames.lock().unwrap();
        assert!(frames.len() >= 3, "expected at least 3 frames");
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(&frame[0..4], &FRAME_MAGIC, "frame {i} magic");
            assert_eq!(frame.len(), FRAME_HEADER_LEN + 4 * 4 * 4, "frame {i} size");
            // Frame index is written little-endian at offset 16.
            assert_eq!(frame[16] as usize, i, "frame {i} index in header");
        }
    }

    /// Integration check through the real command → render path (everything
    /// below the Tauri State/IPC marshaling, which can't run headlessly):
    /// commands feed a `RenderSource` over its mpsc channel exactly as the
    /// Tauri commands do, and the streamed frames come out downsampled to the
    /// pane size. Requires a wgpu adapter (as the other render tests do).
    #[test]
    fn command_path_streams_pane_sized_frames() {
        // Pane 24x18; source 200x200 -> frames must be pane-sized, not source-sized.
        const PANE: (u32, u32) = (24, 18);
        let (commands, rx) = mpsc::channel();
        let source = RenderSource::new(PANE.0, PANE.1, rx).expect("wgpu device");

        // Drive it exactly like load_shader(None) + load_source(None) would.
        let shader = slang_compile::compile_slang(preview_engine::DEFAULT_SHADER, None)
            .expect("compile default shader");
        commands.send(RenderCommand::SetShader(shader)).unwrap();
        commands
            .send(RenderCommand::SetSource(source::test_pattern(200, 200)))
            .unwrap();

        let frames: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicBool::new(true));
        let sink = frames.clone();
        let stop = running.clone();
        let channel = Channel::new(move |body: InvokeResponseBody| {
            let InvokeResponseBody::Raw(bytes) = body else {
                panic!("expected a raw binary frame");
            };
            let mut got = sink.lock().unwrap();
            got.push(bytes);
            if got.len() >= 3 {
                stop.store(false, Ordering::Relaxed);
            }
            Ok(())
        });

        pump_frames(&channel, source, &running, Duration::ZERO);

        let frames = frames.lock().unwrap();
        assert!(frames.len() >= 3, "expected streamed frames");
        for frame in frames.iter() {
            assert_eq!(&frame[0..4], &FRAME_MAGIC, "frame magic");
            let w = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]);
            let h = u32::from_le_bytes([frame[12], frame[13], frame[14], frame[15]]);
            assert_eq!((w, h), PANE, "frames are downsampled to the pane");
            assert_eq!(
                frame.len(),
                FRAME_HEADER_LEN + (PANE.0 * PANE.1 * 4) as usize
            );
        }
        // The render is real (the test pattern is not a solid fill).
        let last = frames.last().unwrap();
        let px = &last[FRAME_HEADER_LEN..];
        assert!(
            px.chunks_exact(4).any(|p| p != &px[0..4]),
            "expected varied pixels from a real render"
        );
    }
}
