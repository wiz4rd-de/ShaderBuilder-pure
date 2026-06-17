# Golden-image regression harness (#32)

The `testing` crate (`crates/testing/`) is the Phase-2 **fidelity gate's
machinery**: it renders `.slangp` presets headlessly to PNG deterministically,
diffs images with a numeric metric + a visual diff artifact, and import-and-
renders a whole *directory* of presets as a smoke/fidelity fuzzer.

This document explains what the harness automates, the self-oracle goldens and
how to re-baseline them, and — crucially — the **manual procedure for capturing
real RetroArch reference images** that closes the Phase-2 fidelity exit gate on a
machine with a working RetroArch + display.

> **Honest scope.** Everything under "Automated" below runs in CI on a headless
> lavapipe (software Vulkan) adapter. The "Manual gate" below is **NOT done** in
> the headless CI/dev environment and is not claimed to pass here. The committed
> goldens are produced by *our own engine* (a self-oracle), not captured from
> RetroArch; they prove the machinery and determinism, **not fidelity vs
> RetroArch**.

---

## 1. Automated (runs in CI)

### Public API (`crates/testing/src/`)

| Item | Module | What it does |
| --- | --- | --- |
| `render_preset_to_image(slangp, source, viewport, frame_index) -> Result<RgbaImage, HarnessError>` | `render` | Parse a `.slangp`, compile every pass (combined `sampler2D` supported), map scale/format/sampler/alias/feedback keys to engine `Pass`es, decode + register LUTs, drive the source pump to `frame_index` (so feedback + history are deterministic), render, read back to an `RgbaImage`. |
| `diff_images(a, b, tolerance, max_fraction) -> DiffReport` | `diff` | Per-pixel max/mean absolute difference + the fraction of pixels whose max-channel diff exceeds `tolerance`; `passed = pct_over <= max_fraction`. A size mismatch is an automatic fail. |
| `diff_image(a, b, amplify) -> RgbaImage` | `diff` | An amplified absolute-difference image, written as a CI artifact on failure. |
| `fuzz_presets(dir, source, viewport, frame_index) -> Vec<PresetResult>` | `fuzz` | Walk a directory of `.slangp`, import-and-render each, catch errors **per preset** (incl. panics), report `{ name, compiled, rendered, error }` without aborting the run. |

`DiffReport { max_abs: u8, mean_abs: f64, pct_pixels_over_threshold: f64, passed: bool }`.

### Determinism

`render_preset_to_image` is a pure function of `(slangp, source, viewport,
frame_index)`: it drives the engine through the same `preview_engine::RenderSource`
command seam the app streams through, paced against render ticks (no wall clock).
Same inputs → byte-identical bytes **on a given adapter**. Across *different* GPU
adapters the bytes can differ at the sub-pixel level (bilinear filtering,
software-vs-hardware rounding), which is why the goldens are diffed with a
tolerance rather than for exact equality. The byte-exact guarantee is asserted by
`render_is_deterministic_byte_for_byte` (same adapter, same process).

### Local fixtures (`crates/testing/fixtures/`)

The real `slang-shaders` corpus is a large external clone and is intentionally
**not vendored**. Instead, small committed fixtures exercise the Phase-2 engine
features. Each is the LOCAL analogue of one exit-criteria preset.

| Fixture | Engine feature exercised | Sampler form | Exit-criteria analogue |
| --- | --- | --- | --- |
| `multipass/multipass.slangp` | Multi-pass chain: `scale0 = 2.0` (source ×2 FBO), `alias0 = FIRST`, final pass samples `<FIRST>` **and** `Original` | combined `sampler2D` | CRT-Royale (a real multi-pass chain with aliases/scales) |
| `feedback/feedback.slangp` | `PassFeedback0` self-feedback double-buffer + the `feedback_pass = 0` preset key (union of both opt-in paths) | separate `texture2D` + `sampler` | the feedback shader |
| `lut/lut.slangp` (+ `pal.png`, a 2×2 LUT) | `textures = PAL` LUT decode/register/bind-by-name with its own per-LUT sampler (`PAL_linear`, `PAL_wrap_mode`) | combined `sampler2D` | NTSC presets (which ship LUTs) |

The fixtures deliberately cover **both** sampler-binding shapes (combined and
separate) so the harness proves the corpus path, since real `slang-shaders` use
the combined form (`slang_compile` splits it for wgpu).

### Self-oracle goldens (`crates/testing/goldens/`)

For each fixture a PNG golden is committed, produced by **our own engine**. The
`golden` test (`crates/testing/tests/golden.rs`) re-renders each fixture over a
fixed 8×8 source at a fixed viewport (32×32) and frame index, then diffs against
the committed golden with:

- `TOLERANCE = 12` (per-channel) — absorbs adapter/filtering noise (a few units);
- `MAX_FRACTION = 0.06` — up to ~6% of pixels may wobble (edges, the alias ×2
  downsample boundary) on a different adapter, while a genuine regression flips a
  much larger share.

These goldens prove **determinism + the whole compile→chain→feedback→LUT→render→
read-back→diff machinery + the re-baseline flow**. They do **not** prove fidelity
versus RetroArch.

On failure the test writes `target/golden-artifacts/<name>.rendered.png` and
`<name>.diff.png` (amplified ×8) for inspection (uploaded by CI).

### Corpus fuzzer over the fixtures

`fuzz_fixtures.rs` runs `fuzz_presets` over `fixtures/` and asserts every fixture
imports-and-renders without error. This is the CI-fast stand-in for the real
corpus run (section 3 below).

### Re-baselining the goldens

When a render change is **intentional**, regenerate the goldens:

```bash
UPDATE_GOLDEN=1 WGPU_BACKEND=vulkan cargo test -p testing --test golden -- --test-threads=1
```

With `UPDATE_GOLDEN=1` the test rewrites every `goldens/*.png` from the current
engine output and skips the comparison (it passes by construction). **Review the
PNG diff in version control before committing.** Without the env var the goldens
are read-only oracles.

### Running locally

```bash
# All harness tests (diff math, fuzzer, goldens, determinism).
WGPU_BACKEND=vulkan cargo test -p testing -- --test-threads=1
```

`--test-threads=1` is required: concurrent wgpu device creation SIGSEGVs on
multi-GPU boxes (a known flake; see CONTRIBUTING / the engine tests).

---

## 2. Why the RetroArch comparison is a *manual* gate here

This repository's CI and the current dev box are **headless** (a blanking
display, software-Vulkan only). RetroArch is installed but:

- it has no simple "render this `.slangp` over this PNG to that PNG" batch CLI,
  and
- driving its GUI to a deterministic single-frame capture needs a real display /
  compositor.

So real RetroArch reference PNGs cannot be captured in this environment. The
machinery to *use* them, however, is complete: `diff_images` / `diff_image` are
exactly what compares a harness render to a captured reference. The remaining
work is to capture the references on a suitable machine and drop them in.

---

## 3. Manual gate — capturing real RetroArch references (Phase-2 fidelity exit)

Do this on a machine with a working RetroArch **and a display/GPU**. The goal is
the exit-criteria trio: **CRT-Royale, an NTSC preset, and a feedback shader**
render within threshold of RetroArch reference images.

### 3.1 Fix the inputs (determinism)

1. **Source frame(s).** Pick fixed source PNG(s) at a fixed resolution (e.g. a
   320×240 test card, or a captured core framebuffer). Commit them under
   `crates/testing/references/sources/` so both RetroArch and the harness render
   the *same* input. For an animated/feedback preset, also fix the **frame
   index** (how many frames to advance before the capture).
2. **Viewport.** Pick a fixed output resolution (e.g. 1280×960, integer scale
   off) and use it for *both* RetroArch and the harness.
3. **Preset.** Pick the exact `.slangp` (e.g. CRT-Royale, an NTSC preset, a
   feedback preset) from a known `slang-shaders` commit; record the commit hash.

### 3.2 Capture in RetroArch

1. Launch RetroArch with the chosen core + the fixed source, load the `.slangp`
   via Quick Menu → Shaders, and set the window/viewport to the fixed resolution
   (Settings → Video; disable integer scale / overlays / bezels to match the
   harness).
2. Advance to the fixed frame index (pause + frame-advance), then take a
   screenshot of the **shader output** (RetroArch screenshot hotkey captures the
   final framebuffer). For a feedback preset, frame-advance the recorded number of
   frames first so the feedback state matches what the harness produces at the
   same `frame_index`.
3. Save the screenshot as a PNG.

### 3.3 Wire the references in

1. Commit each reference PNG under `crates/testing/references/<preset>.png` with a
   small sidecar (`<preset>.toml` or a comment) recording: preset path + corpus
   commit, source PNG, viewport, frame index, RetroArch version, and the GPU/OS
   the capture ran on.
2. Add a test (mirroring `golden.rs`) that, for each reference:
   - loads the committed source PNG,
   - `render_preset_to_image(<preset>.slangp, source, viewport, frame_index)`,
   - `diff_images(rendered, reference, tolerance, max_fraction)` and asserts
     `passed`,
   - writes `diff_image(...)` on failure.
3. **Pick the threshold from the data.** CRT/NTSC shaders are full of high-
   frequency detail (scanlines, masks, ringing) where exact pixels never match
   across renderers, so use a *perceptual* threshold: a per-channel `tolerance`
   that absorbs sub-pixel filtering differences and a `max_fraction` chosen so a
   faithful render passes but a structural regression (wrong mask, missing
   curvature, broken feedback) fails. Record the chosen values and *why* in the
   test and here. Start permissive, then tighten against several known-good
   captures.

### 3.4 The real corpus fuzz

Clone `slang-shaders` somewhere (do **not** vendor it into this repo) and point
the fuzzer at it:

```rust
let results = testing::fuzz_presets(
    Path::new("/path/to/slang-shaders"),
    &source_frame,
    (1280, 960),
    0,
);
// Report every PresetResult where !r.ok().
```

This import-and-renders a broad slice and reports per-preset failures without
aborting. Run it as an opt-in (`#[ignore]`d, or behind an env var pointing at the
clone) so CI — which has no corpus — stays green, and treat the failure list as
the corpus-fidelity worklist.

---

## 4. Status

- **Automated and passing in CI:** headless deterministic render-to-PNG; the
  image diff + visual artifact; the corpus fuzzer over local fixtures; the
  self-oracle goldens for a multi-pass, a feedback, and a LUT preset; the
  re-baseline flow.
- **Manual gate, NOT done in this headless environment:** capturing real
  RetroArch reference images and confirming the CRT-Royale / NTSC / feedback trio
  pass within threshold, plus the fuzz over a real `slang-shaders` clone. These
  close the Phase-2 fidelity exit gate and must be run on a machine with a working
  RetroArch + display, per section 3.
