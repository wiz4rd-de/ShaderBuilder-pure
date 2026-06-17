//! `.slangp` → [`core_model::Project`] import bridge (Architecture §A/§B).
//!
//! [`import_preset`] reads a `.slangp`, parses it with the tolerant tokenizer in
//! [`crate::slangp`], and lowers the parsed [`Preset`] into the canonical
//! [`core_model::Project`] the rest of the app edits and serializes. It carries
//! **all** RetroArch per-pass + preset-level settings onto the model and returns
//! [`ImportDiagnostics`] describing anything noteworthy (preserved unknown keys,
//! shader files that could not be read).
//!
//! ## Whole-pass code nodes — bodies are NOT decomposed (#34, Architecture §C)
//!
//! Each parsed pass becomes **exactly one** [`core_model::PassSource::WholePassCode`]
//! node holding the `.slang` source **byte-for-byte** (the file is read with no
//! normalization — line endings, trailing whitespace, and any BOM are preserved
//! so import → re-export is lossless). The pass body is *intentionally* never
//! reverse-engineered into a visual node graph: whole-pass nodes bypass the
//! node-IR. The only thing we recover from the body is a **light textual scan**
//! ([`crate::scan::scan_references`]) of the RetroArch textures/aliases it
//! references, for pipeline wiring + a LUT cross-check — not a parse.
//!
//! ## Pipeline wiring metadata (#34)
//!
//! Beyond the per-pass settings, the bridge reconstructs the chain *wiring* as
//! [`core_model::PipelineMetadata`]: passes are sequenced by index, each
//! `aliasN` becomes an `alias → pass index` binding (so a later pass's `<alias>`
//! reference resolves), and each pass records the set of RetroArch textures it
//! may legally bind (the always-available `Original`/`Source`, earlier
//! `PassOutputK`, earlier aliases, and all LUTs). This is *metadata about* the
//! chain, never a graph-internal IR.
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

use std::collections::BTreeSet;
use std::path::Path;

use crate::params::{reconcile_parameters, scan_parameters, ParamWarning};
use crate::scan::scan_references;
use crate::slangp::{LutEntry, Pass, Preset, ScaleType, WrapMode};
use core_model::Parameter;

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

    // The preset's alias + LUT name tables — used both to classify references in
    // each pass body (a matched alias/LUT name is `TextureRefKind::Alias`) and to
    // cross-check that every referenced alias/LUT actually resolves.
    let alias_names: BTreeSet<String> = preset
        .passes
        .iter()
        .filter_map(|p| p.alias.clone())
        .collect();
    let lut_names: BTreeSet<String> = preset.luts.iter().map(|l| l.name.clone()).collect();

    // Per-pass `#pragma parameter` declarations, collected during the pass loop
    // and reconciled into the project-level parameter set afterwards (#35).
    let mut per_pass_params: Vec<Vec<Parameter>> = Vec::with_capacity(preset.passes.len());

    let mut passes = Vec::with_capacity(preset.passes.len());
    for (n, pass) in preset.passes.iter().enumerate() {
        // Read the `.slang` BYTE-FOR-BYTE with no normalization (line endings,
        // trailing whitespace, BOM all preserved) so import → re-export is
        // lossless. `read_to_string` errors only on invalid UTF-8, never mutates
        // bytes; slang sources are UTF-8 text.
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

        let filename = pass
            .shader
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_owned);

        // Light textual scan of the (opaque) body for referenced textures — for
        // wiring + the LUT cross-check below. NOT a parse of the pass body.
        let references = scan_references(&source, &alias_names, &lut_names);

        // Tolerant `#pragma parameter` scan (#35): malformed pragma lines warn
        // (with the pass index) rather than failing the import. These per-pass
        // declarations carry onto the `Pass` (raw) and feed reconciliation below.
        let mut param_warnings = Vec::new();
        let parameters = scan_parameters(&source, &mut param_warnings);
        for w in &param_warnings {
            if let ParamWarning::Malformed { line } = w {
                diagnostics.push(ImportDiagnostic::warning(format!(
                    "pass {n}: malformed `#pragma parameter` ignored: `{line}`"
                )));
            }
        }
        per_pass_params.push(parameters.clone());

        let name = pass.alias.clone().unwrap_or_else(|| format!("Pass {n}"));

        passes.push(core_model::Pass {
            id: format!("pass-{n}"),
            name,
            // Exactly one whole-pass code node per pass, source verbatim, marked
            // opaque/non-decomposable (the body is never lowered to node-IR).
            source: core_model::PassSource::WholePassCode {
                source,
                filename,
                opaque: true,
            },
            parameters,
            settings: map_settings(pass),
            references,
        });
    }

    let pipeline = build_pipeline_metadata(preset, &passes);
    cross_check_references(&passes, &pipeline, &lut_names, &mut diagnostics);

    // Reconcile the per-pass `#pragma parameter` declarations into ONE project
    // parameter per id and apply the `.slangp` per-parameter overrides (the
    // preset value wins). Cross-pass definition conflicts become diagnostics.
    let mut param_conflicts = Vec::new();
    let parameters = reconcile_parameters(
        &per_pass_params,
        &preset.parameter_overrides,
        &mut param_conflicts,
    );
    for w in &param_conflicts {
        if let ParamWarning::Conflict { id, detail } = w {
            diagnostics.push(ImportDiagnostic::warning(format!(
                "parameter `{id}` is declared with conflicting definitions across passes \
                 ({detail}); keeping the first declaration"
            )));
        }
    }

    // Map the parsed LUT family into the model with resolved paths + per-texture
    // sampler settings (#35).
    let luts = preset.luts.iter().map(map_lut).collect();

    let feedback_pass = map_feedback_pass(preset.feedback_pass);

    let project = core_model::Project {
        schema_version: core_model::PROJECT_SCHEMA_VERSION,
        name: project_name.to_owned(),
        passes,
        feedback_pass,
        pipeline,
        parameters,
        luts,
    };
    (project, ImportDiagnostics { diagnostics })
}

/// Map a parsed [`LutEntry`] into a [`core_model::Lut`] (#35): the path is already
/// resolved against the preset directory by the parser (`base_dir.join(rel)`).
/// Here it is **lexically normalized** ([`normalize_lexical`]) so a relative LUT
/// that points outside the preset dir (e.g. `../shared/foo.png`) or into a nested
/// subdirectory collapses to a clean path — without touching the filesystem (the
/// file need not exist at import time). The `PathBuf` is rendered to a `String`
/// for the serde/TS model; lossy UTF-8 conversion is acceptable as `.slangp`
/// paths are UTF-8 text.
fn map_lut(lut: &LutEntry) -> core_model::Lut {
    core_model::Lut {
        name: lut.name.clone(),
        path: normalize_lexical(&lut.path).to_string_lossy().into_owned(),
        filter_linear: lut.linear,
        wrap_mode: lut.wrap_mode.map(map_wrap_mode),
        mipmap: lut.mipmap,
    }
}

/// Lexically normalize a path: collapse `.` components and resolve `..` against
/// the preceding **normal** component, **without** consulting the filesystem (so
/// it works for paths whose targets do not yet exist, and never resolves symlinks
/// — purely textual). A leading `..` with no preceding normal component (the path
/// genuinely escapes its base, e.g. `/presets/../shared/x.png` → `/shared/x.png`,
/// or a relative `../shared/x.png` kept as-is) is preserved.
///
/// This is the `path-clean`-style algorithm; we implement it locally to avoid a
/// new dependency for a few lines.
fn normalize_lexical(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                // Pop a preceding normal component (a real dir name).
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // Can't pop a root or another `..`; keep the `..` (the path
                // legitimately steps above its base — outside-the-preset-dir case).
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

/// Reconstruct the chain's wiring as [`core_model::PipelineMetadata`] (#34):
/// passes are sequenced by index; each `aliasN` becomes an `alias → pass index`
/// binding; and each pass records the RetroArch texture names it may bind. This
/// is *metadata about* the pipeline — ordering, alias resolution, causal
/// availability — and never a graph-internal IR (Architecture §C).
fn build_pipeline_metadata(
    preset: &Preset,
    passes: &[core_model::Pass],
) -> core_model::PipelineMetadata {
    // alias -> pass index, in pass order (a later duplicate alias keeps the
    // first; RetroArch reflection binds the first matching name).
    let mut aliases = Vec::new();
    let mut seen = BTreeSet::new();
    for (n, pass) in preset.passes.iter().enumerate() {
        if let Some(alias) = &pass.alias {
            if seen.insert(alias.clone()) {
                aliases.push(core_model::AliasBinding {
                    alias: alias.clone(),
                    pass_index: n as u32,
                });
            }
        }
    }

    // All LUT names are available to every pass (loaded once, static; §7).
    let lut_names: Vec<String> = preset.luts.iter().map(|l| l.name.clone()).collect();

    // Per-pass availability: the always-present built-ins, then every EARLIER
    // pass's `PassOutputK` and its alias (causality: `PassOutputK`/`<alias>` is
    // an error for `K >= i`, §7), then all LUTs. Feedback twins are implied.
    let mut availability = Vec::with_capacity(passes.len());
    for i in 0..passes.len() {
        let mut available = vec!["Original".to_owned(), "Source".to_owned()];
        for k in 0..i {
            available.push(format!("PassOutput{k}"));
            if let Some(alias) = &preset.passes[k].alias {
                available.push(alias.clone());
            }
        }
        available.extend(lut_names.iter().cloned());
        availability.push(core_model::PassAvailability {
            pass_index: i as u32,
            available,
        });
    }

    core_model::PipelineMetadata {
        aliases,
        availability,
    }
}

/// Cross-check each pass's scanned references against the reconstructed wiring
/// (#34): warn when a pass references a `PassOutputK`/`<alias>` that is not
/// available to it (a forward/missing reference) or a LUT-shaped name that has no
/// `textures=` entry. This catches broken presets without parsing the body —
/// the references and availability were both recovered shallowly above.
fn cross_check_references(
    passes: &[core_model::Pass],
    pipeline: &core_model::PipelineMetadata,
    lut_names: &BTreeSet<String>,
    diagnostics: &mut Vec<ImportDiagnostic>,
) {
    use core_model::TextureRefKind;

    let alias_to_index: std::collections::BTreeMap<&str, u32> = pipeline
        .aliases
        .iter()
        .map(|b| (b.alias.as_str(), b.pass_index))
        .collect();

    for (i, pass) in passes.iter().enumerate() {
        for r in &pass.references {
            match r.kind {
                // A `PassOutputK`/`PassK` read must name an earlier pass.
                TextureRefKind::PassOutput => {
                    // Canonicalize `PassK` → `PassOutputK` for the availability set.
                    let idx = r
                        .name
                        .strip_prefix("PassOutput")
                        .or_else(|| r.name.strip_prefix("Pass"))
                        .and_then(|d| d.parse::<usize>().ok());
                    if let Some(k) = idx {
                        if k >= i {
                            diagnostics.push(ImportDiagnostic::warning(format!(
                                "pass {i}: references `{}` but pass {k} is not earlier in the \
                                 chain (PassOutput is causal — only passes < {i} are available)",
                                r.name
                            )));
                        }
                    }
                }
                // An alias read must resolve to an EARLIER pass (or a LUT, which
                // is classified as Alias and is always available).
                TextureRefKind::Alias => {
                    if lut_names.contains(&r.name) {
                        continue; // a LUT — always available.
                    }
                    match alias_to_index.get(r.name.as_str()) {
                        Some(&k) if (k as usize) < i => {}
                        Some(&k) => diagnostics.push(ImportDiagnostic::warning(format!(
                            "pass {i}: references alias `{}` (pass {k}) which is not earlier in \
                             the chain",
                            r.name
                        ))),
                        None => diagnostics.push(ImportDiagnostic::warning(format!(
                            "pass {i}: references `{}` which is neither a known pass alias nor a \
                             `textures=` LUT",
                            r.name
                        ))),
                    }
                }
                // Original/Source/History/Feedback/User are always resolvable
                // (built-ins, the history/feedback ring, or the un-aliased LUT
                // fallback); no cross-check needed here.
                _ => {}
            }
        }
    }
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
                PassSource::WholePassCode { source, .. } => assert!(source.is_empty()),
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
            PassSource::WholePassCode {
                source,
                filename,
                opaque,
            } => {
                assert!(source.contains("#version 450"));
                assert_eq!(filename.as_deref(), Some("a.slang"));
                assert!(opaque, "imported whole-pass code is opaque");
            }
            other => panic!("expected whole-pass code, got {other:?}"),
        }
    }

    #[test]
    fn imported_source_is_byte_for_byte_on_disk_bytes() {
        // ACCEPTANCE (#34): the imported whole-pass source must equal the on-disk
        // file BYTES exactly — no normalization of line endings, trailing
        // whitespace, or a leading BOM.
        let dir = tempfile::tempdir().unwrap();
        // Deliberately gnarly: CRLF, a BOM, trailing spaces, no final newline.
        let raw: &[u8] = b"\xEF\xBB\xBF#version 450\r\n#pragma stage fragment   \r\nvoid main(){}";
        std::fs::write(dir.path().join("p0.slang"), raw).unwrap();
        std::fs::write(dir.path().join("p1.slang"), raw).unwrap();
        std::fs::write(
            dir.path().join("two.slangp"),
            "shaders = 2\nshader0 = p0.slang\nshader1 = p1.slang\n",
        )
        .unwrap();

        let (project, _) = import_preset(dir.path().join("two.slangp")).expect("imports");
        assert_eq!(project.passes.len(), 2, "exactly N whole-pass nodes");

        for (n, pass) in project.passes.iter().enumerate() {
            let on_disk = std::fs::read(dir.path().join(format!("p{n}.slang"))).unwrap();
            match &pass.source {
                PassSource::WholePassCode {
                    source,
                    filename,
                    opaque,
                } => {
                    assert_eq!(
                        source.as_bytes(),
                        on_disk.as_slice(),
                        "pass {n} source must be the on-disk bytes verbatim"
                    );
                    assert_eq!(filename.as_deref(), Some(format!("p{n}.slang").as_str()));
                    assert!(opaque);
                }
                other => panic!("expected whole-pass code, got {other:?}"),
            }
        }
    }

    #[test]
    fn alias_is_referenceable_via_pipeline_metadata() {
        // ACCEPTANCE (#34): aliases + feedback_pass are representable such that a
        // pass can be referenced by its alias.
        let preset = parse_slangp_str(
            "shaders = 3\n\
             feedback_pass = 1\n\
             shader0 = a.slang\n\
             alias0 = First\n\
             shader1 = b.slang\n\
             alias1 = Second\n\
             shader2 = c.slang\n",
            Path::new("/p"),
        )
        .expect("parses");
        let (project, _) = import_parsed_preset(&preset, "x");

        // alias -> pass index resolves both aliases.
        let resolve = |name: &str| {
            project
                .pipeline
                .aliases
                .iter()
                .find(|b| b.alias == name)
                .map(|b| b.pass_index)
        };
        assert_eq!(resolve("First"), Some(0));
        assert_eq!(resolve("Second"), Some(1));
        assert_eq!(resolve("Missing"), None);
        // feedback_pass carried through.
        assert_eq!(project.feedback_pass, Some(1));
    }

    #[test]
    fn per_pass_availability_is_causal() {
        // Each pass may bind only EARLIER passes' PassOutputK/aliases, plus the
        // always-available Original/Source and all LUTs.
        let preset = parse_slangp_str(
            "shaders = 3\n\
             shader0 = a.slang\n\
             alias0 = First\n\
             shader1 = b.slang\n\
             shader2 = c.slang\n\
             textures = LUT\n\
             LUT = lut.png\n",
            Path::new("/p"),
        )
        .expect("parses");
        let (project, _) = import_parsed_preset(&preset, "x");

        let avail = |i: usize| -> Vec<String> {
            project
                .pipeline
                .availability
                .iter()
                .find(|a| a.pass_index == i as u32)
                .map(|a| a.available.clone())
                .unwrap_or_default()
        };

        // Pass 0: only built-ins + LUT, no PassOutput*, no aliases.
        let a0 = avail(0);
        assert!(a0.contains(&"Original".to_owned()) && a0.contains(&"Source".to_owned()));
        assert!(a0.contains(&"LUT".to_owned()));
        assert!(!a0.iter().any(|s| s.starts_with("PassOutput")));
        assert!(!a0.contains(&"First".to_owned()));

        // Pass 2: PassOutput0, PassOutput1 and alias `First` are now available.
        let a2 = avail(2);
        assert!(a2.contains(&"PassOutput0".to_owned()));
        assert!(a2.contains(&"PassOutput1".to_owned()));
        assert!(a2.contains(&"First".to_owned()));
        assert!(a2.contains(&"LUT".to_owned()));
    }

    #[test]
    fn references_are_scanned_and_cross_checked() {
        // A pass body that reads Source, an earlier alias, a LUT, and a forward
        // PassOutput (which must warn).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.slang"), "uniform sampler2D Source;\n").unwrap();
        std::fs::write(
            dir.path().join("b.slang"),
            "uniform sampler2D First;\n\
             uniform sampler2D BORDER;\n\
             uniform sampler2D PassOutput2;\n", // forward ref -> warn
        )
        .unwrap();
        std::fs::write(dir.path().join("c.slang"), "void main(){}\n").unwrap();
        std::fs::write(
            dir.path().join("p.slangp"),
            "shaders = 3\n\
             shader0 = a.slang\n\
             alias0 = First\n\
             shader1 = b.slang\n\
             shader2 = c.slang\n\
             textures = BORDER\n\
             BORDER = border.png\n",
        )
        .unwrap();

        let (project, diags) = import_preset(dir.path().join("p.slangp")).expect("imports");

        // Pass 1's scanned references include the alias, LUT, and forward pass.
        let p1_refs: Vec<&str> = project.passes[1]
            .references
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert!(p1_refs.contains(&"First"), "alias ref scanned: {p1_refs:?}");
        assert!(p1_refs.contains(&"BORDER"), "LUT ref scanned: {p1_refs:?}");
        assert!(p1_refs.contains(&"PassOutput2"));

        // The forward PassOutput2 reference from pass 1 must warn.
        assert!(
            diags
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Warning
                    && d.message.contains("PassOutput2")
                    && d.message.contains("pass 1")),
            "forward PassOutput must warn: {:?}",
            diags.diagnostics
        );
    }

    #[test]
    fn unresolved_alias_reference_warns() {
        // A pass references an alias that no pass declares and no LUT provides.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.slang"), "uniform sampler2D Source;\n").unwrap();
        std::fs::write(dir.path().join("b.slang"), "uniform sampler2D GhostPass;\n").unwrap();
        std::fs::write(
            dir.path().join("p.slangp"),
            "shaders = 2\n\
             shader0 = a.slang\n\
             alias0 = GhostPass\n\
             shader1 = b.slang\n",
        )
        .unwrap();
        // alias0 = GhostPass DOES exist, so this resolves to pass 0 (earlier) — OK.
        let (_, diags) = import_preset(dir.path().join("p.slangp")).expect("imports");
        assert!(
            !diags
                .diagnostics
                .iter()
                .any(|d| d.message.contains("GhostPass") && d.severity == Severity::Warning),
            "GhostPass resolves to pass 0: {:?}",
            diags.diagnostics
        );
    }

    // ---- #35: parameter extraction + reconciliation ------------------------

    #[test]
    fn pragma_parameters_become_project_parameters_with_override() {
        // ACCEPTANCE (#35): pragma params appear with correct default/min/max/step;
        // a `.slangp` override takes effect on the default.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.slang"),
            "#version 450\n\
             #pragma parameter BRIGHT \"Brightness\" 1.0 0.0 2.0 0.01\n\
             #pragma parameter CONTRAST \"Contrast\" 1.0 0.5 1.5\n\
             void main(){}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("p.slangp"),
            "shaders = 1\n\
             shader0 = a.slang\n\
             parameters = \"BRIGHT;CONTRAST\"\n\
             BRIGHT = 1.5\n", // overrides BRIGHT's default
        )
        .unwrap();

        let (project, diags) = import_preset(dir.path().join("p.slangp")).expect("imports");
        assert!(
            !diags.has_warnings(),
            "clean import: {:?}",
            diags.diagnostics
        );

        let by = |id: &str| project.parameters.iter().find(|p| p.name == id).cloned();
        let bright = by("BRIGHT").expect("BRIGHT present");
        // Override applied to default; range/step/label from the pragma.
        assert_eq!(bright.default, 1.5, "preset override wins");
        assert_eq!(bright.min, 0.0);
        assert_eq!(bright.max, 2.0);
        assert_eq!(bright.step, 0.01);
        assert_eq!(bright.label, "Brightness");

        let contrast = by("CONTRAST").expect("CONTRAST present");
        assert_eq!(contrast.default, 1.0, "no override -> pragma initial");
        assert_eq!(contrast.step, 0.0, "step omitted -> 0.0");

        // The raw declarations also live on the pass.
        assert_eq!(project.passes[0].parameters.len(), 2);
    }

    #[test]
    fn duplicate_param_across_passes_collapses_with_conflict_diagnostic() {
        // ACCEPTANCE (#35): a duplicate id across passes collapses to ONE project
        // parameter (first declaration wins) and a conflict diagnostic is emitted.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.slang"),
            "#pragma parameter GAMMA \"Gamma\" 2.2 1.0 3.0 0.1\nvoid main(){}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.slang"),
            // Same id, DIFFERENT max — must keep pass 0's value and warn.
            "#pragma parameter GAMMA \"Gamma\" 2.2 1.0 4.0 0.1\nvoid main(){}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("p.slangp"),
            "shaders = 2\nshader0 = a.slang\nshader1 = b.slang\n",
        )
        .unwrap();

        let (project, diags) = import_preset(dir.path().join("p.slangp")).expect("imports");
        // Collapsed to one knob, keeping the FIRST (pass 0) declaration's max.
        let gammas: Vec<_> = project
            .parameters
            .iter()
            .filter(|p| p.name == "GAMMA")
            .collect();
        assert_eq!(gammas.len(), 1, "duplicate ids collapse to one parameter");
        assert_eq!(gammas[0].max, 3.0, "first declaration wins");
        // A conflict diagnostic mentions the id and the diverging field.
        assert!(
            diags
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Warning
                    && d.message.contains("GAMMA")
                    && d.message.contains("conflicting")
                    && d.message.contains("max")),
            "conflict diagnostic emitted: {:?}",
            diags.diagnostics
        );
    }

    #[test]
    fn identical_param_across_passes_collapses_silently() {
        // Same id declared identically in two passes -> one knob, no conflict.
        let dir = tempfile::tempdir().unwrap();
        let body = "#pragma parameter X \"X\" 0.5 0.0 1.0 0.01\nvoid main(){}\n";
        std::fs::write(dir.path().join("a.slang"), body).unwrap();
        std::fs::write(dir.path().join("b.slang"), body).unwrap();
        std::fs::write(
            dir.path().join("p.slangp"),
            "shaders = 2\nshader0 = a.slang\nshader1 = b.slang\n",
        )
        .unwrap();
        let (project, diags) = import_preset(dir.path().join("p.slangp")).expect("imports");
        assert_eq!(
            project.parameters.iter().filter(|p| p.name == "X").count(),
            1
        );
        assert!(
            !diags
                .diagnostics
                .iter()
                .any(|d| d.message.contains("conflicting")),
            "identical declarations don't conflict: {:?}",
            diags.diagnostics
        );
    }

    #[test]
    fn malformed_pragma_parameter_warns_but_imports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.slang"),
            "#pragma parameter BROKEN\n\
             #pragma parameter OK \"Ok\" 0.0 0.0 1.0\nvoid main(){}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("p.slangp"),
            "shaders = 1\nshader0 = a.slang\n",
        )
        .unwrap();
        let (project, diags) = import_preset(dir.path().join("p.slangp")).expect("imports");
        // The good one still extracts.
        assert!(project.parameters.iter().any(|p| p.name == "OK"));
        assert!(!project.parameters.iter().any(|p| p.name == "BROKEN"));
        // The malformed one warns with the pass index.
        assert!(
            diags
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Warning
                    && d.message.contains("malformed")
                    && d.message.contains("pass 0")),
            "malformed pragma warns: {:?}",
            diags.diagnostics
        );
    }

    // ---- #35: LUT import ---------------------------------------------------

    #[test]
    fn luts_import_with_resolved_paths_and_sampler_settings() {
        // ACCEPTANCE (#35): all declared LUTs import with correct resolved paths +
        // per-texture filter/wrap/mipmap. Two LUTs with DIFFERING sampler settings.
        let preset = parse_slangp_str(
            "shaders = 1\n\
             shader0 = a.slang\n\
             textures = \"BORDER;OVERLAY\"\n\
             BORDER = luts/border.png\n\
             BORDER_linear = true\n\
             BORDER_wrap_mode = clamp_to_edge\n\
             BORDER_mipmap = true\n\
             OVERLAY = luts/overlay.png\n\
             OVERLAY_linear = false\n\
             OVERLAY_wrap_mode = repeat\n",
            Path::new("/presets"),
        )
        .expect("parses");
        let (project, _) = import_parsed_preset(&preset, "x");

        assert_eq!(project.luts.len(), 2, "both LUTs imported");
        let lut = |name: &str| project.luts.iter().find(|l| l.name == name).cloned();

        let border = lut("BORDER").expect("BORDER imported");
        assert_eq!(border.path, "/presets/luts/border.png");
        assert_eq!(border.filter_linear, Some(true));
        assert_eq!(border.wrap_mode, Some(MWrap::ClampToEdge));
        assert_eq!(border.mipmap, Some(true));

        let overlay = lut("OVERLAY").expect("OVERLAY imported");
        assert_eq!(overlay.path, "/presets/luts/overlay.png");
        // Differing sampler settings from BORDER.
        assert_eq!(overlay.filter_linear, Some(false));
        assert_eq!(overlay.wrap_mode, Some(MWrap::Repeat));
        assert_eq!(
            overlay.mipmap, None,
            "unset mipmap -> None (engine default)"
        );
    }

    #[test]
    fn lut_outside_preset_dir_resolves() {
        // ACCEPTANCE (#35): a relative LUT path pointing OUTSIDE the preset dir
        // (`../shared/foo.png`) resolves to a clean lexically-normalized path.
        let preset = parse_slangp_str(
            "shaders = 1\n\
             shader0 = a.slang\n\
             textures = SHARED\n\
             SHARED = ../shared/foo.png\n",
            Path::new("/presets/crt"),
        )
        .expect("parses");
        let (project, _) = import_parsed_preset(&preset, "x");
        // /presets/crt + ../shared/foo.png -> /presets/shared/foo.png (normalized).
        assert_eq!(project.luts[0].path, "/presets/shared/foo.png");
    }

    #[test]
    fn lut_in_nested_subdirectory_resolves() {
        // ACCEPTANCE (#35): a nested-subdirectory LUT resolves correctly.
        let preset = parse_slangp_str(
            "shaders = 1\n\
             shader0 = a.slang\n\
             textures = DEEP\n\
             DEEP = resources/luts/deep/grade.png\n",
            Path::new("/presets/crt"),
        )
        .expect("parses");
        let (project, _) = import_parsed_preset(&preset, "x");
        assert_eq!(
            project.luts[0].path,
            "/presets/crt/resources/luts/deep/grade.png"
        );
    }

    #[test]
    fn absolute_lut_path_is_preserved() {
        let preset = parse_slangp_str(
            "shaders = 1\n\
             shader0 = a.slang\n\
             textures = ABS\n\
             ABS = /opt/shared/lut.png\n",
            Path::new("/presets"),
        )
        .expect("parses");
        let (project, _) = import_parsed_preset(&preset, "x");
        assert_eq!(project.luts[0].path, "/opt/shared/lut.png");
    }

    #[test]
    fn normalize_lexical_collapses_dot_and_parent() {
        use std::path::PathBuf;
        assert_eq!(
            normalize_lexical(Path::new("/presets/crt/../shared/foo.png")),
            PathBuf::from("/presets/shared/foo.png")
        );
        assert_eq!(
            normalize_lexical(Path::new("/presets/./a/./b.png")),
            PathBuf::from("/presets/a/b.png")
        );
        // A relative path that escapes its base keeps the leading `..`.
        assert_eq!(
            normalize_lexical(Path::new("../shared/foo.png")),
            PathBuf::from("../shared/foo.png")
        );
    }

    #[test]
    fn no_luts_or_params_means_empty_project_vecs() {
        let preset =
            parse_slangp_str("shaders = 1\nshader0 = a.slang\n", Path::new("/p")).expect("parses");
        let (project, _) = import_parsed_preset(&preset, "x");
        assert!(project.luts.is_empty());
        assert!(project.parameters.is_empty());
    }
}
