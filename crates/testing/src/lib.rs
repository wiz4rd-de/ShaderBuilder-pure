//! `testing` — the golden-image regression harness (#32; Architecture §G.2/§G.3,
//! Specification §5, `docs/retroarch-slang-runtime.md`).
//!
//! This crate stands up the **machinery** of the Phase-2 fidelity gate: a way to
//! render a `.slangp` preset through the real [`preview_engine`] runtime to a PNG
//! deterministically, diff two images with a numeric metric + a visual diff
//! artifact, and import-and-render a whole *directory* of presets as a
//! smoke/fidelity fuzzer that reports per-preset failures without aborting the
//! run.
//!
//! ## What this proves — and what it deliberately does NOT
//!
//! The committed goldens under `goldens/` are a **self-oracle**: they were
//! produced by *this* engine, not captured from RetroArch. Re-rendering and
//! diffing them proves (a) determinism — same inputs give byte-identical output —
//! and (b) that the whole compile → chain → feedback/history → LUT → render →
//! read-back path runs end-to-end and that the diff / re-baseline flow works.
//!
//! They do **not** prove fidelity *versus RetroArch* — that is the job of the
//! **real-RetroArch reference suite** (`tests/references.rs`, #32 PART B), whose
//! `references/retroarch/*.png` are ACTUAL RetroArch 1.22.2 captures (distinct from
//! the self-oracle goldens). On a box with a working software GL, `crt-geom`, a
//! `scanline` preset, and an NTSC preset render within calibrated thresholds of the
//! RetroArch capture (near-exact for `crt-geom`); feedback + `crt-royale` are
//! documented divergences. The capture procedure (imageviewer core, the forced
//! `--appendconfig` geometry, frame alignment, calibration) and the corpus-fuzz
//! results live in `docs/golden-image-harness.md`. The `references`/`corpus_fuzz`
//! tests are `#[ignore]`d (they need the external `slang-shaders` clone), so CI
//! stays green without the corpus.
//!
//! ## Modules
//! * [`render`] — [`render::render_preset_to_image`]: parse a `.slangp`, compile
//!   each pass, build the engine chain, decode/register LUTs, advance the source
//!   pump to a fixed frame index (so feedback + history are deterministic), render
//!   to an [`image::RgbaImage`].
//! * [`diff`] — [`diff::diff_images`] / [`diff::DiffReport`] (the pass/fail
//!   metric) and [`diff::diff_image`] (the amplified visual diff artifact).
//! * [`fuzz`] — [`fuzz::fuzz_presets`] / [`fuzz::PresetResult`]: walk a directory
//!   of `.slangp` and import-and-render each, catching errors per preset.
//! * [`roundtrip`] — [`roundtrip::round_trip`] / [`roundtrip::compare_projects`]:
//!   the lossless import → export → re-import harness (#37, Phase-3 EXIT gate),
//!   with a canonicalized structural [`roundtrip::ProjectDiff`] and a readable
//!   diff report on any divergence.
//! * [`graph_render`] — [`graph_render::render_graph_to_image`]: the Phase-4 exit
//!   gate's parity harness (#44). Type-checks + lowers + emits slang from a
//!   hand-built [`core_model::ir::IrGraph`] and renders the generated slang
//!   through [`render::render_preset_to_image`], so a generated graph can be
//!   golden-image-diffed against a hand-written-equivalent `.slang`.

pub mod diff;
pub mod fuzz;
pub mod graph_render;
pub mod render;
pub mod roundtrip;

pub use diff::{diff_image, diff_images, DiffReport};
pub use fuzz::{fuzz_presets, PresetResult};
pub use graph_render::{
    parity_fixtures_dir, render_graph_to_image, render_handwritten_slang, screen_uv_node,
    GraphRenderError, ParamOverride,
};
pub use render::{render_preset_to_image, HarnessError};
pub use roundtrip::{compare_projects, round_trip, ProjectDiff, RoundTrip, RoundTripError};

/// Crate identity marker (kept consistent with the other workspace crates' smoke
/// tests so the dependency edges stay live).
pub const NAME: &str = "testing";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(NAME, "testing");
        // The harness → engine / preset-io / source / slang-compile edges are real.
        assert_eq!(preview_engine::NAME, "preview-engine");
        assert_eq!(preset_io::NAME, "preset-io");
        assert_eq!(source::NAME, "source");
        assert_eq!(slang_compile::NAME, "slang-compile");
        // The #44 parity-harness edges to the Phase-4 codegen crates are real.
        assert_eq!(ir::NAME, "ir");
        assert_eq!(codegen_slang::NAME, "codegen-slang");
    }
}
