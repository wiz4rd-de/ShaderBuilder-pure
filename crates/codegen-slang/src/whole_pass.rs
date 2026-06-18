//! Whole-pass code-node **pass-through** + manifest extraction (#43).
//!
//! A pass is *either* a typed [`IrGraph`](core_model::ir::IrGraph) lowered +
//! emitted by [`emit`](crate::emit) (the node path), *or* a verbatim whole-pass
//! `.slang` source taken as-is (the escape hatch,
//! [`PassSource::WholePassCode`]). This module is the codegen side of the second
//! path. Per Architecture §C a whole-pass body is **opaque** — it is never
//! decomposed into node-IR — so codegen does the two minimal things a whole-pass
//! node needs to participate in the pipeline:
//!
//! 1. [`whole_pass_source`] returns the source **byte-for-byte unchanged**. This
//!    is the "emit" for a whole-pass node: the author's slang is the codegen
//!    output, verbatim (no normalization of line endings, trailing whitespace, or
//!    BOM), so a node that wraps imported slang re-emits identically.
//! 2. [`scan_whole_pass`] extracts the node's declared `#pragma parameter`s and
//!    the RetroArch textures it samples into a [`WholePassManifest`] for pipeline
//!    wiring — **reusing** `preset-io`'s existing scanners
//!    ([`scan_parameters`](preset_io::scan_parameters) /
//!    [`scan_references`](preset_io::scan_references)) rather than re-parsing the
//!    source here.
//!
//! [`PassSource::WholePassCode`]: core_model::PassSource::WholePassCode

use std::collections::BTreeSet;

use core_model::{Parameter, PassSource, TextureRef};

/// What a whole-pass code node contributes to pipeline wiring: the
/// `#pragma parameter`s it declares and the RetroArch textures it references.
///
/// This is the whole-pass analogue of [`ir::PassManifest`](ir::PassManifest):
/// where the node path *derives* its manifest from the typed graph, a whole-pass
/// node's manifest is *scanned* from the opaque source (a whole-pass body is
/// never decomposed, Architecture §C). Both feed the same downstream question —
/// "what parameters + textures does this pass need wired?" — so the surrounding
/// pipeline can treat node passes and whole-pass passes uniformly.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WholePassManifest {
    /// The `#pragma parameter` declarations found in the source, in declaration
    /// order. Scanned tolerantly via [`preset_io::scan_parameters`] (a malformed
    /// pragma is skipped, not fatal); does **not** resolve `#include`d headers.
    pub parameters: Vec<Parameter>,
    /// The RetroArch textures/aliases the source references, deduplicated and
    /// sorted by name. Classified by [`preset_io::scan_references`] against the
    /// §7 binding table plus the supplied alias/LUT name tables.
    pub textures: Vec<TextureRef>,
}

/// The verbatim slang source of a whole-pass code node — **byte-for-byte
/// unchanged**. This is codegen's output for the [`PassSource::WholePassCode`]
/// path: the author's source *is* the generated shader, taken as-is (the node-IR
/// lowering is bypassed entirely).
///
/// Returns the inner `source` for [`PassSource::WholePassCode`], or `None` for
/// [`PassSource::Graph`] (which goes through [`emit_pass`](crate::emit_pass)
/// instead).
pub fn whole_pass_source(source: &PassSource) -> Option<&str> {
    match source {
        PassSource::WholePassCode { source, .. } => Some(source.as_str()),
        PassSource::Graph { .. } => None,
    }
}

/// Scan a whole-pass `.slang` `source` into its [`WholePassManifest`]: the
/// declared `#pragma parameter`s and the textures it references.
///
/// `aliases` are the pipeline's known pass-alias names (`#pragma name` / `aliasN`
/// values) and `lut_names` the project's LUT names — a referenced identifier
/// matching one is classified as [`core_model::TextureRefKind::Alias`] (its
/// `…Feedback` twin as `Feedback`). Pass empty sets when none are known.
///
/// This is the single place the whole-pass path touches the source text, and it
/// delegates entirely to `preset-io`'s scanners — no parsing is re-implemented
/// here. Malformed `#pragma parameter` lines are tolerated (skipped); the scan
/// never fails.
pub fn scan_whole_pass(
    source: &str,
    aliases: &BTreeSet<String>,
    lut_names: &BTreeSet<String>,
) -> WholePassManifest {
    // Reuse preset-io's tolerant `#pragma parameter` scanner. We surface no
    // warnings from the codegen path (the importer is the one that diagnoses
    // malformed pragmas); a throwaway sink keeps the shared signature.
    let mut warnings = Vec::new();
    let parameters = preset_io::scan_parameters(source, &mut warnings);
    let textures = preset_io::scan_references(source, aliases, lut_names);
    WholePassManifest {
        parameters,
        textures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::TextureRefKind;

    const WHOLE_PASS: &str = "#version 450\n\
        #pragma parameter WARP \"Warp amount\" 0.5 0.0 1.0 0.01\n\
        #pragma parameter GAMMA \"Gamma\" 2.2 1.0 3.0\n\
        #pragma stage fragment\n\
        layout(set = 0, binding = 2) uniform sampler2D Source;\n\
        layout(set = 0, binding = 3) uniform sampler2D PassOutput0;\n\
        layout(set = 0, binding = 4) uniform sampler2D BORDER;\n\
        void main() { FragColor = texture(Source, vTexCoord); }\n";

    #[test]
    fn whole_pass_source_is_byte_for_byte_unchanged() {
        // Deliberately gnarly: CRLF, trailing whitespace, no trailing newline, a
        // leading BOM — none of which must be normalized.
        let raw = "\u{FEFF}#version 450\r\n#pragma stage fragment   \r\nvoid main(){}";
        let pass = PassSource::WholePassCode {
            source: raw.to_owned(),
            filename: Some("imported.slang".to_owned()),
            opaque: true,
        };
        let out = whole_pass_source(&pass).expect("whole-pass yields its source");
        assert_eq!(out, raw, "source passes through byte-for-byte");
        // And it is the *same* bytes, not a re-encoding.
        assert_eq!(out.as_bytes(), raw.as_bytes());
    }

    #[test]
    fn graph_source_has_no_whole_pass_passthrough() {
        let pass = PassSource::Graph {
            graph: core_model::Graph::default(),
        };
        assert_eq!(whole_pass_source(&pass), None);
    }

    #[test]
    fn scan_extracts_declared_parameters_in_order() {
        let m = scan_whole_pass(WHOLE_PASS, &BTreeSet::new(), &BTreeSet::new());
        let names: Vec<&str> = m.parameters.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["WARP", "GAMMA"]);
        let warp = &m.parameters[0];
        assert_eq!(warp.label, "Warp amount");
        assert_eq!(warp.default, 0.5);
        assert_eq!(warp.min, 0.0);
        assert_eq!(warp.max, 1.0);
        assert_eq!(warp.step, 0.01);
        // Optional step defaults to 0.0.
        assert_eq!(m.parameters[1].step, 0.0);
    }

    #[test]
    fn scan_extracts_sampled_textures_classified() {
        let luts = BTreeSet::from(["BORDER".to_owned()]);
        let m = scan_whole_pass(WHOLE_PASS, &BTreeSet::new(), &luts);
        let kind = |name: &str| m.textures.iter().find(|t| t.name == name).map(|t| t.kind);
        assert_eq!(kind("Source"), Some(TextureRefKind::Source));
        assert_eq!(kind("PassOutput0"), Some(TextureRefKind::PassOutput));
        // A LUT named in the table is an Alias-classified reference.
        assert_eq!(kind("BORDER"), Some(TextureRefKind::Alias));
    }

    #[test]
    fn scan_is_tolerant_and_never_panics_on_malformed_pragma() {
        let src = "#pragma parameter ONLYNAME\n#pragma parameter GOOD \"G\" 0.5 0.0 1.0\n";
        let m = scan_whole_pass(src, &BTreeSet::new(), &BTreeSet::new());
        // The malformed line is skipped; the good one extracts.
        assert_eq!(m.parameters.len(), 1);
        assert_eq!(m.parameters[0].name, "GOOD");
    }
}
