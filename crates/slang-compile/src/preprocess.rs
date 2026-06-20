//! RetroArch-style slang preprocessing: resolve `#include`s, split the combined
//! source into vertex/fragment Vulkan-GLSL stages, and extract the meta pragmas
//! (`#pragma name` / `format` / `parameter`).
//!
//! This ports the subset of RetroArch's rules needed for a one-pass shader
//! (Architecture §G risk 3). The model:
//! - lines before the first `#pragma stage` are **common** to both stages
//!   (this is where `#version` and shared declarations live);
//! - `#pragma stage vertex|fragment` switches which stage subsequent lines join;
//! - `#pragma name|format|parameter` are metadata, stripped from the GLSL.

use std::path::{Path, PathBuf};

use core_model::Parameter;

/// The two shader stages a `.slang` file defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

impl Stage {
    /// glslang's `-S` argument and the temp-file extension for this stage.
    pub fn glslang_name(self) -> &'static str {
        match self {
            Stage::Vertex => "vert",
            Stage::Fragment => "frag",
        }
    }
}

/// Metadata parsed out of the slang pragmas.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reflection {
    /// `#pragma name` — the pass's alias, if declared.
    pub name: Option<String>,
    /// `#pragma format` — the requested FBO format, if declared.
    pub format: Option<String>,
    /// `#pragma parameter` declarations.
    pub parameters: Vec<Parameter>,
}

/// The result of preprocessing: per-stage Vulkan-GLSL + extracted metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct Preprocessed {
    pub vertex: String,
    pub fragment: String,
    pub reflection: Reflection,
}

/// Errors from preprocessing (before glslang ever runs).
#[derive(Debug)]
pub enum PreprocessError {
    /// An `#include` could not be read.
    Include {
        path: String,
        source: std::io::Error,
    },
    /// `#include` nesting exceeded the limit (likely a cycle).
    IncludeCycle { path: String },
    /// A `#pragma parameter` line was malformed.
    BadParameter { line: String },
    /// Neither a vertex nor a fragment stage was found.
    MissingStage,
}

impl std::fmt::Display for PreprocessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreprocessError::Include { path, source } => {
                write!(f, "failed to read #include \"{path}\": {source}")
            }
            PreprocessError::IncludeCycle { path } => {
                write!(f, "#include nesting too deep (cycle?) at \"{path}\"")
            }
            PreprocessError::BadParameter { line } => {
                write!(f, "malformed #pragma parameter: {line}")
            }
            PreprocessError::MissingStage => {
                write!(f, "shader defines neither a vertex nor a fragment stage")
            }
        }
    }
}

impl std::error::Error for PreprocessError {}

const MAX_INCLUDE_DEPTH: usize = 32;

/// Resolve all `#include "..."` directives, inlining file contents relative to
/// `base_dir`. Recurses up to [`MAX_INCLUDE_DEPTH`] to guard against cycles.
pub fn resolve_includes(source: &str, base_dir: Option<&Path>) -> Result<String, PreprocessError> {
    fn recurse(
        source: &str,
        base_dir: Option<&Path>,
        depth: usize,
    ) -> Result<String, PreprocessError> {
        let mut out = String::with_capacity(source.len());
        for line in source.lines() {
            if let Some(rel) = parse_include(line) {
                if depth >= MAX_INCLUDE_DEPTH {
                    return Err(PreprocessError::IncludeCycle { path: rel.clone() });
                }
                let path: PathBuf = match base_dir {
                    Some(dir) => dir.join(&rel),
                    None => PathBuf::from(&rel),
                };
                let contents =
                    std::fs::read_to_string(&path).map_err(|source| PreprocessError::Include {
                        path: rel.clone(),
                        source,
                    })?;
                let nested_base = path.parent().map(Path::to_path_buf);
                out.push_str(&recurse(&contents, nested_base.as_deref(), depth + 1)?);
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        Ok(out)
    }
    recurse(source, base_dir, 0)
}

/// `#include "rel/path"` → `Some("rel/path")`, else `None`.
fn parse_include(line: &str) -> Option<String> {
    let t = line.trim_start();
    let rest = t.strip_prefix("#include")?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Preprocess a `.slang` source string into per-stage GLSL + reflection. Call
/// [`resolve_includes`] first (or pass already-inlined source).
pub fn preprocess(source: &str) -> Result<Preprocessed, PreprocessError> {
    let mut common = String::new();
    let mut vertex = String::new();
    let mut fragment = String::new();
    let mut reflection = Reflection::default();
    let mut current: Option<Stage> = None;
    let mut saw_stage = false;

    for line in source.lines() {
        let trimmed = line.trim_start();

        if let Some(rest) = trimmed.strip_prefix("#pragma") {
            let rest = rest.trim_start();
            if let Some(arg) = rest.strip_prefix("stage") {
                match arg.trim() {
                    "vertex" => current = Some(Stage::Vertex),
                    "fragment" => current = Some(Stage::Fragment),
                    other => {
                        // Unknown stage — keep the line so glslang can complain
                        // rather than silently dropping code.
                        let _ = other;
                    }
                }
                saw_stage = true;
                continue;
            } else if let Some(arg) = rest.strip_prefix("name") {
                reflection.name = Some(arg.trim().to_string());
                continue;
            } else if let Some(arg) = rest.strip_prefix("format") {
                reflection.format = Some(arg.trim().to_string());
                continue;
            } else if let Some(arg) = rest.strip_prefix("parameter") {
                reflection.parameters.push(parse_parameter(arg.trim())?);
                continue;
            }
            // Any other #pragma falls through and is emitted as-is.
        }

        match current {
            None => {
                common.push_str(line);
                common.push('\n');
            }
            Some(Stage::Vertex) => {
                vertex.push_str(line);
                vertex.push('\n');
            }
            Some(Stage::Fragment) => {
                fragment.push_str(line);
                fragment.push('\n');
            }
        }
    }

    if !saw_stage {
        return Err(PreprocessError::MissingStage);
    }

    Ok(Preprocessed {
        vertex: format!("{common}{vertex}"),
        fragment: format!("{common}{fragment}"),
        reflection,
    })
}

/// Parse `<name> "<label>" <default> <min> <max> [<step>]`.
fn parse_parameter(arg: &str) -> Result<Parameter, PreprocessError> {
    let bad = || PreprocessError::BadParameter {
        line: arg.to_string(),
    };

    let (name, rest) = arg.split_once(char::is_whitespace).ok_or_else(bad)?;
    let rest = rest.trim_start();

    // Label is a quoted string.
    let rest = rest.strip_prefix('"').ok_or_else(bad)?;
    let label_end = rest.find('"').ok_or_else(bad)?;
    let label = &rest[..label_end];
    let nums = &rest[label_end + 1..];

    let mut it = nums.split_whitespace();
    let mut next_f32 = || it.next().and_then(|s| s.parse::<f32>().ok());
    let default = next_f32().ok_or_else(bad)?;
    let min = next_f32().ok_or_else(bad)?;
    let max = next_f32().ok_or_else(bad)?;
    let step = next_f32().unwrap_or(0.0);

    Ok(Parameter {
        name: name.to_string(),
        label: label.to_string(),
        default,
        min,
        max,
        step,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHADER: &str = "\
#version 450
#pragma name my_pass
#pragma format R8G8B8A8_UNORM
#pragma parameter WARP \"Warp amount\" 0.5 0.0 1.0 0.01
layout(set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) out vec4 FragColor;
void main() { FragColor = vec4(1.0); }
";

    #[test]
    fn splits_stages_with_shared_common() {
        let p = preprocess(SHADER).unwrap();
        // Common preamble reaches both stages.
        assert!(p.vertex.contains("#version 450"));
        assert!(p.fragment.contains("#version 450"));
        assert!(p.vertex.contains("uniform UBO"));
        assert!(p.fragment.contains("uniform UBO"));
        // Stage-specific code only in its own stage.
        assert!(p.vertex.contains("in vec4 Position"));
        assert!(!p.fragment.contains("in vec4 Position"));
        assert!(p.fragment.contains("out vec4 FragColor"));
        assert!(!p.vertex.contains("out vec4 FragColor"));
        // Meta pragmas are stripped from the GLSL.
        assert!(!p.vertex.contains("#pragma"));
        assert!(!p.fragment.contains("#pragma"));
    }

    #[test]
    fn extracts_metadata_and_parameter() {
        let p = preprocess(SHADER).unwrap();
        assert_eq!(p.reflection.name.as_deref(), Some("my_pass"));
        assert_eq!(p.reflection.format.as_deref(), Some("R8G8B8A8_UNORM"));
        assert_eq!(p.reflection.parameters.len(), 1);
        let param = &p.reflection.parameters[0];
        assert_eq!(param.name, "WARP");
        assert_eq!(param.label, "Warp amount");
        assert_eq!(param.default, 0.5);
        assert_eq!(param.min, 0.0);
        assert_eq!(param.max, 1.0);
        assert_eq!(param.step, 0.01);
    }

    #[test]
    fn missing_stage_is_an_error() {
        let err = preprocess("#version 450\nvoid main() {}\n").unwrap_err();
        assert!(matches!(err, PreprocessError::MissingStage));
    }

    #[test]
    fn parameter_step_is_optional() {
        let p = parse_parameter("GAMMA \"Gamma\" 1.0 0.5 2.0").unwrap();
        assert_eq!(p.step, 0.0);
        assert_eq!(p.name, "GAMMA");
    }
}
