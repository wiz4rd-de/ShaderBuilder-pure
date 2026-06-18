//! The bundled EXAMPLE PROJECT (#66): assert the shipped "CRT Scanlines +
//! Curvature" preset imports into a [`core_model::Project`], EXPORTS cleanly as a
//! RetroArch `.slangp` bundle, and that the bundle re-imports structure-lossless.
//! It also (re)generates the canonical native `.json` resource the onboarding
//! "Open example" flow and the #67 release smoke test load —
//! `crates/app/resources/example-project.json` — so the in-app example, the
//! exportable bundle, and the JSON resource all come from ONE source of truth (the
//! `.slangp` fixture under `fixtures/example/`).
//!
//! Run with `SB_REGEN_EXAMPLE=1` to rewrite the committed JSON resource after an
//! intentional change to the example shaders; otherwise the test asserts the
//! committed JSON still matches what the fixture produces (a drift guard so the
//! frontend resource never silently diverges from the exportable bundle).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use core_model::{PassSource, Project};

/// `fixtures/example/crt-scanlines-curvature.slangp`.
fn example_slangp() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("example")
        .join("crt-scanlines-curvature.slangp")
}

/// The committed native project resource the app bundles + onboarding loads.
fn example_json_resource() -> PathBuf {
    // crates/testing → crates/app/resources/example-project.json
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("app")
        .join("resources")
        .join("example-project.json")
}

/// Import the example preset into the project the editor would hold, with a stable
/// display name (the JSON resource carries this name verbatim).
fn import_example() -> Project {
    let (mut project, diags) =
        preset_io::import_preset(example_slangp()).expect("example preset imports");
    // The import must not warn (a missing shader / malformed pragma would).
    assert!(
        !diags.has_warnings(),
        "example import produced warnings: {:?}",
        diags.diagnostics
    );
    project.name = "CRT Scanlines + Curvature".to_owned();
    project.metadata.description = Some(
        "A two-pass CRT example: screen curvature then scanlines. \
         Open it from the start screen to see a live preview, then tweak the \
         parameters or export it as a .slangp bundle."
            .to_owned(),
    );
    project.metadata.author = Some("ShaderBuilder".to_owned());
    project
}

#[test]
fn example_project_is_two_whole_pass_curvature_then_scanlines() {
    let project = import_example();
    assert_eq!(project.passes.len(), 2, "two passes");
    // Both passes are opaque whole-pass code (export-ready, no graph substitution).
    for pass in &project.passes {
        assert!(
            matches!(pass.source, PassSource::WholePassCode { .. }),
            "pass {} is whole-pass code",
            pass.id
        );
    }
    // Parameters surfaced from the `#pragma parameter` lines reach the project.
    let names: Vec<&str> = project.parameters.iter().map(|p| p.name.as_str()).collect();
    for expected in [
        "CURVATURE",
        "CORNER_SMOOTH",
        "SCANLINE_WEIGHT",
        "BRIGHTNESS",
    ] {
        assert!(names.contains(&expected), "parameter {expected} present");
    }
}

#[test]
fn example_project_exports_cleanly_and_reimports() {
    let project = import_example();

    // The export gate must pass with zero blockers (whole-pass passes are exportable).
    let validation = preset_io::validate_for_export(&project);
    assert!(
        validation.ok(),
        "example must export cleanly: {:?}",
        validation.blockers
    );

    // Write the bundle and re-import it; the round trip must be structure-lossless.
    let out = tempfile::tempdir().expect("temp dir");
    preset_io::export_preset(&project, out.path(), &BTreeMap::new()).expect("export bundle");

    let preset_path = out.path().join("preset.slangp");
    assert!(preset_path.is_file(), "bundle has preset.slangp");
    let (reimported, _) = preset_io::import_preset(&preset_path).expect("re-import bundle");
    let diff = testing::compare_projects(&project, &reimported);
    assert!(
        diff.is_lossless(),
        "export → re-import was lossy:\n{}",
        diff.report()
    );
}

#[test]
fn example_json_resource_matches_the_fixture() {
    let project = import_example();
    let json = serde_json::to_string_pretty(&project).expect("serialize") + "\n";
    let resource = example_json_resource();

    if std::env::var_os("SB_REGEN_EXAMPLE").is_some() {
        if let Some(parent) = resource.parent() {
            std::fs::create_dir_all(parent).expect("create resources dir");
        }
        std::fs::write(&resource, &json).expect("write example JSON resource");
        return;
    }

    let committed = std::fs::read_to_string(&resource).unwrap_or_else(|e| {
        panic!(
            "example JSON resource missing at {} ({e}); regenerate with SB_REGEN_EXAMPLE=1",
            resource.display()
        )
    });
    assert_eq!(
        committed, json,
        "committed example-project.json drifted from the fixture; \
         regenerate with SB_REGEN_EXAMPLE=1"
    );

    // The committed JSON must also load back through the project loader cleanly.
    let loaded: Project = serde_json::from_str(&committed).expect("committed JSON parses");
    assert_eq!(loaded, project, "committed JSON round-trips to the project");
}
