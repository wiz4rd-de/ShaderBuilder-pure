//! Headless render-to-PNG entry point (#32, Architecture §G.2;
//! `docs/retroarch-slang-runtime.md` §1/§2/§4/§5/§7).
//!
//! [`render_preset_to_image`] is the harness's heart: it takes a `.slangp` path,
//! a source [`Frame`], a viewport, and a frame index, and produces an
//! [`image::RgbaImage`] — deterministically (same inputs → same bytes).
//!
//! ## How a preset becomes a rendered image
//!
//! 1. **Parse** the preset ([`preset_io::parse_slangp`]).
//! 2. **Compile** each pass's `.slang` through the real toolchain
//!    ([`slang_compile::compile_slang`], which now splits combined `sampler2D`s so
//!    the corpus path — real shaders use the combined form — works).
//! 3. **Map** the parsed per-pass scale/format/sampler/alias/feedback keys to the
//!    engine's [`preview_engine::Pass`]. This conversion is **copied from**
//!    `crates/app/src/preview.rs::compile_preset_chain` (the prompt says we may
//!    replicate it here so the harness does not depend on the Tauri `app` crate),
//!    with one addition: the preset's global `feedback_pass` is wired onto the
//!    engine `Pass::feedback` flag so a preset that opts into feedback purely via
//!    that key (rather than a `PassFeedbackN` reference) is still double-buffered.
//! 4. **Decode + register** the `textures=` LUTs ([`preview_engine::LutSpec`]).
//! 5. **Drive the source pump to `frame_index`**: we install the source as a
//!    1-frame pump and `Play`, then render exactly `frame_index + 1` frames
//!    through the engine's [`preview_engine::RenderSource`] (the very seam the app
//!    streams through). Each render advances the engine's `FrameCount`, rotates
//!    feedback double-buffers, and rotates the history ring — so feedback and
//!    history are at the deterministic state they would hold after that many
//!    frames. (A still image's pump holds frame 0 forever, so the *source content*
//!    is identical every frame; only `FrameCount` / feedback / history advance —
//!    which is exactly what a fixed-source-frame golden wants.)
//! 6. **Read back** the final RGBA8 frame into an [`image::RgbaImage`].
//!
//! The whole thing is deterministic: no wall clock (the pump paces against render
//! ticks, not `Instant`), and the engine is a pure function of its command stream.

use std::path::Path;

use image::RgbaImage;
use preview_engine::{
    AxisScale, LutSpec, Pass, RenderCommand, RenderSource, ScaleConfig, ScaleType, SourceSpec,
    WrapMode,
};
use source::Frame;

/// Errors from [`render_preset_to_image`]. Each variant carries enough context to
/// identify *which* preset / pass / LUT failed (the fuzzer surfaces these per
/// preset).
#[derive(Debug)]
pub enum HarnessError {
    /// The `.slangp` could not be parsed.
    Parse(String),
    /// A pass's `.slang` could not be read or compiled.
    Compile(String),
    /// A LUT image could not be decoded.
    Lut(String),
    /// The wgpu renderer could not be created (no adapter) or a render/read-back
    /// failed.
    Render(String),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarnessError::Parse(e) => write!(f, "preset parse failed: {e}"),
            HarnessError::Compile(e) => write!(f, "pass compile failed: {e}"),
            HarnessError::Lut(e) => write!(f, "LUT decode failed: {e}"),
            HarnessError::Render(e) => write!(f, "render failed: {e}"),
        }
    }
}

impl std::error::Error for HarnessError {}

/// Render a `.slangp` preset over a fixed `source` frame, at `viewport`
/// resolution, advanced to `frame_index`, and return the read-back RGBA8 image.
///
/// Deterministic: the same `(slangp, source, viewport, frame_index)` yield
/// byte-identical bytes on a given adapter. (Across *different* GPU adapters the
/// bytes can differ at the sub-pixel level — that is why the self-oracle goldens
/// are diffed with a tolerance, and why CRT-style RetroArch references are
/// compared with a perceptual threshold rather than exact equality.)
///
/// `frame_index` is **0-based**: index 0 renders one frame (feedback reads the
/// cold/black previous frame; `FrameCount == 0`); index `n` renders `n + 1`
/// frames so feedback / history / `FrameCount` are at the post-`n`-advance state.
///
/// See the module docs for the full pipeline. Errors are returned (never panic)
/// so the [`crate::fuzz`] runner can catch them per preset.
pub fn render_preset_to_image(
    slangp: &Path,
    source: &Frame,
    viewport: (u32, u32),
    frame_index: u64,
) -> Result<RgbaImage, HarnessError> {
    let preset = preset_io::parse_slangp(slangp).map_err(|e| HarnessError::Parse(e.to_string()))?;
    let passes = compile_preset_chain(&preset)?;
    let luts = compile_preset_luts(&preset)?;

    // Drive the engine through the SAME `RenderSource` seam the app streams
    // through, so feedback + history advance exactly as they do live. The command
    // channel is the deterministic input; we send the chain/LUTs/source, `Play`,
    // and render `frame_index + 1` frames.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut render_source = RenderSource::new(viewport.0.max(1), viewport.1.max(1), rx)
        .map_err(|e| HarnessError::Render(e.to_string()))?;

    // Order matches the app's `load_preset`: LUTs, then the chain, then overrides.
    tx.send(RenderCommand::SetLuts(luts)).ok();
    tx.send(RenderCommand::SetChain(passes)).ok();
    if !preset.parameter_overrides.is_empty() {
        tx.send(RenderCommand::ApplyParameterOverrides(
            preset.parameter_overrides.clone(),
        ))
        .ok();
    }
    // Install the source as a 1-frame still pump and play it. A still pump holds
    // frame 0 (so the SOURCE content is fixed every frame), while `Play` makes the
    // engine advance `FrameCount` / feedback / history each render tick — which is
    // what a fixed-source-frame, frame-indexed golden needs.
    tx.send(RenderCommand::LoadSourcePump(SourceSpec::StillImage(
        source.clone(),
    )))
    .ok();
    tx.send(RenderCommand::Play).ok();
    // Drop the sender so the receiver is finite if we ever loop; we drain it all on
    // the first render anyway.
    drop(tx);

    // Render `frame_index + 1` frames into a reusable buffer; keep the last.
    let mut buf = Vec::new();
    for i in 0..=frame_index {
        use preview_engine::FrameSource;
        render_source.render_into(i, &mut buf);
    }

    // The buffer is a binary preview frame: a fixed-size header then RGBA8 rows.
    // Strip the header and wrap the payload as an `RgbaImage` at the viewport size.
    let (out_w, out_h) = {
        use preview_engine::FrameSource;
        render_source.dimensions()
    };
    let payload = &buf[preview_engine::FRAME_HEADER_LEN..];
    let expected = (out_w as usize) * (out_h as usize) * 4;
    if payload.len() != expected {
        return Err(HarnessError::Render(format!(
            "read-back payload was {} bytes, expected {expected} for {out_w}x{out_h}",
            payload.len()
        )));
    }
    RgbaImage::from_raw(out_w, out_h, payload.to_vec())
        .ok_or_else(|| HarnessError::Render("RGBA payload did not fit the image".into()))
}

/// Compile every pass of a parsed preset into an engine [`Pass`], mapping the
/// parsed scale / format / sampler / alias / `frame_count_mod` keys to the
/// engine's. **Copied from** `app/src/preview.rs::compile_preset_chain` (so the
/// harness has no dependency on the Tauri `app` crate), plus a `feedback_pass`
/// addition (see below).
fn compile_preset_chain(preset: &preset_io::Preset) -> Result<Vec<Pass>, HarnessError> {
    let mut passes = Vec::with_capacity(preset.passes.len());
    for p in &preset.passes {
        let loaded = preset_io::load_slang_file(&p.shader)
            .map_err(|e| HarnessError::Compile(e.to_string()))?;
        let compiled = slang_compile::compile_slang(&loaded.source, loaded.base_dir.as_deref())
            .map_err(|e| HarnessError::Compile(e.to_string()))?;
        let mut pass = Pass::new(compiled);
        if let Some(scale) = scale_config(p) {
            pass = pass.with_scale(scale);
        }
        pass.alias = p.alias.clone();
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
        if let Some(v) = p.frame_count_mod {
            pass.frame_count_mod = v;
        }
        passes.push(pass);
    }

    // Addition over the app's mapping: honor the preset's global `feedback_pass`
    // (#24, §4). The engine *also* auto-detects feedback from a `PassFeedbackN`
    // reflection reference (a union), so this is only load-bearing for a preset
    // that opts into feedback purely through the `feedback_pass` key. Out-of-range
    // indices are ignored.
    if let Some(fp) = preset.feedback_pass {
        if fp >= 0 {
            if let Some(pass) = passes.get_mut(fp as usize) {
                pass.feedback = true;
            }
        }
    }

    Ok(passes)
}

/// Decode a parsed preset's LUTs into engine [`LutSpec`]s. Copied from
/// `app/src/preview.rs::compile_preset_luts` (§7 LUT defaults: nearest filter,
/// `clamp_to_border` wrap, no mipmap when a key is absent).
fn compile_preset_luts(preset: &preset_io::Preset) -> Result<Vec<LutSpec>, HarnessError> {
    let mut luts = Vec::with_capacity(preset.luts.len());
    for entry in &preset.luts {
        let image = source::load_image(&entry.path).map_err(|e| {
            HarnessError::Lut(format!("{:?} ({}): {e}", entry.name, entry.path.display()))
        })?;
        luts.push(LutSpec {
            name: entry.name.clone(),
            image,
            filter_linear: entry.linear.unwrap_or(false),
            wrap_mode: entry.wrap_mode.map(map_wrap_mode).unwrap_or_default(),
            mipmap: entry.mipmap.unwrap_or(false),
        });
    }
    Ok(luts)
}

/// Map a parsed pass's per-axis scale to an engine [`ScaleConfig`], or `None` if
/// the pass declares no scale keys (the engine applies the §2 default). Copied
/// from the app.
fn scale_config(p: &preset_io::Pass) -> Option<ScaleConfig> {
    if !p.has_scale() {
        return None;
    }
    Some(ScaleConfig {
        x: axis_scale(p.scale_type_x(), p.scale_factor_x()),
        y: axis_scale(p.scale_type_y(), p.scale_factor_y()),
    })
}

/// Build one engine axis-scale from a parsed (type, factor) pair (§2). Copied from
/// the app.
fn axis_scale(ty: Option<preset_io::ScaleType>, factor: Option<f32>) -> AxisScale {
    match (ty, factor) {
        (Some(ty), Some(factor)) => AxisScale {
            ty: map_scale_type(ty),
            factor,
        },
        (Some(ty), None) => AxisScale {
            ty: map_scale_type(ty),
            factor: 1.0,
        },
        (None, Some(factor)) => AxisScale {
            ty: ScaleType::Source,
            factor,
        },
        (None, None) => AxisScale::SOURCE_1X,
    }
}

/// Convert the preset parser's scale-type enum to the engine's. Copied from the app.
fn map_scale_type(ty: preset_io::ScaleType) -> ScaleType {
    match ty {
        preset_io::ScaleType::Source => ScaleType::Source,
        preset_io::ScaleType::Viewport => ScaleType::Viewport,
        preset_io::ScaleType::Absolute => ScaleType::Absolute,
    }
}

/// Convert the preset parser's wrap-mode enum to the engine's. Copied from the app.
fn map_wrap_mode(wrap: preset_io::WrapMode) -> WrapMode {
    match wrap {
        preset_io::WrapMode::ClampToBorder => WrapMode::ClampToBorder,
        preset_io::WrapMode::ClampToEdge => WrapMode::ClampToEdge,
        preset_io::WrapMode::Repeat => WrapMode::Repeat,
        preset_io::WrapMode::MirroredRepeat => WrapMode::MirroredRepeat,
    }
}
