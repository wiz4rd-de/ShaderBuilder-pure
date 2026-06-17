//! Thin wrapper over the `spirv-opt` CLI to **inline all helper functions** into a
//! stage's entry point before the rest of the SPIR-V normalization runs (#32).
//!
//! ## Why this exists (the wall it tears down)
//!
//! Real `.slang` shaders — crt-royale most prominently, plus ~38 other corpus
//! presets — factor their sampling through GLSL helper functions that take a
//! `sampler2D` parameter, e.g.
//!
//! ```glsl
//! vec4 tex2Daa(sampler2D tex, vec2 uv) { … texture(tex, uv) … }
//! void main() { FragColor = tex2Daa(Source, vTexCoord); }
//! ```
//!
//! glslang compiles that to SPIR-V where the combined `sampler2D` **variable** is
//! passed as an `OpFunctionCall` argument (glslang hands over the variable POINTER,
//! never loading it in the caller). [`crate::split_samplers`] only knows how to
//! rewrite an *inline* `OpLoad` of a `sampler2D` global into the separate
//! image+sampler form wgpu requires; a combined sampler flowing into an
//! `OpFunctionCall` is — correctly — rejected with
//! [`crate::SplitError::UnsupportedDeclaration`] rather than mis-rewritten into
//! corrupt SPIR-V. So every function-sampler shader failed to compile.
//!
//! Rather than teach `split_samplers` the (hard) inter-procedural rewrite, we run
//! `spirv-opt --merge-return --inline-entry-points-exhaustive` first: it inlines
//! every function body into the entry point, so **no** `OpFunctionCall` survives
//! carrying a sampler — and `split_samplers` then sees only the inline `OpLoad` /
//! `OpImageSample*` it already handles. The dead (now-uninlined) helper functions
//! are removed with `--eliminate-dead-functions` so they don't linger as orphaned
//! definitions that still reference the combined-sampler parameter types.
//!
//! `--merge-return` is load-bearing and runs FIRST: the inliner **refuses** to
//! inline a function whose `OpReturn` is not the last instruction (an early/multiple
//! return), warning *"could not be inlined because the return instruction is not at
//! the end of the function … fixed by running merge-return before inlining"*.
//! crt-royale's `crt-royale-scanlines-horizontal-apply-mask` pass has exactly such
//! helpers (`decode_input`, `sample_rgb_scanline_horizontal`, …), so without
//! `--merge-return` a sampler-carrying `OpFunctionCall` survives and
//! `split_samplers` still rejects the pass. With it, all calls are inlined.
//!
//! ## Why a CLI shell-out (mirroring [`crate::glslang`])
//!
//! Like glslang, we shell out to the `spirv-opt` binary from SPIRV-Tools rather
//! than linking a native library — identical reasoning: no native-linking / ABI /
//! cross-distro concerns, and CI installs it from a package (`spirv-tools`). The
//! invocation reads SPIR-V from a temp file and writes the optimized SPIR-V to
//! another, exactly as `glslang` stages its input/output.
//!
//! ## Conservatism (it must not break shaders that don't need it)
//!
//! * Inlining is **semantics-preserving**: a shader with no helper functions (the
//!   hand-written UBO-only / separate-sampler fixtures, and the inline-`texture()`
//!   shaders) is unchanged in behavior — the entry point already contains all the
//!   work, so there is nothing to inline. (The word stream may be reordered/renumbered
//!   by spirv-opt, but the downstream transforms + naga are id-agnostic, and
//!   `split_samplers` re-derives everything from types/decorations.)
//! * If `spirv-opt` is **absent** (a dev box without SPIRV-Tools), inlining is
//!   skipped with a one-time log and the original SPIR-V is returned: the engine
//!   still works for every shader that doesn't pass a sampler to a function (which
//!   is most of them). For the corpus / CI the binary IS present, so the
//!   function-sampler shaders are unblocked there.
//! * A `spirv-opt` that runs but *fails* (non-zero exit, malformed output) is a
//!   hard [`SpirvOptError`] tagged with the stage — never a silent pass-through of
//!   possibly-corrupt bytes.

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::preprocess::Stage;

/// The optimizer executable. Overridable via `SPIRV_OPT` for unusual setups
/// (mirrors `GLSLANG` for [`crate::glslang`]).
fn spirv_opt_bin() -> String {
    std::env::var("SPIRV_OPT").unwrap_or_else(|_| "spirv-opt".to_string())
}

/// Logged at most once when `spirv-opt` is not on PATH, so a dev box without
/// SPIRV-Tools doesn't spam a line per compile.
static MISSING_LOGGED: AtomicBool = AtomicBool::new(false);

/// Failure running `spirv-opt`. A *missing* binary is NOT an error (it degrades to
/// a skip — see [`inline_functions`]); this is only for a binary that ran and
/// failed, or produced malformed output.
#[derive(Debug)]
pub enum SpirvOptError {
    /// I/O while staging the temp files or reading the result back.
    Io(std::io::Error),
    /// `spirv-opt` ran but exited non-zero (carries the stage + captured output).
    Failed { stage: Stage, raw: String },
    /// `spirv-opt` produced output that is not a whole number of 32-bit words.
    MalformedSpirv,
}

impl std::fmt::Display for SpirvOptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpirvOptError::Io(e) => write!(f, "spirv-opt I/O error: {e}"),
            SpirvOptError::Failed { stage, raw } => {
                write!(f, "{stage:?} stage spirv-opt inlining failed: {raw}")
            }
            SpirvOptError::MalformedSpirv => write!(f, "spirv-opt emitted malformed SPIR-V"),
        }
    }
}

impl std::error::Error for SpirvOptError {}

/// Inline every helper function into the entry point with `spirv-opt` so no
/// `OpFunctionCall` survives carrying a combined sampler (then drop the dead
/// functions). Returns the transformed SPIR-V words.
///
/// **Degrades gracefully** when `spirv-opt` is not installed: it logs once and
/// returns the input unchanged, so the engine still compiles every shader that
/// doesn't pass a sampler to a function. (Those that do will then fail later in
/// [`crate::split_samplers`] with its clear error, exactly as before this module.)
///
/// # Errors
/// [`SpirvOptError`] if `spirv-opt` is present but exits non-zero or emits
/// malformed SPIR-V (tagged with `stage`); a *missing* binary is not an error.
pub fn inline_functions(stage: Stage, words: &[u32]) -> Result<Vec<u32>, SpirvOptError> {
    let dir = tempfile::tempdir().map_err(SpirvOptError::Io)?;
    let in_path = dir.path().join("in.spv");
    let out_path = dir.path().join("out.spv");

    {
        let mut file = std::fs::File::create(&in_path).map_err(SpirvOptError::Io)?;
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        file.write_all(&bytes).map_err(SpirvOptError::Io)?;
    }

    let bin = spirv_opt_bin();
    let output = Command::new(&bin)
        // `--merge-return` FIRST so functions with an early/multiple return become
        // single-return — otherwise the inliner refuses them (see module docs) and
        // a sampler-carrying call would survive. Then exhaustively inline EVERY call
        // into the entry point(s), and sweep the now-dead helper functions so no
        // orphaned definition referencing a combined-sampler parameter type remains.
        .arg("--merge-return")
        .arg("--inline-entry-points-exhaustive")
        .arg("--eliminate-dead-functions")
        .arg(&in_path)
        .arg("-o")
        .arg(&out_path)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => {
            // Binary not found / not runnable: skip inlining (degrade gracefully).
            if !MISSING_LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "slang-compile: '{bin}' not found; skipping SPIR-V function inlining. \
                     Shaders that pass a sampler to a helper function (e.g. crt-royale) \
                     will not compile until SPIRV-Tools is installed. Install it (e.g. \
                     `spirv-tools`) or set SPIRV_OPT."
                );
            }
            return Ok(words.to_vec());
        }
    };

    if !output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        return Err(SpirvOptError::Failed { stage, raw });
    }

    let bytes = std::fs::read(&out_path).map_err(SpirvOptError::Io)?;
    if bytes.len() % 4 != 0 {
        return Err(SpirvOptError::MalformedSpirv);
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `spirv-opt` is present on the dev box / CI; inlining a sampler-in-function
    /// shader's fragment SPIR-V must remove every `OpFunctionCall` that carries the
    /// sampler (leaving only the inline form `split_samplers` handles). Skipped —
    /// not failed — when `spirv-opt` is absent.
    #[test]
    fn inlines_sampler_helper_so_no_function_call_remains() {
        if Command::new(spirv_opt_bin())
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("inlines_sampler_helper_so_no_function_call_remains: spirv-opt absent, skip");
            return;
        }

        // A fragment that samples through a `sampler2D`-taking helper — the exact
        // shape glslang lowers to an OpFunctionCall carrying the sampler.
        let src = "\
#version 450
layout(set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Source;
vec4 sample_it(sampler2D s, vec2 uv) { return texture(s, uv); }
void main() { FragColor = sample_it(Source, vTexCoord); }
";
        // Compile just the fragment stage to raw glslang SPIR-V (no normalization).
        let pre = crate::preprocess::preprocess(src).expect("preprocess");
        let raw = crate::glslang::compile_stage(Stage::Fragment, &pre.fragment).expect("glslang");

        // Before inlining: there IS an OpFunctionCall (sanity check the fixture).
        assert!(
            count_op(&raw, rspirv::spirv::Op::FunctionCall) > 0,
            "fixture should contain an OpFunctionCall before inlining"
        );

        let inlined = inline_functions(Stage::Fragment, &raw).expect("inline");
        assert_eq!(
            count_op(&inlined, rspirv::spirv::Op::FunctionCall),
            0,
            "exhaustive inlining must remove every OpFunctionCall"
        );

        // And the whole compile (glslang → inline → push_to_ubo → split_samplers)
        // now succeeds where it previously hit SplitError::UnsupportedDeclaration.
        let shader = crate::compile_slang(src, None).expect("compile after inlining");
        assert!(!shader.fragment_spirv.is_empty());
    }

    /// A shader with NO helper functions (the inline-`texture()` form) still
    /// compiles end-to-end through the inlining step — inlining is a no-op for
    /// behavior. Runs regardless of `spirv-opt` presence (degrades to a skip).
    #[test]
    fn no_function_shader_still_compiles_through_inlining() {
        let src = "\
#version 450
layout(set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Source;
void main() { FragColor = texture(Source, vTexCoord); }
";
        let shader = crate::compile_slang(src, None).expect("compile");
        assert!(!shader.fragment_spirv.is_empty());
    }

    /// Count instructions with a given opcode in a SPIR-V word stream.
    fn count_op(words: &[u32], op: rspirv::spirv::Op) -> usize {
        let module = rspirv::dr::load_words(words).expect("parse spirv");
        module
            .all_inst_iter()
            .filter(|i| i.class.opcode == op)
            .count()
    }
}
