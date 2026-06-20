//! External-corpus fuzzer (#32, PART A — "import-and-render a broad slice without
//! crashing").
//!
//! This is the opt-in, real-corpus counterpart to `fuzz_fixtures.rs` (which runs
//! [`testing::fuzz_presets`] over the tiny committed fixtures). Pointed at a cloned
//! `slang-shaders` checkout it import-and-renders a broad slice of REAL presets and
//! prints a categorized summary — the per-top-level-category compile/render/fail
//! counts plus the distinct failure messages grouped by kind. Those failure groups
//! are the actionable engine-gap worklist.
//!
//! The corpus is a large external clone and is intentionally **NOT vendored** (see
//! `docs/golden-image-harness.md`), so this test is `#[ignore]`d AND keyed off the
//! `FUZZ_CORPUS_DIR` environment variable. With the var unset it skips cleanly; CI
//! never runs it.
//!
//! ## Running
//!
//! ```bash
//! # Whole corpus (slow on a software GPU — prefer a bounded subset, below).
//! FUZZ_CORPUS_DIR=/path/to/slang-shaders \
//!   WGPU_BACKEND=vulkan cargo test -p testing --test corpus_fuzz \
//!   -- --ignored --nocapture --test-threads=1
//!
//! # Bounded subset: a curated set of category dirs (recommended).
//! FUZZ_CORPUS_DIR=/path/to/slang-shaders \
//!   FUZZ_CORPUS_CATEGORIES=crt,ntsc,blurs,denoisers,interpolation \
//!   WGPU_BACKEND=vulkan cargo test -p testing --test corpus_fuzz \
//!   -- --ignored --nocapture --test-threads=1
//!
//! # Or cap the total preset count (sampled deterministically across the tree):
//! FUZZ_CORPUS_DIR=/path/to/slang-shaders FUZZ_CORPUS_MAX=300 ...
//! ```
//!
//! It never asserts a pass/fail verdict on the corpus (a real shader the engine
//! cannot yet render is a *finding*, not a test failure): it only fails if it found
//! NO presets at all (a mis-pointed `FUZZ_CORPUS_DIR`), so an empty run can't be
//! mistaken for a clean one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use source::Frame;
use testing::{fuzz_presets, PresetResult};

/// Source frame fed to every preset: the committed reference test card if present
/// (so the corpus run uses the same input as the RetroArch references), else a
/// small synthesized gradient.
fn corpus_source() -> Frame {
    let testcard = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("references")
        .join("src")
        .join("testcard_320x240.png");
    if let Ok(img) = image::open(&testcard) {
        let img = img.to_rgba8();
        return Frame::new(img.width(), img.height(), img.into_raw());
    }
    let (w, h) = (64u32, 48u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&[(x * 4) as u8, (y * 5) as u8, 96, 255]);
        }
    }
    Frame::new(w, h, rgba)
}

/// The viewport the corpus is rendered at — modest so a software GPU keeps up,
/// non-square so a degenerate scale shows.
const VIEWPORT: (u32, u32) = (320, 240);

/// Frame index — a couple of frames in so feedback/history/animated shaders are
/// past their cold first frame.
const FRAME_INDEX: u64 = 2;

#[test]
#[ignore = "needs an external slang-shaders clone via FUZZ_CORPUS_DIR"]
fn fuzz_external_corpus() {
    let Some(corpus) = std::env::var_os("FUZZ_CORPUS_DIR").map(PathBuf::from) else {
        eprintln!("FUZZ_CORPUS_DIR unset — skipping the external-corpus fuzz (this is fine).");
        return;
    };
    if !corpus.is_dir() {
        panic!("FUZZ_CORPUS_DIR={} is not a directory", corpus.display());
    }

    // Which top-level category dirs to walk. `FUZZ_CORPUS_CATEGORIES` (comma-list)
    // restricts to a curated subset; absent, the whole tree is walked (cap it with
    // FUZZ_CORPUS_MAX).
    let categories: Option<Vec<String>> = std::env::var("FUZZ_CORPUS_CATEGORIES")
        .ok()
        .map(|s| s.split(',').map(|c| c.trim().to_string()).collect());
    let max: Option<usize> = std::env::var("FUZZ_CORPUS_MAX")
        .ok()
        .and_then(|s| s.parse().ok());

    let source = corpus_source();
    let mut results: Vec<PresetResult> = Vec::new();

    match &categories {
        Some(cats) => {
            for cat in cats {
                let dir = corpus.join(cat);
                let mut r = fuzz_presets(&dir, &source, VIEWPORT, FRAME_INDEX);
                // Prefix each name with its category so the report is grouped.
                for res in &mut r {
                    res.name = format!("{cat}/{}", res.name);
                }
                eprintln!("  fuzzed category {cat}: {} preset(s)", r.len());
                results.extend(r);
            }
        }
        None => {
            results = fuzz_presets(&corpus, &source, VIEWPORT, FRAME_INDEX);
        }
    }

    // Optional deterministic cap: keep an evenly-spaced sample across the sorted
    // list so the subset spans categories rather than the alphabetically-first dir.
    if let Some(max) = max {
        if results.len() > max {
            let stride = results.len() as f64 / max as f64;
            results = (0..max)
                .map(|i| results[(i as f64 * stride) as usize].clone())
                .collect();
        }
    }

    assert!(
        !results.is_empty(),
        "fuzzed 0 presets — is FUZZ_CORPUS_DIR={} (categories={categories:?}) correct?",
        corpus.display()
    );

    print_report(&results);
}

/// Print the categorized summary: per top-level category counts, then the distinct
/// failure messages grouped by normalized kind.
fn print_report(results: &[PresetResult]) {
    let total = results.len();
    let compiled = results.iter().filter(|r| r.compiled).count();
    let rendered = results.iter().filter(|r| r.rendered).count();
    let ok = results.iter().filter(|r| r.ok()).count();

    eprintln!("\n========== CORPUS FUZZ SUMMARY ==========");
    eprintln!("total presets:    {total}");
    eprintln!(
        "compiled:         {compiled} ({:.1}%)",
        pct(compiled, total)
    );
    eprintln!(
        "rendered (ok):    {rendered} ({:.1}%)",
        pct(rendered, total)
    );
    eprintln!("fully ok:         {ok} ({:.1}%)", pct(ok, total));

    // Per top-level-category breakdown.
    let mut by_cat: BTreeMap<String, [usize; 4]> = BTreeMap::new(); // [total, compiled, rendered, ok]
    for r in results {
        let cat = r.name.split('/').next().unwrap_or("<root>").to_string();
        let e = by_cat.entry(cat).or_default();
        e[0] += 1;
        e[1] += r.compiled as usize;
        e[2] += r.rendered as usize;
        e[3] += r.ok() as usize;
    }
    eprintln!("\n---- per category (total / compiled / rendered / ok) ----");
    for (cat, [t, c, rd, o]) in &by_cat {
        eprintln!("  {cat:<24} {t:>4} / {c:>4} / {rd:>4} / {o:>4}");
    }

    // Failure messages grouped by normalized kind.
    let mut groups: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for r in results.iter().filter(|r| !r.ok()) {
        let msg = r.error.clone().unwrap_or_else(|| "<no message>".into());
        let kind = normalize_failure(&msg);
        let entry = groups.entry(kind).or_insert((0, Vec::new()));
        entry.0 += 1;
        if entry.1.len() < 3 {
            entry.1.push(format!("{}: {}", r.name, truncate(&msg, 200)));
        }
    }
    let mut group_vec: Vec<(&String, &(usize, Vec<String>))> = groups.iter().collect();
    group_vec.sort_by_key(|g| std::cmp::Reverse(g.1 .0)); // most frequent first
    eprintln!(
        "\n---- failure modes grouped by kind ({} distinct, {} total failing) ----",
        group_vec.len(),
        total - ok
    );
    for (kind, (count, examples)) in group_vec.iter().take(40) {
        eprintln!("  [{count:>4}x] {kind}");
        for ex in examples.iter() {
            eprintln!("           e.g. {ex}");
        }
    }
    eprintln!("=========================================\n");
}

fn pct(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        100.0 * n as f64 / d as f64
    }
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// Collapse a concrete error message into a stable failure *kind* by stripping
/// paths, ids, line numbers, and other per-preset specifics — so semantically
/// identical failures group together in the report.
fn normalize_failure(msg: &str) -> String {
    let mut out = msg.replace('\n', " ");
    // Strip absolute paths (keep the file name's tail).
    out = strip_paths(&out);
    // Collapse runs of digits and hex ids.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_digit = false;
    for ch in out.chars() {
        if ch.is_ascii_digit() {
            if !prev_digit {
                collapsed.push('#');
            }
            prev_digit = true;
        } else {
            collapsed.push(ch);
            prev_digit = false;
        }
    }
    truncate(&collapsed, 160)
}

/// Replace `/abs/path/to/foo.ext` runs with just `foo.ext` so paths don't make
/// every message unique.
fn strip_paths(s: &str) -> String {
    s.split_whitespace()
        .map(|tok| {
            if tok.contains('/') {
                tok.rsplit('/').next().unwrap_or(tok).to_string()
            } else {
                tok.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
