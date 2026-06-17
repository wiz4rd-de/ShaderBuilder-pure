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
//!    the absolute location. The basename itself is compared modulo the export's
//!    two deterministic file-name rewrites: unsafe-char **sanitization** (a space
//!    → `_`, e.g. `psp border.png` → `psp_border.png`) and the collision **de-dup
//!    suffix** the writer appends when several LUTs resolve to the same source
//!    image (`foo.png` → `foo_3.png`). Both keep the LUT name + bytes intact.
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
            // Compare basenames modulo the export's two deterministic file-name
            // rewrites (see [`basenames_match`]): unsafe-char sanitization (a space
            // → `_`) and the collision de-dup suffix (`foo.png` → `foo_3.png` when
            // several LUTs share one source image). Both are identity rewrites the
            // writer performs by design — the LUT NAME, bytes, and samplers are
            // unchanged — so they are canonicalized away.
            if !basenames_match(&la.path, &lb.path) {
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

/// Whether two LUT path basenames are equal modulo the export's deterministic
/// file-name rewrites: (1) unsafe-char **sanitization** (`psp border.png` →
/// `psp_border.png`) and (2) the collision **de-dup suffix** the writer appends
/// before the extension when several LUTs resolve to the same source file
/// (`foo.png` → `foo_3.png`). The original (`a`) sanitized stem must be a prefix
/// of the re-imported (`b`) sanitized stem, with the remainder either empty or a
/// `_<digits>` de-dup suffix, and the extensions equal.
fn basenames_match(a: &str, b: &str) -> bool {
    let (a_stem, a_ext) = split_stem_ext(&sanitize_basename(path_basename(a)));
    let (b_stem, b_ext) = split_stem_ext(&sanitize_basename(path_basename(b)));
    if a_ext != b_ext {
        return false;
    }
    if a_stem == b_stem {
        return true;
    }
    // `b` may be `a` with a `_<digits>` de-dup suffix appended.
    match b_stem
        .strip_prefix(&a_stem)
        .and_then(|r| r.strip_prefix('_'))
    {
        Some(suffix) => !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// Split a file name into its stem and extension (the extension includes no dot;
/// `""` when there is none). `"foo.bar.png"` → `("foo.bar", "png")`.
fn split_stem_ext(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_owned(), ext.to_owned()),
        None => (name.to_owned(), String::new()),
    }
}

/// Replicate the export's file-name sanitization (`preset_io::export`'s
/// `sanitize_stem`): keep ASCII alphanumerics + `-`, `_`, `.`; map every other
/// char (spaces, path separators, …) to `_`. Kept in lock-step with the writer so
/// the round-trip compare canonicalizes exactly the rewrite the export performs.
fn sanitize_basename(s: &str) -> String {
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
    /// Parallel to [`Project::passes`]. A pass whose original is non-UTF-8 (see
    /// [`Self::non_utf8_passes`]) is recorded as `false` *and* listed there, so the
    /// caller can classify the intrinsic-model limitation distinctly from a bug.
    pub pass_bytes_identical: Vec<bool>,
    /// Indices of passes whose **original** `.slang` is not valid UTF-8. The model
    /// stores a pass body as a `String` (UTF-8), so such a file cannot round-trip
    /// byte-for-byte — an intrinsic limitation, not a writer bug. These are
    /// surfaced for the corpus harness to report as a documented exclusion rather
    /// than a silent byte loss.
    pub non_utf8_passes: Vec<usize>,
    /// FINDING A2 — fields the **source** `.slangp` carried that import could drop
    /// *before* either side of [`compare_projects`] sees them (so a first-import vs
    /// re-import diff would be falsely empty). One readable line per source field
    /// (a `parameter_overrides` value, the `feedback_pass`, or a `luts` name) that
    /// is NOT reflected after the round trip. Empty when the source survived.
    pub source_loss: Vec<String>,
}

impl RoundTrip {
    /// Whether the round trip preserved structure (the diff), the per-pass `.slang`
    /// bytes, AND every salient field the SOURCE preset carried (FINDING A2) — the
    /// full lossless verdict. A non-UTF-8 original (which the `String` model cannot
    /// hold) makes this `false`; check [`Self::non_utf8_passes`] to distinguish
    /// that intrinsic limitation from a genuine byte-loss bug.
    pub fn is_lossless(&self) -> bool {
        self.diff.is_lossless()
            && self.pass_bytes_identical.iter().all(|&b| b)
            && self.source_loss.is_empty()
    }

    /// Whether every byte mismatch is attributable to a non-UTF-8 original (so the
    /// only "loss" is the intrinsic UTF-8-model limitation, and structure is
    /// otherwise lossless). Used by the corpus harness to classify a finding as a
    /// documented exclusion rather than a failure.
    pub fn only_non_utf8_loss(&self) -> bool {
        self.diff.is_lossless()
            && self.source_loss.is_empty()
            && self
                .pass_bytes_identical
                .iter()
                .enumerate()
                .all(|(i, &ok)| ok || self.non_utf8_passes.contains(&i))
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
        if !self.non_utf8_passes.is_empty() {
            out.push_str(&format!(
                "  (pass index(es) {:?} have a non-UTF-8 original — the String model \
                 cannot round-trip those bytes)\n",
                self.non_utf8_passes
            ));
        }
        if !self.source_loss.is_empty() {
            out.push_str(&format!(
                "{} source field(s) were dropped on IMPORT (absent in both \
                 first & re-import, so the structural diff is blind to them):\n",
                self.source_loss.len()
            ));
            for m in &self.source_loss {
                out.push_str("  - ");
                out.push_str(m);
                out.push('\n');
            }
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
    //    A non-UTF-8 original is noted separately — the `String` model can't hold
    //    its bytes, so the export of an empty source legitimately differs (an
    //    intrinsic limitation, classified, not silently passed).
    let mut pass_bytes_identical = Vec::with_capacity(first.passes.len());
    let mut non_utf8_passes = Vec::new();
    for (i, (pass, exported_name)) in preset.passes.iter().zip(&report.pass_files).enumerate() {
        let original = std::fs::read(&pass.shader).ok();
        let exported = std::fs::read(work_dir.join(exported_name)).ok();
        if let Some(o) = &original {
            if std::str::from_utf8(o).is_err() {
                non_utf8_passes.push(i);
            }
        }
        pass_bytes_identical.push(pass_bytes_match(original.as_deref(), exported.as_deref()));
    }

    // 4. Re-import the exported bundle.
    let (second, _diags2) = import_preset(work_dir.join(PRESET_FILENAME))
        .map_err(|e| RoundTripError::ReimportParse(e.to_string()))?;

    let diff = compare_projects(&first, &second);

    // 5. FINDING A2: also compare against the SOURCE preset. `compare_projects`
    //    only sees first-import vs re-import, so any field IMPORT drops is absent
    //    on BOTH sides and the diff is falsely empty. The exported preset text is
    //    read so a value re-emitted as a bare `id = value` (rather than landing in
    //    `second.parameters`) still counts as reflected.
    let exported_text = std::fs::read_to_string(work_dir.join(PRESET_FILENAME)).unwrap_or_default();
    let source_loss = compare_against_source(&preset, &second, &exported_text);

    Ok(RoundTrip {
        first,
        second,
        diff,
        pass_bytes_identical,
        non_utf8_passes,
        source_loss,
    })
}

/// FINDING A2 — verify that every salient field the **source** `.slangp` carried
/// is reflected after the round trip, catching loss that happens at IMPORT (before
/// either side of [`compare_projects`] sees it). Checks:
///
/// * every float-valued `parameter_overrides` id is reflected — either as the
///   overridden value in `second.parameters`, or re-emitted as a bare
///   `id = value` line in the exported preset text;
/// * `feedback_pass` (a non-negative source value) survives in `second`;
/// * every `luts` name survives in `second`.
///
/// Returns one readable line per dropped source field; empty when all survived.
fn compare_against_source(
    source: &preset_io::Preset,
    second: &Project,
    exported_text: &str,
) -> Vec<String> {
    let mut loss = Vec::new();

    // Parameter overrides: the tuned value must be reflected after the round trip.
    let by_id: BTreeMap<&str, &Parameter> = second
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();
    for (id, &value) in &source.parameter_overrides {
        let in_params = by_id
            .get(id.as_str())
            .is_some_and(|p| p.default.to_bits() == value.to_bits());
        // Re-emitted verbatim as a bare `id = value` line (the export's inline
        // override path) also counts as reflected.
        let in_text = exported_text.lines().any(|line| {
            line.split_once('=')
                .map(|(k, v)| k.trim() == id && v.trim().parse::<f32>().ok() == Some(value))
                .unwrap_or(false)
        });
        if !in_params && !in_text {
            loss.push(format!(
                "parameter override `{id} = {value}` was dropped on import \
                 (not in re-imported parameters nor the exported preset text)"
            ));
        }
    }

    // feedback_pass: a non-negative source value must survive.
    if let Some(fp) = source.feedback_pass {
        if fp >= 0 && second.feedback_pass != Some(fp as u32) {
            loss.push(format!(
                "feedback_pass {fp} was not reflected after round trip (got {:?})",
                second.feedback_pass
            ));
        }
    }

    // Every source LUT name must survive into the re-imported project.
    let reimported_luts: std::collections::BTreeSet<&str> =
        second.luts.iter().map(|l| l.name.as_str()).collect();
    for lut in &source.luts {
        if !reimported_luts.contains(lut.name.as_str()) {
            loss.push(format!(
                "LUT `{}` (in source preset) is missing after round trip",
                lut.name
            ));
        }
    }

    loss
}

/// Whether a pass's exported `.slang` bytes match its original (#34/#36). FINDING
/// A3: the arms must be split so a present original with a MISSING export is a
/// failure, not silently treated as identical:
///
/// * `(None, _)` — a missing/unreadable **original** can't be byte-compared; the
///   import already surfaced a warning + empty source, so this is *not* a failure.
/// * `(Some(_), None)` — a present original but a missing/unreadable **export** is
///   a real failure (the writer dropped or could not write the file).
/// * `(Some(o), Some(e))` — both present: compare bytes.
fn pass_bytes_match(original: Option<&[u8]>, exported: Option<&[u8]>) -> bool {
    match (original, exported) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(o), Some(e)) => o == e,
    }
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
    fn lut_space_in_filename_sanitization_is_canonicalized_away() {
        // The export rewrites a space to `_` (psp border.png -> psp_border.png).
        let mut a = one_pass_project();
        a.luts[0].path = "/src/psp border.png".to_owned();
        let mut b = a.clone();
        b.luts[0].path = "/bundle/textures/psp_border.png".to_owned();
        let diff = compare_projects(&a, &b);
        assert!(
            diff.is_lossless(),
            "space sanitization must be canonicalized: {}",
            diff.report()
        );
    }

    #[test]
    fn lut_collision_dedup_suffix_is_canonicalized_away() {
        // Several LUTs sharing one source file get a de-dup suffix on export
        // (placeholder.png -> placeholder_3.png). The LUT NAME + bytes are
        // unchanged, so this is canonicalized.
        let a = one_pass_project();
        let mut b = a.clone();
        b.luts[0].path = "/bundle/textures/pal_3.png".to_owned();
        let diff = compare_projects(&a, &b);
        assert!(
            diff.is_lossless(),
            "de-dup suffix must be canonicalized: {}",
            diff.report()
        );
    }

    #[test]
    fn basenames_match_only_for_sanitize_or_dedup_rewrites() {
        // Identity, sanitization, and de-dup match…
        assert!(basenames_match("a/pal.png", "b/pal.png"));
        assert!(basenames_match("a/psp border.png", "b/psp_border.png"));
        assert!(basenames_match("a/pal.png", "b/pal_3.png"));
        assert!(basenames_match("a/pal.png", "b/pal_12.png"));
        // …but a genuinely different file, a non-numeric suffix, or a changed
        // extension does NOT.
        assert!(!basenames_match("a/pal.png", "b/other.png"));
        assert!(!basenames_match("a/pal.png", "b/pal_x.png"));
        assert!(!basenames_match("a/pal.png", "b/pal.jpg"));
        assert!(!basenames_match("a/pal.png", "b/pal.png.bak"));
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

    // ---- FINDING A3: byte-identity arm split --------------------------------

    #[test]
    fn pass_bytes_match_splits_missing_export_from_missing_original() {
        // A missing/unreadable ORIGINAL is tolerated (import warned + empty source).
        assert!(pass_bytes_match(None, None));
        assert!(pass_bytes_match(None, Some(b"anything")));
        // Equal bytes match; differing bytes don't.
        assert!(pass_bytes_match(Some(b"abc"), Some(b"abc")));
        assert!(!pass_bytes_match(Some(b"abc"), Some(b"abd")));
        // A present original but a MISSING export is a FAILURE (the writer dropped
        // the file) — it must NOT be swallowed as identical. This is the bug arm.
        assert!(
            !pass_bytes_match(Some(b"present"), None),
            "a present original with a missing export must be a failure, not 'identical'"
        );
    }

    // ---- FINDING A2: oracle sees import-side loss ---------------------------

    fn parse_source(text: &str) -> preset_io::Preset {
        preset_io::parse_slangp_str(text, Path::new("/p")).expect("source preset parses")
    }

    #[test]
    fn source_compare_flags_a_dropped_override() {
        // If an override id is reflected NEITHER in the re-imported parameters NOR
        // in the exported preset text, compare_against_source must flag it — this
        // is exactly the loss FINDING A1 had that the first-vs-re-import diff was
        // blind to.
        let source = parse_source("shaders = 1\nshader0 = a.slang\nHEADER_KNOB = 0.625\n");
        // A `second` project that DROPPED the override (no such parameter) and an
        // exported text that never re-emitted it.
        let second = one_pass_project(); // has no HEADER_KNOB parameter
        let loss = compare_against_source(&source, &second, "shaders = 1\nshader0 = a.slang\n");
        assert!(
            loss.iter()
                .any(|m| m.contains("HEADER_KNOB") && m.contains("dropped on import")),
            "dropped override must be flagged: {loss:?}"
        );
    }

    #[test]
    fn source_compare_accepts_override_reflected_in_params_or_text() {
        let source = parse_source("shaders = 1\nshader0 = a.slang\nHEADER_KNOB = 0.625\n");
        // (a) reflected as a re-imported parameter carrying the value.
        let mut second = one_pass_project();
        second.parameters.push(Parameter {
            name: "HEADER_KNOB".to_owned(),
            label: "HEADER_KNOB".to_owned(),
            default: 0.625,
            min: 0.625,
            max: 0.625,
            step: 0.0,
        });
        assert!(
            compare_against_source(&source, &second, "shaders = 1\n").is_empty(),
            "override reflected in re-imported parameters is not a loss"
        );
        // (b) reflected only as a bare `id = value` line in the exported text.
        let plain = one_pass_project();
        assert!(
            compare_against_source(
                &source,
                &plain,
                "shaders = 1\nshader0 = a.slang\nHEADER_KNOB = 0.625\n"
            )
            .is_empty(),
            "override re-emitted in the exported text is not a loss"
        );
    }

    #[test]
    fn source_compare_flags_dropped_feedback_pass_and_lut() {
        let source = parse_source(
            "shaders = 1\n\
             shader0 = a.slang\n\
             feedback_pass = 0\n\
             textures = PAL\n\
             PAL = pal.png\n",
        );
        // `second` lost the feedback pass and the LUT.
        let mut second = one_pass_project();
        second.feedback_pass = None;
        second.luts.clear();
        let loss = compare_against_source(&source, &second, "shaders = 1\n");
        assert!(
            loss.iter().any(|m| m.contains("feedback_pass")),
            "dropped feedback_pass flagged: {loss:?}"
        );
        assert!(
            loss.iter().any(|m| m.contains("PAL")),
            "dropped LUT flagged: {loss:?}"
        );
    }

    #[test]
    fn orphan_override_round_trips_through_full_harness() {
        // End-to-end A1+A2: a preset whose pass body declares NO pragma for the
        // overridden id (it would live in an #include) round-trips losslessly now,
        // and the strengthened oracle confirms the source value survived.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.slang"),
            "#version 450\n#include \"knobs.inc\"\nvoid main(){}\n",
        )
        .unwrap();
        let slangp = dir.path().join("orphan.slangp");
        std::fs::write(
            &slangp,
            "shaders = 1\nshader0 = a.slang\nHEADER_KNOB = 0.625\n",
        )
        .unwrap();

        let work = tempfile::tempdir().unwrap();
        let rt = round_trip(&slangp, work.path()).expect("round trip");
        assert!(
            rt.is_lossless(),
            "orphan override must round-trip losslessly:\n{}",
            rt.report()
        );
        assert!(
            rt.source_loss.is_empty(),
            "no source loss: {:?}",
            rt.source_loss
        );
        // The value is reflected in the re-imported parameters.
        let knob = rt
            .second
            .parameters
            .iter()
            .find(|p| p.name == "HEADER_KNOB")
            .expect("orphan override preserved as a parameter after round trip");
        assert_eq!(knob.default, 0.625);
    }
}
