//! The `scan_pass_source` Tauri command (#52): a thin wrapper over the Phase-3
//! `preset-io` textual scanners so the **frontend** can drive parameter sliders
//! (#53) and pipeline-view wiring (#46) for a WHOLE-PASS code node from its source
//! string, WITHOUT writing the source to disk first.
//!
//! A whole-pass code node is an opaque `.slang` body: it is never lowered into
//! node-IR (Architecture §C), so the only thing we recover from it is what the
//! import path already recovers — the `#pragma parameter` declarations
//! ([`scan_parameters`]) and the RetroArch textures it textually references
//! ([`scan_references`]). This command reuses those exact scanners (no
//! reimplementation in TS), keeping the taxonomy of recognised semantics in ONE
//! place. It is pure (no IO, no GPU) and infallible: a malformed `#pragma
//! parameter` line is simply skipped (tolerant import semantics).

use std::collections::BTreeSet;

use core_model::{Parameter, TextureRef};
use preset_io::{scan_parameters, scan_references};
use serde::Serialize;

/// What [`scan_pass_source`] recovers from a whole-pass `.slang` source string.
///
/// Not a `#[ts(export)]` type (so it generates no binding / can't drift): the
/// frontend declares the matching shape by hand. Both fields ARE already-bound
/// core-model types, so they need no conversion at the IPC boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// The `#pragma parameter` declarations, in declaration order (malformed
    /// lines skipped). The frontend renders these as sliders (#53).
    pub parameters: Vec<Parameter>,
    /// The RetroArch textures/aliases the body references, deduplicated and
    /// sorted by name. Drives the pipeline-view wiring (#46).
    pub references: Vec<TextureRef>,
}

/// Scan a whole-pass `.slang` `source` string for its declared parameters +
/// referenced textures (#52), reusing the Phase-3 import scanners.
///
/// `aliases` and `luts` are the preset-known alias / LUT names so a referenced
/// alias is classified as such (else it would look like an unknown identifier);
/// pass empty lists when authoring a fresh whole-pass node with no chain context.
#[tauri::command]
pub fn scan_pass_source(source: String, aliases: Vec<String>, luts: Vec<String>) -> ScanResult {
    // `scan_parameters` is tolerant: a malformed pragma is dropped (we ignore the
    // warnings here — the frontend pre-check is a nicety, not a gate).
    let mut warnings = Vec::new();
    let parameters = scan_parameters(&source, &mut warnings);

    let alias_set: BTreeSet<String> = aliases.into_iter().collect();
    let lut_set: BTreeSet<String> = luts.into_iter().collect();
    let references = scan_references(&source, &alias_set, &lut_set);

    ScanResult {
        parameters,
        references,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::TextureRefKind;

    const PASS: &str = "#version 450\n\
        #pragma parameter BRIGHT \"Brightness\" 1.0 0.0 2.0 0.01\n\
        #pragma parameter not a valid line\n\
        #pragma stage fragment\n\
        layout(set = 0, binding = 2) uniform sampler2D Source;\n\
        layout(set = 0, binding = 3) uniform sampler2D OriginalHistory2;\n\
        void main() { FragColor = texture(Source, vTexCoord) * texture(OriginalHistory2, vTexCoord); }\n";

    #[test]
    fn scans_pragma_parameters_skipping_malformed() {
        let result = scan_pass_source(PASS.to_owned(), vec![], vec![]);
        // The one well-formed pragma is recovered; the malformed line is skipped.
        assert_eq!(result.parameters.len(), 1);
        let p = &result.parameters[0];
        assert_eq!(p.name, "BRIGHT");
        assert_eq!(p.label, "Brightness");
        assert_eq!(p.default, 1.0);
    }

    #[test]
    fn scans_texture_references() {
        let result = scan_pass_source(PASS.to_owned(), vec![], vec![]);
        let kinds: Vec<(&str, &TextureRefKind)> = result
            .references
            .iter()
            .map(|r| (r.name.as_str(), &r.kind))
            .collect();
        assert!(kinds.contains(&("Source", &TextureRefKind::Source)));
        assert!(kinds.contains(&("OriginalHistory2", &TextureRefKind::History)));
    }

    #[test]
    fn classifies_a_known_alias_and_lut() {
        let src = "void main() { vec4 a = texture(MyAlias, uv); vec4 b = texture(BORDER, uv); }";
        let result = scan_pass_source(
            src.to_owned(),
            vec!["MyAlias".to_owned()],
            vec!["BORDER".to_owned()],
        );
        let by_name: std::collections::BTreeMap<&str, &TextureRefKind> = result
            .references
            .iter()
            .map(|r| (r.name.as_str(), &r.kind))
            .collect();
        assert_eq!(by_name.get("MyAlias"), Some(&&TextureRefKind::Alias));
        assert_eq!(by_name.get("BORDER"), Some(&&TextureRefKind::Alias));
    }
}
