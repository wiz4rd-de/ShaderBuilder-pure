//! Thin wrapper over the `glslangValidator` CLI to compile one Vulkan-GLSL stage
//! to SPIR-V.
//!
//! We shell out to the same `glslang` compiler RetroArch uses (so SPIR-V output
//! is faithful) rather than linking libglslang/libshaderc — this works
//! identically on a dev box and a CI runner (`apt install glslang-tools`) with
//! no native-linking, ABI, or cross-distro library-path concerns.

use std::io::Write;
use std::process::Command;

use crate::preprocess::Stage;

/// The compiler executable. Overridable via `GLSLANG` for unusual setups.
fn glslang_bin() -> String {
    std::env::var("GLSLANG").unwrap_or_else(|_| "glslangValidator".to_string())
}

/// A single glslang diagnostic (`ERROR: file:line: message`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: Option<u32>,
    pub message: String,
}

/// Failure compiling one stage.
#[derive(Debug)]
pub enum GlslangError {
    /// `glslangValidator` could not be spawned (not installed / not on PATH).
    ToolNotFound { bin: String, source: std::io::Error },
    /// I/O while staging the temp files or reading SPIR-V back.
    Io(std::io::Error),
    /// glslang rejected the source; carries parsed diagnostics + raw output.
    Compile {
        stage: Stage,
        diagnostics: Vec<Diagnostic>,
        raw: String,
    },
    /// glslang produced SPIR-V that is not a whole number of 32-bit words.
    MalformedSpirv,
}

impl std::fmt::Display for GlslangError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GlslangError::ToolNotFound { bin, source } => {
                write!(f, "could not run '{bin}' (install glslang): {source}")
            }
            GlslangError::Io(e) => write!(f, "glslang I/O error: {e}"),
            GlslangError::Compile {
                stage, diagnostics, ..
            } => {
                write!(f, "{stage:?} stage failed to compile:")?;
                for d in diagnostics {
                    match d.line {
                        Some(l) => write!(f, "\n  line {l}: {}", d.message)?,
                        None => write!(f, "\n  {}", d.message)?,
                    }
                }
                Ok(())
            }
            GlslangError::MalformedSpirv => write!(f, "glslang emitted malformed SPIR-V"),
        }
    }
}

impl std::error::Error for GlslangError {}

/// Compile one Vulkan-GLSL stage source to SPIR-V words via `glslangValidator`.
pub fn compile_stage(stage: Stage, source: &str) -> Result<Vec<u32>, GlslangError> {
    let dir = tempfile::tempdir().map_err(GlslangError::Io)?;
    let ext = stage.glslang_name();
    let in_path = dir.path().join(format!("shader.{ext}"));
    let out_path = dir.path().join("shader.spv");

    {
        let mut file = std::fs::File::create(&in_path).map_err(GlslangError::Io)?;
        file.write_all(source.as_bytes())
            .map_err(GlslangError::Io)?;
    }

    let bin = glslang_bin();
    let output = Command::new(&bin)
        .arg("-V") // Vulkan SPIR-V
        .arg("--target-env")
        .arg("vulkan1.0")
        .arg("-S")
        .arg(ext)
        .arg("-o")
        .arg(&out_path)
        .arg(&in_path)
        .output()
        .map_err(|source| GlslangError::ToolNotFound { bin, source })?;

    if !output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        return Err(GlslangError::Compile {
            stage,
            diagnostics: parse_diagnostics(&raw),
            raw,
        });
    }

    let bytes = std::fs::read(&out_path).map_err(GlslangError::Io)?;
    if bytes.len() % 4 != 0 {
        return Err(GlslangError::MalformedSpirv);
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Parse glslang's `ERROR: file:line: message` lines into structured diagnostics.
fn parse_diagnostics(raw: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        let body = match line
            .strip_prefix("ERROR: ")
            .or_else(|| line.strip_prefix("WARNING: "))
        {
            Some(b) => b,
            None => continue,
        };
        // Forms: "file:line: message" or "count compilation errors. ...".
        // Extract a line number when the "<path>:<line>: " shape is present.
        let (line_no, message) = parse_location(body);
        if message.is_empty() {
            continue;
        }
        out.push(Diagnostic {
            line: line_no,
            message: message.to_string(),
        });
    }
    out
}

/// From `path:line: rest` pull out the line number and the trailing message.
fn parse_location(body: &str) -> (Option<u32>, &str) {
    // Find "<...>:<digits>:" near the start.
    if let Some(first_colon) = body.find(':') {
        let after = &body[first_colon + 1..];
        if let Some(second_colon) = after.find(':') {
            let num = after[..second_colon].trim();
            if let Ok(line) = num.parse::<u32>() {
                return (Some(line), after[second_colon + 1..].trim());
            }
        }
    }
    (None, body.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_error_line_numbers() {
        let raw = "ERROR: shader.frag:3: 'nope' : undeclared identifier\n\
                   ERROR: 1 compilation errors.  No code generated.\n";
        let diags = parse_diagnostics(raw);
        assert!(diags
            .iter()
            .any(|d| d.line == Some(3) && d.message.contains("undeclared")));
    }
}
