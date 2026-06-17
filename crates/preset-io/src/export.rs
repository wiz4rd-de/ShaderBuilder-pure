//! [`core_model::Project`] → RetroArch-conventional `.slangp` **bundle writer**
//! (Architecture §B; the inverse of [`crate::import`]).
//!
//! [`export_preset`] serializes a [`core_model::Project`] into a self-contained,
//! RetroArch-runnable bundle directory:
//!
//! ```text
//! <dest>/
//!   preset.slangp          # the preset, RELATIVE paths + inline parameter defaults
//!   <pass-0>.slang         # per-pass shader sources, byte-exact for imported passes
//!   <pass-1>.slang
//!   …
//!   textures/<LUT>.png     # LUT images, copied in and referenced relatively
//! ```
//!
//! ## Round-trip fidelity (#36)
//!
//! - **Whole-pass sources are written byte-for-byte.** An unmodified imported pass
//!   ([`core_model::PassSource::WholePassCode`]) is written exactly as it was read
//!   on import (the import bridge stores the on-disk bytes verbatim), so an
//!   import → export round trip leaves each `.slang` file byte-identical.
//! - **Paths are relative.** `shaderN` is a bare basename; `textures` entries live
//!   under `textures/`. No absolute path ever appears in `preset.slangp`.
//! - **Parameter defaults are inline.** Every reconciled [`core_model::Parameter`]
//!   whose current `default` differs from the pass `#pragma parameter` initial is
//!   emitted as a bare `id = value` override line (§8). Parameters left at their
//!   pragma default are not re-emitted (RetroArch reads the pragma initial).
//! - **Preserved unknown keys reappear.** The unrecognized keys the parser kept in
//!   [`crate::Preset::extras`] on import (#33) are threaded back in here so a
//!   round trip drops nothing.
//!
//! ## Graph passes
//!
//! A pass authored as a node [`core_model::Graph`] cannot be serialized to slang
//! until the Phase-5 codegen lands, so [`export_preset`] returns
//! [`ExportError::GraphPassUnsupported`] for one. Import-produced projects are
//! entirely whole-pass code, so the export path this issue targets is unaffected.
//!
//! ## Known limitation — per-pass `#include` dependencies are NOT bundled (B5)
//!
//! A pass `.slang` body may `#include` (or `#pragma include_optional`) other files
//! — shared headers, parameter blocks, library helpers. The export bundle writes
//! each whole-pass source byte-for-byte but does **not** copy those included files,
//! nor rewrite the directives, so an include-using preset may fail to load in
//! RetroArch as-is. The full fix — capturing and reproducing the transitive
//! include closure while preserving its relative directory layout — is a **tracked
//! Phase-3 follow-up** (deferred, beyond this issue's scope). Until then the gap is
//! made NON-SILENT: [`export_preset`] scans each pass source for include directives
//! and pushes a clear [`ExportReport::warnings`] entry when any are present, so the
//! caller is told the bundle may be incomplete rather than discovering it at load
//! time.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use core_model::{Lut, Parameter, PassSource, Project, ScaleAxis, ScaleType, WrapMode};

/// The canonical name of the preset file written at the bundle root.
pub const PRESET_FILENAME: &str = "preset.slangp";

/// The subdirectory (relative to the bundle root) LUT images are copied into.
pub const TEXTURES_DIR: &str = "textures";

/// What [`export_preset`] wrote, for the caller to surface. Deterministic order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// Absolute path of the written `preset.slangp`.
    pub preset_path: PathBuf,
    /// Per-pass `.slang` file names written (relative to the bundle root), in
    /// pass order — parallel to [`Project::passes`].
    pub pass_files: Vec<String>,
    /// LUT file names written under `textures/` (relative to the bundle root), in
    /// project order — parallel to [`Project::luts`].
    pub texture_files: Vec<String>,
    /// Non-fatal notes (e.g. a LUT source image that did not exist on disk and so
    /// could not be copied — the relative reference is still emitted).
    pub warnings: Vec<String>,
}

/// Errors writing an export bundle.
#[derive(Debug)]
pub enum ExportError {
    /// An I/O error creating the directory or writing a file.
    Io(std::io::Error),
    /// A pass is authored as a node [`core_model::Graph`]; slang codegen for graph
    /// passes is a later phase, so it cannot be exported yet. Carries the pass id.
    GraphPassUnsupported(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Io(e) => write!(f, "could not write export bundle: {e}"),
            ExportError::GraphPassUnsupported(id) => write!(
                f,
                "pass `{id}` is a node graph; exporting graph passes to slang is not yet \
                 supported (whole-pass / imported passes only)"
            ),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        ExportError::Io(e)
    }
}

/// Serialize `project` into a RetroArch-conventional bundle under `dest_dir`,
/// threading the `extras` (the unknown keys preserved on import, #33) back into
/// the emitted preset so a round trip drops nothing. Pass `&Default::default()`
/// for a hand-built project that has no preserved extras.
///
/// Creates `dest_dir` (and its `textures/` subdir, if there are LUTs) if absent.
/// Returns an [`ExportReport`] describing what was written. Whole-pass sources are
/// written byte-for-byte; all references in `preset.slangp` are **relative**.
pub fn export_preset(
    project: &Project,
    dest_dir: impl AsRef<Path>,
    extras: &BTreeMap<String, String>,
) -> Result<ExportReport, ExportError> {
    let dest_dir = dest_dir.as_ref();
    std::fs::create_dir_all(dest_dir)?;

    let mut report = ExportReport {
        preset_path: dest_dir.join(PRESET_FILENAME),
        ..Default::default()
    };

    // 1. Per-pass `.slang` files with stable, collision-free names.
    let pass_files = pass_file_names(project)?;
    for (pass, file) in project.passes.iter().zip(&pass_files) {
        let PassSource::WholePassCode { source, .. } = &pass.source else {
            // `pass_file_names` already rejected graph passes; unreachable.
            return Err(ExportError::GraphPassUnsupported(pass.id.clone()));
        };
        // Byte-for-byte: write the stored bytes with no normalization so an
        // unmodified imported pass is byte-identical to its original.
        std::fs::write(dest_dir.join(file), source.as_bytes())?;
        // B5 (minimal mitigation): this pass body may `#include` other files that
        // are NOT copied into the bundle. The full fix — capturing and reproducing
        // the transitive include closure with its relative layout — is a tracked
        // Phase-3 follow-up beyond this issue's scope. For now, make the gap
        // NON-SILENT: warn so the exporter never quietly produces a preset that
        // won't load in RetroArch.
        if source_has_include(source) {
            report.warnings.push(format!(
                "pass `{file}`: #include dependencies are not copied into the bundle \
                 (known limitation); the exported preset may not load in RetroArch as-is"
            ));
        }
    }
    report.pass_files = pass_files.clone();

    // 2. LUT PNGs into `textures/`, with collision-free names.
    let texture_files = texture_file_names(&project.luts);
    if !project.luts.is_empty() {
        std::fs::create_dir_all(dest_dir.join(TEXTURES_DIR))?;
    }
    for (lut, file) in project.luts.iter().zip(&texture_files) {
        let rel = format!("{TEXTURES_DIR}/{file}");
        let dst = dest_dir.join(&rel);
        // Copy the source image in if it exists; otherwise still emit the
        // relative reference and note that the bundle is missing the bytes.
        match std::fs::copy(&lut.path, &dst) {
            Ok(_) => {}
            Err(e) => report.warnings.push(format!(
                "LUT `{}`: could not copy source image `{}` into the bundle: {e}",
                lut.name, lut.path
            )),
        }
    }
    report.texture_files = texture_files.clone();

    // 3. The preset text itself, with relative refs + inline defaults + extras.
    let text = render_slangp(project, &pass_files, &texture_files, extras);
    std::fs::write(&report.preset_path, text)?;

    Ok(report)
}

/// Compute a stable, collision-free `.slang` file name per pass, in pass order.
/// Prefers the pass's stored `filename` (the imported `shaderN` basename) so an
/// unmodified imported pass keeps its original name; falls back to a sanitized
/// pass name, then `passN`. Duplicate names get a `_N` suffix so two passes never
/// collide on disk. Returns [`ExportError::GraphPassUnsupported`] for a graph pass.
fn pass_file_names(project: &Project) -> Result<Vec<String>, ExportError> {
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut names = Vec::with_capacity(project.passes.len());
    for (n, pass) in project.passes.iter().enumerate() {
        let PassSource::WholePassCode { filename, .. } = &pass.source else {
            return Err(ExportError::GraphPassUnsupported(pass.id.clone()));
        };
        // Candidate stem (without extension): the imported basename's stem, else
        // a sanitized pass name, else `passN`.
        let stem = filename
            .as_deref()
            .map(|f| {
                Path::new(f)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(f)
            })
            .map(sanitize_stem)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let s = sanitize_stem(&pass.name);
                (!s.is_empty()).then_some(s)
            })
            .unwrap_or_else(|| format!("pass{n}"));
        names.push(unique_name(&format!("{stem}.slang"), &mut used));
    }
    Ok(names)
}

/// Compute a stable, collision-free file name per LUT, in project order. Prefers
/// the LUT source image's basename; falls back to a sanitized LUT name; ensures a
/// `.png` extension; de-duplicates with a `_N` suffix.
fn texture_file_names(luts: &[Lut]) -> Vec<String> {
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut names = Vec::with_capacity(luts.len());
    for lut in luts {
        let raw = Path::new(&lut.path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(sanitize_stem)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| sanitize_stem(&lut.name));
        // Guarantee a file-extension (LUTs are images; default `.png` if the
        // source name had none). The basename is sanitized above.
        let base = if Path::new(&raw).extension().is_some() {
            raw
        } else {
            format!("{raw}.png")
        };
        names.push(unique_name(&base, &mut used));
    }
    names
}

/// Reserve `name` in `used`, appending `_1`, `_2`, … before the extension until
/// it is unique. The reserved name is inserted so later calls avoid it.
fn unique_name(name: &str, used: &mut BTreeSet<String>) -> String {
    if used.insert(name.to_owned()) {
        return name.to_owned();
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|s| s.to_str());
    for i in 1.. {
        let candidate = match ext {
            Some(ext) => format!("{stem}_{i}.{ext}"),
            None => format!("{stem}_{i}"),
        };
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("the `1..` range yields a unique suffix")
}

/// Sanitize a string into a safe file-name stem: keep ASCII alphanumerics, `-`,
/// `_`, `.`; replace any other char (path separators, spaces, …) with `_`. This
/// keeps a generated name from escaping the bundle dir or breaking the parser.
fn sanitize_stem(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Render the full `preset.slangp` text from a project, the resolved relative
/// pass and texture file names, and the preserved `extras`. Canonical RetroArch
/// key ordering (see the function body).
fn render_slangp(
    project: &Project,
    pass_files: &[String],
    texture_files: &[String],
    extras: &BTreeMap<String, String>,
) -> String {
    let mut out = String::new();

    // `shaders` first.
    let _ = writeln!(out, "shaders = {}", project.passes.len());

    // Then, per pass in order: shaderN, then its scale / filter / wrap / mipmap /
    // format / alias / frame_count_mod keys (only those the model actually sets).
    for (n, (pass, file)) in project.passes.iter().zip(pass_files).enumerate() {
        let _ = writeln!(out);
        let _ = writeln!(out, "shader{n} = {file}");
        write_pass_settings(&mut out, n, &pass.settings);
    }

    // feedback_pass (global), if any.
    if let Some(fp) = project.feedback_pass {
        let _ = writeln!(out);
        let _ = writeln!(out, "feedback_pass = {fp}");
    }

    // textures = "A;B" plus each LUT's path + sampler sub-keys.
    if !project.luts.is_empty() {
        let _ = writeln!(out);
        let names: Vec<&str> = project.luts.iter().map(|l| l.name.as_str()).collect();
        let _ = writeln!(out, "textures = \"{}\"", names.join(";"));
        for (lut, file) in project.luts.iter().zip(texture_files) {
            write_lut(&mut out, lut, &format!("{TEXTURES_DIR}/{file}"));
        }
    }

    // Parameter defaults inline: only those whose value differs from the pass
    // `#pragma parameter` initial (see [`changed_parameters`]).
    let changed = changed_parameters(project);
    if !changed.is_empty() {
        let _ = writeln!(out);
        // Informational `parameters = "..."` list (all reconciled ids, in order).
        let ids: Vec<&str> = project.parameters.iter().map(|p| p.name.as_str()).collect();
        let _ = writeln!(out, "parameters = \"{}\"", ids.join(";"));
        for p in &changed {
            let _ = writeln!(out, "{} = {}", p.name, fmt_f32(p.default));
        }
    }

    // Preserved unknown keys (#33) re-emitted verbatim so nothing is dropped.
    if !extras.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "# preserved unrecognized keys (round-tripped on import)"
        );
        for (key, value) in extras {
            let _ = writeln!(out, "{key} = {value}");
        }
    }

    out
}

/// Write a pass's per-pass settings keys in canonical order: scale, then
/// filter_linear, wrap_mode, mipmap_input, the format keys, alias, frame_count_mod.
/// Only keys the model actually set (`Some`) are emitted, so an unset key stays
/// absent and RetroArch applies its position-dependent default.
fn write_pass_settings(out: &mut String, n: usize, s: &core_model::PassSettings) {
    write_scale(out, n, &s.scale_x, &s.scale_y);

    if let Some(v) = s.filter_linear {
        let _ = writeln!(out, "filter_linear{n} = {v}");
    }
    if let Some(w) = s.wrap_mode {
        let _ = writeln!(out, "wrap_mode{n} = {}", wrap_mode_str(w));
    }
    if let Some(v) = s.mipmap_input {
        let _ = writeln!(out, "mipmap_input{n} = {v}");
    }
    if let Some(v) = s.float_framebuffer {
        let _ = writeln!(out, "float_framebuffer{n} = {v}");
    }
    if let Some(v) = s.srgb_framebuffer {
        let _ = writeln!(out, "srgb_framebuffer{n} = {v}");
    }
    if let Some(alias) = &s.alias {
        let _ = writeln!(out, "alias{n} = {alias}");
    }
    if let Some(v) = s.frame_count_mod {
        let _ = writeln!(out, "frame_count_mod{n} = {v}");
    }
}

/// Write a pass's scale keys. When both axes carry the **same** type and factor,
/// the combined `scale_typeN`/`scaleN` form is emitted (matching how most presets
/// are authored); otherwise the per-axis `_x`/`_y` forms are emitted. An axis with
/// neither key is left absent (position-dependent default applies).
fn write_scale(out: &mut String, n: usize, x: &ScaleAxis, y: &ScaleAxis) {
    if x == &ScaleAxis::default() && y == &ScaleAxis::default() {
        return; // no scale keys -> RetroArch position default.
    }
    if x == y {
        if let Some(ty) = x.scale_type {
            let _ = writeln!(out, "scale_type{n} = {}", scale_type_str(ty));
        }
        if let Some(factor) = x.scale {
            let _ = writeln!(out, "scale{n} = {}", fmt_scale(x.scale_type, factor));
        }
        return;
    }
    write_axis(out, "x", n, x);
    write_axis(out, "y", n, y);
}

/// Write one per-axis scale (`scale_type_{axis}N` / `scale_{axis}N`).
fn write_axis(out: &mut String, axis: &str, n: usize, a: &ScaleAxis) {
    if let Some(ty) = a.scale_type {
        let _ = writeln!(out, "scale_type_{axis}{n} = {}", scale_type_str(ty));
    }
    if let Some(factor) = a.scale {
        let _ = writeln!(out, "scale_{axis}{n} = {}", fmt_scale(a.scale_type, factor));
    }
}

/// Write a LUT's `<NAME> = textures/<file>` line plus its `_linear`/`_wrap_mode`/
/// `_mipmap` sub-keys (only those the model set).
fn write_lut(out: &mut String, lut: &Lut, rel_path: &str) {
    let _ = writeln!(out, "{} = {rel_path}", lut.name);
    if let Some(v) = lut.filter_linear {
        let _ = writeln!(out, "{}_linear = {v}", lut.name);
    }
    if let Some(w) = lut.wrap_mode {
        let _ = writeln!(out, "{}_wrap_mode = {}", lut.name, wrap_mode_str(w));
    }
    if let Some(v) = lut.mipmap {
        let _ = writeln!(out, "{}_mipmap = {v}", lut.name);
    }
}

/// The project parameters whose current `default` **differs** from the pass
/// `#pragma parameter` initial, so only genuine overrides are emitted inline (§8).
/// The pragma initial is taken from the first pass declaration of that id (the
/// reconciliation canonical), bitwise-compared so a no-op override is omitted.
fn changed_parameters(project: &Project) -> Vec<&Parameter> {
    // First-seen pragma initial per id, across passes (matches reconciliation).
    let mut pragma_initial: BTreeMap<&str, f32> = BTreeMap::new();
    for pass in &project.passes {
        if let PassSource::WholePassCode { .. } = &pass.source {
            for p in &pass.parameters {
                pragma_initial.entry(p.name.as_str()).or_insert(p.default);
            }
        }
    }
    project
        .parameters
        .iter()
        .filter(|p| match pragma_initial.get(p.name.as_str()) {
            // Differs from the pragma initial -> emit the override.
            Some(initial) => initial.to_bits() != p.default.to_bits(),
            // No pragma declared it (hand-built / orphan): emit so the value is
            // not lost on re-import.
            None => true,
        })
        .collect()
}

/// RetroArch `.slangp` string for a [`ScaleType`] (matches the parser's accepted
/// strings, so export → import round-trips).
fn scale_type_str(ty: ScaleType) -> &'static str {
    match ty {
        ScaleType::Source => "source",
        ScaleType::Viewport => "viewport",
        ScaleType::Absolute => "absolute",
    }
}

/// RetroArch `.slangp` string for a [`WrapMode`] (the snake_case parser strings).
fn wrap_mode_str(w: WrapMode) -> &'static str {
    match w {
        WrapMode::ClampToBorder => "clamp_to_border",
        WrapMode::ClampToEdge => "clamp_to_edge",
        WrapMode::Repeat => "repeat",
        WrapMode::MirroredRepeat => "mirrored_repeat",
    }
}

/// Whether a whole-pass source carries any `#include` / `#pragma include_optional`
/// directive (B5). A textual line scan — not a preprocessor — matching a directive
/// at the start of a line (after leading whitespace). Used only to emit a
/// non-silent export warning, since the export bundle does NOT copy the included
/// files (a tracked Phase-3 follow-up; see the module docs).
fn source_has_include(source: &str) -> bool {
    source.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with("#include")
            || t.starts_with("#pragma include")
            || t.starts_with("#pragma include_optional")
    })
}

/// Format a scale factor: an `absolute` factor is a literal integer pixel count
/// (§2), so it is written without a fractional part; every other type writes the
/// float via [`fmt_f32`].
///
/// The `absolute` factor is **truncated** (not rounded) to match RetroArch, which
/// parses it with `config_get_int` (truncation) — so a hand-built `319.6` exports
/// as `319`, agreeing with the import-side truncation (B2). Import already stores
/// absolute factors truncated, so for an imported project this is a no-op; the
/// `trunc` here keeps a directly-constructed project conformant too.
fn fmt_scale(ty: Option<ScaleType>, factor: f32) -> String {
    if ty == Some(ScaleType::Absolute) {
        format!("{}", factor.trunc() as i64)
    } else {
        fmt_f32(factor)
    }
}

/// Format an `f32` for a `.slangp` value: a whole number gets a trailing `.0`
/// (so `2` reads as `2.0`, the float form RetroArch and our parser accept), and a
/// fractional value uses its shortest round-trippable form. Avoids exponent and
/// trailing-zero noise so re-import parses back to the same value.
fn fmt_f32(v: f32) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 1e15 {
        format!("{:.1}", v)
    } else {
        // `{}` on f32 prints the shortest decimal that round-trips.
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{Pass, PassSettings, PipelineMetadata};

    fn wpc(source: &str, filename: &str) -> PassSource {
        PassSource::WholePassCode {
            source: source.to_owned(),
            filename: Some(filename.to_owned()),
            opaque: true,
        }
    }

    /// A two-pass project with a LUT, params, settings, and feedback — the export
    /// shape the acceptance criteria exercise. Shader sources are arbitrary bytes.
    fn sample_project() -> Project {
        Project {
            schema_version: core_model::PROJECT_SCHEMA_VERSION,
            name: "demo".to_owned(),
            feedback_pass: Some(1),
            pipeline: PipelineMetadata::default(),
            parameters: vec![
                Parameter {
                    name: "BRIGHT".to_owned(),
                    label: "Brightness".to_owned(),
                    default: 1.5, // overridden (pragma initial 1.0)
                    min: 0.0,
                    max: 2.0,
                    step: 0.01,
                },
                Parameter {
                    name: "CONTRAST".to_owned(),
                    label: "Contrast".to_owned(),
                    default: 1.0, // unchanged from pragma initial -> not emitted
                    min: 0.5,
                    max: 1.5,
                    step: 0.0,
                },
            ],
            luts: vec![Lut {
                name: "BORDER".to_owned(),
                path: "/abs/luts/border.png".to_owned(),
                filter_linear: Some(true),
                wrap_mode: Some(WrapMode::ClampToEdge),
                mipmap: Some(false),
            }],
            passes: vec![
                Pass {
                    id: "pass-0".to_owned(),
                    name: "First".to_owned(),
                    source: wpc(
                        "#version 450\n#pragma parameter BRIGHT \"Brightness\" 1.0 0.0 2.0 0.01\n",
                        "first.slang",
                    ),
                    parameters: vec![Parameter {
                        name: "BRIGHT".to_owned(),
                        label: "Brightness".to_owned(),
                        default: 1.0,
                        min: 0.0,
                        max: 2.0,
                        step: 0.01,
                    }],
                    settings: PassSettings {
                        scale_x: ScaleAxis {
                            scale_type: Some(ScaleType::Source),
                            scale: Some(2.0),
                        },
                        scale_y: ScaleAxis {
                            scale_type: Some(ScaleType::Source),
                            scale: Some(2.0),
                        },
                        filter_linear: Some(false),
                        srgb_framebuffer: Some(true),
                        alias: Some("FIRST".to_owned()),
                        ..Default::default()
                    },
                    references: vec![],
                },
                Pass {
                    id: "pass-1".to_owned(),
                    name: "Second".to_owned(),
                    source: wpc(
                        "#version 450\n#pragma parameter CONTRAST \"Contrast\" 1.0 0.5 1.5\n",
                        "second.slang",
                    ),
                    parameters: vec![Parameter {
                        name: "CONTRAST".to_owned(),
                        label: "Contrast".to_owned(),
                        default: 1.0,
                        min: 0.5,
                        max: 1.5,
                        step: 0.0,
                    }],
                    settings: PassSettings {
                        scale_x: ScaleAxis {
                            scale_type: Some(ScaleType::Absolute),
                            scale: Some(320.0),
                        },
                        scale_y: ScaleAxis {
                            scale_type: Some(ScaleType::Viewport),
                            scale: Some(1.0),
                        },
                        frame_count_mod: Some(60),
                        ..Default::default()
                    },
                    references: vec![],
                },
            ],
            // Document metadata + library refs (#38) play no part in export — the
            // bundle writer ignores them — but the literal must be complete.
            metadata: core_model::ProjectMetadata::default(),
            library_refs: Vec::new(),
        }
    }

    #[test]
    fn writes_expected_directory_structure() {
        let dir = tempfile::tempdir().unwrap();
        // A real source PNG so the LUT copy succeeds.
        std::fs::create_dir_all("/abs/luts").ok(); // best-effort; ignored if denied
        let project = {
            let mut p = sample_project();
            // Point the LUT at a real temp file so the copy works without /abs.
            let lut_src = dir.path().join("src_border.png");
            std::fs::write(&lut_src, b"\x89PNG\r\n").unwrap();
            p.luts[0].path = lut_src.to_string_lossy().into_owned();
            p
        };

        let out = dir.path().join("bundle");
        let report = export_preset(&project, &out, &BTreeMap::new()).expect("exports");

        assert!(out.join("preset.slangp").is_file(), "preset written");
        assert_eq!(report.pass_files, vec!["first.slang", "second.slang"]);
        for f in &report.pass_files {
            assert!(out.join(f).is_file(), "pass file {f} written");
        }
        assert_eq!(report.texture_files, vec!["src_border.png"]);
        assert!(
            out.join("textures/src_border.png").is_file(),
            "LUT copied into textures/"
        );
        assert!(
            report.warnings.is_empty(),
            "no warnings: {:?}",
            report.warnings
        );
    }

    #[test]
    fn no_absolute_paths_in_preset() {
        let dir = tempfile::tempdir().unwrap();
        let project = {
            let mut p = sample_project();
            let lut_src = dir.path().join("src_border.png");
            std::fs::write(&lut_src, b"\x89PNG\r\n").unwrap();
            p.luts[0].path = lut_src.to_string_lossy().into_owned();
            p
        };
        let out = dir.path().join("bundle");
        export_preset(&project, &out, &BTreeMap::new()).expect("exports");

        let text = std::fs::read_to_string(out.join("preset.slangp")).unwrap();
        // No line carries an absolute path: neither the temp source path nor a
        // leading-slash token anywhere.
        let src_path = dir.path().to_string_lossy().into_owned();
        assert!(
            !text.contains(&src_path),
            "preset must not leak the source path:\n{text}"
        );
        for line in text.lines() {
            let value = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
            assert!(
                !value.starts_with('/'),
                "value is an absolute path: {line:?}"
            );
        }
        // The shaderN + texture refs are relative.
        assert!(text.contains("shader0 = first.slang"));
        assert!(text.contains("shader1 = second.slang"));
        assert!(text.contains("BORDER = textures/src_border.png"));
    }

    #[test]
    fn emits_settings_in_canonical_order() {
        let dir = tempfile::tempdir().unwrap();
        let project = sample_project();
        let out = dir.path().join("bundle");
        // LUT copy will warn (no /abs file) but the preset still writes.
        export_preset(&project, &out, &BTreeMap::new()).expect("exports");
        let text = std::fs::read_to_string(out.join("preset.slangp")).unwrap();

        assert!(text.starts_with("shaders = 2\n"), "shaders first:\n{text}");
        // Pass 0 combined scale (both axes equal).
        assert!(text.contains("scale_type0 = source"));
        assert!(text.contains("scale0 = 2.0"));
        assert!(text.contains("filter_linear0 = false"));
        assert!(text.contains("srgb_framebuffer0 = true"));
        assert!(text.contains("alias0 = FIRST"));
        // Pass 1 per-axis scale (axes differ); absolute is an integer.
        assert!(text.contains("scale_type_x1 = absolute"));
        assert!(text.contains("scale_x1 = 320"));
        assert!(!text.contains("scale_x1 = 320.0"), "absolute is integer");
        assert!(text.contains("scale_type_y1 = viewport"));
        assert!(text.contains("scale_y1 = 1.0"));
        assert!(text.contains("frame_count_mod1 = 60"));
        // Globals + LUT.
        assert!(text.contains("feedback_pass = 1"));
        assert!(text.contains("textures = \"BORDER\""));
        assert!(text.contains("BORDER_linear = true"));
        assert!(text.contains("BORDER_wrap_mode = clamp_to_edge"));
        assert!(text.contains("BORDER_mipmap = false"));
    }

    #[test]
    fn only_changed_parameters_are_emitted_inline() {
        let dir = tempfile::tempdir().unwrap();
        let project = sample_project();
        let out = dir.path().join("bundle");
        export_preset(&project, &out, &BTreeMap::new()).expect("exports");
        let text = std::fs::read_to_string(out.join("preset.slangp")).unwrap();

        // BRIGHT was changed (1.5 vs pragma 1.0) -> emitted; CONTRAST unchanged.
        assert!(
            text.contains("BRIGHT = 1.5"),
            "changed param inline:\n{text}"
        );
        assert!(
            !text
                .lines()
                .any(|l| l.trim_start().starts_with("CONTRAST =")),
            "unchanged param must NOT be emitted:\n{text}"
        );
        // The informational list still names every reconciled id.
        assert!(text.contains("parameters = \"BRIGHT;CONTRAST\""));
    }

    #[test]
    fn preserved_extras_reappear() {
        let dir = tempfile::tempdir().unwrap();
        let project = sample_project();
        let out = dir.path().join("bundle");
        let extras = BTreeMap::from([
            ("vendor_flag".to_owned(), "on".to_owned()),
            ("some_future_key".to_owned(), "some_string".to_owned()),
        ]);
        export_preset(&project, &out, &extras).expect("exports");
        let text = std::fs::read_to_string(out.join("preset.slangp")).unwrap();
        assert!(text.contains("vendor_flag = on"));
        assert!(text.contains("some_future_key = some_string"));
    }

    #[test]
    fn whole_pass_source_is_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = sample_project();
        // Gnarly bytes: BOM, CRLF, trailing spaces, no final newline.
        let raw = "\u{feff}#version 450\r\n#pragma stage fragment   \r\nvoid main(){}";
        project.passes[0].source = wpc(raw, "first.slang");
        let out = dir.path().join("bundle");
        export_preset(&project, &out, &BTreeMap::new()).expect("exports");

        let on_disk = std::fs::read(out.join("first.slang")).unwrap();
        assert_eq!(
            on_disk.as_slice(),
            raw.as_bytes(),
            "pass source must be written byte-for-byte"
        );
    }

    #[test]
    fn include_in_pass_source_yields_export_warning() {
        // B5: the export bundle does NOT copy a pass's #include dependencies, so an
        // include-using preset may not load in RetroArch as-is. The gap must be
        // NON-SILENT: a clear ExportReport warning naming the pass file fires, while
        // a pass with no includes produces no such warning.
        let dir = tempfile::tempdir().unwrap();
        let mut project = sample_project();
        project.passes[0].source = wpc(
            "#version 450\n#include \"common.inc\"\nvoid main(){}\n",
            "first.slang",
        );
        // Also exercise the #pragma include_optional spelling.
        project.passes[1].source = wpc(
            "#version 450\n  #pragma include_optional \"opt.inc\"\nvoid main(){}\n",
            "second.slang",
        );
        let out = dir.path().join("bundle");
        let report = export_preset(&project, &out, &BTreeMap::new()).expect("exports");

        let include_warnings: Vec<&String> = report
            .warnings
            .iter()
            .filter(|w| w.contains("#include dependencies are not copied"))
            .collect();
        assert_eq!(
            include_warnings.len(),
            2,
            "both include-using passes warn: {:?}",
            report.warnings
        );
        assert!(
            include_warnings.iter().any(|w| w.contains("first.slang")),
            "warning names the pass file: {:?}",
            report.warnings
        );
        assert!(
            include_warnings.iter().any(|w| w.contains("second.slang")),
            "include_optional also warns: {:?}",
            report.warnings
        );
    }

    #[test]
    fn no_include_yields_no_include_warning() {
        // A pass with no #include must NOT trip the B5 warning (no false positive).
        let dir = tempfile::tempdir().unwrap();
        let mut project = sample_project();
        project.passes[0].source = wpc("#version 450\nvoid main(){}\n", "first.slang");
        project.passes[1].source = wpc("#version 450\nvoid main(){}\n", "second.slang");
        let out = dir.path().join("bundle");
        let report = export_preset(&project, &out, &BTreeMap::new()).expect("exports");
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.contains("#include dependencies are not copied")),
            "no include -> no include warning: {:?}",
            report.warnings
        );
    }

    #[test]
    fn graph_pass_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = sample_project();
        project.passes[0].source = PassSource::Graph {
            graph: core_model::Graph::default(),
        };
        let out = dir.path().join("bundle");
        let err = export_preset(&project, &out, &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, ExportError::GraphPassUnsupported(id) if id == "pass-0"));
    }

    #[test]
    fn collision_free_pass_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = sample_project();
        // Both passes claim the same filename.
        project.passes[0].source = wpc("a", "dup.slang");
        project.passes[1].source = wpc("b", "dup.slang");
        let out = dir.path().join("bundle");
        let report = export_preset(&project, &out, &BTreeMap::new()).expect("exports");
        assert_eq!(report.pass_files, vec!["dup.slang", "dup_1.slang"]);
        assert!(out.join("dup.slang").is_file());
        assert!(out.join("dup_1.slang").is_file());
    }

    #[test]
    fn fmt_f32_round_trips_cleanly() {
        assert_eq!(fmt_f32(2.0), "2.0");
        assert_eq!(fmt_f32(1.0), "1.0");
        assert_eq!(fmt_f32(0.5), "0.5");
        assert_eq!(fmt_f32(0.01), "0.01");
        assert_eq!(fmt_f32(2.4), "2.4");
        assert_eq!(fmt_scale(Some(ScaleType::Absolute), 320.0), "320");
        assert_eq!(fmt_scale(Some(ScaleType::Source), 2.0), "2.0");
        // B2: a fractional `absolute` factor is TRUNCATED (not rounded), matching
        // RetroArch's config_get_int. `319.6 -> 319`, never `320`.
        assert_eq!(fmt_scale(Some(ScaleType::Absolute), 319.6), "319");
        assert_eq!(fmt_scale(Some(ScaleType::Absolute), 12.9), "12");
    }

    #[test]
    fn fractional_absolute_scale_round_trips_truncated() {
        // B2 end-to-end: a fractional `absolute` factor survives import -> export
        // -> re-import as the conformant truncated integer (a fixed point), never
        // drifting upward (319.6 -> 320) as `round()` on export would have done.
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.slang"), "void main(){}\n").unwrap();
        std::fs::write(
            src.path().join("p.slangp"),
            "shaders = 1\n\
             shader0 = a.slang\n\
             scale_type0 = absolute\n\
             scale0 = 319.6\n",
        )
        .unwrap();

        let (project, _) = crate::import_preset(src.path().join("p.slangp")).expect("import");
        // Import already truncated the model value (no upward round).
        assert_eq!(project.passes[0].settings.scale_x.scale, Some(319.0));

        let out = tempfile::tempdir().unwrap();
        export_preset(&project, out.path(), &BTreeMap::new()).expect("export");
        let text = std::fs::read_to_string(out.path().join(PRESET_FILENAME)).unwrap();
        assert!(
            text.contains("scale0 = 319"),
            "exports truncated int:\n{text}"
        );
        assert!(!text.contains("320"), "must not round up to 320:\n{text}");

        let (reimported, _) =
            crate::import_preset(out.path().join(PRESET_FILENAME)).expect("re-import");
        assert_eq!(
            reimported.passes[0].settings.scale_x.scale,
            Some(319.0),
            "absolute factor is a fixed point across the round trip"
        );
    }

    // ---- import → export → re-import round-trip fidelity (#36) ---------------

    /// Write a small but representative multi-pass preset (scale, alias, format,
    /// feedback, a LUT, params + an override, and an unknown key) to a temp dir,
    /// returning the `.slangp` path. The two pass `.slang` files carry gnarly
    /// bytes (BOM, CRLF, trailing whitespace, no final newline) to prove the
    /// byte-exact contract survives the round trip.
    fn write_source_preset(dir: &Path) -> PathBuf {
        let pass0 = "\u{feff}#version 450\r\n\
             #pragma parameter BRIGHT \"Brightness\" 1.0 0.0 2.0 0.01   \r\n\
             void main(){}";
        let pass1 = "#version 450\n#pragma stage fragment\nvoid main(){}\n";
        std::fs::write(dir.join("first.slang"), pass0).unwrap();
        std::fs::write(dir.join("second.slang"), pass1).unwrap();
        std::fs::write(dir.join("border.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        let slangp = dir.join("crt.slangp");
        std::fs::write(
            &slangp,
            "shaders = 2\n\
             feedback_pass = 1\n\
             shader0 = first.slang\n\
             alias0 = FIRST\n\
             scale_type0 = source\n\
             scale0 = 2.0\n\
             filter_linear0 = false\n\
             srgb_framebuffer0 = true\n\
             shader1 = second.slang\n\
             scale_type_x1 = absolute\n\
             scale_x1 = 320\n\
             scale_type_y1 = viewport\n\
             scale_y1 = 1.0\n\
             frame_count_mod1 = 60\n\
             textures = BORDER\n\
             BORDER = border.png\n\
             BORDER_linear = true\n\
             BORDER_wrap_mode = clamp_to_edge\n\
             parameters = \"BRIGHT\"\n\
             BRIGHT = 1.5\n\
             vendor_unknown = some_value\n",
        )
        .unwrap();
        slangp
    }

    #[test]
    fn round_trip_preserves_passes_settings_luts_and_params() {
        let src = tempfile::tempdir().unwrap();
        let slangp = write_source_preset(src.path());

        // Import the source preset.
        let preset = crate::parse_slangp(&slangp).expect("parses");
        let (project, _) = crate::import_parsed_preset(&preset, "crt");

        // Export it to a fresh bundle dir, threading the preserved extras back in.
        let out = tempfile::tempdir().unwrap();
        let report = export_preset(&project, out.path(), &preset.extras).expect("export succeeds");
        assert!(
            report.warnings.is_empty(),
            "no warnings: {:?}",
            report.warnings
        );

        // Re-import the EXPORTED bundle and compare the salient model fields.
        let (reimported, _) =
            crate::import_preset(out.path().join(PRESET_FILENAME)).expect("re-import");

        assert_eq!(reimported.passes.len(), project.passes.len());
        assert_eq!(reimported.feedback_pass, project.feedback_pass);

        // Per-pass settings survive.
        for (a, b) in project.passes.iter().zip(&reimported.passes) {
            assert_eq!(a.settings, b.settings, "pass settings round-trip");
        }

        // The reconciled parameter (with its preset override applied) survives.
        let bright = reimported
            .parameters
            .iter()
            .find(|p| p.name == "BRIGHT")
            .expect("BRIGHT present after round trip");
        assert_eq!(bright.default, 1.5, "override survives round trip");
        assert_eq!(bright.min, 0.0);
        assert_eq!(bright.max, 2.0);
        assert_eq!(bright.step, 0.01);

        // The LUT survives with its sampler settings + a path under textures/.
        let lut = &reimported.luts[0];
        assert_eq!(lut.name, "BORDER");
        assert_eq!(lut.filter_linear, Some(true));
        assert_eq!(lut.wrap_mode, Some(WrapMode::ClampToEdge));
        assert!(
            lut.path.ends_with("textures/border.png"),
            "LUT path is under textures/: {}",
            lut.path
        );

        // The unknown key was re-emitted, so the re-import preserves it again.
        let (_, diags) = crate::import_preset(out.path().join(PRESET_FILENAME)).unwrap();
        assert!(
            diags
                .diagnostics
                .iter()
                .any(|d| d.message.contains("vendor_unknown")),
            "preserved extra reappears on re-import: {:?}",
            diags.diagnostics
        );
    }

    #[test]
    fn round_trip_pass_slang_files_are_byte_identical() {
        // ACCEPTANCE (#36): an unmodified imported pass's `.slang` is byte-identical
        // to its original after export.
        let src = tempfile::tempdir().unwrap();
        let slangp = write_source_preset(src.path());

        let (project, _) = crate::import_preset(&slangp).expect("import");
        let out = tempfile::tempdir().unwrap();
        let report = export_preset(&project, out.path(), &BTreeMap::new()).expect("export");

        for (n, file) in report.pass_files.iter().enumerate() {
            let original_name = if n == 0 {
                "first.slang"
            } else {
                "second.slang"
            };
            let original = std::fs::read(src.path().join(original_name)).unwrap();
            let exported = std::fs::read(out.path().join(file)).unwrap();
            assert_eq!(
                exported, original,
                "pass {n} ({file}) must be byte-identical to its original"
            );
        }
    }
}
