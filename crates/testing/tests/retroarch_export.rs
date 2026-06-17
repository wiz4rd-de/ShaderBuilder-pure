//! Committed RetroArch-loadable export bundle gate (#37, Phase-3 EXIT gate).
//!
//! The acceptance criteria require a checked-in exported bundle that is known to
//! load in real RetroArch (the manual procedure is documented in
//! `fixtures/retroarch_export/README.md`). This test runs in CI **without** the
//! external corpus: it asserts the committed bundle still parses, has the expected
//! shape, and re-imports + round-trips losslessly — so a regression in the export
//! writer (or an accidental edit to the committed bundle) is caught automatically.
//!
//! The *manual* RetroArch-loads-it step cannot run in CI (it needs a RetroArch
//! install + a GPU); this test guards everything around it so the committed
//! artifact stays valid between manual verifications.

use std::path::{Path, PathBuf};

use core_model::PassSource;
use preset_io::import_preset;
use testing::round_trip;

fn bundle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retroarch_export")
}

#[test]
fn committed_bundle_is_present_and_well_formed() {
    let dir = bundle_dir();
    let preset = dir.join("preset.slangp");
    assert!(preset.is_file(), "committed preset present: {preset:?}");
    assert!(
        dir.join("scanline.slang").is_file(),
        "committed pass source present"
    );
    assert!(
        dir.join("README.md").is_file(),
        "the manual-verification README is committed alongside the bundle"
    );

    // Paths in the preset must be RELATIVE (a RetroArch bundle is portable): no
    // value may be an absolute path, and the shader is referenced by bare name.
    let text = std::fs::read_to_string(&preset).unwrap();
    for line in text.lines() {
        let value = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or("");
        assert!(
            !value.starts_with('/'),
            "bundle preset must not carry an absolute path: {line:?}"
        );
    }
    assert!(
        text.contains("shader0 = scanline.slang"),
        "pass referenced by relative name:\n{text}"
    );
}

#[test]
fn committed_bundle_imports_as_a_single_whole_pass_preset() {
    let (project, diags) =
        import_preset(bundle_dir().join("preset.slangp")).expect("bundle re-imports");

    // A clean, fully-readable bundle imports without warnings.
    assert!(
        !diags.has_warnings(),
        "committed bundle should import cleanly: {:?}",
        diags.diagnostics
    );

    assert_eq!(project.passes.len(), 1, "single-pass preset");
    match &project.passes[0].source {
        PassSource::WholePassCode { source, .. } => {
            assert!(
                source.contains("#version 450"),
                "the pass source survived re-import"
            );
        }
        other => panic!("expected whole-pass code, got {other:?}"),
    }
    // `scale_type0 = viewport` (the only setting in the source preset) survived.
    let s = &project.passes[0].settings;
    assert_eq!(
        s.scale_x.scale_type,
        Some(core_model::ScaleType::Viewport),
        "viewport scale type re-imports"
    );
}

#[test]
fn committed_bundle_round_trips_losslessly() {
    let work = tempfile::tempdir().expect("temp bundle dir");
    let rt = round_trip(&bundle_dir().join("preset.slangp"), work.path())
        .expect("committed bundle round trip");
    assert!(
        rt.is_lossless(),
        "the committed RetroArch bundle must round-trip losslessly:\n{}",
        rt.report()
    );
    assert!(
        rt.pass_bytes_identical.iter().all(|&b| b),
        "the pass `.slang` must export byte-identically"
    );
}
