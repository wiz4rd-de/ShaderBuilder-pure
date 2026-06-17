//! Light **textual** scan of a whole-pass `.slang` source for the RetroArch
//! textures/aliases it references (`docs/retroarch-slang-runtime.md` §7).
//!
//! This is *not* a parser. Per Architecture §C, a whole-pass body is **opaque**
//! and is never decomposed into node-IR — so this module deliberately does the
//! shallowest thing that recovers enough for pipeline wiring + a LUT cross-check:
//! it walks the source's identifier tokens, ignoring comment and string spans,
//! and classifies any token that names a known RetroArch sampler semantic
//! (`Original`, `Source`, `PassOutputK`/`PassK`, `PassFeedbackK`,
//! `OriginalHistoryK`, `UserK`) or any caller-supplied alias/LUT name.
//!
//! It does **not** try to confirm the token is a `sampler2D` declaration, an
//! actual `texture(...)` read, or even fragment-stage code: identifying *where*
//! a read happens would require parsing the body, which is exactly what §C says
//! we must not do. A spuriously-matched identifier (e.g. a local variable named
//! `Source`) is harmless — it only widens the wiring/availability cross-check —
//! and is overwhelmingly unlikely given these are reserved RetroArch names.

use std::collections::{BTreeMap, BTreeSet};

use core_model::{TextureRef, TextureRefKind};

/// Scan `source` for referenced RetroArch textures/aliases, classifying each by
/// the §7 binding table. `aliases` and `lut_names` are the names known to the
/// preset (pass `#pragma name`/`aliasN` values and `textures=` LUT names); a
/// matched identifier that is one of them is classified as
/// [`TextureRefKind::Alias`] (including its `…Feedback` twin).
///
/// Returns the references **deduplicated** and in a deterministic (sorted by
/// name) order, so import output is stable.
pub fn scan_references(
    source: &str,
    aliases: &BTreeSet<String>,
    lut_names: &BTreeSet<String>,
) -> Vec<TextureRef> {
    // Dedup by name; the classification is deterministic per name, so the first
    // (and every) occurrence agrees. `BTreeMap` keeps the output sorted by name.
    let mut found: BTreeMap<String, TextureRefKind> = BTreeMap::new();

    for ident in identifiers(source) {
        if let Some(kind) = classify(ident, aliases, lut_names) {
            found.entry(ident.to_owned()).or_insert(kind);
        }
    }

    found
        .into_iter()
        .map(|(name, kind)| TextureRef { name, kind })
        .collect()
}

/// Classify a single identifier against the §7 semantics + the preset's alias /
/// LUT name tables. `None` if it is not a texture reference we recognize.
fn classify(
    ident: &str,
    aliases: &BTreeSet<String>,
    lut_names: &BTreeSet<String>,
) -> Option<TextureRefKind> {
    // Preset-declared names take precedence (the runtime adds aliases + LUT
    // names to the binding table before reflection, §7 rule 3). An alias is
    // bindable both directly and as `<alias>Feedback`.
    if aliases.contains(ident) || lut_names.contains(ident) {
        return Some(TextureRefKind::Alias);
    }
    if let Some(base) = ident.strip_suffix("Feedback") {
        if aliases.contains(base) {
            return Some(TextureRefKind::Feedback);
        }
    }

    // Built-in, non-indexed semantics.
    match ident {
        "Original" => return Some(TextureRefKind::Original),
        "Source" => return Some(TextureRefKind::Source),
        _ => {}
    }

    // Built-in, index-suffixed semantics: `<base><digits>`.
    for (base, kind) in [
        ("PassOutput", TextureRefKind::PassOutput),
        ("Pass", TextureRefKind::PassOutput), // `PassK` is the accepted PassOutputK spelling
        ("PassFeedback", TextureRefKind::Feedback),
        ("OriginalHistory", TextureRefKind::History),
        ("User", TextureRefKind::User),
    ] {
        if let Some(rest) = ident.strip_prefix(base) {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                return Some(kind);
            }
        }
    }
    None
}

/// Yield every GLSL identifier token in `source`, skipping `//`/`/* */` comment
/// spans and `"…"` string literals so an identifier-looking word inside a
/// comment or path string is not treated as a texture reference. A GLSL
/// identifier is `[A-Za-z_][A-Za-z0-9_]*`; a run starting with a digit (a number)
/// is skipped.
fn identifiers(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            // Line comment.
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // Block comment.
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 2; // skip the closing */ (saturating past EOF is fine)
            }
            // String literal (e.g. an #include path) — skip to the close quote.
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    // No escape handling needed: a `\"` inside a slang path is
                    // not a thing, and over-skipping a literal can't create a
                    // false texture match.
                    i += 1;
                }
                i += 1;
            }
            // Identifier start.
            _ if is_ident_start(b) => {
                let start = i;
                i += 1;
                while i < bytes.len() && is_ident_continue(bytes[i]) {
                    i += 1;
                }
                // `source` is valid UTF-8 and identifiers are ASCII, so this
                // byte range is a valid str slice.
                out.push(&source[start..i]);
            }
            // A digit-led run is a number, not an identifier — skip it whole so
            // a trailing identifier char can't accidentally start mid-number.
            _ if b.is_ascii_digit() => {
                i += 1;
                while i < bytes.len() && is_ident_continue(bytes[i]) {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// GLSL identifier-start byte: ASCII letter or underscore.
fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

/// GLSL identifier-continue byte: letter, digit, or underscore.
fn is_ident_continue(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(refs: &[TextureRef]) -> Vec<&str> {
        refs.iter().map(|r| r.name.as_str()).collect()
    }

    fn kind_of<'a>(refs: &'a [TextureRef], name: &str) -> Option<&'a TextureRefKind> {
        refs.iter().find(|r| r.name == name).map(|r| &r.kind)
    }

    #[test]
    fn classifies_builtin_semantics() {
        let src = r#"
            layout(set = 0, binding = 2) uniform sampler2D Source;
            layout(set = 0, binding = 3) uniform sampler2D Original;
            layout(set = 0, binding = 4) uniform sampler2D PassOutput0;
            layout(set = 0, binding = 5) uniform sampler2D Pass1;
            layout(set = 0, binding = 6) uniform sampler2D PassFeedback2;
            layout(set = 0, binding = 7) uniform sampler2D OriginalHistory3;
            layout(set = 0, binding = 8) uniform sampler2D User0;
        "#;
        let refs = scan_references(src, &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(kind_of(&refs, "Source"), Some(&TextureRefKind::Source));
        assert_eq!(kind_of(&refs, "Original"), Some(&TextureRefKind::Original));
        assert_eq!(
            kind_of(&refs, "PassOutput0"),
            Some(&TextureRefKind::PassOutput)
        );
        assert_eq!(kind_of(&refs, "Pass1"), Some(&TextureRefKind::PassOutput));
        assert_eq!(
            kind_of(&refs, "PassFeedback2"),
            Some(&TextureRefKind::Feedback)
        );
        assert_eq!(
            kind_of(&refs, "OriginalHistory3"),
            Some(&TextureRefKind::History)
        );
        assert_eq!(kind_of(&refs, "User0"), Some(&TextureRefKind::User));
    }

    #[test]
    fn classifies_aliases_and_lut_names() {
        let aliases = BTreeSet::from(["BlurPass".to_owned()]);
        let luts = BTreeSet::from(["BORDER".to_owned()]);
        let src = "texture(BlurPass, c); texture(BlurPassFeedback, c); texture(BORDER, c);";
        let refs = scan_references(src, &aliases, &luts);
        assert_eq!(kind_of(&refs, "BlurPass"), Some(&TextureRefKind::Alias));
        assert_eq!(
            kind_of(&refs, "BlurPassFeedback"),
            Some(&TextureRefKind::Feedback)
        );
        assert_eq!(kind_of(&refs, "BORDER"), Some(&TextureRefKind::Alias));
    }

    #[test]
    fn ignores_comments_and_strings() {
        let src = r#"
            // Source is mentioned in a line comment
            /* and Original in a block comment */
            #include "PassOutput9.slang"
            uniform sampler2D Source;
        "#;
        let refs = scan_references(src, &BTreeSet::new(), &BTreeSet::new());
        // Only the real `Source` declaration survives the comment/string filter.
        assert_eq!(names(&refs), vec!["Source"]);
    }

    #[test]
    fn plain_identifiers_are_not_references() {
        // Words that merely look texture-ish but aren't a recognized semantic.
        let src = "vec4 color = OriginalColor + Passenger + Sources;";
        let refs = scan_references(src, &BTreeSet::new(), &BTreeSet::new());
        assert!(refs.is_empty(), "no false positives: {:?}", names(&refs));
    }

    #[test]
    fn output_is_deduplicated_and_sorted() {
        let src = "Source; PassOutput1; Source; Original; PassOutput1;";
        let refs = scan_references(src, &BTreeSet::new(), &BTreeSet::new());
        assert_eq!(names(&refs), vec!["Original", "PassOutput1", "Source"]);
    }
}
