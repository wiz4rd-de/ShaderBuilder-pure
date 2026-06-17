//! Lossless round-trip suite over the committed fixture corpus (#37, Phase-3 EXIT
//! gate). For every `.slangp` under `fixtures/` this asserts that
//! **import → export → re-import** is structure-lossless (the canonicalized
//! [`testing::compare_projects`]) AND that every unmodified pass's `.slang` is
//! **byte-identical** after export (the #34/#36 byte-exact contract).
//!
//! This runs under the normal `cargo test` workspace step — no GPU, no external
//! corpus, no new CI deps — so the EXIT gate is enforced on every push. The
//! `fixtures/roundtrip/` "kitchen sink" preset alone exercises every parsed
//! feature (multi-pass, all scale types, feedback, aliases, varied-sampler LUTs,
//! parameters-with-overrides, a preserved unknown key); the other feature
//! fixtures (`multipass`, `feedback`, `lut`, `params`, `orphan_override`) are
//! swept in too so a regression in any of them is caught here.
//!
//! On a non-lossless round trip the failure prints the readable
//! [`testing::ProjectDiff`] report naming the exact diverging field(s).

use std::path::{Path, PathBuf};

use testing::round_trip;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Recursively collect every `*.slangp` under `dir`, sorted for a deterministic run.
fn collect_slangp(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "slangp") {
                out.push(path);
            }
        }
    }
    walk(dir, &mut out);
    out.sort();
    out
}

#[test]
fn every_fixture_preset_round_trips_losslessly() {
    let presets = collect_slangp(&fixtures_dir());

    // We must actually find the fixtures (a wrong path would silently pass): the
    // four feature fixtures plus the kitchen-sink preset.
    assert!(
        presets.len() >= 5,
        "expected at least 5 fixture presets, found {}: {:?}",
        presets.len(),
        presets
    );

    let mut failures: Vec<String> = Vec::new();
    for slangp in &presets {
        let work = tempfile::tempdir().expect("temp bundle dir");
        match round_trip(slangp, work.path()) {
            Ok(rt) => {
                if !rt.is_lossless() {
                    failures.push(format!("{}:\n{}", slangp.display(), indent(&rt.report())));
                }
            }
            Err(e) => failures.push(format!("{}: round trip errored: {e}", slangp.display())),
        }
    }

    assert!(
        failures.is_empty(),
        "{} fixture preset(s) did not round-trip losslessly:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn kitchen_sink_round_trips_losslessly() {
    // The single preset that exercises every parsed feature — called out on its
    // own so a regression names it directly (not buried in the corpus sweep).
    let slangp = fixtures_dir().join("roundtrip/kitchen_sink.slangp");
    assert!(slangp.is_file(), "kitchen-sink fixture present: {slangp:?}");

    let work = tempfile::tempdir().expect("temp bundle dir");
    let rt = round_trip(&slangp, work.path()).expect("kitchen-sink round trip");

    assert!(
        rt.is_lossless(),
        "kitchen-sink preset must round-trip losslessly:\n{}",
        rt.report()
    );

    // Spot-check the salient features actually survived (not just "no diff"):
    // four passes, the feedback pass, both LUTs, and the overridden parameter.
    assert_eq!(rt.second.passes.len(), 4, "four passes survive");
    assert_eq!(rt.second.feedback_pass, Some(2), "feedback_pass survives");
    assert_eq!(rt.second.luts.len(), 2, "both LUTs survive");
    let bright = rt
        .second
        .parameters
        .iter()
        .find(|p| p.name == "BRIGHTNESS")
        .expect("BRIGHTNESS survives");
    assert_eq!(bright.default, 1.25, "the parameter override survives");

    // Every pass exported byte-identically to its original `.slang`.
    assert!(
        rt.pass_bytes_identical.iter().all(|&b| b),
        "every pass `.slang` must be byte-identical after export: {:?}",
        rt.pass_bytes_identical
    );
}

#[test]
fn orphan_override_fixture_round_trips_losslessly() {
    // FINDING A1/A2: a bare `id = value` override whose `#pragma parameter` lives
    // in an #included header (which import does NOT resolve) was silently dropped
    // on import -> export — the tuned value vanished, a gate #1 violation the
    // first-vs-re-import oracle was blind to. Called out on its own so a
    // regression names it directly.
    let slangp = fixtures_dir().join("orphan_override/orphan.slangp");
    assert!(
        slangp.is_file(),
        "orphan-override fixture present: {slangp:?}"
    );

    let work = tempfile::tempdir().expect("temp bundle dir");
    let rt = round_trip(&slangp, work.path()).expect("orphan-override round trip");

    assert!(
        rt.is_lossless(),
        "orphan-override preset must round-trip losslessly:\n{}",
        rt.report()
    );
    assert!(
        rt.source_loss.is_empty(),
        "no source-side loss: {:?}",
        rt.source_loss
    );

    // The orphan override's tuned value survives (synthesized into a parameter)…
    let header = rt
        .second
        .parameters
        .iter()
        .find(|p| p.name == "HEADER_KNOB")
        .expect("orphan override HEADER_KNOB survives the round trip");
    assert_eq!(header.default, 0.625, "orphan value survives");

    // …and so does the normal reconciled override whose pragma IS in the body.
    let real = rt
        .second
        .parameters
        .iter()
        .find(|p| p.name == "REAL_KNOB")
        .expect("reconciled override REAL_KNOB survives");
    assert_eq!(real.default, 0.75, "pragma-backed override survives");
    // Its range/step came from the inline pragma (not synthesized).
    assert_eq!(real.min, 0.0);
    assert_eq!(real.max, 1.0);
}

/// Indent a multi-line report so it nests readably under the preset path.
fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
