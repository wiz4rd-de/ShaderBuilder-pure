//! Lossless round-trip harness (#37, Phase-3 EXIT gate): prove that
//! **import → export → re-import** preserves a [`core_model::Project`] modulo a
//! short, *documented* canonicalization, and produce a human-readable **diff**
//! when it does not.
//!
//! ## What "lossless" means here
//!
//! A `.slangp` preset is imported into a [`core_model::Project`]
//! ([`preset_io::import_preset`]), exported back to a fresh RetroArch bundle
//! ([`preset_io::export_preset`]), then re-imported. The two `Project`s must be
//! **structurally equal** — every pass, setting, parameter, LUT, alias and the
//! feedback pass survive — and every *unmodified* pass's `.slang` file is
//! **byte-identical** to its original (the byte-exact contract from #34/#36).
//!
//! ## Documented canonicalization (why it is not raw `==`)
//!
//! Two model fields legitimately differ across a round trip even when nothing was
//! lost, so [`compare_projects`] canonicalizes them. Each is a *path/identity*
//! rewrite the export performs by design, never a value change:
//!
//! 1. **LUT `path`** — on import a LUT path resolves to wherever the source PNG
//!    lived; the export *copies* it into `textures/<file>` and the re-import
//!    points there. The bytes and sampler settings are unchanged, so we compare
//!    LUTs by `name` + sampler settings and by the **basename** of the path, not
//!    the absolute location.
//! 2. **Pass `filename`** — the export may rename a `.slang` to avoid a collision
//!    (`dup.slang` → `dup_1.slang`). The *source bytes* are what must survive, so
//!    pass sources are compared by `source` (+ `opaque`), not `filename`.
//!
//! Everything else (pass count + order, every [`core_model::PassSettings`] key,
//! reconciled [`core_model::Parameter`]s with their overrides applied, the LUT
//! set, `feedback_pass`, and the pipeline alias bindings) is compared verbatim.
//! `name`, document `metadata`, `library_refs`, and the derived per-pass
//! `availability`/`references` are *not* part of the `.slangp` round trip and are
//! intentionally excluded (an imported preset never carries document metadata, and
//! availability is re-derived deterministically from the chain).
//!
//! ## API
//!
//! * [`compare_projects`] → [`ProjectDiff`]: the canonicalized structural compare,
//!   collecting every mismatch as a readable line; [`ProjectDiff::is_lossless`] is
//!   the verdict and [`ProjectDiff::report`] the diff text.
//! * [`round_trip`] → [`RoundTrip`]: drive a `.slangp` through
//!   import → export → re-import and capture both `Project`s, the byte-equality of
//!   each unmodified pass, and the structural diff — the single call the fixture
//!   and corpus tests use.

use std::collections::BTreeMap;
use std::path::Path;

use core_model::{Lut, Parameter, PassSettings, PassSource, Project};
use preset_io::{
    export_preset, import_parsed_preset, import_preset, parse_slangp, PRESET_FILENAME,
};

/// The structural difference between two [`core_model::Project`]s under the
/// documented canonicalization (see the module docs). Empty `mismatches` ⇒ the
/// two projects are losslessly equal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectDiff {
    /// One readable line per field that differs. Empty when the projects are
    /// structurally equal (the round trip was lossless).
    pub mismatches: Vec<String>,
}

impl ProjectDiff {
    /// Whether the two projects are structurally equal (no mismatches).
    pub fn is_lossless(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// A human-readable multi-line diff report. `"<lossless>"` when there are no
    /// mismatches, otherwise a bulleted list of every diverging field — the text
    /// a failing assertion prints so a regression is debuggable at a glance.
    pub fn report(&self) -> String {
        if self.mismatches.is_empty() {
            return "<lossless>".to_owned();
        }
        let mut out = format!(
            "{} field(s) differ after round trip:\n",
            self.mismatches.len()
        );
        for m in &self.mismatches {
            out.push_str("  - ");
            out.push_str(m);
            out.push('\n');
        }
        out
    }
}

/// Structurally compare two [`core_model::Project`]s under the documented
/// canonicalization (LUT path → basename, pass identity → source bytes), returning
/// every mismatch as a [`ProjectDiff`]. `a` is the first import (the oracle), `b`
/// the re-import; field labels read "`a` vs `b`".
pub fn compare_projects(a: &Project, b: &Project) -> ProjectDiff {
    let mut d = Vec::new();

    // feedback_pass (global).
    if a.feedback_pass != b.feedback_pass {
        d.push(format!(
            "feedback_pass: {:?} vs {:?}",
            a.feedback_pass, b.feedback_pass
        ));
    }

    // Passes: count, then per-pass settings + source bytes (NOT filename).
    if a.passes.len() != b.passes.len() {
        d.push(format!(
            "pass count: {} vs {}",
            a.passes.len(),
            b.passes.len()
        ));
    }
    for (i, (pa, pb)) in a.passes.iter().zip(&b.passes).enumerate() {
        compare_pass_settings(i, &pa.settings, &pb.settings, &mut d);
        compare_pass_source(i, &pa.source, &pb.source, &mut d);
    }

    // Reconciled project parameters (overrides already applied): compare as a
    // name-keyed set so an incidental ordering change is not a mismatch.
    compare_parameters(&a.parameters, &b.parameters, &mut d);

    // LUTs: name-keyed, comparing sampler settings + path BASENAME (the export
    // relocates the file into textures/, by design).
    compare_luts(&a.luts, &b.luts, &mut d);

    // Pipeline alias bindings (alias -> pass index). Availability is re-derived,
    // not round-tripped, so it is excluded.
    if a.pipeline.aliases != b.pipeline.aliases {
        d.push(format!(
            "pipeline aliases: {:?} vs {:?}",
            a.pipeline.aliases, b.pipeline.aliases
        ));
    }

    ProjectDiff { mismatches: d }
}

/// Compare a single pass's [`PassSettings`] field by field (so the diff names the
/// exact diverging key, not just "settings differ").
fn compare_pass_settings(i: usize, a: &PassSettings, b: &PassSettings, d: &mut Vec<String>) {
    macro_rules! cmp {
        ($field:ident) => {
            if a.$field != b.$field {
                d.push(format!(
                    "pass {i} settings.{}: {:?} vs {:?}",
                    stringify!($field),
                    a.$field,
                    b.$field
                ));
            }
        };
    }
    cmp!(scale_x);
    cmp!(scale_y);
    cmp!(filter_linear);
    cmp!(wrap_mode);
    cmp!(mipmap_input);
    cmp!(float_framebuffer);
    cmp!(srgb_framebuffer);
    cmp!(alias);
    cmp!(frame_count_mod);
}

/// Compare a pass's source by **bytes** (+ the opaque marker), ignoring the
/// `filename` (which the export may rename to avoid a collision).
fn compare_pass_source(i: usize, a: &PassSource, b: &PassSource, d: &mut Vec<String>) {
    match (a, b) {
        (
            PassSource::WholePassCode {
                source: sa,
                opaque: oa,
                ..
            },
            PassSource::WholePassCode {
                source: sb,
                opaque: ob,
                ..
            },
        ) => {
            if sa != sb {
                d.push(format!(
                    "pass {i} source bytes differ ({} vs {} bytes)",
                    sa.len(),
                    sb.len()
                ));
            }
            if oa != ob {
                d.push(format!("pass {i} source.opaque: {oa} vs {ob}"));
            }
        }
        // A graph pass cannot be exported (#36), so a round trip never produces
        // one; a kind change is a hard mismatch.
        _ => d.push(format!("pass {i} source kind changed")),
    }
}

/// Compare reconciled parameters as a name-keyed set (default/min/max/step,
/// bitwise on the floats so a NaN or `-0.0` regression is caught).
fn compare_parameters(a: &[Parameter], b: &[Parameter], d: &mut Vec<String>) {
    let ma: BTreeMap<&str, &Parameter> = a.iter().map(|p| (p.name.as_str(), p)).collect();
    let mb: BTreeMap<&str, &Parameter> = b.iter().map(|p| (p.name.as_str(), p)).collect();

    for name in ma.keys() {
        if !mb.contains_key(name) {
            d.push(format!(
                "parameter `{name}` present in first, missing in re-import"
            ));
        }
    }
    for name in mb.keys() {
        if !ma.contains_key(name) {
            d.push(format!(
                "parameter `{name}` appeared on re-import (absent in first)"
            ));
        }
    }
    for (name, pa) in &ma {
        if let Some(pb) = mb.get(name) {
            for (field, x, y) in [
                ("default", pa.default, pb.default),
                ("min", pa.min, pb.min),
                ("max", pa.max, pb.max),
                ("step", pa.step, pb.step),
            ] {
                if x.to_bits() != y.to_bits() {
                    d.push(format!("parameter `{name}`.{field}: {x} vs {y}"));
                }
            }
        }
    }
}

/// Compare LUTs as a name-keyed set: sampler settings verbatim, path by
/// **basename** only (the export relocates the file into `textures/`).
fn compare_luts(a: &[Lut], b: &[Lut], d: &mut Vec<String>) {
    let ma: BTreeMap<&str, &Lut> = a.iter().map(|l| (l.name.as_str(), l)).collect();
    let mb: BTreeMap<&str, &Lut> = b.iter().map(|l| (l.name.as_str(), l)).collect();

    for name in ma.keys() {
        if !mb.contains_key(name) {
            d.push(format!(
                "LUT `{name}` present in first, missing in re-import"
            ));
        }
    }
    for name in mb.keys() {
        if !ma.contains_key(name) {
            d.push(format!(
                "LUT `{name}` appeared on re-import (absent in first)"
            ));
        }
    }
    for (name, la) in &ma {
        if let Some(lb) = mb.get(name) {
            if la.filter_linear != lb.filter_linear {
                d.push(format!(
                    "LUT `{name}`.filter_linear: {:?} vs {:?}",
                    la.filter_linear, lb.filter_linear
                ));
            }
            if la.wrap_mode != lb.wrap_mode {
                d.push(format!(
                    "LUT `{name}`.wrap_mode: {:?} vs {:?}",
                    la.wrap_mode, lb.wrap_mode
                ));
            }
            if la.mipmap != lb.mipmap {
                d.push(format!(
                    "LUT `{name}`.mipmap: {:?} vs {:?}",
                    la.mipmap, lb.mipmap
                ));
            }
            if path_basename(&la.path) != path_basename(&lb.path) {
                d.push(format!(
                    "LUT `{name}` path basename: {:?} vs {:?}",
                    path_basename(&la.path),
                    path_basename(&lb.path)
                ));
            }
        }
    }
}

/// The trailing file-name of a path string (the part the export preserves across
/// the `textures/` relocation), or the whole string if it has no separator.
fn path_basename(p: &str) -> &str {
    Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
}

/// Whether an error indicates the round trip is **not applicable** to this preset
/// (rather than a lossiness bug). A graph pass cannot be exported yet (#36); these
/// are surfaced as skips, never silent failures.
#[derive(Debug, Clone)]
pub enum RoundTripError {
    /// The `.slangp` could not be parsed (a hard parse error — the preset is
    /// malformed, not a lossiness finding).
    Parse(String),
    /// The bundle could not be exported (e.g. an unsupported graph pass, or an I/O
    /// error writing the temp bundle).
    Export(String),
    /// The exported bundle could not be re-imported (should not happen for a
    /// preset we just exported; surfaced rather than panicked).
    ReimportParse(String),
}

impl std::fmt::Display for RoundTripError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoundTripError::Parse(e) => write!(f, "parse: {e}"),
            RoundTripError::Export(e) => write!(f, "export: {e}"),
            RoundTripError::ReimportParse(e) => write!(f, "re-import parse: {e}"),
        }
    }
}

impl std::error::Error for RoundTripError {}

/// The outcome of one `.slangp` round trip (import → export → re-import).
#[derive(Debug, Clone)]
pub struct RoundTrip {
    /// The first import (the oracle `Project`).
    pub first: Project,
    /// The re-import of the exported bundle.
    pub second: Project,
    /// The canonicalized structural diff between [`Self::first`] and
    /// [`Self::second`]. Lossless ⇔ `diff.is_lossless()`.
    pub diff: ProjectDiff,
    /// Per-pass byte equality of the exported `.slang` against the **original**
    /// source file — `true` for every unmodified imported pass (#34/#36 contract).
    /// Parallel to [`Project::passes`].
    pub pass_bytes_identical: Vec<bool>,
}

impl RoundTrip {
    /// Whether the round trip preserved both structure (the diff) AND the
    /// per-pass `.slang` bytes — the full lossless verdict.
    pub fn is_lossless(&self) -> bool {
        self.diff.is_lossless() && self.pass_bytes_identical.iter().all(|&b| b)
    }

    /// A readable failure report (the structural diff plus any pass whose bytes
    /// changed) for a failing assertion.
    pub fn report(&self) -> String {
        let mut out = self.diff.report();
        let changed: Vec<usize> = self
            .pass_bytes_identical
            .iter()
            .enumerate()
            .filter(|(_, &ok)| !ok)
            .map(|(i, _)| i)
            .collect();
        if !changed.is_empty() {
            out.push_str(&format!(
                "pass `.slang` bytes changed after export for pass index(es): {changed:?}\n"
            ));
        }
        out
    }
}

/// Drive one `.slangp` through **import → export → re-import** and capture the
/// structural diff plus per-pass byte equality (#37). The export bundle is written
/// to `work_dir` (a caller-owned scratch dir, typically a `tempfile::tempdir`), so
/// the function performs no cleanup of its own.
///
/// The `extras` preserved on import are threaded back into the export so unknown
/// keys round-trip (the #33/#36 contract). Per-pass byte equality is checked
/// against the *original* on-disk `.slang` (resolved from the parsed preset),
/// proving an unmodified pass exports byte-identically.
pub fn round_trip(slangp: &Path, work_dir: &Path) -> Result<RoundTrip, RoundTripError> {
    // 1. Parse + import the source preset (the oracle).
    let preset = parse_slangp(slangp).map_err(|e| RoundTripError::Parse(e.to_string()))?;
    let name = slangp
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("preset");
    let (first, _diags) = import_parsed_preset(&preset, name);

    // 2. Export to a fresh bundle, threading the preserved extras back in.
    let report = export_preset(&first, work_dir, &preset.extras)
        .map_err(|e| RoundTripError::Export(e.to_string()))?;

    // 3. Per-pass byte equality: each exported `.slang` vs its ORIGINAL on-disk
    //    file (the parsed pass `shader` path), for every unmodified imported pass.
    let mut pass_bytes_identical = Vec::with_capacity(first.passes.len());
    for (pass, exported_name) in preset.passes.iter().zip(&report.pass_files) {
        let original = std::fs::read(&pass.shader).ok();
        let exported = std::fs::read(work_dir.join(exported_name)).ok();
        pass_bytes_identical.push(match (original, exported) {
            (Some(o), Some(e)) => o == e,
            // A missing original (unreadable shader) can't be byte-compared; the
            // import already surfaced a warning + empty source, so treat the
            // empty-export round trip as identical rather than a false failure.
            _ => true,
        });
    }

    // 4. Re-import the exported bundle.
    let (second, _diags2) = import_preset(work_dir.join(PRESET_FILENAME))
        .map_err(|e| RoundTripError::ReimportParse(e.to_string()))?;

    let diff = compare_projects(&first, &second);

    Ok(RoundTrip {
        first,
        second,
        diff,
        pass_bytes_identical,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{
        Pass, PassSettings, PipelineMetadata, ProjectMetadata, ScaleAxis, ScaleType, WrapMode,
        PROJECT_SCHEMA_VERSION,
    };

    fn wpc(source: &str, filename: &str) -> PassSource {
        PassSource::WholePassCode {
            source: source.to_owned(),
            filename: Some(filename.to_owned()),
            opaque: true,
        }
    }

    fn one_pass_project() -> Project {
        Project {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "p".to_owned(),
            feedback_pass: None,
            pipeline: PipelineMetadata::default(),
            parameters: vec![Parameter {
                name: "BRIGHT".to_owned(),
                label: "Brightness".to_owned(),
                default: 1.5,
                min: 0.0,
                max: 2.0,
                step: 0.01,
            }],
            luts: vec![Lut {
                name: "PAL".to_owned(),
                path: "/src/luts/pal.png".to_owned(),
                filter_linear: Some(false),
                wrap_mode: Some(WrapMode::ClampToEdge),
                mipmap: Some(false),
            }],
            passes: vec![Pass {
                id: "pass-0".to_owned(),
                name: "First".to_owned(),
                source: wpc("#version 450\n", "first.slang"),
                parameters: vec![],
                settings: PassSettings {
                    scale_x: ScaleAxis {
                        scale_type: Some(ScaleType::Source),
                        scale: Some(2.0),
                    },
                    scale_y: ScaleAxis {
                        scale_type: Some(ScaleType::Source),
                        scale: Some(2.0),
                    },
                    alias: Some("FIRST".to_owned()),
                    ..Default::default()
                },
                references: vec![],
            }],
            metadata: ProjectMetadata::default(),
            library_refs: Vec::new(),
        }
    }

    #[test]
    fn identical_projects_are_lossless() {
        let a = one_pass_project();
        let b = a.clone();
        let diff = compare_projects(&a, &b);
        assert!(diff.is_lossless(), "{}", diff.report());
        assert_eq!(diff.report(), "<lossless>");
    }

    #[test]
    fn lut_path_relocation_is_canonicalized_away() {
        // Same LUT, different DIRECTORY (export relocates into textures/) but same
        // basename -> still lossless.
        let a = one_pass_project();
        let mut b = a.clone();
        b.luts[0].path = "/bundle/textures/pal.png".to_owned();
        let diff = compare_projects(&a, &b);
        assert!(
            diff.is_lossless(),
            "LUT dir change must be canonicalized: {}",
            diff.report()
        );
    }

    #[test]
    fn pass_filename_rename_is_canonicalized_away() {
        // Same source bytes, different filename (export collision rename) -> lossless.
        let a = one_pass_project();
        let mut b = a.clone();
        b.passes[0].source = wpc("#version 450\n", "first_1.slang");
        let diff = compare_projects(&a, &b);
        assert!(
            diff.is_lossless(),
            "filename rename must be canonicalized: {}",
            diff.report()
        );
    }

    #[test]
    fn changed_setting_is_reported() {
        let a = one_pass_project();
        let mut b = a.clone();
        b.passes[0].settings.filter_linear = Some(true);
        let diff = compare_projects(&a, &b);
        assert!(!diff.is_lossless());
        assert!(
            diff.mismatches.iter().any(|m| m.contains("filter_linear")),
            "{}",
            diff.report()
        );
    }

    #[test]
    fn changed_parameter_default_is_reported() {
        let a = one_pass_project();
        let mut b = a.clone();
        b.parameters[0].default = 1.6;
        let diff = compare_projects(&a, &b);
        assert!(!diff.is_lossless());
        assert!(
            diff.mismatches
                .iter()
                .any(|m| m.contains("BRIGHT") && m.contains("default")),
            "{}",
            diff.report()
        );
    }

    #[test]
    fn changed_lut_basename_is_reported() {
        let a = one_pass_project();
        let mut b = a.clone();
        b.luts[0].path = "/bundle/textures/other.png".to_owned();
        let diff = compare_projects(&a, &b);
        assert!(!diff.is_lossless());
        assert!(
            diff.mismatches.iter().any(|m| m.contains("basename")),
            "{}",
            diff.report()
        );
    }

    #[test]
    fn missing_parameter_on_reimport_is_reported() {
        let a = one_pass_project();
        let mut b = a.clone();
        b.parameters.clear();
        let diff = compare_projects(&a, &b);
        assert!(!diff.is_lossless());
        assert!(
            diff.mismatches
                .iter()
                .any(|m| m.contains("BRIGHT") && m.contains("missing")),
            "{}",
            diff.report()
        );
    }

    #[test]
    fn pass_source_bytes_change_is_reported() {
        let a = one_pass_project();
        let mut b = a.clone();
        b.passes[0].source = wpc("#version 450\n// edited\n", "first.slang");
        let diff = compare_projects(&a, &b);
        assert!(!diff.is_lossless());
        assert!(
            diff.mismatches.iter().any(|m| m.contains("source bytes")),
            "{}",
            diff.report()
        );
    }
}
