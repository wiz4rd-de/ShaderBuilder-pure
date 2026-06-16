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

### Branching

- **Phase work:** one short-lived branch per phase, named `phase-N` (e.g. `phase-1`). Branch from the latest `main`.
- **Non-phase work** (tooling, docs, hotfixes that aren't tied to a phase): `chore/...`, `docs/...`, or `fix/...`.

### Commits

- Keep commits **granular** — one focused commit per task-checklist item, not one giant commit per phase.
- Reference the sub-issue in the commit subject so the work is traceable, e.g.

  ```
  feat(preview-engine): single offscreen pass renders an image through SPIR-V (#18)
  ```

- A loose [Conventional Commits](https://www.conventionalcommits.org/) prefix (`feat` / `fix` / `refactor` / `test` / `chore` / `docs`) is encouraged but not enforced.
- Rebase the phase branch onto `main` to stay current rather than merging `main` in.

### Pull requests — **squash-merge per phase**

1. Open the PR when the phase's sub-issues are complete. Title it for the phase (e.g. `Phase 1 — Preview-engine vertical slice`).
2. In the PR description, close the work with `Closes #<epic>` and list the sub-issues it resolves.
3. Merge with **Squash and merge**.

This keeps `main` at **exactly one commit per phase** — a clean, roadmap-aligned history — while the full granular commit history of the phase stays visible inside the merged PR. Pick a clear squash-commit message (the phase name + a one-line summary).

> Recommended repo setting (Settings → General → Pull Requests): enable **Allow squash merging** and, for tidiness, disable the merge-commit and rebase-merge options so squash is the default.

---

## Development

> Status: design phase — the crate skeleton and Tauri shell land in **Phase 0**. Until then this section is a placeholder.

The stack is **Tauri** (Rust core + React / React Flow web UI), Linux-first for v1. The Rust workspace is split into the crates listed in [Architecture §B](https://github.com/wiz4rd-de/ShaderBuilder-pure/wiki/Architecture); the web UI lives in `web/`. Build/test commands will be documented here once Phase 0 establishes the workspace and CI.
