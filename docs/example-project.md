# The bundled example project

ShaderBuilder ships one example project — **"CRT Scanlines + Curvature"** — used by
onboarding (the start screen's **Open example**) and by the release smoke test
(#67). It is a small, two-pass CRT preset chosen because it loads, previews live,
and exports cleanly, exercising the whole import → preview → export loop.

## What it is

Two **whole-pass code** passes:

1. **Curvature** (`curvature.slang`) — barrel-distorts the source UVs so the image
   bulges like a curved CRT tube, with a corner vignette. Parameters:
   `CURVATURE`, `CORNER_SMOOTH`.
2. **Scanlines** (`scanlines.slang`) — modulates the curved image with a vertical
   sine pattern locked to the source's vertical resolution, plus a brightness
   boost. Parameters: `SCANLINE_WEIGHT`, `BRIGHTNESS`.

Both passes are plain, self-contained slang (no external textures/LUTs), so the
project is exportable as-is and round-trips losslessly.

## Where it lives (one source of truth)

- **Source fixture** —
  `crates/testing/fixtures/example/crt-scanlines-curvature.slangp` plus its two
  `.slang` files. This is the authored source.
- **Native project resource** — `crates/app/resources/example-project.json`. The
  editor/onboarding load *this*. It is **generated** from the source fixture and
  committed; the `testing` crate's `example_project` test regenerates it (run with
  `SB_REGEN_EXAMPLE=1`) and otherwise asserts the committed JSON has not drifted
  from the fixture. The app embeds this resource (`include_str!`) and serves it via
  the `load_example_project` command; it is also listed as a Tauri bundle resource.

## What the tests guarantee

`crates/testing/tests/example_project.rs` asserts the example:

- imports without warnings and is two whole-pass curvature-then-scanlines passes
  with the expected parameters,
- passes the export validation gate and **exports → re-imports structure-lossless**,
- matches the committed `example-project.json` (drift guard) and that JSON loads
  back to the same project.

The `app` crate's `import` tests additionally assert `load_example_project`
returns the named, two-pass, export-ready project.

## Regenerating after an edit

If you intentionally change the example shaders, regenerate the committed JSON:

```sh
SB_REGEN_EXAMPLE=1 cargo test -p testing --test example_project
```

then commit the updated `crates/app/resources/example-project.json` alongside the
fixture change.
