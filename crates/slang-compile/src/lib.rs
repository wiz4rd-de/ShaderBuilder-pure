//! `slang-compile` — RetroArch-faithful slang preprocessing
//! (`#pragma stage/name/format/parameter`, `#include`), VS/FS split, then
//! glslang → SPIR-V (Architecture §D). The riskiest single link in the toolchain
//! (Architecture §G risk 3).
//!
//! [`compile_slang`] is the pure entry point: source string in, SPIR-V + parsed
//! reflection out. [`cache::CompileCache`] wraps it with a content-hash cache so
//! identical input skips glslang.

pub mod cache;
mod glslang;
pub mod preprocess;
pub mod push_to_ubo;
pub mod reflect;
pub mod spirv_opt;
pub mod split_samplers;

use std::path::Path;

pub use core_model::Parameter;
pub use glslang::{Diagnostic, GlslangError};
pub use preprocess::{PreprocessError, Preprocessed, Reflection, Stage};
pub use push_to_ubo::PushToUboError;
pub use reflect::{
    reflect, BlockBinding, MemberKind, ReflectError, ResourceBinding, ScalarType, SpirvReflection,
    UniformBlock, UniformMember,
};
pub use spirv_opt::SpirvOptError;
pub use split_samplers::SplitError;

/// Crate identity marker (kept from the Phase 0 scaffold so dependent crates'
/// smoke tests keep the dependency edge live).
pub const NAME: &str = "slang-compile";

/// A compiled shader: SPIR-V for both stages plus the parsed slang reflection.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledShader {
    /// Vertex-stage SPIR-V words.
    pub vertex_spirv: Vec<u32>,
    /// Fragment-stage SPIR-V words.
    pub fragment_spirv: Vec<u32>,
    /// Pass name, FBO format, and `#pragma parameter` declarations.
    pub reflection: Reflection,
}

/// Anything that can go wrong turning a `.slang` source into SPIR-V.
#[derive(Debug)]
pub enum CompileError {
    /// Preprocessing (includes / stage split / pragma parsing) failed.
    Preprocess(PreprocessError),
    /// glslang rejected a stage or could not be run.
    Glslang(GlslangError),
    /// The combined-image-sampler → separate-sampler SPIR-V transform failed
    /// (unparseable SPIR-V, or a combined-sampler shape the transform refuses to
    /// rewrite — see [`SplitError`]). Carries which stage hit it.
    SplitSamplers {
        /// Which stage's SPIR-V failed to transform.
        stage: Stage,
        /// The underlying split error.
        source: SplitError,
    },
    /// The push-constant → UBO SPIR-V transform failed (unparseable SPIR-V — see
    /// [`PushToUboError`]). Carries which stage hit it.
    PushToUbo {
        /// Which stage's SPIR-V failed to transform.
        stage: Stage,
        /// The underlying rewrite error.
        source: PushToUboError,
    },
    /// `spirv-opt` function-inlining (#32) ran but failed for a stage. A *missing*
    /// `spirv-opt` binary is NOT this error — inlining degrades to a skip (see
    /// [`spirv_opt::inline_functions`]); this is only a binary that ran and failed.
    SpirvOpt {
        /// Which stage's SPIR-V failed to inline.
        stage: Stage,
        /// The underlying optimizer error.
        source: SpirvOptError,
    },
}

impl From<PreprocessError> for CompileError {
    fn from(e: PreprocessError) -> Self {
        CompileError::Preprocess(e)
    }
}

impl From<GlslangError> for CompileError {
    fn from(e: GlslangError) -> Self {
        CompileError::Glslang(e)
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Preprocess(e) => write!(f, "{e}"),
            CompileError::Glslang(e) => write!(f, "{e}"),
            CompileError::SplitSamplers { stage, source } => {
                write!(f, "{stage:?} stage sampler split failed: {source}")
            }
            CompileError::PushToUbo { stage, source } => {
                write!(f, "{stage:?} stage push-constant rewrite failed: {source}")
            }
            CompileError::SpirvOpt { stage, source } => {
                write!(f, "{stage:?} stage spirv-opt inlining failed: {source}")
            }
        }
    }
}

impl std::error::Error for CompileError {}

/// Compile a `.slang` source string to SPIR-V. `base_dir` resolves `#include`
/// paths (pass the directory the `.slang` file lives in). Pure — no caching; use
/// [`cache::CompileCache`] for that.
pub fn compile_slang(
    source: &str,
    base_dir: Option<&Path>,
) -> Result<CompiledShader, CompileError> {
    let inlined = preprocess::resolve_includes(source, base_dir)?;
    let pre = preprocess::preprocess(&inlined)?;
    compile_preprocessed(&pre)
}

/// Compile already-preprocessed per-stage GLSL. Shared by [`compile_slang`] and
/// the cache (which preprocesses once to form its key).
///
/// Both stages are compiled to SPIR-V, then normalized into the binding model the
/// engine speaks (three SPIR-V→SPIR-V transforms, applied per stage by
/// [`normalize_stage`]):
///
/// 1. **spirv-opt function inlining** (#32): inline every helper function into the
///    entry point so no `OpFunctionCall` carries a combined sampler — without this,
///    `split_samplers` rejects the ~38 corpus presets (crt-royale included) that
///    factor sampling through a `sampler2D`-taking helper. Degrades to a no-op skip
///    when `spirv-opt` is absent (see [`spirv_opt`]).
/// 2. **push-constant → UBO** (#32): real slang shaders put their parameter block
///    in a Vulkan `push_constant`, which wgpu/naga reject as the unsupported
///    `IMMEDIATES` capability. The block is rewritten to an ordinary UBO at a
///    binding chosen to be free across **both** stages (so the same block lands at
///    the same binding in each stage — otherwise reflection sees it at two
///    bindings, one colliding with a texture; see [`push_to_ubo`]).
/// 3. **combined → separate samplers** (`split_samplers`): glslang emits
///    `sampler2D` as a combined `OpTypeSampledImage` that naga cannot parse; it is
///    split into the separate image + sampler form wgpu requires.
///
/// All three are no-ops on SPIR-V that doesn't use the respective construct (the
/// hand-written UBO / separate-sampler fixtures pass through unchanged in behavior;
/// inlining a function-free shader changes nothing it does). Each error is tagged
/// with the failing stage.
pub(crate) fn compile_preprocessed(pre: &Preprocessed) -> Result<CompiledShader, CompileError> {
    let raw_vertex = glslang::compile_stage(Stage::Vertex, &pre.vertex)?;
    let raw_fragment = glslang::compile_stage(Stage::Fragment, &pre.fragment)?;

    // Pick ONE binding for the push-constant-turned-UBO that is free in BOTH
    // stages, so the rewritten block is bound consistently (a per-stage choice
    // would clash with a fragment-only texture — #32 corpus finding).
    let push_binding = push_to_ubo::free_binding_across(&[&raw_vertex, &raw_fragment]);

    let vertex_spirv = normalize_stage(Stage::Vertex, raw_vertex, push_binding)?;
    let fragment_spirv = normalize_stage(Stage::Fragment, raw_fragment, push_binding)?;
    Ok(CompiledShader {
        vertex_spirv,
        fragment_spirv,
        reflection: pre.reflection.clone(),
    })
}

/// Apply the SPIR-V normalization transforms to one stage's raw glslang SPIR-V,
/// tagging each error with the failing stage. The order is load-bearing:
///
/// 1. **spirv-opt function inlining** (#32) FIRST — inline every helper function
///    into the entry point so no `OpFunctionCall` survives carrying a combined
///    sampler. This must precede `split_samplers`, which only handles an inline
///    `OpLoad` of a `sampler2D` global and would otherwise reject a function-sampler
///    shader (crt-royale and ~38 corpus presets). Degrades to a no-op skip when
///    `spirv-opt` is absent (see [`spirv_opt::inline_functions`]).
/// 2. **push-constant → UBO** — so the sampler split and final reflection see the
///    normalized form, using the caller-chosen cross-stage `push_binding`.
/// 3. **combined → separate samplers** — the inline form is now all that remains.
fn normalize_stage(
    stage: Stage,
    spirv: Vec<u32>,
    push_binding: u32,
) -> Result<Vec<u32>, CompileError> {
    let spirv = spirv_opt::inline_functions(stage, &spirv)
        .map_err(|source| CompileError::SpirvOpt { stage, source })?;
    let spirv = push_to_ubo::push_constant_to_ubo(&spirv, push_binding)
        .map_err(|source| CompileError::PushToUbo { stage, source })?;
    split_samplers::split_combined_samplers(&spirv)
        .map_err(|source| CompileError::SplitSamplers { stage, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const PASSTHROUGH: &str = "\
#version 450
#pragma parameter BRIGHT \"Brightness\" 1.0 0.0 2.0 0.01
layout(set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Source;
void main() { FragColor = texture(Source, vTexCoord); }
";

    const SPIRV_MAGIC: u32 = 0x0723_0203;

    #[test]
    fn compiles_both_stages_and_reflects_parameter() {
        let shader = compile_slang(PASSTHROUGH, None).expect("compile");
        assert!(!shader.vertex_spirv.is_empty());
        assert!(!shader.fragment_spirv.is_empty());
        assert_eq!(shader.vertex_spirv[0], SPIRV_MAGIC);
        assert_eq!(shader.fragment_spirv[0], SPIRV_MAGIC);
        assert_eq!(shader.reflection.parameters.len(), 1);
        assert_eq!(shader.reflection.parameters[0].name, "BRIGHT");
    }

    #[test]
    fn syntax_error_yields_diagnostic_not_panic() {
        let broken = PASSTHROUGH.replace(
            "FragColor = texture(Source, vTexCoord);",
            "FragColor = nope;",
        );
        let err = compile_slang(&broken, None).unwrap_err();
        match err {
            CompileError::Glslang(GlslangError::Compile { diagnostics, .. }) => {
                assert!(diagnostics.iter().any(|d| d.line.is_some()));
                assert!(diagnostics.iter().any(|d| d.message.contains("nope")));
            }
            other => panic!("expected a glslang compile error, got {other:?}"),
        }
    }

    #[test]
    fn resolves_includes_before_compiling() {
        let dir = tempfile::tempdir().unwrap();
        // The fragment body lives in an included file.
        let inc = dir.path().join("common.inc");
        std::fs::File::create(&inc)
            .unwrap()
            .write_all(b"vec4 shade() { return vec4(0.25); }\n")
            .unwrap();

        let main = "\
#version 450
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = Position; }
#pragma stage fragment
#include \"common.inc\"
layout(location = 0) out vec4 FragColor;
void main() { FragColor = shade(); }
";
        let shader = compile_slang(main, Some(dir.path())).expect("compile with include");
        assert_eq!(shader.fragment_spirv[0], SPIRV_MAGIC);
    }
}
