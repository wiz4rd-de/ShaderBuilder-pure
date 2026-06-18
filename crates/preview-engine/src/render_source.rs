//! Live preview producer: a [`FrameSource`] backed by the headless wgpu
//! [`Renderer`]. This is what replaces Phase 0's [`crate::GradientSource`] —
//! the app's `tauri::ipc::Channel` transport drives it through the very same
//! [`FrameSource`] seam (Architecture §F).
//!
//! Each [`FrameSource::render_into`] call first drains any pending
//! [`RenderCommand`]s (sent from the app's Tauri commands over an mpsc channel),
//! then renders one frame into the offscreen target and reads it back into a
//! binary preview frame. The offscreen target is the **preview-pane size**, so
//! sampling a larger source image through the shader downsamples it on the GPU
//! to the pane — bounding the per-frame transfer cost regardless of how large
//! the source or the simulated viewport gets (Architecture §F). A configurable
//! *higher-than-pane* simulated viewport with an explicit box-filter downsample
//! is a Phase 2 refinement.
//!
//! Until both a source image and a shader have been loaded, `render_into`
//! emits a solid "waiting" frame at the pane size so the transport and canvas
//! stay exercised end-to-end (and so a missing/!ready state never starves the
//! stream).

use std::sync::mpsc::Receiver;

use slang_compile::CompiledShader;
use source::{Frame, FramePump, PngSequencePump, StillImage, TestPattern, TestPatternPump};

use crate::frame::{FrameHeader, FRAME_HEADER_LEN};
use crate::pass::Pass;
use crate::renderer::{Renderer, RendererError};
use crate::FrameSource;

/// The built-in default preview shader: a passthrough that samples the source
/// 1:1, using **separate** texture + sampler as wgpu's binding model requires
/// (see [`crate::renderer`]). Compiled by the app when a preview is requested
/// without a `.slang` file selected yet, so the pane shows the real source
/// image rather than nothing.
pub const DEFAULT_SHADER: &str = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D Source;
layout(set = 0, binding = 2) uniform sampler Smp;
void main() { FragColor = texture(sampler2D(Source, Smp), vTexCoord); }
";

/// The solid color emitted before a source + shader are loaded — a neutral dark
/// gray, distinct from a black clear and from the Phase 0 gradient.
const WAITING_RGBA: [u8; 4] = [32, 32, 32, 255];

/// What a [`RenderCommand::LoadSourcePump`] installs as the live source pump
/// (#31). The app builds one of these — doing any file IO (PNG decode) **in the
/// command, off the render loop** — and ships it over the channel; the render
/// thread turns it into a `Box<dyn FramePump + Send>` in
/// [`RenderSource::install_pump`]. PNG frames are decoded *in the app* and carried
/// as a `Vec<Frame>` (not a path) so the render thread never touches the disk.
pub enum SourceSpec {
    /// A single still image, presented forever (a 1-frame pump).
    StillImage(Frame),
    /// A built-in procedural test pattern rendered at `width × height`. A static
    /// pattern is a 1-frame pump; [`TestPattern::MotionSweep`] is animated.
    TestPattern {
        /// Which pattern to render.
        pattern: TestPattern,
        /// Pattern width in pixels (clamped to >= 1 by the pump).
        width: u32,
        /// Pattern height in pixels (clamped to >= 1 by the pump).
        height: u32,
    },
    /// A PNG sequence whose frames were **already decoded by the app** (file IO
    /// off the render loop). Frames are presented in the given order and loop.
    PngSequence(Vec<Frame>),
}

/// A command driving the live preview renderer. Produced by the app's Tauri
/// commands and applied to the [`RenderSource`] on the render thread between
/// frames (so heavy IO/compile happens off this loop — the app does it before
/// sending).
pub enum RenderCommand {
    /// Replace the source image the shader samples (one-shot still image). Kept
    /// for back-compat with the Phase-1 single-image path; internally it installs
    /// a [`StillImage`] pump (#31), so the source/history semantics match.
    SetSource(Frame),
    /// Install a source **pump** (#31): still image, test pattern, or decoded
    /// PNG sequence. The first frame after install is a reload
    /// ([`Renderer::set_source`] — resets history); subsequent advances rotate
    /// history. A new pump starts **paused at frame 0** unless a later `Play`
    /// arrives.
    LoadSourcePump(SourceSpec),
    /// Start advancing the pump at its fps (the §10 content clock). No-op without
    /// a pump, or for a 1-frame pump (nothing to advance).
    Play,
    /// Stop advancing the pump (hold the current frame).
    Pause,
    /// Advance the pump exactly one source frame **even when paused** (#31): one
    /// history rotation, one position step (loops at the end).
    Step,
    /// Seek the pump to a frame index (#31): jumps the position (modulo `len`),
    /// **resets the history ring AND feedback** (the new frame is a discontinuity
    /// — `set_source` + `reset_feedback`).
    Seek(usize),
    /// Set the pump's advance rate in source frames per second (#31). Clamped to a
    /// sane minimum; takes effect on the next advance pacing.
    SetFps(f32),
    /// Replace the active shader with a single-pass chain (back-compat with the
    /// Phase-1 single-shader path).
    SetShader(CompiledShader),
    /// Replace the active render chain with an ordered N-pass pipeline (#22). A
    /// `.slangp` preset is loaded by the app into a `Vec<Pass>` and sent here.
    SetChain(Vec<Pass>),
    /// Resize the offscreen target — i.e. the preview-pane size that frames are
    /// downsampled to.
    SetViewport(u32, u32),
    /// Set (or clear) the **simulated viewport** (#30): the output resolution +
    /// integer-scale the final pass renders at, distinct from the pane. `None`
    /// makes the viewport track the pane (the default). When `Some`, `viewport`-
    /// scaled FBOs, the final pass's `OutputSize`, and `FinalViewportSize` reflect
    /// the §9 content rect, which is composited (with letterbox bars) into the pane.
    SetSimulatedViewport(Option<crate::ViewportConfig>),
    /// Update a `#pragma parameter`'s current value live (#29). Applied to the
    /// chain's global-by-name parameter store; the next frame re-packs it — no
    /// recompile or pipeline rebuild. An unknown name is a no-op.
    SetParameter { name: String, value: f32 },
    /// Apply a preset's `parameter_overrides` (#29): for each `name -> value`, set
    /// the current value (clamped). Sent after a `SetChain` so the override lands
    /// on the freshly-collected defaults.
    ApplyParameterOverrides(std::collections::BTreeMap<String, f32>),
    /// Replace the registered LUTs (#27): the preset's `textures` family, decoded
    /// by the app and bound by name as `<NAME>`. An empty list clears stale LUTs.
    SetLuts(Vec<crate::LutSpec>),
}

/// The transport's render rate (the §10 *animation* clock): `render_into` is
/// called ~60×/s by `pump_frames`. The pump advance is paced against this without
/// a wall clock — `advance_period_ticks` derives how many render ticks pass per
/// source frame from this and the pump's fps — keeping the engine deterministic
/// and tests free of `Instant` (#31).
const TRANSPORT_FPS: f32 = 60.0;

/// The default pump advance rate (source frames per second) until a `SetFps`
/// arrives (#31). Matches the transport so a freshly played sequence advances
/// once per render tick.
const DEFAULT_PUMP_FPS: f32 = 60.0;

/// A source-position update emitted when the pump's position changes (#31): the
/// new frame `index` and the sequence `len`. The app forwards this to the webview
/// as the `source-position` Tauri event. Carried over an optional channel so the
/// render thread never touches the `AppHandle` (it does not hold one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionUpdate {
    /// The current frame index in `0..len`.
    pub index: usize,
    /// The number of frames in the pump (`>= 1`).
    pub len: usize,
}

/// The render engine's liveness state (#62), reported from the render thread so
/// the preview pane can distinguish a fresh render from a held last-good frame
/// from a stopped engine. The app maps this to `core_model::EngineStatus` at the
/// IPC boundary (the engine deliberately has no dependency on `core-model`, like
/// [`crate::ViewportConfig`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineStatus {
    /// A fresh frame was rendered this tick — the preview is live.
    Live,
    /// The latest render could not produce a fresh frame; the pane holds the last
    /// good output (or the neutral waiting frame before the first render).
    LastGood,
    /// The render thread has stopped — no further frames will arrive.
    Stopped,
}

/// A structured engine event reported from the render thread (#62): either a
/// liveness-status transition or a render error. Carried over an optional channel
/// (like [`PositionUpdate`]) so the render thread never blocks and never touches
/// the `AppHandle`; the app forwards it to the webview as the `engine-event` Tauri
/// event, mapping it to the `core_model` wire types at the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// A liveness-status transition (live / last-good / stopped).
    Status(EngineStatus),
    /// A render error: a short stable `code` + a human-readable `message`. Engine
    /// errors are not pass-tagged here (the engine renders a chain it was given);
    /// pass/node tagging for slang-compile failures is added by the app, which
    /// owns the pass→source mapping.
    Error {
        /// A short, stable, machine-readable category tag (e.g. `"deviceLost"`).
        code: String,
        /// The human-readable explanation of the problem.
        message: String,
    },
}

/// A [`FrameSource`] that renders a source image through a compiled slang shader
/// on wgpu and reads the result back as a pane-sized binary frame.
pub struct RenderSource {
    renderer: Renderer,
    commands: Receiver<RenderCommand>,
    have_source: bool,
    have_shader: bool,
    /// The active source pump (#31), if one has been loaded. `None` falls back to
    /// the legacy one-shot `SetSource` still-image path.
    pump: Option<Box<dyn FramePump + Send>>,
    /// Whether the pump is advancing (`play`/`pause`). Ignored without a pump.
    playing: bool,
    /// The pump's advance rate (source frames per second) — the content clock.
    fps: f32,
    /// Render-tick accumulator (#31): incremented per `render_into`; when it
    /// reaches `advance_period_ticks()` the pump advances one frame and the
    /// accumulator resets. Paces advances against the transport's ~60 fps with no
    /// wall clock, so playback is deterministic in tests.
    tick_accum: u32,
    /// Set once the pump's current frame has been uploaded as the initial
    /// `Original` ([`Renderer::set_source`]); reset whenever a new pump is loaded
    /// so the first frame is a history-resetting reload, not an advance.
    pump_primed: bool,
    /// Optional sink for position updates (#31). When set, a position change
    /// (play advance / step / seek / load) sends a [`PositionUpdate`]; the app's
    /// forwarder thread emits the `source-position` event. `None` in headless
    /// tests that introspect the renderer directly.
    position_tx: Option<std::sync::mpsc::Sender<PositionUpdate>>,
    /// The last position reported on `position_tx`, to suppress duplicate emits.
    last_reported_position: Option<usize>,
    /// Optional sink for engine status/error events (#62). When set, each
    /// `render_into` reports a live/last-good status *transition* (duplicates
    /// suppressed) and render failures send an [`EngineEvent::Error`]. The app
    /// forwards these as the `engine-event` Tauri event. `None` in headless tests.
    event_tx: Option<std::sync::mpsc::Sender<EngineEvent>>,
    /// The last status reported on `event_tx`, to suppress duplicate emits (so a
    /// steadily-live or steadily-last-good stream sends one transition, not 60/s).
    last_reported_status: Option<EngineStatus>,
}

impl RenderSource {
    /// Initialize the wgpu renderer at the given pane size. The heavy device
    /// init runs here, so construct this on the render thread (keeping the
    /// `start_preview_stream` command non-blocking). `commands` is the receiving
    /// end of the channel the app's load/viewport commands send on.
    pub fn new(
        width: u32,
        height: u32,
        commands: Receiver<RenderCommand>,
    ) -> Result<Self, RendererError> {
        Self::with_position_sink(width, height, commands, None)
    }

    /// Like [`RenderSource::new`] but with an optional position-update sink (#31):
    /// position changes are reported on `position_tx` so the app can forward them
    /// as the `source-position` event without the render thread holding an
    /// `AppHandle`.
    pub fn with_position_sink(
        width: u32,
        height: u32,
        commands: Receiver<RenderCommand>,
        position_tx: Option<std::sync::mpsc::Sender<PositionUpdate>>,
    ) -> Result<Self, RendererError> {
        Self::with_sinks(width, height, commands, position_tx, None)
    }

    /// Like [`RenderSource::with_position_sink`] but also wiring an engine
    /// status/error sink (#62): live/last-good status transitions and render
    /// failures are reported on `event_tx` for the app to forward as the
    /// `engine-event` Tauri event. Either sink may be `None` (headless tests).
    pub fn with_sinks(
        width: u32,
        height: u32,
        commands: Receiver<RenderCommand>,
        position_tx: Option<std::sync::mpsc::Sender<PositionUpdate>>,
        event_tx: Option<std::sync::mpsc::Sender<EngineEvent>>,
    ) -> Result<Self, RendererError> {
        Ok(Self {
            renderer: Renderer::new(width, height)?,
            commands,
            have_source: false,
            have_shader: false,
            pump: None,
            playing: false,
            fps: DEFAULT_PUMP_FPS,
            tick_accum: 0,
            pump_primed: false,
            position_tx,
            last_reported_position: None,
            event_tx,
            last_reported_status: None,
        })
    }

    /// Report an engine status TRANSITION on the event sink (#62), suppressing
    /// duplicates so a steady stream sends one event per change, not per frame. A
    /// `None` sink (headless tests) is a no-op.
    fn report_status(&mut self, status: EngineStatus) {
        if self.last_reported_status == Some(status) {
            return;
        }
        self.last_reported_status = Some(status);
        if let Some(tx) = &self.event_tx {
            // A closed receiver (app torn down) is harmless — drop the event.
            let _ = tx.send(EngineEvent::Status(status));
        }
    }

    /// Report a render error on the event sink (#62). A `None` sink is a no-op.
    fn report_error(&self, code: &str, message: String) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(EngineEvent::Error {
                code: code.to_string(),
                message,
            });
        }
    }

    /// Drain and apply every queued command. Loads are applied in arrival order;
    /// the actual render happens once afterwards, so source/shader/viewport
    /// updates in the same batch all take effect on the next frame together.
    fn apply_commands(&mut self) {
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                RenderCommand::SetSource(frame) => {
                    // Back-compat one-shot still image (#31): install a StillImage
                    // pump so source/history semantics are uniform. Priming on the
                    // next render calls `set_source` (resets history) once.
                    self.install_pump(Box::new(StillImage::new(frame)));
                }
                RenderCommand::LoadSourcePump(spec) => {
                    self.install_pump(build_pump(spec));
                }
                RenderCommand::Play => {
                    self.playing = true;
                    // Restart the pacing window so the first played frame is one
                    // full period away, not an immediate jump.
                    self.tick_accum = 0;
                }
                RenderCommand::Pause => {
                    self.playing = false;
                }
                RenderCommand::Step => self.step_pump(),
                RenderCommand::Seek(index) => self.seek_pump(index),
                RenderCommand::SetFps(fps) => {
                    // Clamp to a sane floor so the pacing divisor never blows up.
                    self.fps = fps.max(0.01);
                    self.tick_accum = 0;
                }
                RenderCommand::SetShader(shader) => {
                    self.renderer.set_shader(&shader);
                    self.have_shader = true;
                }
                RenderCommand::SetChain(passes) => {
                    // An empty chain is ignored (keeps the previous one running).
                    if self.renderer.set_chain(&passes).is_ok() {
                        self.have_shader = true;
                    }
                }
                RenderCommand::SetViewport(width, height) => {
                    self.renderer.set_viewport(width, height);
                }
                RenderCommand::SetSimulatedViewport(config) => {
                    // #30: takes effect next frame — `viewport`-scaled FBOs and the
                    // final-pass composite recompute from the new content rect.
                    self.renderer.set_simulated_viewport(config);
                }
                RenderCommand::SetParameter { name, value } => {
                    // Live param update: no recompile, takes effect next frame.
                    self.renderer.set_parameter(&name, value);
                }
                RenderCommand::ApplyParameterOverrides(overrides) => {
                    let mut store = self.renderer.collected_params().clone();
                    store.apply_overrides(&overrides);
                    self.renderer.set_params(store);
                }
                RenderCommand::SetLuts(luts) => {
                    self.renderer.set_luts(luts);
                }
            }
        }
    }

    /// Whether a real render can run (both a source image and a shader are set).
    fn ready(&self) -> bool {
        self.have_source && self.have_shader
    }

    /// Install a new pump (#31): replace any current pump, reset playback state to
    /// **paused at frame 0**, and arrange for the next render to prime it (upload
    /// its current frame as the `Original` via `set_source`, resetting history).
    fn install_pump(&mut self, pump: Box<dyn FramePump + Send>) {
        self.pump = Some(pump);
        self.pump_primed = false;
        self.playing = false;
        self.tick_accum = 0;
        self.last_reported_position = None;
        // The first render will `set_source` + report position; nothing to do here
        // beyond marking the source available.
        self.have_source = true;
    }

    /// The number of render ticks between pump advances (#31), `round(transport
    /// fps / pump fps)`, at least 1. At 60 fps both, this is 1 (advance every
    /// render); at 30 fps pump it is 2 (advance every other render); etc.
    fn advance_period_ticks(&self) -> u32 {
        (TRANSPORT_FPS / self.fps).round().max(1.0) as u32
    }

    /// Advance the pump one source frame and rotate the renderer's history once
    /// (#31, §5/§10 step 5): `pump.advance()` then `renderer.advance_source`. Only
    /// meaningful once the pump is primed and has > 1 frame. Reports the new
    /// position.
    fn step_pump(&mut self) {
        // Take the pump out to satisfy the borrow checker (we touch `renderer`
        // while reading the pump), then put it back.
        let Some(mut pump) = self.pump.take() else {
            return;
        };
        if pump.len() <= 1 {
            // A 1-frame pump (still image / static pattern): stepping is a no-op,
            // but ensure it is primed so the source is established.
            if !self.pump_primed {
                self.renderer.set_source(pump.current());
                self.pump_primed = true;
                self.report_position(&*pump);
            }
            self.pump = Some(pump);
            return;
        }
        if !self.pump_primed {
            // Not primed yet: priming IS the first frame; do that instead of
            // advancing past it (so step before the first render shows frame 0).
            self.renderer.set_source(pump.current());
            self.pump_primed = true;
        } else {
            pump.advance();
            self.renderer.advance_source(pump.current());
        }
        self.report_position(&*pump);
        self.pump = Some(pump);
    }

    /// Seek the pump to `index` (#31): jump the position, then re-establish the new
    /// current frame as `Original` via `set_source` (which **resets the history
    /// ring**) and `reset_feedback` (the seek is a discontinuity, §4). Reports the
    /// new position.
    fn seek_pump(&mut self, index: usize) {
        let Some(mut pump) = self.pump.take() else {
            return;
        };
        pump.seek(index);
        self.renderer.set_source(pump.current());
        self.renderer.reset_feedback();
        self.pump_primed = true;
        self.report_position(&*pump);
        self.pump = Some(pump);
    }

    /// Per-frame pump pacing (#31): prime on the first frame after a load
    /// (`set_source`, resets history), then — if playing and the pump has > 1
    /// frame — advance one source frame every [`RenderSource::advance_period_ticks`]
    /// render ticks (`advance_source`, rotates history once). A paused or held
    /// frame does NOT re-upload the source (avoids needless GPU work per frame).
    fn pace_pump(&mut self) {
        let Some(mut pump) = self.pump.take() else {
            return;
        };
        if !self.pump_primed {
            // First frame after a load: establish the current frame as Original
            // (reset history). No advance — the pump is at frame 0.
            self.renderer.set_source(pump.current());
            self.pump_primed = true;
            self.report_position(&*pump);
            self.pump = Some(pump);
            return;
        }
        if self.playing && pump.len() > 1 {
            self.tick_accum += 1;
            if self.tick_accum >= self.advance_period_ticks() {
                self.tick_accum = 0;
                pump.advance();
                self.renderer.advance_source(pump.current());
                self.report_position(&*pump);
            }
        }
        self.pump = Some(pump);
    }

    /// Report the pump's current position on the sink, suppressing duplicates so a
    /// held frame doesn't spam the channel (#31). A `None` sink (headless tests) is
    /// a no-op.
    fn report_position(&mut self, pump: &(dyn FramePump + Send)) {
        let index = pump.position();
        if self.last_reported_position == Some(index) {
            return;
        }
        self.last_reported_position = Some(index);
        if let Some(tx) = &self.position_tx {
            // A closed receiver (app torn down) is harmless — drop the update.
            let _ = tx.send(PositionUpdate {
                index,
                len: pump.len(),
            });
        }
    }
}

impl FrameSource for RenderSource {
    fn dimensions(&self) -> (u32, u32) {
        self.renderer.viewport()
    }

    fn render_into(&mut self, index: u64, buf: &mut Vec<u8>) {
        self.apply_commands();
        // Pace the pump (prime / advance) before rendering this frame (#31).
        self.pace_pump();
        let (width, height) = self.renderer.viewport();
        buf.clear();

        // Render one frame and read it back; on any failure fall through to the
        // waiting frame so the stream keeps a valid, pane-sized output. A fresh
        // frame reports `Live`; a not-ready or failed render holds the last-good
        // frame and reports `LastGood` (NOT an error — the §F "waiting" frame is a
        // valid rendering state, #62). A hard render/readback FAILURE (distinct
        // from "not ready yet") additionally reports a structured error event.
        if self.ready() {
            match self.renderer.render() {
                Ok(()) => match self.renderer.read_back() {
                    Ok(frame) => {
                        let header = FrameHeader::rgba8(frame.width, frame.height, index);
                        buf.reserve(FRAME_HEADER_LEN + header.payload_len());
                        header.write_to(buf);
                        buf.extend_from_slice(&frame.rgba);
                        self.report_status(EngineStatus::Live);
                        return;
                    }
                    Err(err) => self.report_error("readback", err.to_string()),
                },
                Err(err) => self.report_error("renderFailed", err.to_string()),
            }
        }

        // Not ready yet, or a render/readback failure: hold the last-good frame.
        self.report_status(EngineStatus::LastGood);
        write_solid_frame(buf, width, height, index, WAITING_RGBA);
    }
}

/// Turn a [`SourceSpec`] (built by the app, IO already done) into a live pump
/// (#31). PNG frames arrive pre-decoded as a `Vec<Frame>`, so this does no IO.
fn build_pump(spec: SourceSpec) -> Box<dyn FramePump + Send> {
    match spec {
        SourceSpec::StillImage(frame) => Box::new(StillImage::new(frame)),
        SourceSpec::TestPattern {
            pattern,
            width,
            height,
        } => Box::new(TestPatternPump::new(pattern, width, height)),
        SourceSpec::PngSequence(frames) => Box::new(PngSequencePump::from_frames(frames)),
    }
}

/// Write a header + a solid-color RGBA8 payload of `width * height` pixels.
fn write_solid_frame(buf: &mut Vec<u8>, width: u32, height: u32, index: u64, rgba: [u8; 4]) {
    let header = FrameHeader::rgba8(width, height, index);
    buf.reserve(FRAME_HEADER_LEN + header.payload_len());
    header.write_to(buf);
    for _ in 0..(width as usize * height as usize) {
        buf.extend_from_slice(&rgba);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FRAME_MAGIC, PIXEL_FORMAT_RGBA8};
    use slang_compile::compile_slang;
    use source::test_pattern;
    use std::sync::mpsc;

    /// Read `(width, height)` out of a binary frame header.
    fn frame_dims(buf: &[u8]) -> (u32, u32) {
        assert_eq!(&buf[0..4], &FRAME_MAGIC, "frame magic");
        assert_eq!(buf[6], PIXEL_FORMAT_RGBA8, "pixel format");
        let w = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let h = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        (w, h)
    }

    fn pixels(buf: &[u8]) -> &[u8] {
        &buf[FRAME_HEADER_LEN..]
    }

    #[test]
    fn default_shader_compiles() {
        // The built-in passthrough must survive the real toolchain.
        let shader = compile_slang(DEFAULT_SHADER, None).expect("default shader compiles");
        assert!(shader.reflection.parameters.is_empty());
    }

    #[test]
    fn downsamples_large_source_to_pane_size() {
        // Pane 32x24, source 128x128 -> the streamed frame is pane-sized, not
        // source-sized: the GPU downsamples while sampling.
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(32, 24, rx).expect("wgpu device");

        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::SetSource(test_pattern(128, 128)))
            .unwrap();

        let mut buf = Vec::new();
        src.render_into(0, &mut buf);

        assert_eq!(frame_dims(&buf), (32, 24), "frame is pane-sized");
        assert_eq!(buf.len(), FRAME_HEADER_LEN + 32 * 24 * 4, "payload size");
        // The render is real: the high-contrast test pattern is not a solid fill.
        let px = pixels(&buf);
        assert!(
            px.chunks_exact(4).any(|p| p != &px[0..4]),
            "expected varied pixels from a real render, got a solid frame"
        );
        assert_ne!(
            &px[0..4],
            WAITING_RGBA.as_slice(),
            "should not be the waiting frame"
        );
    }

    #[test]
    fn emits_waiting_frame_before_ready() {
        // No source/shader loaded: a valid, pane-sized, solid waiting frame.
        let (_tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(8, 6, rx).expect("wgpu device");

        let mut buf = Vec::new();
        src.render_into(0, &mut buf);

        assert_eq!(frame_dims(&buf), (8, 6));
        assert_eq!(buf.len(), FRAME_HEADER_LEN + 8 * 6 * 4);
        let px = pixels(&buf);
        assert!(
            px.chunks_exact(4).all(|p| p == WAITING_RGBA.as_slice()),
            "every pixel should be the waiting color until ready"
        );
    }

    /// A param-only shader (R = X) driven over the command channel: the
    /// `SetParameter` command must change the streamed frame within one render,
    /// exactly as the Tauri command will (#29).
    #[test]
    fn set_parameter_command_changes_streamed_frame() {
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(8, 8, rx).expect("wgpu device");

        let shader = compile_slang(
            "#version 450
#pragma parameter X \"X\" 0.5 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
layout(std140, set = 0, binding = 3) uniform Params { float X; } params;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D Source;
layout(set = 0, binding = 2) uniform sampler Smp;
void main() { FragColor = vec4(params.X, 0.0, 0.0, 1.0); }
",
            None,
        )
        .expect("compile param shader");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::SetSource(test_pattern(8, 8)))
            .unwrap();

        // Default X=0.5 -> R~128.
        let mut buf = Vec::new();
        src.render_into(0, &mut buf);
        assert!((pixels(&buf)[0] as i32 - 128).abs() <= 4, "default X");

        // Live set X=0.9 -> R~230 on the very next frame.
        tx.send(RenderCommand::SetParameter {
            name: "X".to_string(),
            value: 0.9,
        })
        .unwrap();
        src.render_into(1, &mut buf);
        assert!(
            (pixels(&buf)[0] as i32 - 230).abs() <= 6,
            "live SetParameter took effect, got {}",
            pixels(&buf)[0]
        );
    }

    #[test]
    fn set_viewport_command_resizes_emitted_frames() {
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(16, 16, rx).expect("wgpu device");

        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::SetSource(test_pattern(64, 64)))
            .unwrap();
        tx.send(RenderCommand::SetViewport(40, 30)).unwrap();

        let mut buf = Vec::new();
        src.render_into(0, &mut buf);
        assert_eq!(frame_dims(&buf), (40, 30), "viewport command took effect");
    }

    // ---- Source-pump integration (#31). ----
    //
    // These drive a PNG-sequence pump (synthetic solid-color frames) through the
    // command channel + `render_into` exactly as the app does, and assert the
    // STREAMED FRAME CONTENT (the passthrough reproduces the source) tracks the
    // sequence: play advances it, pause holds it, step advances one frame even
    // when paused, seek jumps + resets. Reading content (rather than history via a
    // shader) keeps the tests light while still proving advance/seek wiring.

    /// A solid-color RGBA8 frame (distinct frames produce distinct streamed pixels).
    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Frame {
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            data.extend_from_slice(&rgba);
        }
        Frame::new(w, h, data)
    }

    /// A three-frame sequence with distinct red channels (10, 20, 30).
    fn rgb_sequence() -> Vec<Frame> {
        vec![
            solid(8, 8, [10, 0, 0, 255]),
            solid(8, 8, [20, 0, 0, 255]),
            solid(8, 8, [30, 0, 0, 255]),
        ]
    }

    /// Drive one render and return the streamed frame's top-left R channel.
    fn render_r(src: &mut RenderSource, index: u64, buf: &mut Vec<u8>) -> u8 {
        src.render_into(index, buf);
        pixels(buf)[0]
    }

    #[test]
    fn pump_play_advances_through_the_sequence_and_pause_holds() {
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(8, 8, rx).expect("wgpu device");
        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::LoadSourcePump(SourceSpec::PngSequence(
            rgb_sequence(),
        )))
        .unwrap();
        // 60 fps pump @ 60 fps transport -> advance every render tick.
        tx.send(RenderCommand::SetFps(60.0)).unwrap();
        tx.send(RenderCommand::Play).unwrap();

        let mut buf = Vec::new();
        // Frame 0 primes the pump (frame 0 of the sequence, R=10) — no advance.
        assert_eq!(render_r(&mut src, 0, &mut buf), 10, "primed at frame 0");
        // Each subsequent render advances one frame: 20, 30, then loop to 10.
        assert_eq!(render_r(&mut src, 1, &mut buf), 20, "advanced to frame 1");
        assert_eq!(render_r(&mut src, 2, &mut buf), 30, "advanced to frame 2");
        assert_eq!(render_r(&mut src, 3, &mut buf), 10, "looped to frame 0");

        // Pause: the frame must HOLD across renders (no further advance).
        tx.send(RenderCommand::Pause).unwrap();
        let held = render_r(&mut src, 4, &mut buf);
        assert_eq!(render_r(&mut src, 5, &mut buf), held, "paused frame holds");
        assert_eq!(render_r(&mut src, 6, &mut buf), held, "still held");
    }

    #[test]
    fn pump_step_advances_one_frame_even_when_paused() {
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(8, 8, rx).expect("wgpu device");
        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::LoadSourcePump(SourceSpec::PngSequence(
            rgb_sequence(),
        )))
        .unwrap();
        // Default is paused.

        let mut buf = Vec::new();
        assert_eq!(render_r(&mut src, 0, &mut buf), 10, "paused, primed at 0");
        assert_eq!(render_r(&mut src, 1, &mut buf), 10, "paused: no advance");

        // Step once (while paused) -> frame 1 (R=20).
        tx.send(RenderCommand::Step).unwrap();
        assert_eq!(
            render_r(&mut src, 2, &mut buf),
            20,
            "step advanced one frame"
        );
        assert_eq!(
            render_r(&mut src, 3, &mut buf),
            20,
            "still paused after step"
        );

        // Step again -> frame 2 (R=30).
        tx.send(RenderCommand::Step).unwrap();
        assert_eq!(render_r(&mut src, 4, &mut buf), 30, "second step");
    }

    #[test]
    fn pump_seek_jumps_position_and_streams_that_frame() {
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(8, 8, rx).expect("wgpu device");
        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::LoadSourcePump(SourceSpec::PngSequence(
            rgb_sequence(),
        )))
        .unwrap();

        let mut buf = Vec::new();
        assert_eq!(render_r(&mut src, 0, &mut buf), 10, "primed at 0");

        // Seek to frame 2 (R=30); the next streamed frame must be that frame.
        tx.send(RenderCommand::Seek(2)).unwrap();
        assert_eq!(
            render_r(&mut src, 1, &mut buf),
            30,
            "seek jumped to frame 2"
        );

        // Seek with an out-of-range index wraps modulo len (4 % 3 == 1, R=20).
        tx.send(RenderCommand::Seek(4)).unwrap();
        assert_eq!(render_r(&mut src, 2, &mut buf), 20, "seek wraps modulo len");
    }

    #[test]
    fn position_updates_are_reported_on_advance_step_and_seek() {
        let (tx, rx) = mpsc::channel();
        let (pos_tx, pos_rx) = mpsc::channel();
        let mut src =
            RenderSource::with_position_sink(8, 8, rx, Some(pos_tx)).expect("wgpu device");
        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::LoadSourcePump(SourceSpec::PngSequence(
            rgb_sequence(),
        )))
        .unwrap();
        tx.send(RenderCommand::SetFps(60.0)).unwrap();
        tx.send(RenderCommand::Play).unwrap();

        let mut buf = Vec::new();
        // Prime (pos 0) + two advances (pos 1, 2) + loop (pos 0).
        for i in 0..4u64 {
            src.render_into(i, &mut buf);
        }
        let reported: Vec<usize> = pos_rx.try_iter().map(|u| u.index).collect();
        assert_eq!(
            reported,
            vec![0, 1, 2, 0],
            "play advances should each emit a position update"
        );

        // A seek reports its target.
        tx.send(RenderCommand::Pause).unwrap();
        tx.send(RenderCommand::Seek(2)).unwrap();
        src.render_into(4, &mut buf);
        let after_seek: Vec<PositionUpdate> = pos_rx.try_iter().collect();
        assert!(
            after_seek.iter().any(|u| u.index == 2 && u.len == 3),
            "seek must report position 2 of len 3, got {after_seek:?}"
        );
    }

    #[test]
    fn engine_status_reports_last_good_before_ready_then_live() {
        // #62: before a shader+source are loaded the engine holds the waiting frame
        // and reports `LastGood` (NOT an error — the waiting frame is a valid
        // rendering state); once both are set it renders and reports `Live`. The
        // status is a TRANSITION, deduped, so a steady stream emits one per change.
        let (tx, rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let mut src = RenderSource::with_sinks(8, 8, rx, None, Some(ev_tx)).expect("wgpu device");

        let mut buf = Vec::new();
        // No shader/source yet: two renders, but only ONE LastGood transition.
        src.render_into(0, &mut buf);
        src.render_into(1, &mut buf);

        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::SetSource(test_pattern(8, 8)))
            .unwrap();
        // Now ready: two renders, only ONE Live transition.
        src.render_into(2, &mut buf);
        src.render_into(3, &mut buf);

        let events: Vec<EngineEvent> = ev_rx.try_iter().collect();
        assert_eq!(
            events,
            vec![
                EngineEvent::Status(EngineStatus::LastGood),
                EngineEvent::Status(EngineStatus::Live),
            ],
            "expected exactly one LastGood then one Live transition, got {events:?}"
        );
    }

    #[test]
    fn motion_sweep_pattern_pump_animates_when_played() {
        // A built-in animated test pattern through the pump: distinct frames as it
        // advances (proves the TestPattern path + pacing, no PNG fixtures needed).
        let (tx, rx) = mpsc::channel();
        let mut src = RenderSource::new(32, 8, rx).expect("wgpu device");
        let shader = compile_slang(DEFAULT_SHADER, None).expect("compile");
        tx.send(RenderCommand::SetShader(shader)).unwrap();
        tx.send(RenderCommand::LoadSourcePump(SourceSpec::TestPattern {
            pattern: TestPattern::MotionSweep,
            width: 32,
            height: 8,
        }))
        .unwrap();
        tx.send(RenderCommand::SetFps(60.0)).unwrap();
        tx.send(RenderCommand::Play).unwrap();

        let mut buf = Vec::new();
        src.render_into(0, &mut buf);
        let frame_a = pixels(&buf).to_vec();
        // Advance several frames so the sweep bar visibly moves.
        for i in 1..6u64 {
            src.render_into(i, &mut buf);
        }
        let frame_b = pixels(&buf).to_vec();
        assert_ne!(frame_a, frame_b, "the motion-sweep pattern must animate");
    }
}
