//! Corpus-fuzzer smoke gate (#32, Architecture §G.2/§G.3).
//!
//! Runs [`testing::fuzz_presets`] over the committed `fixtures/` directory: every
//! feature-exercising fixture preset must import-and-render without crashing.
//! This is the CI-fast stand-in for the REAL corpus run, which points the very
//! same `fuzz_presets` at a cloned `slang-shaders` checkout — documented as a
//! manual gate in `docs/golden-image-harness.md` (the corpus is a large external
//! clone and is intentionally NOT vendored).
//!
//! Failures are reported PER preset (the run never aborts on one), so a future
//! fixture that breaks is named in the assertion rather than crashing the suite.

use std::path::{Path, PathBuf};

use source::Frame;
use testing::fuzz_presets;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// A small deterministic source frame for the smoke render.
fn smoke_source() -> Frame {
    let (w, h) = (8u32, 8u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&[(x * 32) as u8, (y * 32) as u8, 64, 255]);
        }
    }
    Frame::new(w, h, rgba)
}

#[test]
fn every_fixture_preset_imports_and_renders() {
    let results = fuzz_presets(&fixtures_dir(), &smoke_source(), (24, 24), 2);

    // We must actually find the fixtures (a wrong path would silently pass).
    assert!(
        results.len() >= 3,
        "expected at least the 3 feature fixtures, found {}: {:?}",
        results.len(),
        results.iter().map(|r| &r.name).collect::<Vec<_>>()
    );

    // Every preset must have compiled AND rendered with no error. A failure names
    // the offending preset and its error so the corpus run is actionable.
    let failures: Vec<String> = results
        .iter()
        .filter(|r| !r.ok())
        .map(|r| {
            format!(
                "{} (compiled={}, rendered={}, error={:?})",
                r.name, r.compiled, r.rendered, r.error
            )
        })
        .collect();
    assert!(
        failures.is_empty(),
        "fuzzer found {} failing fixture preset(s):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );

    // Sanity: the three named fixtures are present by path.
    let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
    for expected in ["multipass", "feedback", "lut"] {
        assert!(
            names.iter().any(|n| n.contains(expected)),
            "fuzzer should have found the {expected} fixture, got {names:?}"
        );
    }
}
