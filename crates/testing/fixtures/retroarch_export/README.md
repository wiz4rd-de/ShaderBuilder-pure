# RetroArch-loadable export bundle (#37, Phase-3 EXIT gate)

This directory is a **checked-in export bundle** produced by ShaderBuilder's
import → export path, kept as proof that an exported preset **loads and renders in
real RetroArch**. It is one of the two Phase-3 exit gates (the other is the
automated lossless round-trip suite in `crates/testing/tests/roundtrip_*.rs`).

## What's here

```
retroarch_export/
  preset.slangp     # the exported preset — relative paths, single pass
  scanline.slang    # the pass source, BYTE-IDENTICAL to the corpus original
  README.md         # this file
```

`preset.slangp`:

```
shaders = 1

shader0 = scanline.slang
scale_type0 = viewport
```

## Provenance (how it was produced)

It is **not hand-written**. It is the output of running the real importer and the
real export bundle writer over a known-good corpus preset:

- **Source preset:** `scanlines/scanline.slangp` from the
  [`libretro/slang-shaders`](https://github.com/libretro/slang-shaders) corpus,
  pinned at commit `74bf541845e65cca7f09b3c6b3baeeea8f52afb3`.
- This is the **same** single-pass preset the Phase-2 reference suite
  (`crates/testing/tests/references.rs`,
  `crates/testing/references/retroarch/scanline.toml`) confirms renders in
  **RetroArch 1.22.2** within calibrated thresholds, so it is independently known
  to be RetroArch-valid.
- Regenerate with:

  ```bash
  FUZZ_CORPUS_DIR=/home/mfunk/Code/slang-shaders \
    cargo run -p testing --example gen_retroarch_bundle
  ```

  (`crates/testing/examples/gen_retroarch_bundle.rs`). The exported `scanline.slang`
  is byte-identical to the corpus original — the export writes imported pass
  sources verbatim (#34/#36).

## Manual verification procedure (RetroArch 1.22.2)

"Loads without errors" means: RetroArch parses `preset.slangp`, compiles the slang
pass, builds the shader chain, and renders the running content through it with **no
error dialog and no shader-compile error in the log** (the picture changes — the
scanline modulation is visible — confirming the chain is actually applied).

1. Install **RetroArch 1.22.2** with a slang-capable video driver (`glcore` or
   `vulkan`). Verify under *Information → System Information* that
   *Slang/SPIRV support* is present.
2. Copy this whole `retroarch_export/` directory somewhere RetroArch can read it
   (e.g. `~/.config/retroarch/shaders/shaderbuilder_scanline/`). Keep
   `preset.slangp` and `scanline.slang` **together** — the preset references the
   shader by the relative name `scanline.slang`.
3. Launch any core + content (a still image via the *imageviewer* core is enough;
   the Phase-2 reference capture used exactly that — see
   `docs/golden-image-harness.md` §3).
4. *Main Menu → Shaders → Load Shader Preset →* navigate to and select
   `preset.slangp`.
5. **Pass criterion:** the preset loads with no error notification, the on-screen
   notification reads *"Shader preset loaded"* (or equivalent), and the visible
   image now shows the horizontal scanline modulation. Open
   *Settings → Logging → Frontend Logging Level = Debug* and confirm the log shows
   the slang pass compiling with no `[GLCore]`/`[Vulkan]` shader error.

A headless equivalent (the procedure the reference suite automates) is documented
in `docs/golden-image-harness.md` §3.3: build the bundled `imageviewer` core and
run `retroarch --appendconfig … <still>.png` with the preset, then screenshot.

## Why this bundle and not the kitchen-sink fixture

The `fixtures/roundtrip/kitchen_sink.slangp` preset exercises **every parsed
`.slangp` feature** for the lossless round-trip suite, but its `.slang` bodies are
minimal stubs that are not meant to render. This bundle is deliberately a *real*,
known-rendering preset so the "loads in RetroArch" gate is meaningful.
