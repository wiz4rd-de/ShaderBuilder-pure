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

use preview_engine::{FrameSource, RenderCommand, RenderSource};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

/// Default preview resolution. Small and fixed: transfer cost is bounded by the
/// pane size, not the simulated viewport (Architecture §F).
const DEFAULT_PREVIEW_WIDTH: u32 = 512;
const DEFAULT_PREVIEW_HEIGHT: u32 = 384;
/// Frame period for ~60 fps.
const FRAME_PERIOD: Duration = Duration::from_micros(16_667);

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

    std::thread::spawn(move || match RenderSource::new(width, height, rx) {
        Ok(source) => pump_frames(&channel, source, &running, FRAME_PERIOD),
        Err(err) => {
            eprintln!("preview: renderer unavailable, no frames will stream: {err}");
            running.store(false, Ordering::Relaxed);
        }
    });
}

/// Load the preview **source image**. With a path, decodes that file; with
/// `None`, uses the built-in checkerboard test pattern sized to the pane. The
/// decoded image is handed to the render thread; rendering continues
/// asynchronously.
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
    state.send(RenderCommand::SetSource(frame))
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
