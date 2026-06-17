//! `.slangp` → [`core_model::Project`] import bridge (Architecture §A/§B).
//!
//! [`import_preset`] reads a `.slangp`, parses it with the tolerant tokenizer in
//! [`crate::slangp`], and lowers the parsed [`Preset`] into the canonical
//! [`core_model::Project`] the rest of the app edits and serializes. It carries
//! **all** RetroArch per-pass + preset-level settings onto the model and returns
//! [`ImportDiagnostics`] describing anything noteworthy (preserved unknown keys,
//! shader files that could not be read).
//!
//! ## Scale precedence (`docs/retroarch-slang-runtime.md` §2)
//!
//! The preset may set scale combined (`scale_typeN`/`scaleN`, both axes) or
//! per-axis (`scale_type_xN`/`scale_xN` and `_y`). The per-axis key wins over the
//! combined key **for its axis**, and a combined key applies to whichever axis
//! has no per-axis override. This bridge resolves that precedence here — via
//! [`Pass::scale_type_x`]/[`Pass::scale_factor_x`] (and `_y`) — and stores the
//! already-effective per-axis values in [`core_model::ScaleAxis`], so the model
//! never carries the raw combined/per-axis ambiguity. A pass that declares **no**
//! scale key at all maps to a default [`core_model::ScaleAxis`] (both fields
//! `None`) so the engine applies the position-dependent §2 default.

use std::path::Path;

use crate::slangp::{Pass, Preset, ScaleType, WrapMode};

/// The severity of an [`ImportDiagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Informational — the import succeeded but something is worth surfacing
    /// (e.g. an unrecognized preset key was preserved verbatim).
    Info,
    /// A recoverable problem — the import produced a usable model but degraded
    /// (e.g. a `shaderN` file could not be read, so its source is empty).
    Warning,
}

/// One diagnostic emitted while importing a preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDiagnostic {
    /// How serious this is.
    pub severity: Severity,
    /// Human-readable description.
    pub message: String,
}

impl ImportDiagnostic {
    fn info(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            message: message.into(),
        }
    }

    fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
        }
    }
}

/// Everything the import wants to tell the caller without failing. Empty on a
/// clean import of a fully-recognized, fully-readable preset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportDiagnostics {
    /// The diagnostics, in deterministic order (extras first — sorted by key via
    /// the parser's `BTreeMap` — then any per-pass shader-read warnings).
    pub diagnostics: Vec<ImportDiagnostic>,
}

impl ImportDiagnostics {
    /// Whether any diagnostic is a [`Severity::Warning`].
    pub fn has_warnings(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning)
    }

    /// Whether there are no diagnostics at all.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Read and import a `.slangp` into a [`core_model::Project`] plus
/// [`ImportDiagnostics`]. Parse errors (missing `shaders`, bad scale type, …)
/// surface as the [`crate::ParseError`]; everything tolerable (unknown keys,
/// unreadable shader files) becomes a diagnostic rather than an error.
///
/// The project name defaults to the preset file's stem.
pub fn import_preset(
    path: impl AsRef<Path>,
) -> Result<(core_model::Project, ImportDiagnostics), crate::ParseError> {
    let path = path.as_ref();
    let preset = crate::parse_slangp(path)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("imported")
        .to_owned();
    Ok(import_parsed_preset(&preset, &name))
}

/// Lower an already-parsed [`Preset`] into a [`core_model::Project`]. Split from
/// [`import_preset`] so the mapping is unit-testable without touching disk:
/// shader sources are read here (a read failure → empty source + a warning).
pub fn import_parsed_preset(
    preset: &Preset,
    project_name: &str,
) -> (core_model::Project, ImportDiagnostics) {
    let mut diagnostics = Vec::new();

    // Surface every preserved unknown key as an info diagnostic, so nothing the
    // parser kept in `extras` is invisible to the user (issue #33 contract).
    for (key, value) in &preset.extras {
        diagnostics.push(ImportDiagnostic::info(format!(
            "unrecognized preset key preserved: `{key} = {value}`"
        )));
    }

    let mut passes = Vec::with_capacity(preset.passes.len());
    for (n, pass) in preset.passes.iter().enumerate() {
        let source = match std::fs::read_to_string(&pass.shader) {
            Ok(text) => text,
            Err(e) => {
                diagnostics.push(ImportDiagnostic::warning(format!(
                    "pass {n}: could not read shader `{}`: {e}; using empty source",
                    pass.shader.display()
                )));
                String::new()
            }
        };

        let name = pass.alias.clone().unwrap_or_else(|| format!("Pass {n}"));

        passes.push(core_model::Pass {
            id: format!("pass-{n}"),
            name,
            source: core_model::PassSource::WholePassCode { source },
            parameters: Vec::new(),
            settings: map_settings(pass),
        });
    }

    let feedback_pass = map_feedback_pass(preset.feedback_pass);

    let project = core_model::Project {
        schema_version: core_model::PROJECT_SCHEMA_VERSION,
        name: project_name.to_owned(),
        passes,
        feedback_pass,
    };
    (project, ImportDiagnostics { diagnostics })
}

/// Map a parsed [`Pass`]'s RetroArch keys into a [`core_model::PassSettings`],
/// resolving the combined-vs-per-axis scale precedence (see the module docs).
fn map_settings(pass: &Pass) -> core_model::PassSettings {
    core_model::PassSettings {
        scale_x: core_model::ScaleAxis {
            scale_type: pass.scale_type_x().map(map_scale_type),
            scale: pass.scale_factor_x(),
        },
        scale_y: core_model::ScaleAxis {
            scale_type: pass.scale_type_y().map(map_scale_type),
            scale: pass.scale_factor_y(),
        },
        filter_linear: pass.filter_linear,
        wrap_mode: pass.wrap_mode.map(map_wrap_mode),
        mipmap_input: pass.mipmap_input,
        float_framebuffer: pass.float_framebuffer,
        srgb_framebuffer: pass.srgb_framebuffer,
        alias: pass.alias.clone(),
        frame_count_mod: pass.frame_count_mod,
    }
}

/// Map the parser's `feedback_pass` (RetroArch `int`, default `-1` = none) to the
/// model's `Option<u32>`: a negative value (or absent) → `None`.
fn map_feedback_pass(raw: Option<i32>) -> Option<u32> {
    match raw {
        Some(n) if n >= 0 => Some(n as u32),
        _ => None,
    }
}

/// Convert the parser's [`ScaleType`] to the model's.
fn map_scale_type(ty: ScaleType) -> core_model::ScaleType {
    match ty {
        ScaleType::Source => core_model::ScaleType::Source,
        ScaleType::Viewport => core_model::ScaleType::Viewport,
        ScaleType::Absolute => core_model::ScaleType::Absolute,
    }
}

/// Convert the parser's [`WrapMode`] to the model's.
fn map_wrap_mode(wrap: WrapMode) -> core_model::WrapMode {
    match wrap {
        WrapMode::ClampToBorder => core_model::WrapMode::ClampToBorder,
        WrapMode::ClampToEdge => core_model::WrapMode::ClampToEdge,
        WrapMode::Repeat => core_model::WrapMode::Repeat,
        WrapMode::MirroredRepeat => core_model::WrapMode::MirroredRepeat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_slangp_str;
    use core_model::{PassSource, ScaleType as MScale, WrapMode as MWrap};
    use std::path::Path;

    // A hand-written multi-pass preset exercising both scale forms (combined +
    // per-axis), both scale_type forms, format/sampler keys, aliases, feedback,
    // and an unrecognized key. Shader files don't exist on disk, so the import
    // emits read warnings and empty sources — the settings mapping is what we
    // assert here.
    const FIXTURE: &str = r#"
shaders = 3
feedback_pass = 1

# pass 0: combined scale + format/sampler + alias
shader0 = a.slang
alias0 = MAIN
scale_type0 = source
scale0 = 2.0
filter_linear0 = true
srgb_framebuffer0 = true
mipmap_input0 = true

# pass 1: per-axis scale (different type/factor per axis)
shader1 = b.slang
scale_type_x1 = absolute
scale_x1 = 320
scale_type_y1 = viewport
scale_y1 = 1.0
float_framebuffer1 = true
wrap_mode1 = repeat
frame_count_mod1 = 60

# pass 2: no scale keys -> position default (None/None), final pass
shader2 = c.slang

# an unrecognized key the parser must preserve + import must surface
mystery_setting = banana
"#;

    fn import_fixture() -> (core_model::Project, ImportDiagnostics) {
        let preset = parse_slangp_str(FIXTURE, Path::new("/presets")).expect("fixture parses");
        import_parsed_preset(&preset, "fixture")
    }

    #[test]
    fn pass_count_and_feedback_match_the_file() {
        let (project, _) = import_fixture();
        assert_eq!(project.schema_version, core_model::PROJECT_SCHEMA_VERSION);
        assert_eq!(project.name, "fixture");
        assert_eq!(project.passes.len(), 3, "three passes");
        assert_eq!(project.feedback_pass, Some(1));
    }

    #[test]
    fn combined_scale_applies_to_both_axes() {
        let (project, _) = import_fixture();
        let s = &project.passes[0].settings;
        assert_eq!(s.scale_x.scale_type, Some(MScale::Source));
        assert_eq!(s.scale_x.scale, Some(2.0));
        assert_eq!(s.scale_y.scale_type, Some(MScale::Source));
        assert_eq!(s.scale_y.scale, Some(2.0));
    }

    #[test]
    fn per_axis_scale_overrides_resolve() {
        let (project, _) = import_fixture();
        let s = &project.passes[1].settings;
        // Different type AND factor per axis — the per-axis keys win.
        assert_eq!(s.scale_x.scale_type, Some(MScale::Absolute));
        assert_eq!(s.scale_x.scale, Some(320.0));
        assert_eq!(s.scale_y.scale_type, Some(MScale::Viewport));
        assert_eq!(s.scale_y.scale, Some(1.0));
    }

    #[test]
    fn pass_with_no_scale_keys_maps_to_empty_axes() {
        let (project, _) = import_fixture();
        let s = &project.passes[2].settings;
        // No scale keys -> both axes None/None so the engine applies the §2
        // position default (final pass = viewport).
        assert_eq!(s.scale_x, core_model::ScaleAxis::default());
        assert_eq!(s.scale_y, core_model::ScaleAxis::default());
    }

    #[test]
    fn format_sampler_alias_settings_match() {
        let (project, _) = import_fixture();
        let p0 = &project.passes[0].settings;
        assert_eq!(p0.filter_linear, Some(true));
        assert_eq!(p0.srgb_framebuffer, Some(true));
        assert_eq!(p0.float_framebuffer, None);
        assert_eq!(p0.mipmap_input, Some(true));
        assert_eq!(p0.alias.as_deref(), Some("MAIN"));

        let p1 = &project.passes[1].settings;
        assert_eq!(p1.float_framebuffer, Some(true));
        assert_eq!(p1.wrap_mode, Some(MWrap::Repeat));
        assert_eq!(p1.frame_count_mod, Some(60));
        assert_eq!(p1.alias, None);
    }

    #[test]
    fn alias_becomes_pass_name_else_indexed() {
        let (project, _) = import_fixture();
        assert_eq!(
            project.passes[0].name, "MAIN",
            "aliased pass uses its alias"
        );
        assert_eq!(
            project.passes[1].name, "Pass 1",
            "unaliased pass is indexed"
        );
    }

    #[test]
    fn unrecognized_keys_surface_as_diagnostics() {
        let (_, diags) = import_fixture();
        assert!(
            diags
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Info
                    && d.message.contains("mystery_setting")
                    && d.message.contains("banana")),
            "preserved extra surfaced as an info diagnostic: {:?}",
            diags.diagnostics
        );
    }

    #[test]
    fn missing_shader_file_warns_and_uses_empty_source() {
        let (project, diags) = import_fixture();
        // The fixture's shader paths don't exist -> each pass warns + empty source.
        assert!(diags.has_warnings());
        for pass in &project.passes {
            match &pass.source {
                PassSource::WholePassCode { source } => assert!(source.is_empty()),
                other => panic!("expected whole-pass code, got {other:?}"),
            }
        }
    }

    #[test]
    fn negative_feedback_pass_maps_to_none() {
        // RetroArch default `feedback_pass = -1` means "no global feedback pass".
        let preset = parse_slangp_str(
            "shaders = 1\nshader0 = a.slang\nfeedback_pass = -1\n",
            Path::new("/p"),
        )
        .expect("parses");
        let (project, _) = import_parsed_preset(&preset, "x");
        assert_eq!(project.feedback_pass, None);
    }

    #[test]
    fn reads_shader_source_when_present() {
        // End-to-end through the filesystem: a real shader file is loaded verbatim.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.slang"), "#version 450\n// hi\n").unwrap();
        std::fs::write(
            dir.path().join("p.slangp"),
            "shaders = 1\nshader0 = a.slang\n",
        )
        .unwrap();

        let (project, diags) = import_preset(dir.path().join("p.slangp")).expect("imports");
        assert_eq!(project.name, "p", "name from the preset file stem");
        assert!(!diags.has_warnings(), "shader read OK -> no warnings");
        match &project.passes[0].source {
            PassSource::WholePassCode { source } => assert!(source.contains("#version 450")),
            other => panic!("expected whole-pass code, got {other:?}"),
        }
    }
}
