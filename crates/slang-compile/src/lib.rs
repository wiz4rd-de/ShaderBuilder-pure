//! `slang-compile` — RetroArch-faithful slang preprocessing
//! (`#pragma stage/name/format/parameter`, `#include`), VS/FS split, then
//! glslang → SPIR-V (Architecture §D). The riskiest single link in the toolchain
//! (Architecture §G risk 3).
//!
//! [`compile_slang`] is the pure entry point: source string in, SPIR-V + parsed
//! reflection out.

mod glslang;
pub mod preprocess;

use std::path::Path;

pub use core_model::Parameter;
pub use glslang::{Diagnostic, GlslangError};
pub use preprocess::{PreprocessError, Preprocessed, Reflection, Stage};

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
pub(crate) fn compile_preprocessed(pre: &Preprocessed) -> Result<CompiledShader, CompileError> {
    let vertex_spirv = glslang::compile_stage(Stage::Vertex, &pre.vertex)?;
    let fragment_spirv = glslang::compile_stage(Stage::Fragment, &pre.fragment)?;
    Ok(CompiledShader {
        vertex_spirv,
        fragment_spirv,
        reflection: pre.reflection.clone(),
    })
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
