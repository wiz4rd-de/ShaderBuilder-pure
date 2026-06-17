//! End-to-end import test for #35: parameter extraction + reconciliation and LUT
//! texture import, against the committed `fixtures/params/` preset.
//!
//! Unlike the `preset-io` unit tests (which use in-memory presets), this exercises
//! the real on-disk fixture through [`preset_io::import_preset`]: a pass with
//! several `#pragma parameter` knobs, a `.slangp` overriding some of their
//! defaults, and two LUTs with differing sampler settings.

use std::path::{Path, PathBuf};

use core_model::WrapMode;
use preset_io::import_preset;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[test]
fn params_fixture_extracts_reconciles_and_imports_luts() {
    let preset_path = fixtures_dir().join("params/params.slangp");
    let (project, diags) = import_preset(&preset_path).expect("fixture imports");

    // The fixture is well-formed: no warnings (no missing shaders, malformed
    // pragmas, or parameter conflicts).
    assert!(
        !diags.has_warnings(),
        "params fixture should import cleanly: {:?}",
        diags.diagnostics
    );

    // ---- Parameters: four knobs, two overridden by the preset. ----
    let param = |id: &str| {
        project
            .parameters
            .iter()
            .find(|p| p.name == id)
            .cloned()
            .unwrap_or_else(|| panic!("parameter `{id}` missing: {:?}", project.parameters))
    };
    assert_eq!(project.parameters.len(), 4, "four #pragma parameters");

    // BRIGHTNESS: pragma default 1.0, preset override 1.25 -> default becomes 1.25;
    // range/step come from the pragma unchanged.
    let bright = param("BRIGHTNESS");
    assert_eq!(bright.default, 1.25, "preset override wins");
    assert_eq!(bright.min, 0.0);
    assert_eq!(bright.max, 2.0);
    assert_eq!(bright.step, 0.01);
    assert_eq!(bright.label, "Brightness");

    // GAMMA: pragma default 2.2, preset override 2.4.
    let gamma = param("GAMMA");
    assert_eq!(gamma.default, 2.4);
    assert_eq!(gamma.min, 1.0);
    assert_eq!(gamma.max, 3.0);

    // CONTRAST / SATURATION: not overridden -> pragma initial values stand.
    assert_eq!(param("CONTRAST").default, 1.0);
    assert_eq!(param("SATURATION").default, 1.0);
    assert_eq!(param("SATURATION").step, 0.05);

    // ---- LUTs: two, with differing sampler settings + resolved paths. ----
    assert_eq!(project.luts.len(), 2, "two LUTs imported");
    let lut = |name: &str| {
        project
            .luts
            .iter()
            .find(|l| l.name == name)
            .cloned()
            .unwrap_or_else(|| panic!("LUT `{name}` missing: {:?}", project.luts))
    };

    let grade = lut("GRADE");
    assert!(
        grade.path.ends_with("params/grade.png"),
        "GRADE path resolved against the preset dir: {}",
        grade.path
    );
    assert_eq!(grade.filter_linear, Some(true));
    assert_eq!(grade.wrap_mode, Some(WrapMode::Repeat));
    assert_eq!(grade.mipmap, Some(true));

    let border = lut("BORDER");
    assert!(
        border.path.ends_with("params/border.png"),
        "{}",
        border.path
    );
    // Differing sampler settings from GRADE.
    assert_eq!(border.filter_linear, Some(false));
    assert_eq!(border.wrap_mode, Some(WrapMode::ClampToEdge));
    assert_eq!(border.mipmap, Some(false));
}
