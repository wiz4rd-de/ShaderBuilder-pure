# `crates/testing/fixtures` — round-trip & import/export fixtures

These fixtures back the Phase-3 import/export gates:

- `roundtrip/` — the `kitchen_sink.slangp` preset (+ its stub `.slang` bodies and
  `border.png`/`grade.png` LUTs) exercising **every parsed `.slangp` feature** for
  the lossless import → export → re-import suite
  (`crates/testing/tests/roundtrip_fixtures.rs`).
- `retroarch_export/` — a checked-in, RetroArch-loadable export bundle (see its own
  `README.md`).
- `multipass/`, `lut/`, `params/`, `feedback/`, `orphan_override/` — focused
  fixtures for the per-feature import/export tests.

## Tracked Phase-3 follow-up — `#include` dependencies are NOT bundled on export (B5)

A pass `.slang` body may pull in other files with `#include` or
`#pragma include_optional` — shared headers, parameter blocks, library helpers. The
export bundle writer (`preset-io::export_preset`) writes each whole-pass source
**byte-for-byte** but does **not**:

- copy the included files into the bundle, nor
- capture/reproduce the transitive include closure preserving its relative layout, nor
- rewrite the `#include` paths.

So an include-using preset exported today **may fail to load in RetroArch as-is**.

This is a **known, deferred limitation** — the full fix (walking the include graph
and reproducing it in the bundle with relative layout intact) is a tracked
**Phase-3 follow-up**, beyond the scope of the issue that introduced the exporter.
The gap is deliberately **non-silent**: `export_preset` scans each pass source for
include directives and pushes a clear `ExportReport::warnings` entry naming the pass
file whenever any are present, so a caller is told the bundle may be incomplete
rather than discovering it at RetroArch load time. See the `preset-io::export`
module docs for the runtime behavior and `export.rs`'s
`include_in_pass_source_yields_export_warning` test for the asserted contract.

Until the follow-up lands, corpus presets that depend on `#include`d files are
expected to surface this export warning; the round-trip losslessness gate
(`roundtrip_corpus.rs`) classifies any preset it cannot import losslessly as a
documented exclusion (`KNOWN_EXCLUSIONS`) rather than a silent skip.
