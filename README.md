# ShaderBuilder-pure

A desktop app for building **RetroArch shader presets** as a **node-based visual
code generator**, with a **full real-time preview** of the multi-pass pipeline.
Aimed at power users who know GLSL — the value is instant visual feedback,
trivial parameter wiring, and reuse, *not* hiding the code.

> **slang-first** (`.slangp` / `.slang`), Linux for v1, open source. Built with
> **Tauri** (Rust core + React / React Flow web UI) and a faithful **wgpu**
> re-implementation of RetroArch's slang runtime for the preview.

The *what* and *why* live in the
[**wiki**](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki)
([Specification](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Specification) ·
[Architecture](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Architecture) ·
[Implementation Plan](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Implementation-Plan) ·
[Decision Log](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Decision-Log)).
The *how it's built and merged* lives in
[**CONTRIBUTING.md**](./CONTRIBUTING.md).

## Status

**Phase 0 — Scaffolding & contracts.** An empty-but-runnable Tauri app: the Rust
workspace, the React/React Flow frontend shell, the shared `core-model` schema
(with generated TypeScript), and the binary frame transport are in place. No real
shader compilation or rendering yet — that is Phase 1+.

## Repository layout

```
crates/                 Rust workspace (one crate per Architecture §B module)
  core-model/           Project/Graph/IR types, serde, TS-type export — the shared contract
  ir/                   graph → IR lowering, type checking, diagnostics
  codegen-slang/        IR → slang emitter (primary, previewed)
  codegen-glslp/        IR → glslp emitter (post-v1)
  slang-compile/        slang preprocess → glslang → SPIR-V; shader cache
  preview-engine/       wgpu device, pass graph, feedback/history, render thread
  source/               image / test-pattern / PNG-seq frame pump
  preset-io/            .slangp/.slang import + export bundle writer
  app/                  Tauri binary: commands, channels, window, wiring
web/                    React + React Flow frontend (Vite + TypeScript)
  src/bindings/         TypeScript types generated from core-model (do not edit by hand)
```

## Development

Prerequisites (Linux):

- **Rust** (pinned by [`rust-toolchain.toml`](./rust-toolchain.toml); install via [rustup](https://rustup.rs)).
- **Node.js** 20+ and npm.
- **Tauri system deps** — WebKitGTK 4.1, libsoup-3.0, GTK 3, and a C toolchain.
  On Arch/Manjaro: `webkit2gtk-4.1 libsoup3 gtk3 base-devel`.
  See the [Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/).
- The [Tauri CLI](https://v2.tauri.app/reference/cli/): `cargo install tauri-cli --version "^2"`.

```bash
# Rust workspace
cargo build --workspace
cargo test  --workspace

# Regenerate the TypeScript bindings from core-model (committed; CI checks for drift)
cargo run -p core-model --bin gen-bindings

# Frontend
cd web
npm install
npm run typecheck
npm run build

# Run the app (from the repo root)
cargo tauri dev
```

### Manual preview run (curvature shader over the test image)

The Phase 1 exit slice — a curvature/warp shader rendered over a test image —
is proven headlessly by the `e2e_curvature` integration test:

```bash
# Real, deterministic GPU run of the full slice (no webview). Writes the live
# rendered snapshot to target/e2e-artifacts/curvature_output.png.
cargo test -p preview-engine --test e2e_curvature
```

The bundled fixtures it uses live under
[`crates/preview-engine/tests/fixtures/`](crates/preview-engine/tests/fixtures/):

- `curvature.slang` — one-pass barrel-distortion warp shader,
- `passthrough.slang` — the control shader,
- `test_source.png` — the 128×128 four-quadrant/grid/diagonal source image,
- `curvature_reference.png` — a committed reference snapshot (documentation /
  Phase-2 golden-suite seed only; **not** byte-compared in CI, since the CI
  software-Vulkan/lavapipe output won't match a hardware GPU).

To see the same warp on the app's `<canvas>`, launch the app and load the
bundled `test_source.png` + `curvature.slang`. On a headless box (or one whose
GDK/WebKit GPU path misbehaves) launch with the software-render env:

```bash
GDK_BACKEND=x11 WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 \
  cargo tauri dev
```

CI runs all of the above on Linux for every push and PR — see
[`.github/workflows/ci.yml`](./.github/workflows/ci.yml) and the
[Development section of CONTRIBUTING.md](./CONTRIBUTING.md#development).

## License

Open source; the specific license is yet to be chosen (tracked for a later
phase). Until then all rights are reserved by the author.
