# Contributing & development workflow

This is a solo project, but the workflow is deliberately structured so that history,
issues, and the roadmap stay aligned. This document is the source of truth for **how
work is organised and merged**. The *what* and *why* live in the
[**wiki**](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki):

- [Specification](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Specification) — what the app is and does
- [Architecture](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Architecture) — module boundaries, the IR, the preview engine, the IPC contract
- [Implementation Plan](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Implementation-Plan) — the risk-ordered phases
- [Decision Log](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Decision-Log) — every major decision and its reasoning

---

## Roadmap → issues

The Implementation Plan is tracked as GitHub issues:

- **One milestone per phase** — `Phase 0 — Scaffolding & contracts` … `Phase 7 — v1 polish`, plus `Post-v1 backlog`.
- **One epic per phase** — a tracking issue (label `epic`) with the phase Goal / Scope / Dependencies / Exit criteria / References.
- **Sub-issues** under each epic (label `task`) — each a cohesive, independently reviewable unit of work, linked to its epic via GitHub's native sub-issues. A sub-issue's task checklist breaks the work into **commit-sized steps**.

A phase is **done** when every sub-issue under its epic is closed and the phase's exit criteria are met.

### Labels

| Label | Meaning |
|---|---|
| `phase-0` … `phase-7`, `post-v1` | Which phase the issue belongs to |
| `epic` | Phase-level tracking issue |
| `task` | Granular sub-issue (commit-sized work) |
| `area:*` | Subsystem / crate touched — `area:core-model`, `area:ir`, `area:codegen-slang`, `area:codegen-glslp`, `area:slang-compile`, `area:preview-engine`, `area:source`, `area:preset-io`, `area:tauri-app`, `area:ipc`, `area:frontend`, `area:ci`, `area:testing`, `area:docs` (crate names per [Architecture §B](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Architecture)) |

---

## Git workflow

The roadmap is risk-ordered, so the unit of integration is **one phase = one PR** (Decision Log #16).

### Integration branch

`develop` is the integration branch. Each phase is built on its `phase-N` branch
and **squash-merged into `develop`**. When a release is ready, `develop` is
merged into `main` via a single release PR — so `main` only ever advances through
reviewed releases.

### Branching

- **Phase work:** one short-lived branch per phase, named `phase-N` (e.g. `phase-1`). Branch from the latest `develop`.
- **Non-phase work** (tooling, docs, hotfixes that aren't tied to a phase): `chore/...`, `docs/...`, or `fix/...`.

### Commits

- Keep commits **granular** — one focused commit per task-checklist item, not one giant commit per phase.
- Reference the sub-issue in the commit subject so the work is traceable, e.g.

  ```
  feat(preview-engine): single offscreen pass renders an image through SPIR-V (#18)
  ```

- A loose [Conventional Commits](https://www.conventionalcommits.org/) prefix (`feat` / `fix` / `refactor` / `test` / `chore` / `docs`) is encouraged but not enforced.
- Rebase the phase branch onto `develop` to stay current rather than merging `develop` in.

### Pull requests — **squash-merge per phase**

1. Open the PR against **`develop`** when the phase's sub-issues are complete. Title it for the phase (e.g. `Phase 1 — Preview-engine vertical slice`).
2. In the PR description, close the work with `Closes #<epic>` and list the sub-issues it resolves.
3. Merge with **Squash and merge**.

This keeps `develop` at **exactly one commit per phase** — a clean, roadmap-aligned history — while the full granular commit history of the phase stays visible inside the merged PR. Pick a clear squash-commit message (the phase name + a one-line summary).

> Recommended repo setting (Settings → General → Pull Requests): enable **Allow squash merging** and, for tidiness, disable the merge-commit and rebase-merge options so squash is the default.

---

## Development

The stack is **Tauri** (Rust core + React / React Flow web UI), Linux-first for
v1. The Rust workspace is split into the crates listed in
[Architecture §B](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Architecture);
the web UI lives in `web/`.

### Prerequisites (Linux)

- **Rust** — installed automatically from [`rust-toolchain.toml`](./rust-toolchain.toml) by rustup.
- **Node.js 20+** and npm.
- **Tauri system deps** — WebKitGTK 4.1, libsoup-3.0, GTK 3, librsvg, a C toolchain.
  - Arch / Manjaro: `sudo pacman -S webkit2gtk-4.1 libsoup3 gtk3 librsvg base-devel`
  - Debian / Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libsoup-3.0-dev build-essential`
- Tauri CLI: `cargo install tauri-cli --version "^2"` (or use the bundled `npm --prefix web run tauri`).

### Common commands

| Task | Command |
|------|---------|
| Build the workspace | `cargo build --workspace` |
| Test the workspace | `cargo test --workspace` |
| Lint (as CI does) | `cargo clippy --workspace --all-targets -- -D warnings` |
| Format | `cargo fmt --all` |
| Regenerate TS bindings | `cargo test -p core-model` (writes `web/src/bindings/`) |
| Frontend typecheck / build | `cd web && npm run typecheck` / `npm run build` |
| Run the app | `cargo tauri dev` (from the repo root) |

> The frontend is a build input for the Tauri `app` crate — its assets are
> embedded by `generate_context!` — so run a frontend build (or `cargo tauri dev`,
> which does it for you) before compiling the `app` crate on its own.

### Running in a headless / VM environment

If you run the app over a remote or nested display without a GPU and hit a blank
window or a `Failed to create GBM buffer` error, force software rendering:

```bash
GDK_BACKEND=x11 WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 cargo tauri dev
```

### Continuous integration

[`.github/workflows/ci.yml`](./.github/workflows/ci.yml) runs on every push to
`main` / `develop` and on every pull request, on Linux:

- **frontend** job — `npm ci`, `npm run typecheck`, `npm run build`.
- **rust** job — `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`, and a **TypeScript
  bindings drift check** (`git diff --exit-code web/src/bindings` after
  regeneration) so the Rust schema and the generated TypeScript can never drift.

Once the repository is configured, mark these checks **required** for merging in
*Settings → Branches → Branch protection*.
