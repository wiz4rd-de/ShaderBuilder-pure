//! `#pragma parameter` extraction + reconciliation for preset import (#35).
//!
//! RetroArch exposes runtime knobs via per-pass
//! `#pragma parameter <id> "<label>" <default> <min> <max> [<step>]` lines
//! (`docs/retroarch-slang-runtime.md` §8). Parameters are **global by id**: the
//! same id declared (identically) across several passes is *one* knob, and a bare
//! `id = value` line in the `.slangp` overrides that knob's initial value.
//!
//! This module does the import-side bookkeeping:
//! - [`scan_parameters`] pulls the `#pragma parameter` declarations out of one
//!   pass's source. It is deliberately **tolerant** (import must not fail on a
//!   shader it can't fully compile): a malformed `#pragma parameter` line is
//!   skipped and reported as a [`ParamWarning`] rather than aborting — unlike the
//!   strict `slang_compile::preprocess` parser, which errors. It also does not
//!   require a `#pragma stage`, so a bare snippet still yields its parameters.
//! - [`reconcile_parameters`] collapses the per-pass declarations into ONE
//!   project parameter per id (documented rule below) and applies the `.slangp`
//!   per-parameter overrides (the preset value wins).
//!
//! ## Reconciliation rule (duplicate ids across passes)
//!
//! The **first** pass to declare an id defines the canonical parameter
//! (default/label/min/max/step). This matches RetroArch reflection order — the
//! first matching declaration binds — and is stable regardless of how many later
//! passes re-declare it. When a later pass re-declares the same id with a
//! **different** numeric field (default, min, max, or step) or a different label,
//! the first declaration is kept and a [`ParamWarning::Conflict`] is emitted so
//! the divergence is surfaced (the spec's "must match exactly … or error" is
//! softened to a diagnostic here so a real-world preset with a benign mismatch
//! still imports).

use std::collections::BTreeMap;

use core_model::Parameter;

/// A non-fatal problem found while extracting / reconciling parameters. Surfaced
/// by the import bridge as an [`crate::ImportDiagnostic`] rather than failing.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamWarning {
    /// A `#pragma parameter` line could not be parsed; carries the offending text.
    Malformed { line: String },
    /// An id was declared in more than one pass with a differing definition. The
    /// first declaration is kept; this records what diverged.
    Conflict {
        /// The parameter id that diverged.
        id: String,
        /// Human-readable description of the field(s) that differ.
        detail: String,
    },
}

/// Extract the `#pragma parameter` declarations from one pass's `.slang` source,
/// in declaration order. Lines that look like a parameter pragma but don't parse
/// are skipped and reported in `warnings` (tolerant — import must not fail here).
///
/// This does not resolve `#include`s: a parameter declared in an included header
/// is the includer's responsibility upstream. Whole-pass import reads the file
/// verbatim, so only that file's own `#pragma parameter` lines are seen — which
/// matches what the user authored in that pass file.
pub fn scan_parameters(source: &str, warnings: &mut Vec<ParamWarning>) -> Vec<Parameter> {
    let mut params = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("#pragma") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(arg) = rest.strip_prefix("parameter") else {
            continue;
        };
        // Require a separator after `parameter` so `#pragma parameterize` (a
        // hypothetical unrelated pragma) doesn't match.
        if !arg.starts_with(|c: char| c.is_whitespace()) {
            continue;
        }
        match parse_parameter(arg.trim()) {
            Some(p) => params.push(p),
            None => warnings.push(ParamWarning::Malformed {
                line: trimmed.to_string(),
            }),
        }
    }
    params
}

/// Parse `<id> "<label>" <default> <min> <max> [<step>]` from the text after
/// `#pragma parameter`. `None` if malformed (tolerant caller turns that into a
/// warning). Mirrors the strict `slang_compile` parser's grammar but never
/// panics or errors.
fn parse_parameter(arg: &str) -> Option<Parameter> {
    let (name, rest) = arg.split_once(char::is_whitespace)?;
    if name.is_empty() {
        return None;
    }
    let rest = rest.trim_start();

    // Label is a double-quoted string.
    let rest = rest.strip_prefix('"')?;
    let label_end = rest.find('"')?;
    let label = &rest[..label_end];
    let nums = &rest[label_end + 1..];

    let mut it = nums.split_whitespace();
    let mut next_f32 = || it.next().and_then(|s| s.parse::<f32>().ok());
    let default = next_f32()?;
    let min = next_f32()?;
    let max = next_f32()?;
    // STEP is optional; default to 0.0 (matches the strict parser).
    let step = it.next().map_or(Some(0.0), |s| s.parse::<f32>().ok())?;

    Some(Parameter {
        name: name.to_string(),
        label: label.to_string(),
        default,
        min,
        max,
        step,
    })
}

/// Collapse per-pass parameter declarations (`per_pass[i]` = pass `i`'s scanned
/// parameters, in declaration order) into ONE project parameter per id, then
/// apply the `.slangp` per-parameter `overrides` (the preset value wins over the
/// pragma default). See the module docs for the duplicate-id reconciliation rule.
///
/// Returns the reconciled parameters in **first-seen declaration order** (stable:
/// pass order, then within-pass order) plus any [`ParamWarning::Conflict`]s for
/// ids that diverged across passes.
pub fn reconcile_parameters(
    per_pass: &[Vec<Parameter>],
    overrides: &BTreeMap<String, f32>,
    warnings: &mut Vec<ParamWarning>,
) -> Vec<Parameter> {
    // First-seen wins; preserve declaration order via a parallel index map.
    let mut order: Vec<String> = Vec::new();
    let mut by_id: BTreeMap<String, Parameter> = BTreeMap::new();

    for pass_params in per_pass {
        for p in pass_params {
            match by_id.get(&p.name) {
                None => {
                    order.push(p.name.clone());
                    by_id.insert(p.name.clone(), p.clone());
                }
                Some(existing) => {
                    if let Some(detail) = describe_conflict(existing, p) {
                        warnings.push(ParamWarning::Conflict {
                            id: p.name.clone(),
                            detail,
                        });
                    }
                    // First declaration is canonical: keep `existing`.
                }
            }
        }
    }

    // Apply preset overrides onto the canonical default (the preset value wins).
    let mut reconciled: Vec<Parameter> = order
        .into_iter()
        .map(|id| by_id.remove(&id).expect("id was inserted above"))
        .collect();
    for p in &mut reconciled {
        if let Some(&value) = overrides.get(&p.name) {
            p.default = value;
        }
    }
    reconciled
}

/// Describe how two same-id parameter declarations differ, or `None` if they
/// match exactly. Floats are compared bitwise via `to_bits` so two pragma lines
/// that wrote the same literal agree (and `NaN` literals, though absurd here,
/// compare equal to themselves).
fn describe_conflict(a: &Parameter, b: &Parameter) -> Option<String> {
    let mut diffs = Vec::new();
    if a.label != b.label {
        diffs.push(format!("label ({:?} vs {:?})", a.label, b.label));
    }
    if a.default.to_bits() != b.default.to_bits() {
        diffs.push(format!("default ({} vs {})", a.default, b.default));
    }
    if a.min.to_bits() != b.min.to_bits() {
        diffs.push(format!("min ({} vs {})", a.min, b.min));
    }
    if a.max.to_bits() != b.max.to_bits() {
        diffs.push(format!("max ({} vs {})", a.max, b.max));
    }
    if a.step.to_bits() != b.step.to_bits() {
        diffs.push(format!("step ({} vs {})", a.step, b.step));
    }
    if diffs.is_empty() {
        None
    } else {
        Some(diffs.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(src: &str) -> (Vec<Parameter>, Vec<ParamWarning>) {
        let mut w = Vec::new();
        let p = scan_parameters(src, &mut w);
        (p, w)
    }

    #[test]
    fn scans_a_well_formed_parameter() {
        let (params, warns) =
            scan("#pragma parameter WARP \"Warp amount\" 0.5 0.0 1.0 0.01\nvoid main(){}\n");
        assert!(warns.is_empty());
        assert_eq!(params.len(), 1);
        let p = &params[0];
        assert_eq!(p.name, "WARP");
        assert_eq!(p.label, "Warp amount");
        assert_eq!(p.default, 0.5);
        assert_eq!(p.min, 0.0);
        assert_eq!(p.max, 1.0);
        assert_eq!(p.step, 0.01);
    }

    #[test]
    fn step_is_optional() {
        let (params, warns) = scan("#pragma parameter GAMMA \"Gamma\" 1.0 0.5 2.0\n");
        assert!(warns.is_empty());
        assert_eq!(params[0].step, 0.0);
    }

    #[test]
    fn scans_multiple_in_declaration_order() {
        let (params, _) = scan(
            "#pragma parameter A \"Alpha\" 1.0 0.0 2.0 0.1\n\
             #pragma parameter B \"Beta\" 0.0 -1.0 1.0\n",
        );
        assert_eq!(
            params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["A", "B"]
        );
    }

    #[test]
    fn leading_whitespace_is_tolerated() {
        let (params, warns) = scan("   #pragma parameter X \"X\" 0.0 0.0 1.0\n");
        assert!(warns.is_empty());
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn malformed_lines_warn_but_do_not_abort() {
        // Missing the numeric triple, and an unterminated label.
        let (params, warns) = scan(
            "#pragma parameter ONLYNAME\n\
             #pragma parameter NOQUOTE 1.0 0.0 1.0\n\
             #pragma parameter GOOD \"Good\" 0.5 0.0 1.0 0.01\n",
        );
        // The one good line still extracts.
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "GOOD");
        // Two malformed lines warned.
        assert_eq!(
            warns
                .iter()
                .filter(|w| matches!(w, ParamWarning::Malformed { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn parameterize_is_not_a_parameter() {
        // A pragma whose keyword merely starts with `parameter` must not match.
        let (params, warns) = scan("#pragma parameterize FOO\n");
        assert!(params.is_empty());
        assert!(warns.is_empty());
    }

    #[test]
    fn reconcile_first_declaration_wins_and_warns_on_conflict() {
        let pass0 = vec![Parameter {
            name: "GAMMA".to_owned(),
            label: "Gamma".to_owned(),
            default: 2.2,
            min: 1.0,
            max: 3.0,
            step: 0.1,
        }];
        // Same id, DIFFERENT max + step.
        let pass1 = vec![Parameter {
            name: "GAMMA".to_owned(),
            label: "Gamma".to_owned(),
            default: 2.2,
            min: 1.0,
            max: 4.0,
            step: 0.05,
        }];
        let mut warns = Vec::new();
        let out = reconcile_parameters(&[pass0, pass1], &BTreeMap::new(), &mut warns);
        // Collapsed to one knob, keeping the FIRST declaration's values.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].max, 3.0);
        assert_eq!(out[0].step, 0.1);
        // The divergence is reported.
        assert!(matches!(
            warns.as_slice(),
            [ParamWarning::Conflict { id, detail }]
                if id == "GAMMA" && detail.contains("max") && detail.contains("step")
        ));
    }

    #[test]
    fn reconcile_identical_redeclaration_is_silent() {
        let p = Parameter {
            name: "X".to_owned(),
            label: "X".to_owned(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        };
        let mut warns = Vec::new();
        let out = reconcile_parameters(
            &[vec![p.clone()], vec![p.clone()]],
            &BTreeMap::new(),
            &mut warns,
        );
        assert_eq!(out.len(), 1);
        assert!(warns.is_empty(), "identical re-declaration must not warn");
    }

    #[test]
    fn reconcile_applies_preset_override_to_default() {
        let pass0 = vec![Parameter {
            name: "BRIGHT".to_owned(),
            label: "Brightness".to_owned(),
            default: 1.0,
            min: 0.0,
            max: 2.0,
            step: 0.01,
        }];
        let overrides = BTreeMap::from([("BRIGHT".to_owned(), 1.5_f32)]);
        let mut warns = Vec::new();
        let out = reconcile_parameters(&[pass0], &overrides, &mut warns);
        assert_eq!(
            out[0].default, 1.5,
            "preset override wins over pragma default"
        );
        // Range/step come from the pragma, unchanged by the override.
        assert_eq!(out[0].min, 0.0);
        assert_eq!(out[0].max, 2.0);
        assert_eq!(out[0].step, 0.01);
        assert!(warns.is_empty());
    }

    #[test]
    fn reconcile_preserves_first_seen_order() {
        let pass0 = vec![
            Parameter {
                name: "B".into(),
                label: "B".into(),
                default: 0.0,
                min: 0.0,
                max: 1.0,
                step: 0.0,
            },
            Parameter {
                name: "A".into(),
                label: "A".into(),
                default: 0.0,
                min: 0.0,
                max: 1.0,
                step: 0.0,
            },
        ];
        let pass1 = vec![Parameter {
            name: "C".into(),
            label: "C".into(),
            default: 0.0,
            min: 0.0,
            max: 1.0,
            step: 0.0,
        }];
        let mut warns = Vec::new();
        let out = reconcile_parameters(&[pass0, pass1], &BTreeMap::new(), &mut warns);
        assert_eq!(
            out.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["B", "A", "C"],
            "declaration order is preserved (not alphabetized)"
        );
    }

    #[test]
    fn override_for_unknown_id_is_ignored() {
        // A bare `id = value` with no matching pragma has nothing to apply to.
        let pass0 = vec![Parameter {
            name: "KNOWN".into(),
            label: "K".into(),
            default: 1.0,
            min: 0.0,
            max: 2.0,
            step: 0.1,
        }];
        let overrides = BTreeMap::from([("UNKNOWN".to_owned(), 9.0_f32)]);
        let mut warns = Vec::new();
        let out = reconcile_parameters(&[pass0], &overrides, &mut warns);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "KNOWN");
        assert_eq!(out[0].default, 1.0);
    }
}
