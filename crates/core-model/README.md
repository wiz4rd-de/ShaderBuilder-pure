# core-model

The **single shared serialization contract** for ShaderBuilder.

The `Project / Pass / Graph / Node / Parameter` model is defined here once as
Rust `serde` types. The matching TypeScript is **generated** from these types
with [`ts-rs`], so there is exactly one source of truth and **no hand-written TS
mirror to drift**. The same schema is used for all three of:

1. **IPC** — Tauri command/event payloads (Architecture §E).
2. **The native project file** — JSON on disk (Specification §6).
3. **Import / export** — the in-memory model a `.slangp` maps to.

See Architecture §A ("one shared serialization contract").

## Generated TypeScript bindings

The bindings live in [`web/src/bindings/`](../../web/src/bindings/) and are
**committed**. Regenerate them after changing any type here:

```bash
cargo test -p core-model
```

`ts-rs`'s `#[ts(export)]` emits a test per type that writes its `.ts` file when
`cargo test` runs. The output directory is the `TS_RS_EXPORT_DIR` set in the
workspace [`.cargo/config.toml`](../../.cargo/config.toml). CI regenerates and
runs `git diff --exit-code` on the bindings, so a stale binding fails the build.

> **Do not edit `web/src/bindings/` by hand** — it is generated.

## Conventions

- Fields serialize as `camelCase`.
- Tagged unions use an internal `"kind"` discriminator → a TypeScript
  discriminated union.

[`ts-rs`]: https://docs.rs/ts-rs
