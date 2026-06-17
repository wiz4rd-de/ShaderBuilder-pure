//! SPIR-V → SPIR-V transform that rewrites a **push-constant block** into an
//! ordinary **uniform buffer (UBO)** at a free `(set = 0, binding)` (#32, PART A).
//!
//! ## Why this exists (the wall it tears down)
//!
//! Real RetroArch `.slang` shaders put their per-pass parameter block (`FrameCount`
//! plus every `#pragma parameter`) in a Vulkan **push-constant** block:
//!
//! ```glsl
//! layout(push_constant) uniform Push { uint FrameCount; float CRTgamma; … } registers;
//! ```
//!
//! glslang compiles that to a SPIR-V `OpVariable` in the `PushConstant` storage
//! class. Over **72 %** of the corpus's `.slang` files (1134 / 1571 at the pinned
//! commit) use this form. wgpu ingests SPIR-V through naga, and naga reports a
//! `PushConstant` global as the `IMMEDIATES` capability — which this engine's
//! device is **not** created with (push constants have spotty/feature-gated wgpu
//! support and a tiny size limit, and the renderer never wired an immediate range).
//! The result before this transform: every push-constant shader fails
//! `create_shader_module` with *"Capability Capabilities(IMMEDIATES) is not
//! supported"*, which the corpus fuzzer caught as the single dominant failure mode.
//!
//! Rather than add push-constant plumbing to the renderer (a non-portable feature
//! with a 128-byte floor that the RetroArch `Push` blocks routinely exceed), we
//! normalize the *binary* — exactly like [`crate::split_samplers`] does for
//! combined samplers — into the UBO form the whole engine already speaks. After
//! this transform a `push_constant` block looks — to [`crate::reflect`], to
//! `preview_engine::bindtable::pass_layout`, and to wgpu — identical to a
//! hand-written `layout(set = 0, binding = N) uniform` block: a normal UBO the
//! existing reflection-driven bind table and the `pack_builtins`/`pack_params`
//! offset-by-name packing handle with **zero renderer changes**.
//!
//! ## What the transform does
//!
//! glslang's push-constant block is already laid out with explicit `Offset` member
//! decorations and a `Block` decoration on the struct — the same decorations a UBO
//! carries. For the scalar / `vec4` / `mat4` members RetroArch `Push`/`UBO` blocks
//! use, the std430 push-constant offsets and the std140 UBO offsets coincide, so
//! the byte layout is preserved verbatim. The transform therefore only changes the
//! *storage class* and adds the descriptor decorations:
//!
//! 1. Rewrite **every** `OpTypePointer` whose storage class is `PushConstant` to
//!    `Uniform` — both the block pointer and the per-member access-chain pointers
//!    glslang emits (`OpTypePointer PushConstant <member-type>`).
//! 2. Rewrite each push-constant `OpVariable`'s storage class to `Uniform`.
//! 3. Add `OpDecorate <var> DescriptorSet 0` and `OpDecorate <var> Binding N`,
//!    where `N` is a binding **the caller chose** to be free across *both* stages
//!    (see below) — so the new UBO never collides with the real `UBO` block
//!    (binding 0) or a `Source`/`Original` sampler.
//!
//! ## Cross-stage binding (why the caller picks the binding)
//!
//! The same `Push` block is declared in both the vertex and fragment stages, but
//! each stage's SPIR-V is rewritten independently — and a stage references only
//! *its own* textures (the vertex stage usually samples nothing, the fragment
//! stage binds `Source`/`Original`/…). If each stage picked its own "lowest free"
//! binding, the vertex stage would choose binding 1 (only the UBO at 0 is taken)
//! while the fragment stage would choose binding 3 (UBO@0, Source@1, Original@2
//! taken). Reflection then sees the *same* block at two different bindings, one of
//! which collides with `Source` — a `Conflicting binding` validation error.
//!
//! To keep the block at ONE binding, [`crate::compile_preprocessed`] scans the
//! set-0 bindings used by **both** stages, picks the lowest binding free in the
//! union with [`free_binding_across`], and passes that single `target_binding` to
//! [`push_constant_to_ubo`] for each stage. Allocation is therefore deterministic
//! and stage-consistent.
//!
//! The function bodies need **no** rewrite: an `OpAccessChain` / `OpLoad` over the
//! variable is storage-class-agnostic in its operands (only the pointer *types*
//! carry the class, and those were retyped in step 1).
//!
//! ## Conservatism (it must not corrupt working shaders)
//!
//! * On SPIR-V with **no** push-constant variable (every existing UBO-only fixture,
//!   and the hand-written separate-sampler fixtures), the transform finds nothing
//!   and returns the input words **verbatim** — a guaranteed no-op.
//! * Only the `PushConstant` storage class is touched; `Uniform`, `UniformConstant`
//!   (textures/samplers), `Input`/`Output`, `Function`, etc. are left exactly as-is.
//! * A SPIR-V module that does not parse is a hard [`PushToUboError::Parse`], never
//!   a silent passthrough.

use std::collections::HashSet;

use rspirv::binary::Assemble;
use rspirv::dr::{self, Instruction, Operand};
use rspirv::spirv::{Decoration, Op, StorageClass};

/// Errors from [`push_constant_to_ubo`].
#[derive(Debug)]
pub enum PushToUboError {
    /// The input words were not parseable SPIR-V (truncated, wrong magic, …).
    Parse(String),
}

impl std::fmt::Display for PushToUboError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushToUboError::Parse(e) => {
                write!(f, "could not parse SPIR-V to rewrite push constants: {e}")
            }
        }
    }
}

impl std::error::Error for PushToUboError {}

/// Rewrite a push-constant block in `words` into a UBO at `(set = 0,
/// target_binding)`, returning the transformed SPIR-V word stream.
///
/// `target_binding` must be a binding the caller has verified is free across
/// **both** stages of the shader (see the module docs and [`free_binding_across`]),
/// so the same block lands at the same binding in the vertex and fragment SPIR-V.
/// If a shader somehow declares more than one push-constant variable, each gets the
/// next free binding after `target_binding` (skipping `used`).
///
/// A **no-op** (returns the input unchanged) when the module declares no
/// push-constant variable — which is every UBO-only fixture — so this is safe to
/// run unconditionally on both stages of every compile.
///
/// # Errors
/// [`PushToUboError::Parse`] if the words are not valid SPIR-V.
pub fn push_constant_to_ubo(
    words: &[u32],
    target_binding: u32,
) -> Result<Vec<u32>, PushToUboError> {
    let mut module = dr::load_words(words).map_err(|e| PushToUboError::Parse(e.to_string()))?;

    // 1. Find every push-constant variable. If there are none, return the input
    // verbatim (the common UBO-only case).
    let push_vars: Vec<u32> = module
        .types_global_values
        .iter()
        .filter(|inst| {
            inst.class.opcode == Op::Variable
                && matches!(
                    inst.operands.first(),
                    Some(Operand::StorageClass(StorageClass::PushConstant))
                )
        })
        .filter_map(|inst| inst.result_id)
        .collect();
    if push_vars.is_empty() {
        return Ok(words.to_vec());
    }

    // 2. The bindings already used in *this* stage's set 0 (for the rare
    // multi-push-block case, to step past them after `target_binding`).
    let used = used_set0_bindings(&module);

    // 3. Rewrite every PushConstant pointer type → Uniform (block + member access
    // chains), and every push-constant variable's storage class → Uniform.
    for inst in &mut module.types_global_values {
        // Both `OpTypePointer` and `OpVariable` carry their storage class as the
        // first operand; retype any PushConstant one to Uniform.
        if matches!(inst.class.opcode, Op::TypePointer | Op::Variable)
            && matches!(
                inst.operands.first(),
                Some(Operand::StorageClass(StorageClass::PushConstant))
            )
        {
            inst.operands[0] = Operand::StorageClass(StorageClass::Uniform);
        }
    }

    // 4. Decorate each former push-constant variable with (set 0, binding),
    // starting at the caller-chosen `target_binding` (cross-stage consistent).
    let mut next_binding = target_binding;
    while used.contains(&next_binding) {
        next_binding += 1;
    }
    let mut new_decorations: Vec<Instruction> = Vec::with_capacity(push_vars.len() * 2);
    for var in push_vars {
        new_decorations.push(decorate(var, Decoration::DescriptorSet, 0));
        new_decorations.push(decorate(var, Decoration::Binding, next_binding));
        next_binding += 1;
        while used.contains(&next_binding) {
            next_binding += 1;
        }
    }
    module.annotations.extend(new_decorations);

    Ok(module.assemble())
}

/// The lowest set-0 binding free across the given stages' SPIR-V — the binding the
/// push-constant block should be rewritten to so it lands at the SAME binding in
/// every stage (see the module docs). `stages` is the per-stage raw SPIR-V word
/// streams (e.g. `[&vertex, &fragment]`); a stream that fails to parse is treated
/// as contributing no used bindings (the rewrite would fail that stage anyway).
pub fn free_binding_across(stages: &[&[u32]]) -> u32 {
    let mut used = HashSet::new();
    for words in stages {
        if let Ok(module) = dr::load_words(words) {
            used.extend(used_set0_bindings(&module));
        }
    }
    lowest_free(&used)
}

/// Build an `OpDecorate <target> <decoration> <literal>` instruction.
fn decorate(target: u32, decoration: Decoration, literal: u32) -> Instruction {
    Instruction::new(
        Op::Decorate,
        None,
        None,
        vec![
            Operand::IdRef(target),
            Operand::Decoration(decoration),
            Operand::LiteralBit32(literal),
        ],
    )
}

/// Collect every `Binding` literal already decorated in descriptor set 0 (the only
/// set the slang toolchain emits). A binding is counted only when its target also
/// carries `DescriptorSet 0` (or no `DescriptorSet`, which defaults to set 0).
fn used_set0_bindings(module: &dr::Module) -> HashSet<u32> {
    use std::collections::HashMap;
    // target id → (set, binding) as decorated.
    let mut set_of: HashMap<u32, u32> = HashMap::new();
    let mut binding_of: HashMap<u32, u32> = HashMap::new();
    for inst in &module.annotations {
        if inst.class.opcode != Op::Decorate {
            continue;
        }
        let Some(Operand::IdRef(target)) = inst.operands.first() else {
            continue;
        };
        match (inst.operands.get(1), inst.operands.get(2)) {
            (
                Some(Operand::Decoration(Decoration::DescriptorSet)),
                Some(Operand::LiteralBit32(s)),
            ) => {
                set_of.insert(*target, *s);
            }
            (Some(Operand::Decoration(Decoration::Binding)), Some(Operand::LiteralBit32(b))) => {
                binding_of.insert(*target, *b);
            }
            _ => {}
        }
    }
    binding_of
        .into_iter()
        .filter(|(target, _)| set_of.get(target).copied().unwrap_or(0) == 0)
        .map(|(_, b)| b)
        .collect()
}

/// The lowest non-negative binding not present in `used`.
fn lowest_free(used: &HashSet<u32>) -> u32 {
    let mut b = 0;
    while used.contains(&b) {
        b += 1;
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{preprocess, Stage};

    /// Compile one stage of a tiny `.slang` source to raw glslang SPIR-V (no
    /// sampler split, no push rewrite) for the transform's unit tests.
    fn raw_spirv(stage: Stage, src: &str) -> Vec<u32> {
        let inlined = preprocess::resolve_includes(src, None).unwrap();
        let pre = preprocess::preprocess(&inlined).unwrap();
        let glsl = match stage {
            Stage::Vertex => &pre.vertex,
            Stage::Fragment => &pre.fragment,
        };
        crate::glslang::compile_stage(stage, glsl).unwrap()
    }

    const PUSH_SHADER: &str = "\
#version 450
layout(push_constant) uniform Push { uint FrameCount; float A; float B; } registers;
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; vec4 OutputSize; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) in vec2 vT;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 2) uniform sampler2D Source;
void main() { FragColor = texture(Source, vec2(registers.A, registers.B)) * float(registers.FrameCount); }
";

    const UBO_ONLY_SHADER: &str = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 2) uniform sampler2D Source;
void main() { FragColor = texture(Source, vec2(0.5)); }
";

    /// Count `OpVariable`s in the given storage class.
    fn count_vars(module: &dr::Module, sc: StorageClass) -> usize {
        module
            .types_global_values
            .iter()
            .filter(|i| {
                i.class.opcode == Op::Variable
                    && matches!(i.operands.first(), Some(Operand::StorageClass(s)) if *s == sc)
            })
            .count()
    }

    /// Count `OpTypePointer`s in the given storage class.
    fn count_ptrs(module: &dr::Module, sc: StorageClass) -> usize {
        module
            .types_global_values
            .iter()
            .filter(|i| {
                i.class.opcode == Op::TypePointer
                    && matches!(i.operands.first(), Some(Operand::StorageClass(s)) if *s == sc)
            })
            .count()
    }

    #[test]
    fn ubo_only_shader_is_unchanged() {
        // A shader with no push-constant block must pass through verbatim.
        let frag = raw_spirv(Stage::Fragment, UBO_ONLY_SHADER);
        let out = push_constant_to_ubo(&frag, 1).unwrap();
        assert_eq!(out, frag, "no push constant → byte-identical passthrough");
    }

    #[test]
    fn free_binding_across_stages_picks_the_union_lowest() {
        // Vertex samples nothing (UBO@0 used → free 1); fragment binds Source@2
        // (UBO@0, Source@2 used → free 1). Across both the lowest free is 1.
        let vert = raw_spirv(Stage::Vertex, PUSH_SHADER);
        let frag = raw_spirv(Stage::Fragment, PUSH_SHADER);
        // The fragment's Source is at binding 2 in PUSH_SHADER; binding 1 is free
        // in both, so the shared target is 1 (NOT 1 for vertex / 3 for fragment).
        assert_eq!(free_binding_across(&[&vert, &frag]), 1);
    }

    #[test]
    fn push_block_becomes_uniform_with_a_free_binding() {
        let frag = raw_spirv(Stage::Fragment, PUSH_SHADER);
        let before = dr::load_words(&frag).unwrap();
        assert_eq!(
            count_vars(&before, StorageClass::PushConstant),
            1,
            "starts with one push-constant variable"
        );
        assert!(
            count_ptrs(&before, StorageClass::PushConstant) >= 1,
            "starts with push-constant pointer types"
        );

        // Use the cross-stage free binding (1, since Source is at 2 here).
        let out = push_constant_to_ubo(&frag, 1).unwrap();
        let after = dr::load_words(&out).unwrap();

        // No PushConstant storage class remains (variable nor pointers).
        assert_eq!(
            count_vars(&after, StorageClass::PushConstant),
            0,
            "push variable rewritten to Uniform"
        );
        assert_eq!(
            count_ptrs(&after, StorageClass::PushConstant),
            0,
            "every push pointer type rewritten to Uniform"
        );

        // The former push variable now has DescriptorSet 0 + a binding != 0/2
        // (0 = the real UBO, 2 = Source). The lowest free is 1.
        let push_var = before
            .types_global_values
            .iter()
            .find(|i| {
                i.class.opcode == Op::Variable
                    && matches!(
                        i.operands.first(),
                        Some(Operand::StorageClass(StorageClass::PushConstant))
                    )
            })
            .and_then(|i| i.result_id)
            .unwrap();
        let mut found_set = None;
        let mut found_binding = None;
        for inst in &after.annotations {
            if inst.class.opcode != Op::Decorate {
                continue;
            }
            if inst.operands.first() != Some(&Operand::IdRef(push_var)) {
                continue;
            }
            match (inst.operands.get(1), inst.operands.get(2)) {
                (
                    Some(Operand::Decoration(Decoration::DescriptorSet)),
                    Some(Operand::LiteralBit32(s)),
                ) => found_set = Some(*s),
                (
                    Some(Operand::Decoration(Decoration::Binding)),
                    Some(Operand::LiteralBit32(b)),
                ) => found_binding = Some(*b),
                _ => {}
            }
        }
        assert_eq!(found_set, Some(0), "new UBO is in set 0");
        assert_eq!(
            found_binding,
            Some(1),
            "binding 1 is the lowest free (0=UBO, 2=Source taken)"
        );
    }

    #[test]
    fn invalid_spirv_is_a_parse_error() {
        let err = push_constant_to_ubo(&[0xDEAD_BEEF, 0, 0, 0], 1).unwrap_err();
        assert!(matches!(err, PushToUboError::Parse(_)));
    }

    #[test]
    fn lowest_free_skips_taken_bindings() {
        let used: HashSet<u32> = [0u32, 1, 2].into_iter().collect();
        assert_eq!(lowest_free(&used), 3);
        assert_eq!(lowest_free(&HashSet::new()), 0);
        let sparse: HashSet<u32> = [0u32, 2].into_iter().collect();
        assert_eq!(lowest_free(&sparse), 1);
    }
}
