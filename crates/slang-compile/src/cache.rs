//! A content-hash cache in front of [`crate::compile_slang`] so identical shader
//! input returns cached SPIR-V instead of re-running preprocessing + glslang
//! (Architecture §D: recompile only changed passes). The render thread holds one
//! of these; the pure [`crate::compile_slang`] stays cache-free for testability.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use crate::{compile_preprocessed, preprocess, CompileError, CompiledShader};

/// In-memory shader cache keyed on the post-`#include` source.
#[derive(Default)]
pub struct CompileCache {
    entries: HashMap<u64, Arc<CompiledShader>>,
    /// Number of real glslang compilations performed (cache misses).
    compiles: u64,
}

impl CompileCache {
    /// A fresh, empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a cached compile, or compile and cache it. The key is a hash of
    /// the **post-`#include`** source, so editing an included file (which changes
    /// the inlined text) invalidates the entry.
    pub fn get_or_compile(
        &mut self,
        source: &str,
        base_dir: Option<&Path>,
    ) -> Result<Arc<CompiledShader>, CompileError> {
        let inlined = preprocess::resolve_includes(source, base_dir)?;
        let key = hash_str(&inlined);
        if let Some(hit) = self.entries.get(&key) {
            return Ok(hit.clone());
        }
        let pre = preprocess::preprocess(&inlined)?;
        let shader = Arc::new(compile_preprocessed(&pre)?);
        self.compiles += 1;
        self.entries.insert(key, shader.clone());
        Ok(shader)
    }

    /// How many real compilations have run (cache misses). For tests/metrics.
    pub fn compile_count(&self) -> u64 {
        self.compiles
    }

    /// Number of cached shaders.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn hash_str(s: &str) -> u64 {
    // DefaultHasher uses fixed keys, so this is deterministic within a run —
    // fine for an in-process (non-persisted) cache.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHADER: &str = "\
#version 450
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = Position; }
#pragma stage fragment
layout(location = 0) out vec4 FragColor;
void main() { FragColor = vec4(0.5); }
";

    #[test]
    fn hit_on_identical_input_miss_on_change() {
        let mut cache = CompileCache::new();

        let a = cache.get_or_compile(SHADER, None).unwrap();
        let b = cache.get_or_compile(SHADER, None).unwrap();
        // Second call was a cache hit: glslang ran exactly once, same Arc.
        assert_eq!(cache.compile_count(), 1);
        assert!(Arc::ptr_eq(&a, &b));

        // A one-character change forces a recompile.
        let changed = SHADER.replace("vec4(0.5)", "vec4(0.6)");
        cache.get_or_compile(&changed, None).unwrap();
        assert_eq!(cache.compile_count(), 2);
        assert_eq!(cache.len(), 2);
    }
}
