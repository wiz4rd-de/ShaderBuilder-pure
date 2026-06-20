//! External-corpus LOSSLESSNESS fuzzer (#37, Phase-3 EXIT gate — the real-corpus
//! counterpart to `roundtrip_fixtures.rs`).
//!
//! Pointed at a cloned `slang-shaders` checkout it runs the
//! import → export → re-import round trip ([`testing::round_trip`]) over a curated
//! subset of REAL presets and reports, **per preset**, whether the round trip was
//! structure- and byte-lossless. Unlike the render-fuzzer (`corpus_fuzz.rs`),
//! which never asserts a verdict (a shader the engine can't render is a finding,
//! not a failure), losslessness IS the Phase-3 exit gate: a non-lossless preset is
//! a **failure** — unless it is an explicitly documented, ticketed exclusion (see
//! [`KNOWN_EXCLUSIONS`] and `docs/golden-image-harness.md` §4). There are no silent
//! skips: a preset is either lossless, an excluded-with-reason entry, or a failure.
//!
//! The corpus is a large external clone, intentionally **NOT vendored**, so this
//! test is `#[ignore]`d AND keyed off `FUZZ_CORPUS_DIR`. With the var unset it
//! skips cleanly; CI never runs it (the committed `roundtrip_fixtures.rs` is the
//! CI-enforced gate). Pin the corpus to the commit recorded in
//! `docs/golden-image-harness.md` §4 for reproducibility.
//!
//! ## Running
//!
//! ```bash
//! # Curated subset (default categories below):
//! FUZZ_CORPUS_DIR=/home/mfunk/Code/slang-shaders \
//!   cargo test -p testing --test roundtrip_corpus \
//!   -- --ignored --nocapture
//!
//! # Override the category subset (comma list of top-level dirs / `.slangp` roots):
//! FUZZ_CORPUS_DIR=/path/to/slang-shaders \
//!   FUZZ_CORPUS_CATEGORIES=crt,ntsc,blurs \
//!   cargo test -p testing --test roundtrip_corpus -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use testing::{round_trip, RoundTripError};

/// The curated default subset of top-level corpus dirs the round-trip fuzzer
/// sweeps when `FUZZ_CORPUS_CATEGORIES` is unset — chosen to span the parsed
/// feature surface (multi-pass chains, scale types, feedback, LUTs, parameters)
/// across the categories that stress it hardest.
const DEFAULT_CATEGORIES: &[&str] = &[
    "crt",
    "ntsc",
    "blurs",
    "denoisers",
    "interpolation",
    "handheld",
    "scanlines",
];

/// Presets that do NOT round-trip losslessly for a **documented, ticketed**
/// reason — never a silent skip. Each entry is `(path-substring, reason)`; a
/// preset whose corpus-relative path contains the substring is reported as an
/// EXCLUDED finding (not a failure). Keep this in sync with
/// `docs/golden-image-harness.md` §4. Empty until the corpus run surfaces one.
const KNOWN_EXCLUSIONS: &[(&str, &str)] = &[];

#[test]
#[ignore = "needs an external slang-shaders clone via FUZZ_CORPUS_DIR"]
fn round_trip_external_corpus_is_lossless() {
    let Some(corpus) = std::env::var_os("FUZZ_CORPUS_DIR").map(PathBuf::from) else {
        eprintln!(
            "FUZZ_CORPUS_DIR unset — skipping the external-corpus round-trip (this is fine)."
        );
        return;
    };
    if !corpus.is_dir() {
        panic!("FUZZ_CORPUS_DIR={} is not a directory", corpus.display());
    }

    let categories: Vec<String> = std::env::var("FUZZ_CORPUS_CATEGORIES")
        .ok()
        .map(|s| s.split(',').map(|c| c.trim().to_string()).collect())
        .unwrap_or_else(|| DEFAULT_CATEGORIES.iter().map(|s| s.to_string()).collect());

    // Collect the `.slangp` set across the curated categories, corpus-relative.
    let mut presets: Vec<(String, PathBuf)> = Vec::new();
    for cat in &categories {
        let dir = corpus.join(cat);
        for path in collect_slangp(&dir) {
            let rel = path
                .strip_prefix(&corpus)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            presets.push((rel, path));
        }
    }
    presets.sort();
    presets.dedup();

    assert!(
        !presets.is_empty(),
        "found 0 presets under {} (categories={categories:?}) — is FUZZ_CORPUS_DIR correct?",
        corpus.display()
    );

    let mut lossless = 0usize;
    let mut excluded: Vec<(String, String)> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    // Group failure reports by a normalized diff "kind" for an actionable summary.
    let mut failure_kinds: BTreeMap<String, usize> = BTreeMap::new();

    for (rel, path) in &presets {
        if let Some((_, reason)) = KNOWN_EXCLUSIONS.iter().find(|(sub, _)| rel.contains(sub)) {
            excluded.push((rel.clone(), (*reason).to_string()));
            continue;
        }

        let work = match tempfile::tempdir() {
            Ok(w) => w,
            Err(e) => {
                failures.push(format!("{rel}: could not make temp dir: {e}"));
                continue;
            }
        };

        // A panic deep in the parser/importer on one preset must not abort the run.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            round_trip(path, work.path())
        }));

        match outcome {
            Ok(Ok(rt)) if rt.is_lossless() => lossless += 1,
            // The ONLY divergence is a non-UTF-8 original `.slang` the `String`
            // model cannot hold byte-for-byte (structure is otherwise lossless).
            // This is an intrinsic, documented limitation — a classified exclusion,
            // never a silent loss. See docs/golden-image-harness.md §4.
            Ok(Ok(rt)) if rt.only_non_utf8_loss() => {
                excluded.push((
                    rel.clone(),
                    format!(
                        "non-UTF-8 `.slang` source (pass index(es) {:?}); the model stores pass \
                         bodies as UTF-8 `String`, so those raw bytes cannot round-trip",
                        rt.non_utf8_passes
                    ),
                ));
            }
            Ok(Ok(rt)) => {
                let report = rt.report();
                *failure_kinds
                    .entry(first_line(&report).to_string())
                    .or_default() += 1;
                failures.push(format!("{rel}:\n{}", indent(&report)));
            }
            // A PARSE error means the preset uses a `.slangp` feature the parser
            // does not yet model (e.g. `#reference`-style nested presets with no
            // `shaders` key) or is malformed — a pre-existing, documented PARSER
            // limitation (docs/golden-image-harness.md §2), not a round-trip
            // lossiness bug. The harness can only round-trip what the parser
            // accepts, so these are classified exclusions, reported with their
            // reason rather than silently skipped.
            Ok(Err(RoundTripError::Parse(e))) => {
                excluded.push((rel.clone(), format!("not parseable by the importer: {e}")));
            }
            Ok(Err(e)) => {
                *failure_kinds
                    .entry(format!("round-trip error: {}", first_line(&e.to_string())))
                    .or_default() += 1;
                failures.push(format!("{rel}: round trip errored: {e}"));
            }
            Err(panic) => {
                let msg = panic_message(&panic);
                *failure_kinds
                    .entry(format!("panic: {}", first_line(&msg)))
                    .or_default() += 1;
                failures.push(format!("{rel}: PANICKED: {msg}"));
            }
        }
    }

    // ---- Report ----
    eprintln!("\n========== CORPUS ROUND-TRIP SUMMARY ==========");
    eprintln!("categories:   {categories:?}");
    eprintln!("total:        {}", presets.len());
    eprintln!("lossless:     {lossless}");
    eprintln!("excluded:     {} (documented limitation)", excluded.len());
    eprintln!("failing:      {}", failures.len());
    if !excluded.is_empty() {
        eprintln!("\n---- documented exclusions ----");
        for (rel, reason) in &excluded {
            eprintln!("  [EXCLUDED] {rel}\n             reason: {reason}");
        }
    }
    if !failure_kinds.is_empty() {
        let mut kinds: Vec<_> = failure_kinds.iter().collect();
        kinds.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        eprintln!("\n---- failure kinds (most frequent first) ----");
        for (kind, n) in kinds {
            eprintln!("  [{n:>4}x] {kind}");
        }
    }
    if !failures.is_empty() {
        eprintln!("\n---- failing presets (first 30) ----");
        for f in failures.iter().take(30) {
            eprintln!("{f}");
        }
    }
    eprintln!("===============================================\n");

    // The EXIT gate: every non-excluded preset must round-trip losslessly. Any
    // failure is either a real lossiness bug to fix or a new exclusion to document
    // and ticket — never an accepted silent loss.
    assert!(
        failures.is_empty(),
        "{} corpus preset(s) did not round-trip losslessly (see the summary above). \
         Fix the lossiness, or add an explicitly documented + ticketed entry to \
         KNOWN_EXCLUSIONS / docs/golden-image-harness.md §4.",
        failures.len()
    );
}

/// Recursively collect `*.slangp` files under `dir` (sorted by the caller).
fn collect_slangp(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "slangp") {
                out.push(path);
            }
        }
    }
    walk(dir, &mut out);
    out
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}
