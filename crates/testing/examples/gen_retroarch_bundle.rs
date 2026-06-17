//! Regenerate the checked-in, RetroArch-loadable export bundle (#37, Phase-3 EXIT
//! gate). The acceptance criteria require **>=1 exported bundle that loads in real
//! RetroArch** to be committed, with a documented manual verification procedure
//! (see `crates/testing/fixtures/retroarch_export/README.md`).
//!
//! This example imports a known-good single-pass preset from the external
//! `slang-shaders` corpus — `scanlines/scanline.slangp`, the same preset the
//! Phase-2 reference suite confirms renders in **RetroArch 1.22.2** — runs it
//! through the real import bridge ([`preset_io::import_preset`]) and the real
//! export bundle writer ([`preset_io::export_preset`]), and writes the result to
//! `crates/testing/fixtures/retroarch_export/`. The committed bundle is therefore
//! a genuine product of our import → export path, not a hand-written file.
//!
//! Run (needs the corpus; pin it to the commit in the bundle README):
//!
//! ```bash
//! FUZZ_CORPUS_DIR=/home/mfunk/Code/slang-shaders \
//!   cargo run -p testing --example gen_retroarch_bundle
//! ```
//!
//! It is intentionally NOT a test (it needs the external corpus); the committed
//! bundle is what CI and reviewers see, and `tests/retroarch_export.rs` asserts
//! that committed bundle re-imports + round-trips losslessly without the corpus.

use std::path::{Path, PathBuf};

fn main() {
    let Some(corpus) = std::env::var_os("FUZZ_CORPUS_DIR").map(PathBuf::from) else {
        eprintln!(
            "FUZZ_CORPUS_DIR unset — point it at a slang-shaders clone to regenerate the bundle:\n\
             \n  FUZZ_CORPUS_DIR=/path/to/slang-shaders \\\n    \
             cargo run -p testing --example gen_retroarch_bundle\n"
        );
        std::process::exit(1);
    };

    let src_preset = corpus.join("scanlines/scanline.slangp");
    if !src_preset.is_file() {
        eprintln!(
            "source preset {} not found — is FUZZ_CORPUS_DIR correct?",
            src_preset.display()
        );
        std::process::exit(1);
    }

    // Import the known-good preset, then export a self-contained bundle.
    let (project, diags) = preset_io::import_preset(&src_preset).expect("import scanline.slangp");
    for d in &diags.diagnostics {
        eprintln!("  import diagnostic: {:?} {}", d.severity, d.message);
    }

    let dest = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retroarch_export");
    // Clean any prior bundle contents we own (preset + slang + textures), keeping
    // the committed README in place.
    let _ = std::fs::remove_file(dest.join(preset_io::PRESET_FILENAME));
    let report = preset_io::export_preset(&project, &dest, &Default::default())
        .expect("export scanline bundle");

    eprintln!("wrote bundle to {}", dest.display());
    eprintln!("  preset:  {}", report.preset_path.display());
    eprintln!("  passes:  {:?}", report.pass_files);
    eprintln!("  textures:{:?}", report.texture_files);
    for w in &report.warnings {
        eprintln!("  warning: {w}");
    }
}
