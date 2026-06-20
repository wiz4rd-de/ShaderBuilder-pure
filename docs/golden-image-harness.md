# Golden-image regression harness (#32)

The `testing` crate (`crates/testing/`) is the Phase-2 **fidelity gate's
machinery**: it renders `.slangp` presets headlessly to PNG deterministically,
diffs images with a numeric metric + a visual diff artifact, and import-and-
renders a whole *directory* of presets as a smoke/fidelity fuzzer.

This document explains what the harness automates, the self-oracle goldens and
how to re-baseline them, the **corpus fuzzer over a real `slang-shaders` clone**,
and the **real-RetroArch reference capture** that closes the Phase-2 fidelity exit
gate. §4 covers the Phase-3 **lossless import → export → re-import** harness (the
Phase-3 exit gate); §5 is the overall status.

> **Honest scope.** Everything under "Automated (runs in CI)" below runs in CI on
> a headless lavapipe (software Vulkan) adapter. The committed `goldens/*.png` are
> produced by *our own engine* (a self-oracle): they prove machinery + determinism,
> **not fidelity vs RetroArch**. Section 2 (corpus fuzz) and section 3 (real
> RetroArch references) are now **partly automated and demonstrated on a dev box
> with a working software GL** — `crt-geom`, `scanline`, and an NTSC preset match
> RetroArch within calibrated thresholds (metrics in §3). They are `#[ignore]`d
> opt-in suites (they need the external corpus clone and were calibrated on this
> box's llvmpipe), so CI stays green without the corpus. `crt-geom`, `scanline`,
> an NTSC preset, and (after the #32 `PassFeedbackSize0` fix) `feedback` match
> RetroArch within calibrated thresholds (metrics in §3). What remains un-closed is
> documented in §5: `crt-royale` now compiles + renders (after the #32 inlining
> fix) but diverges in fidelity, and part of the corpus (the failure-mode worklist).

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
corpus run (section 2 below).

### push-constant → UBO normalization (#32)

Real `slang-shaders` put their per-pass parameter block (`FrameCount` + every
`#pragma parameter`) in a Vulkan **`push_constant`** block — **72 %** of the
corpus's `.slang` files (1134 / 1571 at the pinned commit). wgpu ingests SPIR-V
through naga, which reports a push-constant global as the `IMMEDIATES` capability,
and the engine's device is not created with it — so before #32 every such shader
failed `create_shader_module` with *"Capability Capabilities(IMMEDIATES) is not
supported"*. `slang_compile::push_to_ubo` rewrites the push-constant block into an
ordinary UBO at a binding free across both stages (a SPIR-V→SPIR-V transform in
the same vein as `split_samplers`), so the existing reflection-driven bind table +
`pack_builtins`/`pack_params` handle it with **no renderer change**. The renderer
also now packs builtins+params into *both* reflected blocks, since the standard
layout splits them across `UBO{MVP,…Size}` and `Push{FrameCount,params}`. Together
these took the curated-category corpus run from **4 % → 63 %** rendered (§2).

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

## 2. The real corpus fuzz (`corpus_fuzz.rs`, opt-in)

`crates/testing/tests/corpus_fuzz.rs` runs `fuzz_presets` over a cloned
`slang-shaders` checkout and prints a categorized summary: per-top-level-category
compile/render/ok counts, then the distinct failure messages grouped by
normalized kind (paths/ids/numbers stripped so semantically equal failures
group). That failure list is the corpus-fidelity worklist. The test is
`#[ignore]`d and keyed off `FUZZ_CORPUS_DIR`, so CI — which has no corpus — skips
it cleanly; it only fails if it found **zero** presets (a mis-pointed dir).

```bash
# A curated, bounded subset (recommended — the full 2526 is slow on software GPU):
FUZZ_CORPUS_DIR=/path/to/slang-shaders \
  FUZZ_CORPUS_CATEGORIES=crt,ntsc,blurs,denoisers,interpolation,scanlines,sharpen \
  WGPU_BACKEND=vulkan cargo test -p testing --test corpus_fuzz \
  -- --ignored --nocapture --test-threads=1

# Or cap the count, sampled deterministically across the tree:
FUZZ_CORPUS_DIR=/path/to/slang-shaders FUZZ_CORPUS_MAX=300 ...
```

### Results on this box (curated categories, 246 presets)

After the #32 `spirv-opt` function-inlining fix (§3.7), **211 / 246 (85.8 %)**
import-and-render without error — up from **171 / 246 (69.5 %)** before inlining
(itself up from **10 / 246 (4 %)** before #32's push-constant→UBO work). The +40
presets are the formerly-rejected sampler-in-function shaders. Per category
(rendered ok, before-inlining → after-inlining):

| Category | total | before | after |
| --- | --- | --- | --- |
| crt | 131 | 75 | 103 |
| ntsc | 32 | 28 | 29 |
| interpolation | 44 | 41 | 42 |
| scanlines | 9 | 9 | 9 |
| blurs | 17 | 8 | 17 |
| denoisers | 7 | 6 | 6 |
| sharpen | 6 | 4 | 5 |

The 38-strong "sampler passed to a function" failure category is now **entirely
gone** from the grouped failure report; the 35 remaining failures are all other
kinds (nested `#reference` presets with no `shaders` key, naga function/entry-point
shapes, missing `#include` files, non-UTF-8 sources, one `float_framebuffer`
parser edge case) — the worklist for future tickets.

The remaining failures fall in a small number of distinct ways — the **engine-gap
worklist for future tickets** (none crash the run; each is caught per preset):

| Failures | Kind | Cause |
| --- | --- | --- |
| 38 → **0** | sampler passed to a function | **CLOSED (#32):** `spirv-opt --merge-return --inline-entry-points-exhaustive` inlines the helper into the entry point before `split_samplers`, so only the inline `OpLoad` form (which it handles) remains. |
| 10 | `missing required preset key shaders` | `#reference`-style nested presets the parser doesn't follow. |
| 6 | pipeline interface mismatch | a stage's varyings/bindings don't match across VS/FS as our pipeline expects. |
| ~8 | naga `Function … is invalid` | naga rejects a SPIR-V function shape (`testPattern(vf2;`, `ratios(`, `slot(vf2;`, some `main`) — a front-end limitation. |
| ~7 | parser: inline `#`/`//` comment after a value, quoted numbers, `true"` | the `.slangp` parser doesn't strip trailing comments / tolerate stray quotes (blocks `crt-royale` and its variants). |
| 3 each | missing `#include` file; non-UTF-8 source | corpus files our `#include`/read path can't load. |

The earlier **20 `Conflicting binding at index 1`** failures (push block bound at
two different bindings across the vertex/fragment stages) are **resolved** by
`push_to_ubo::free_binding_across` choosing one cross-stage binding — they moved
into the rendered set, which is most of the 63 % → 69.5 % gain.

---

## 3. Real RetroArch references (Phase-2 fidelity exit) — automated here

This is now **demonstrated on a dev box with a working software GL** (no GUI, no
hardware GPU needed). The pieces:

### 3.1 The imageviewer core (deterministic FIXED source)

RetroArch needs *content* to run a shader over. To feed a deterministic still
image, build the bundled `imageviewer` core from the RetroArch source (it loads a
PNG as content):

```bash
git clone --depth 1 --filter=blob:none --sparse https://github.com/libretro/RetroArch
cd RetroArch && git sparse-checkout set cores/libretro-imageviewer libretro-common deps/stb
make -C cores/libretro-imageviewer          # -> image_core.so
cp cores/libretro-imageviewer/image_core.so ~/.config/retroarch/cores/imageviewer_libretro.so
```

### 3.2 The fixed source image

`cargo run -p testing --example gen_reference_source` writes the committed,
deterministic 320×240 test card to `crates/testing/references/src/testcard_320x240.png`
(eight color bars + a luma gradient with a fine checker — real signal for
CRT/NTSC/blur shaders). **Both** RetroArch and our engine consume this exact PNG.

### 3.3 Capture in RetroArch (headless, slang backend)

The slang shader backend needs the **`glcore`** (or `vulkan`) video driver — the
plain `gl` driver only does GLSL and silently falls back to stock shaders. An
`--appendconfig` forces a clean, predictable, full-frame 1:1 geometry so the
output WxH is known and matches what our engine renders:

```ini
video_driver = "glcore"          # slang backend (gl = GLSL only!)
video_shader_enable = "true"
menu_driver = "null"
video_scale_integer = "false"
aspect_ratio_index = "23"        # custom — full-frame, no bars
custom_viewport_width  = "320"   # the known output WxH
custom_viewport_height = "240"
video_smooth = "false"
video_threaded = "false"
video_vsync = "false"
```

```bash
env DISPLAY=:1 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe retroarch \
  -L ~/.config/retroarch/cores/imageviewer_libretro.so \
  crates/testing/references/src/testcard_320x240.png \
  --appendconfig=fidelity.cfg \
  --set-shader=/path/to/slang-shaders/crt/crt-geom.slangp \
  --max-frames=60 --max-frames-ss --max-frames-ss-path=crt-geom.png
```

**Gotchas found and worked around:**

- The headless `glcore` path occasionally emits a **black first frame**; capture
  twice and verify non-black (the two non-black captures are byte-identical — the
  reference IS deterministic once non-black).
- A still source needs a handful of frames to load + present through the slang
  pipeline; `--max-frames=2` can be black. Use `--max-frames=60`.
- **Frame alignment:** imageviewer re-presents the same still each frame, so the
  *content* is fixed and only `FrameCount` advances — exactly our pump's
  still-image behavior. We render through our engine at `frame_index = 60` to
  match `--max-frames=60`.

### 3.4 Wire-in + calibration (`references.rs`)

Each reference PNG lives under `crates/testing/references/retroarch/<name>.png`
with a `<name>.toml` sidecar (preset, source, viewport, frame, RA version, driver,
GPU, calibrated `diff_tolerance`/`diff_max_fraction`). `references.rs` renders the
preset through our engine and `diff_images` against the capture. It is `#[ignore]`d
(needs the `slang-shaders` clone via `SLANG_SHADERS_DIR`, and was calibrated on
this box's llvmpipe — CI's lavapipe may round differently at the tight `crt-geom`
tolerance):

```bash
SLANG_SHADERS_DIR=/path/to/slang-shaders \
  WGPU_BACKEND=vulkan cargo test -p testing --test references \
  -- --ignored --test-threads=1
```

### 3.5 What matches vs what diverges

| Preset | Result | tol / max_frac | Observed (our engine vs RetroArch) |
| --- | --- | --- | --- |
| `crt/crt-geom.slangp` | **MATCH** (near-exact) | 4 / 0.001 | max_abs **2**, mean 0.001, 0 % over tol |
| `scanlines/scanline.slangp` | **MATCH** | 16 / 0.02 | max_abs 12, mean 0.155, 0.01 % over tol 8 |
| `ntsc/ntsc-256px-svideo-scanline.slangp` | **MATCH** | 24 / 0.05 | max_abs 53, mean 0.166, 0.73 % over tol 16 |
| `test/feedback.slangp` | **MATCH** (converged) | 4 / 0.001 | max_abs **4**, mean 0.67, 0 % over tol 4 at frame 60 |
| `crt/crt-royale.slangp` | **RENDERS, DIVERGES** (no test) | — | compiles all 12 passes + renders after the spirv-opt inlining fix, but max_abs **205**, mean 81.6, 85 % over tol 4 vs RA — our output is systematically brighter. Suspected: the sRGB-framebuffer (10/12 passes) gamma encode/decode and/or the mask-resize scale chain don't match RA's exact tonal response. A multi-feature fidelity gap — future ticket. |

The near-exact `crt-geom` match is because BOTH renderers ran on llvmpipe
(software): same rasterizer, same rounding. That such a non-trivial CRT shader
(curvature, vignette, dot-mask, interlace, gamma) lands within `max_abs = 2` is
strong evidence the engine's uniform packing, scaling, sampler attribution, and
fragment math are faithful to RetroArch.

### 3.6 feedback — the `PassFeedbackSize0` builtin fix (#32)

`test/feedback.slang` is `FragColor = mix(current, prev, 0.8)` =
`0.2*Source + 0.8*PassFeedback0`, whose fixed point is the source: after ~60
frames the previous-frame term has decayed and the output equals the source up
to 8-bit accumulation rounding. RetroArch converges to `source − ~2` and so does
our engine — they agree within `max_abs 4`.

Closing this required a **real engine fix**, not just frame alignment. The
shader snaps its sample coordinate with `floor(PassFeedbackSize0.xy * vTexCoord)`,
but our `BuiltinValues::member_bytes` never populated `PassFeedbackSize0`: the
size-member parser accepted only the alias spelling `PassFeedback0Size`, while
RetroArch's `slang_process.cpp` builds the name as `"PassFeedbackSize"` **then**
appends the index → `PassFeedbackSize0` (Size BEFORE the number). With the member
left zero the shader did `floor(0 · uv) = 0` and sampled texel (0,0) everywhere —
the source's white corner — so every pixel converged to ~253 ("accumulated to
white"), which is exactly the divergence the earlier report saw. The same
off-by-spelling affected `PassOutputSizeN` and `OriginalHistorySizeN` (the corpus
uses the `…SizeN` spelling exclusively). `parse_indexed_size` in
`crates/preview-engine/src/uniforms.rs` now accepts BOTH spellings RetroArch
emits/accepts.

### 3.7 crt-royale — the sampler-in-function inlining fix (#32)

crt-royale (and ~38 other corpus presets) factor sampling through GLSL helper
functions that take a `sampler2D` parameter. glslang lowers those to an
`OpFunctionCall` carrying the combined-sampler variable, which `split_samplers`
(handling only an inline `OpLoad` of a `sampler2D` global) correctly refused to
rewrite. `slang_compile::spirv_opt::inline_functions` now runs `spirv-opt
--merge-return --inline-entry-points-exhaustive --eliminate-dead-functions` as the
FIRST normalization step, folding every helper into the entry point so only the
inline form remains. (`--merge-return` is required: the inliner skips functions
with an early/multiple return, which crt-royale's mask-apply pass has.) It
degrades to a no-op skip when `spirv-opt` is absent. With it, all 12 crt-royale
passes compile and the preset renders — but it **diverges** from RetroArch (§3.5),
so no passing reference is committed; the divergence is the documented finding.

---

## 4. Lossless round-trip (#37, Phase-3 EXIT gate)

Phase 3 added preset **import** (`.slangp` → `core_model::Project`) and **export**
(the inverse bundle writer). Its exit gate is **losslessness**: import → export →
re-import must preserve the project, and an unmodified pass's `.slang` must export
byte-for-byte. The harness lives in `crates/testing/src/roundtrip.rs`
(`compare_projects` → a canonicalized `ProjectDiff`; `round_trip` drives one
preset and reports the structural diff + per-pass byte equality).

### Documented canonicalization

A round trip is compared modulo a few **deterministic identity rewrites the export
performs by design** (never value changes) — `compare_projects` canonicalizes
them so they are not false mismatches:

- **LUT path → basename**, since the export copies LUT images into `textures/`.
  The basename is further compared modulo (a) unsafe-char **sanitization**
  (`psp border.png` → `psp_border.png`) and (b) the shared-source **de-dup suffix**
  (several LUTs pointing at one image → `foo.png`, `foo_1.png`, …). The LUT name +
  bytes + sampler settings are unchanged.
- **Pass `filename`**, since the export may rename a `.slang` to avoid a collision
  (`dup.slang` → `dup_1.slang`); pass sources are compared by their bytes.
- **Not part of the `.slangp` round trip:** project `name`, document `metadata`,
  `library_refs`, and the *derived* pipeline `availability`/per-pass `references`
  (re-derived deterministically from the chain).

### Suites

- **CI (always-on, no GPU, no corpus):** `tests/roundtrip_fixtures.rs` sweeps every
  `.slangp` under `crates/testing/fixtures/` — including the
  `fixtures/roundtrip/kitchen_sink.slangp` "kitchen sink" that exercises every
  parsed feature in one bundle (multi-pass, all scale types, feedback, aliases,
  varied-sampler LUTs, parameter overrides, a preserved unknown key) — and asserts
  structure-lossless + per-pass byte equality. `tests/retroarch_export.rs` guards
  the committed RetroArch bundle (§ below).
- **Corpus losslessness fuzzer (opt-in, `#[ignore]` + `FUZZ_CORPUS_DIR`):**
  `tests/roundtrip_corpus.rs` runs the round trip over the real `slang-shaders`
  corpus (pinned commit `74bf541845e65cca7f09b3c6b3baeeea8f52afb3`) and asserts
  per-preset losslessness. Unlike the render fuzzer (§2), losslessness is the gate:
  a non-lossless preset is a **failure** unless it is a **classified exclusion**
  (reported with a reason — never a silent skip).

  ```bash
  FUZZ_CORPUS_DIR=/home/mfunk/Code/slang-shaders \
    cargo test -p testing --test roundtrip_corpus -- --ignored --nocapture
  ```

### Results on this box

- **Curated default subset** (`crt, ntsc, blurs, denoisers, interpolation,
  handheld, scanlines` — 310 presets): **297 lossless, 13 documented exclusions, 0
  failures**.
- **Full 33-category tree** (2210 presets): **1498 lossless, 712 documented
  exclusions, 0 failures**.

### Documented exclusions (the two classes — both intrinsic, neither a writer bug)

1. **Not parseable by the importer** — presets that use a `.slangp` feature the
   parser does not model, chiefly `#reference`-style **nested presets** (no
   `shaders` key of their own — e.g. all of `crt-yah/*`, the Mega_Bezel preset
   tree), plus the occasional malformed preset (e.g. a stray-quote value
   `float_framebuffer0 = true"`). This is the **same** pre-existing parser worklist
   as §2; the round trip can only cover what the parser accepts. Following
   `#reference` is a future parser ticket.
2. **Non-UTF-8 `.slang` source** — the model stores a pass body as a UTF-8
   `String`, so a shader file with non-UTF-8 bytes (e.g. an author name in Latin-1:
   `interpolation/quilez.slang` byte `0xf1`, `catmull-rom-4-taps.slang` byte
   `0x96`) cannot round-trip byte-for-byte. `round_trip` detects this
   (`RoundTrip::non_utf8_passes`) and the corpus test classifies it as an
   exclusion. Holding pass bodies as raw bytes would be a model change for a future
   ticket.

To add a *new* documented exclusion, add a `(path-substring, reason)` entry to
`KNOWN_EXCLUSIONS` in `tests/roundtrip_corpus.rs` and record it here, with a ticket
— there are no silent skips.

### Checked-in RetroArch-loadable bundle

`crates/testing/fixtures/retroarch_export/` is a real export bundle (produced by
the import → export path over the corpus `scanlines/scanline.slangp`, the same
preset §3 confirms renders in RetroArch 1.22.2). Its `README.md` documents the
manual RetroArch-1.22.2 load procedure; `examples/gen_retroarch_bundle.rs`
regenerates it; `tests/retroarch_export.rs` is the corpus-free CI gate that it
stays well-formed and round-trips losslessly.

---

## 5. Status — how much of the Phase-2 fidelity gate is closed

- **Automated and passing in CI:** headless deterministic render-to-PNG; the image
  diff + visual artifact; the corpus fuzzer over local fixtures; the self-oracle
  goldens (multi-pass, feedback, LUT); the re-baseline flow.
- **Automated + demonstrated here (opt-in `#[ignore]`, needs the corpus clone):**
  - the **real corpus fuzz** over `slang-shaders` — a curated 246-preset slice,
    further improved by #32's `spirv-opt` inlining unblocking the 38
    sampler-in-function presets (§2);
  - **real RetroArch references**: `crt-geom`, `scanline`, an NTSC preset, **and
    `feedback`** match RetroArch within calibrated thresholds (§3.5/§3.6).
- **Closed by #32:**
  - **feedback fidelity** — was a real engine bug (the `PassFeedbackSize0` builtin
    was never populated; §3.6). Now matches RetroArch (`max_abs 4`).
  - **`crt-royale` parse + compile** — the inline-comment `.slangp` parser gap and
    the sampler-in-function compile gap are both fixed; crt-royale now renders all
    12 passes (§3.7).
- **Still open (documented findings, NOT forced to pass):**
  - **`crt-royale` fidelity** — renders but diverges from RetroArch (`max_abs 205`,
    systematically brighter; suspected sRGB-framebuffer gamma / mask-resize scale
    chain — §3.5). A multi-feature gap for a future ticket.
  - **the remaining corpus worklist** — nested `#reference` presets, naga function
    shapes, and the other §2 failure kinds.
