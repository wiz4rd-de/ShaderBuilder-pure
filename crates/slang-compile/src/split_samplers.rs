//! SPIR-V → SPIR-V transform that splits **combined image-samplers** into
//! **separate** `OpTypeImage` + `OpTypeSampler` variables.
//!
//! ## Why this exists (the wall it tears down)
//!
//! Real RetroArch `.slang` shaders declare their input textures the GLSL way —
//! `layout(set = 0, binding = 1) uniform sampler2D Source;` — and sample with
//! `texture(Source, uv)`. glslang faithfully compiles that to SPIR-V using an
//! `OpTypeSampledImage`: a single *combined* image+sampler descriptor, exactly as
//! Vulkan's `COMBINED_IMAGE_SAMPLER` allows.
//!
//! WebGPU/wgpu has **no** combined image-sampler: its binding model is strictly
//! *separate* `texture2D` + `sampler`. Two independent links in this engine choke
//! on the combined form:
//!
//! 1. [`crate::reflect`] parses the binary with `naga::front::spv`, and naga's
//!    SPIR-V front end cannot represent a combined sampler — a combined-`sampler2D`
//!    passthrough fails to reflect with `invalid id %14` (the id of the
//!    `OpLoad %sampledimage` glslang emits).
//! 2. wgpu ingests the same SPIR-V through naga again to build the pipeline, so it
//!    fails identically.
//!
//! Both consumers are downstream of glslang and both are naga; rather than patch
//! two parsers we normalize the *binary* once, right after glslang emits it, into
//! the separate form the whole engine already speaks. After this transform a
//! shader that wrote `sampler2D Source` looks — to reflection, to the bind-table
//! layout ([`preview_engine::bindtable::pass_layout`]), and to wgpu — exactly like
//! the hand-written separate-sampler fixtures the engine was built around.
//!
//! ## What the transform does
//!
//! For each `OpVariable` in the `UniformConstant` storage class whose pointee type
//! is `OpTypeSampledImage(OpTypeImage)`:
//!
//! * The **original variable** is retyped to a pointer-to-`OpTypeImage` and keeps
//!   its ORIGINAL `(DescriptorSet, Binding)` decorations. A shader's `binding = 1`
//!   texture therefore stays at binding 1 — matching the engine's expectations and
//!   the existing fixtures (`Source` at b1).
//! * A **new sampler variable** (pointer-to-`OpTypeSampler`, `UniformConstant`) is
//!   synthesized in the *same descriptor set*, at a freshly allocated,
//!   collision-free binding (see [`SamplerBindingAllocator`]). It inherits the
//!   original's name with a `Smp` suffix for readability.
//!
//! Then every *use* is rewritten (see [`rewrite_body`]):
//!
//! * `OpLoad %sampledimage %var` becomes `OpLoad %image %var` (the variable is now
//!   image-typed); a paired `OpLoad %sampler %newvar` is inserted. Each original
//!   sampled-image SSA value is thus split into an `(image, sampler)` pair.
//! * Consumers that take a **sampled image** (`OpImageSample*`, `OpImageGather`,
//!   `OpImageDrefGather`, `OpImageSampleProj*`, `OpImageSampleDref*`, `OpImageQueryLod`,
//!   and the `OpImageSparse*` sampling forms) get an `OpSampledImage %sampledimage
//!   %img %smp` reconstructed immediately before them, and their first operand is
//!   repointed at it.
//! * Consumers that take a plain **image** (`OpImage` to extract it, `OpImageFetch`,
//!   `OpImageQuerySizeLod`, `OpImageQuerySize`, `OpImageQueryLevels`,
//!   `OpImageQuerySamples`, `OpImageSparseFetch`) are repointed straight at the
//!   image load. glslang lowers `texelFetch`/`textureSize` as `OpImage %img
//!   %sampledimage_load` followed by the fetch/query on `%img`; after the split the
//!   `OpImage`'s operand IS already an image, so we forward the `OpImage` result to
//!   the image load and elide the redundant extraction.
//!
//! ## Conservatism (it must not corrupt working shaders)
//!
//! * On SPIR-V with **no** combined sampler (every existing separate-sampler
//!   fixture), the transform parses, finds nothing to split, and returns the
//!   original words **verbatim** — a guaranteed no-op so nothing downstream shifts.
//! * Anything the transform cannot prove it handles correctly is a hard
//!   [`SplitError`], never a silent rewrite. In particular: a combined-sampler SSA
//!   value reaching an opcode not in the known image/sampled-image tables, or used
//!   as anything other than the image operand of such an op, aborts the compile.
//!   This is deliberate — corrupt SPIR-V that *parses* is far worse than a clear
//!   "unsupported shader" error a future ticket can act on.
//!
//! ## Out of scope (explicit `SplitError`, see [`SplitError`])
//!
//! Storage images, arrays of (combined) samplers, and combined samplers passed
//! through function calls / stored to `Function`-class pointers are rejected rather
//! than mis-handled. The slang corpus the engine targets uses module-scope
//! `sampler2D` globals sampled inline, which is what this covers.

use std::collections::HashMap;

use rspirv::binary::Assemble;
use rspirv::dr::{self, Instruction, Operand};
use rspirv::spirv::{Decoration, Op, StorageClass};

/// Everything that can go wrong splitting combined samplers. Each variant marks a
/// shape the transform refuses to rewrite blindly; the compile fails loudly rather
/// than emit SPIR-V that merely happens to parse.
#[derive(Debug)]
pub enum SplitError {
    /// The input words were not parseable SPIR-V (truncated, wrong magic, …).
    /// Carries rspirv's loader error rendered to a string.
    Parse(String),
    /// A combined-sampler value flowed into an opcode the transform does not know
    /// how to re-wire (not in the image / sampled-image operand tables). Rejecting
    /// is safer than guessing which operand position is the image.
    UnsupportedConsumer {
        /// The offending opcode (e.g. `ImageSparseDrefGather`).
        opcode: Op,
    },
    /// A combined sampler is declared in a way beyond the supported module-scope
    /// `uniform sampler2D` global (array of samplers, stored to a `Function`
    /// pointer, passed to a function, …). Carries a short human reason.
    UnsupportedDeclaration(String),
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitError::Parse(e) => write!(f, "could not parse SPIR-V to split samplers: {e}"),
            SplitError::UnsupportedConsumer { opcode } => write!(
                f,
                "combined image-sampler used by an unsupported opcode {opcode:?}; \
                 this shader form is not yet handled by the sampler-splitting transform"
            ),
            SplitError::UnsupportedDeclaration(why) => {
                write!(f, "unsupported combined image-sampler declaration: {why}")
            }
        }
    }
}

impl std::error::Error for SplitError {}

/// Split every combined image-sampler in `words` into a separate image + sampler,
/// returning the transformed SPIR-V word stream.
///
/// A **no-op** (returns the input unchanged) when the module declares no combined
/// sampler — which is every existing separate-sampler fixture, so this is safe to
/// run unconditionally on both stages of every compile.
///
/// # Errors
/// [`SplitError::Parse`] if the words are not valid SPIR-V;
/// [`SplitError::UnsupportedConsumer`] / [`SplitError::UnsupportedDeclaration`] if a
/// combined sampler appears in a form the transform will not rewrite blindly.
pub fn split_combined_samplers(words: &[u32]) -> Result<Vec<u32>, SplitError> {
    let mut module = dr::load_words(words).map_err(|e| SplitError::Parse(e.to_string()))?;

    // 1. Index the type section: which ids are sampled-image / image / sampler
    // types, and which sampled-image type wraps which image type.
    let types = TypeIndex::build(&module);

    // 2. Find the combined-sampler variables (UniformConstant globals whose pointee
    // is a sampled image). No combined samplers → return the input verbatim.
    let combined_vars = find_combined_variables(&module, &types)?;
    if combined_vars.is_empty() {
        return Ok(words.to_vec());
    }

    // 2b. Reject combined samplers used by anything other than an inline OpLoad
    // (e.g. passed to a function) BEFORE retyping — otherwise the body rewrite would
    // emit corrupt SPIR-V (a caller/callee type mismatch) instead of a clear error
    // (#32 review finding 1).
    validate_combined_var_uses(&module, &combined_vars)?;

    // 3. Allocate the new ids and the supporting types/variables, retype each
    // combined variable to an image, and record the per-variable (image-type,
    // sampler-var, sampler-type) mapping the body rewrite needs.
    let mut ids = IdAllocator::new(&module);
    let plan = build_split_plan(&mut module, &types, &combined_vars, &mut ids);

    // 4. Rewrite every function body: split combined loads and re-wire consumers.
    rewrite_bodies(&mut module, &plan, &mut ids)?;

    // 5. The new type/pointer/sampler instructions were appended to the end of the
    // global section, but the retyped variables reference them and may appear
    // *earlier* in textual order. SPIR-V requires every id be defined before it is
    // referenced (the global section is ordered, not dominance-checked), so
    // topologically reorder the global section to satisfy that.
    reorder_global_section(&mut module);

    // 6. Update the header bound to cover the new ids and re-assemble. Both the
    // global-section allocator (`ids`) and the body rewrite mint ids; `bound` must
    // exceed the largest of them (SPIR-V `bound` = max id + 1). `rewrite_bodies`
    // already raised `header.bound` to its own high-water mark, so take the max.
    if let Some(header) = module.header.as_mut() {
        header.bound = header.bound.max(ids.next_id);
    }
    Ok(module.assemble())
}

/// Reorder `types_global_values` so every instruction comes after the instructions
/// defining the ids it references — the def-before-use ordering SPIR-V's global
/// section requires.
///
/// glslang's output is already correctly ordered; the only thing that perturbs it
/// is our appended types being referenced by an earlier (retyped) variable. A
/// stable topological sort fixes exactly that while leaving everything else in its
/// original relative order (so the output diff stays minimal and `OpLine`/debug
/// adjacency, where it existed, is preserved as much as possible).
fn reorder_global_section(module: &mut dr::Module) {
    let insts = std::mem::take(&mut module.types_global_values);

    // id → index of the instruction that defines it (within `insts`).
    let mut defined_by: HashMap<u32, usize> = HashMap::new();
    for (i, inst) in insts.iter().enumerate() {
        if let Some(rid) = inst.result_id {
            defined_by.insert(rid, i);
        }
    }

    let n = insts.len();
    let mut emitted = vec![false; n];
    let mut order: Vec<usize> = Vec::with_capacity(n);

    // Iterative post-order DFS (emit dependencies first), visiting instructions in
    // original order so the result is a stable topological sort.
    //
    // `stack` holds `(index, processed?)`: on first pop we push its dependencies,
    // on second pop (deps done) we emit it.
    for start in 0..n {
        if emitted[start] {
            continue;
        }
        let mut stack = vec![(start, false)];
        while let Some((idx, processed)) = stack.pop() {
            if processed {
                if !emitted[idx] {
                    emitted[idx] = true;
                    order.push(idx);
                }
                continue;
            }
            if emitted[idx] {
                continue;
            }
            stack.push((idx, true));
            // Push dependencies (ids this instruction references that are defined in
            // this section) so they're emitted first. result_type counts too.
            let inst = &insts[idx];
            let push_dep = |dep_id: u32, stack: &mut Vec<(usize, bool)>| {
                if let Some(&dep_idx) = defined_by.get(&dep_id) {
                    if dep_idx != idx && !emitted[dep_idx] {
                        stack.push((dep_idx, false));
                    }
                }
            };
            if let Some(rt) = inst.result_type {
                push_dep(rt, &mut stack);
            }
            for op in &inst.operands {
                if let Operand::IdRef(id) = op {
                    push_dep(*id, &mut stack);
                }
            }
        }
    }

    let mut reordered: Vec<Instruction> = Vec::with_capacity(n);
    for idx in order {
        reordered.push(insts[idx].clone());
    }
    module.types_global_values = reordered;
}

/// A classification of the relevant type ids in the module's type section, so the
/// body rewrite can ask "is this load's result type a sampled image?" in O(1).
struct TypeIndex {
    /// `OpTypeSampledImage` id → the `OpTypeImage` id it wraps.
    sampled_image_to_image: HashMap<u32, u32>,
    /// All `OpTypeSampler` ids present (we reuse one rather than always adding).
    sampler_type_ids: Vec<u32>,
    /// Aggregate type id → the component type ids it references (array/runtime-array
    /// element, struct members). Used to detect a sampled image hidden inside an
    /// array/struct so it can be rejected with a clear error (#32 review) rather
    /// than silently skipped (which would leave combined SPIR-V naga can't parse).
    component_types: HashMap<u32, Vec<u32>>,
}

impl TypeIndex {
    fn build(module: &dr::Module) -> Self {
        let mut sampled_image_to_image = HashMap::new();
        let mut sampler_type_ids = Vec::new();
        let mut component_types: HashMap<u32, Vec<u32>> = HashMap::new();
        for inst in &module.types_global_values {
            match inst.class.opcode {
                Op::TypeSampledImage => {
                    if let (Some(id), Some(Operand::IdRef(image))) =
                        (inst.result_id, inst.operands.first())
                    {
                        sampled_image_to_image.insert(id, *image);
                    }
                }
                Op::TypeSampler => {
                    if let Some(id) = inst.result_id {
                        sampler_type_ids.push(id);
                    }
                }
                // Aggregates that can wrap a sampled image: array/runtime-array
                // (element is operand 0), struct (every operand is a member type).
                Op::TypeArray | Op::TypeRuntimeArray | Op::TypeStruct => {
                    if let Some(id) = inst.result_id {
                        let members: Vec<u32> = inst
                            .operands
                            .iter()
                            .filter_map(|o| match o {
                                Operand::IdRef(r) => Some(*r),
                                _ => None,
                            })
                            .collect();
                        component_types.insert(id, members);
                    }
                }
                _ => {}
            }
        }
        Self {
            sampled_image_to_image,
            sampler_type_ids,
            component_types,
        }
    }

    /// Whether `type_id` is, or transitively (through arrays/structs) contains, a
    /// combined `OpTypeSampledImage` — used to reject an array/aggregate of combined
    /// samplers with a clear error (#32 review) instead of a silent no-op. A small
    /// visited set guards against any pathological cycle.
    fn contains_sampled_image(&self, type_id: u32) -> bool {
        let mut stack = vec![type_id];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if self.sampled_image_to_image.contains_key(&id) {
                return true;
            }
            if let Some(components) = self.component_types.get(&id) {
                stack.extend(components.iter().copied());
            }
        }
        false
    }
}

/// A combined-sampler global to split: its variable id and the sampled-image /
/// image type ids involved.
struct CombinedVar {
    var_id: u32,
    /// The `OpTypeSampledImage` id (the pointee). Currently informational, but kept
    /// because the body rewrite re-derives the sampled-image type from each load's
    /// result type independently (more robust against type-section reuse).
    #[allow(dead_code)]
    sampled_image_type_id: u32,
    /// The `OpTypeImage` id the sampled image wraps.
    image_type_id: u32,
}

/// Scan global variables for combined-sampler declarations. Returns one
/// [`CombinedVar`] per `UniformConstant` variable whose pointee is a sampled image.
/// A sampled image declared in any non-pointer-to-sampled-image form (e.g. an array
/// of samplers) is an [`SplitError::UnsupportedDeclaration`].
fn find_combined_variables(
    module: &dr::Module,
    types: &TypeIndex,
) -> Result<Vec<CombinedVar>, SplitError> {
    // Map pointer-type id → (storage class, pointee id) so a variable's pointee can
    // be resolved without re-scanning.
    let mut pointer_pointee: HashMap<u32, (StorageClass, u32)> = HashMap::new();
    for inst in &module.types_global_values {
        if inst.class.opcode == Op::TypePointer {
            if let (Some(id), Some(Operand::StorageClass(sc)), Some(Operand::IdRef(pointee))) =
                (inst.result_id, inst.operands.first(), inst.operands.get(1))
            {
                pointer_pointee.insert(id, (*sc, *pointee));
            }
        }
    }

    let mut out = Vec::new();
    for inst in &module.types_global_values {
        if inst.class.opcode != Op::Variable {
            continue;
        }
        // OpVariable: result_type is the pointer type, first operand the storage class.
        let Some(ptr_type_id) = inst.result_type else {
            continue;
        };
        let Some(var_id) = inst.result_id else {
            continue;
        };
        let Some(Operand::StorageClass(sc)) = inst.operands.first() else {
            continue;
        };
        if *sc != StorageClass::UniformConstant {
            continue;
        }
        let Some((_, pointee)) = pointer_pointee.get(&ptr_type_id) else {
            continue;
        };
        // Only split when the pointee is a sampled-image TYPE directly. A pointee
        // that is itself an array/struct WRAPPING a sampled image (e.g.
        // `uniform sampler2D Tex[2]`) is NOT handled: silently skipping it would
        // leave combined SPIR-V that naga can't parse (an opaque `invalid id`
        // downstream — the exact failure this transform exists to prevent), so
        // reject it with a clear, actionable error instead (#32 review finding 2).
        if let Some(&image_type_id) = types.sampled_image_to_image.get(pointee) {
            let _ = ptr_type_id; // the pointer type is replaced via the variable id
            out.push(CombinedVar {
                var_id,
                sampled_image_type_id: *pointee,
                image_type_id,
            });
        } else if types.contains_sampled_image(*pointee) {
            return Err(SplitError::UnsupportedDeclaration(format!(
                "global variable %{var_id} is an array/aggregate of combined \
                 image-samplers (pointee type %{pointee}); the sampler-splitting \
                 transform only handles a scalar `sampler2D` global"
            )));
        }
    }
    Ok(out)
}

/// Validate that every reference to a combined-sampler variable is the pointer
/// operand of an `OpLoad` (#32 review finding 1). glslang loads a combined sampler
/// inline at each use, so the variable id should appear ONLY as operand 0 of an
/// `OpLoad`. If it instead flows into an `OpFunctionCall` (a `sampler2D` function
/// parameter), an `OpAccessChain`, an `OpCopyObject`, a store, etc., the body
/// rewrite would retype the variable to a pointer-to-image while the consuming
/// instruction's expected type stays pointer-to-sampled-image — producing **corrupt
/// SPIR-V** that only fails far downstream. The transform's contract is to reject
/// such forms with a clear error rather than mis-handle them, so enforce that here
/// before any retyping happens.
fn validate_combined_var_uses(
    module: &dr::Module,
    combined: &[CombinedVar],
) -> Result<(), SplitError> {
    let ids: std::collections::HashSet<u32> = combined.iter().map(|c| c.var_id).collect();
    for func in &module.functions {
        for block in &func.blocks {
            for inst in &block.instructions {
                let is_load = inst.class.opcode == Op::Load;
                for (i, op) in inst.operands.iter().enumerate() {
                    if let Operand::IdRef(id) = op {
                        if ids.contains(id) && !(is_load && i == 0) {
                            return Err(SplitError::UnsupportedDeclaration(format!(
                                "combined image-sampler variable %{id} is used by {:?} \
                                 (operand {i}); only an inline `OpLoad` of a scalar \
                                 `sampler2D` global is supported — it was likely passed \
                                 to a function or aliased through a pointer",
                                inst.class.opcode
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Hands out fresh, never-before-used SPIR-V result ids. Seeded from the module's
/// existing maximum id so new ids never collide; `next_id` becomes the new header
/// `bound` when assembly finishes.
struct IdAllocator {
    next_id: u32,
}

impl IdAllocator {
    fn new(module: &dr::Module) -> Self {
        // The header bound is "max id + 1"; trust it, but also defend against a
        // generator that under-reports by scanning for the true max.
        let mut max_id = module.header.as_ref().map(|h| h.bound).unwrap_or(1);
        for inst in module.all_inst_iter() {
            if let Some(rt) = inst.result_type {
                max_id = max_id.max(rt + 1);
            }
            if let Some(rid) = inst.result_id {
                max_id = max_id.max(rid + 1);
            }
            for op in &inst.operands {
                if let Operand::IdRef(id) = op {
                    max_id = max_id.max(id + 1);
                }
            }
        }
        Self { next_id: max_id }
    }

    fn fresh(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// Allocates the split-out sampler binding for each combined sampler, guaranteeing
/// no collision with any binding already present in the same descriptor set
/// (existing images, UBOs, or other split samplers).
///
/// Scheme (documented for reproducibility): each new sampler is assigned the
/// **lowest free binding strictly above the maximum binding the set already uses**,
/// allocated in declaration order. So a separate-sampler fixture's `Source`@1 +
/// `Smp`@2 + `Params`@3 layout, were `Source` combined, would split its sampler to
/// binding 4 — never colliding with an image, UBO, or another split sampler.
///
/// Why "max + 1 upward" rather than a large fixed offset: wgpu enforces
/// `maxBindingsPerBindGroup` (1000 on the adapters this engine targets), and a
/// `binding = 1001` is rejected at bind-group-layout creation. Packing tightly
/// above the existing bindings keeps every binding small and valid while staying
/// deterministic and collision-free. The original image keeps its low binding, so
/// downstream code and the fixtures that reason about `Source`@1 are unaffected.
struct SamplerBindingAllocator {
    /// Per descriptor set, the set of bindings already in use.
    used: HashMap<u32, std::collections::BTreeSet<u32>>,
}

impl SamplerBindingAllocator {
    /// Build from the module's existing `Binding`/`DescriptorSet` decorations so we
    /// know every occupied `(set, binding)` before allocating.
    fn from_module(module: &dr::Module) -> Self {
        let mut set_of: HashMap<u32, u32> = HashMap::new();
        let mut binding_of: HashMap<u32, u32> = HashMap::new();
        for inst in &module.annotations {
            if inst.class.opcode != Op::Decorate {
                continue;
            }
            let Some(Operand::IdRef(target)) = inst.operands.first() else {
                continue;
            };
            match inst.operands.get(1) {
                Some(Operand::Decoration(Decoration::DescriptorSet)) => {
                    if let Some(Operand::LiteralBit32(set)) = inst.operands.get(2) {
                        set_of.insert(*target, *set);
                    }
                }
                Some(Operand::Decoration(Decoration::Binding)) => {
                    if let Some(Operand::LiteralBit32(b)) = inst.operands.get(2) {
                        binding_of.insert(*target, *b);
                    }
                }
                _ => {}
            }
        }
        let mut used: HashMap<u32, std::collections::BTreeSet<u32>> = HashMap::new();
        for (target, binding) in &binding_of {
            let set = set_of.get(target).copied().unwrap_or(0);
            used.entry(set).or_default().insert(*binding);
        }
        Self { used }
    }

    /// Allocate a free sampler binding in `set`: the lowest binding strictly above
    /// the set's current maximum (so it cannot collide with any existing image,
    /// UBO, or previously split sampler). `orig_binding` is unused by the scheme but
    /// kept in the signature for documentation of the call site's intent.
    fn allocate(&mut self, set: u32, _orig_binding: u32) -> u32 {
        let used = self.used.entry(set).or_default();
        // One past the highest binding currently in the set; if the set is empty,
        // start at 0. Updating `used` keeps subsequent allocations in the same set
        // strictly increasing and collision-free.
        let mut candidate = used.iter().next_back().map_or(0, |&max| max + 1);
        while used.contains(&candidate) {
            candidate = candidate.saturating_add(1);
        }
        used.insert(candidate);
        candidate
    }
}

/// Per-combined-variable rewrite data: the variable's new image type and the new
/// sampler variable + its type. Keyed by the original variable id.
struct VarSplit {
    image_type_id: u32,
    sampler_var_id: u32,
    sampler_type_id: u32,
}

/// The complete plan handed to the body rewrite: how to split each combined
/// variable. The `OpTypeSampledImage` id needed to rebuild combined values at
/// sampling sites is recovered per-load from the load's result type, so it need
/// not be threaded through here.
struct SplitPlan {
    /// Original combined-variable id → its split data.
    by_var: HashMap<u32, VarSplit>,
}

/// Mutate the type/global section: ensure a sampler type and the two new pointer
/// types exist, retype each combined variable to a pointer-to-image, add the new
/// sampler variables, and emit their decorations. Returns the [`SplitPlan`].
fn build_split_plan(
    module: &mut dr::Module,
    types: &TypeIndex,
    combined: &[CombinedVar],
    ids: &mut IdAllocator,
) -> SplitPlan {
    let mut bindings = SamplerBindingAllocator::from_module(module);

    // Reuse an existing OpTypeSampler if the module already has one; otherwise add a
    // single shared one (all 2D combined samplers share the same sampler type).
    let sampler_type_id = types.sampler_type_ids.first().copied().unwrap_or_else(|| {
        let id = ids.fresh();
        module
            .types_global_values
            .push(Instruction::new(Op::TypeSampler, None, Some(id), vec![]));
        id
    });

    // One pointer-to-sampler (UniformConstant) type shared by all sampler vars.
    let sampler_ptr_type_id = ids.fresh();
    module.types_global_values.push(Instruction::new(
        Op::TypePointer,
        None,
        Some(sampler_ptr_type_id),
        vec![
            Operand::StorageClass(StorageClass::UniformConstant),
            Operand::IdRef(sampler_type_id),
        ],
    ));

    // A pointer-to-image (UniformConstant) type per distinct image type, so each
    // retyped combined variable points at an image, not a sampled image.
    let mut image_ptr_type_of: HashMap<u32, u32> = HashMap::new();
    let mut by_var = HashMap::new();
    // Resolve each original variable's set/binding from the existing decorations.
    let (set_of, binding_of) = decoration_maps(module);

    for cv in combined {
        let image_ptr_type_id = *image_ptr_type_of
            .entry(cv.image_type_id)
            .or_insert_with(|| {
                let id = ids.fresh();
                module.types_global_values.push(Instruction::new(
                    Op::TypePointer,
                    None,
                    Some(id),
                    vec![
                        Operand::StorageClass(StorageClass::UniformConstant),
                        Operand::IdRef(cv.image_type_id),
                    ],
                ));
                id
            });

        // Retype the original variable in place: result_type pointer → image ptr.
        retype_variable(module, cv.var_id, image_ptr_type_id);

        // Synthesize the sampler variable + its decorations in the original's set.
        let set = set_of.get(&cv.var_id).copied().unwrap_or(0);
        let orig_binding = binding_of.get(&cv.var_id).copied().unwrap_or(0);
        let sampler_binding = bindings.allocate(set, orig_binding);
        let sampler_var_id = ids.fresh();
        module.types_global_values.push(Instruction::new(
            Op::Variable,
            Some(sampler_ptr_type_id),
            Some(sampler_var_id),
            vec![Operand::StorageClass(StorageClass::UniformConstant)],
        ));
        add_resource_decorations(module, sampler_var_id, set, sampler_binding);
        add_sampler_name(module, sampler_var_id, cv.var_id);

        by_var.insert(
            cv.var_id,
            VarSplit {
                image_type_id: cv.image_type_id,
                sampler_var_id,
                sampler_type_id,
            },
        );
    }

    SplitPlan { by_var }
}

/// Collect `id → set` and `id → binding` from the module's `OpDecorate`s.
fn decoration_maps(module: &dr::Module) -> (HashMap<u32, u32>, HashMap<u32, u32>) {
    let mut set_of = HashMap::new();
    let mut binding_of = HashMap::new();
    for inst in &module.annotations {
        if inst.class.opcode != Op::Decorate {
            continue;
        }
        let Some(Operand::IdRef(target)) = inst.operands.first() else {
            continue;
        };
        match inst.operands.get(1) {
            Some(Operand::Decoration(Decoration::DescriptorSet)) => {
                if let Some(Operand::LiteralBit32(set)) = inst.operands.get(2) {
                    set_of.insert(*target, *set);
                }
            }
            Some(Operand::Decoration(Decoration::Binding)) => {
                if let Some(Operand::LiteralBit32(b)) = inst.operands.get(2) {
                    binding_of.insert(*target, *b);
                }
            }
            _ => {}
        }
    }
    (set_of, binding_of)
}

/// Change a variable's result type (its pointer type) in the global section.
fn retype_variable(module: &mut dr::Module, var_id: u32, new_ptr_type: u32) {
    for inst in &mut module.types_global_values {
        if inst.class.opcode == Op::Variable && inst.result_id == Some(var_id) {
            inst.result_type = Some(new_ptr_type);
            return;
        }
    }
}

/// Emit `OpDecorate <id> DescriptorSet <set>` and `OpDecorate <id> Binding
/// <binding>` for a freshly created resource variable.
fn add_resource_decorations(module: &mut dr::Module, id: u32, set: u32, binding: u32) {
    module.annotations.push(Instruction::new(
        Op::Decorate,
        None,
        None,
        vec![
            Operand::IdRef(id),
            Operand::Decoration(Decoration::DescriptorSet),
            Operand::LiteralBit32(set),
        ],
    ));
    module.annotations.push(Instruction::new(
        Op::Decorate,
        None,
        None,
        vec![
            Operand::IdRef(id),
            Operand::Decoration(Decoration::Binding),
            Operand::LiteralBit32(binding),
        ],
    ));
}

/// Give the new sampler variable a debug name derived from the original's name
/// (e.g. `Source` → `SourceSmp`), purely for capture readability. Skipped if the
/// original is unnamed.
fn add_sampler_name(module: &mut dr::Module, sampler_var_id: u32, orig_var_id: u32) {
    let orig_name = module.debug_names.iter().find_map(|inst| {
        if inst.class.opcode == Op::Name
            && inst.operands.first() == Some(&Operand::IdRef(orig_var_id))
        {
            if let Some(Operand::LiteralString(s)) = inst.operands.get(1) {
                return Some(s.clone());
            }
        }
        None
    });
    if let Some(name) = orig_name {
        module.debug_names.push(Instruction::new(
            Op::Name,
            None,
            None,
            vec![
                Operand::IdRef(sampler_var_id),
                Operand::LiteralString(format!("{name}Smp")),
            ],
        ));
    }
}

/// An opcode's relationship to a combined-sampler SSA value.
enum ConsumerKind {
    /// Takes a *sampled image* as its image operand (operand index 0): we must
    /// reconstruct an `OpSampledImage` and repoint it there.
    SampledImage,
    /// Takes a plain *image* as its image operand (operand index 0): repoint
    /// straight at the image load.
    Image,
    /// `OpImage`: extracts the image from a sampled image. After the split its
    /// operand is already the image; forward its result to the image load.
    ImageExtract,
}

/// Classify an opcode by how it consumes its image operand. `None` means the opcode
/// does not consume a combined-sampler value as an image operand at index 0 — if a
/// combined value nonetheless reaches it, that's an [`SplitError::UnsupportedConsumer`].
fn classify_consumer(op: Op) -> Option<ConsumerKind> {
    match op {
        // Sampling: first operand is a sampled image.
        Op::ImageSampleImplicitLod
        | Op::ImageSampleExplicitLod
        | Op::ImageSampleDrefImplicitLod
        | Op::ImageSampleDrefExplicitLod
        | Op::ImageSampleProjImplicitLod
        | Op::ImageSampleProjExplicitLod
        | Op::ImageSampleProjDrefImplicitLod
        | Op::ImageSampleProjDrefExplicitLod
        | Op::ImageGather
        | Op::ImageDrefGather
        | Op::ImageQueryLod
        | Op::ImageSparseSampleImplicitLod
        | Op::ImageSparseSampleExplicitLod
        | Op::ImageSparseSampleDrefImplicitLod
        | Op::ImageSparseSampleProjImplicitLod
        | Op::ImageSparseSampleProjExplicitLod
        | Op::ImageSparseSampleProjDrefImplicitLod
        | Op::ImageSparseDrefGather
        | Op::ImageSparseGather => Some(ConsumerKind::SampledImage),
        // Image-domain ops: first operand is a plain image.
        Op::ImageFetch
        | Op::ImageQuerySizeLod
        | Op::ImageQuerySize
        | Op::ImageQueryLevels
        | Op::ImageQuerySamples
        | Op::ImageSparseFetch => Some(ConsumerKind::Image),
        // OpImage extracts the image from a sampled image.
        Op::Image => Some(ConsumerKind::ImageExtract),
        _ => None,
    }
}

/// Rewrite every function body for the split (see module docs). Walks each basic
/// block instruction-by-instruction, splitting combined loads and re-wiring every
/// downstream consumer, inserting `OpSampledImage` reconstructions where needed.
///
/// Shares the single [`IdAllocator`] used for the global-section rewrite so all
/// minted ids — sampler loads, `OpSampledImage` reconstructions — are globally
/// unique and the final header `bound` covers them.
fn rewrite_bodies(
    module: &mut dr::Module,
    plan: &SplitPlan,
    ids: &mut IdAllocator,
) -> Result<(), SplitError> {
    for func in &mut module.functions {
        for block in &mut func.blocks {
            let rewritten = rewrite_block(&block.instructions, plan, ids)?;
            block.instructions = rewritten;
        }
    }
    Ok(())
}

/// How a combined-sampler value is represented after splitting a load: the image
/// SSA id and the sampler SSA id it was split into, plus the original sampled-image
/// type (so consumers can rebuild an `OpSampledImage`).
#[derive(Clone, Copy)]
struct SplitValue {
    image_id: u32,
    sampler_id: u32,
    sampled_image_type: u32,
}

/// Rewrite a single basic block's instruction list, returning the new list.
fn rewrite_block(
    insts: &[Instruction],
    plan: &SplitPlan,
    ids: &mut IdAllocator,
) -> Result<Vec<Instruction>, SplitError> {
    // Original sampled-image SSA id → its split (image, sampler) pair.
    let mut split_values: HashMap<u32, SplitValue> = HashMap::new();
    // `OpImage` result id → the image SSA id it forwards to (so its later consumers
    // see the image directly).
    let mut image_forward: HashMap<u32, u32> = HashMap::new();

    let mut out: Vec<Instruction> = Vec::with_capacity(insts.len());

    for inst in insts {
        // (a) A load of a combined sampler variable → image load + sampler load.
        if inst.class.opcode == Op::Load {
            if let Some(Operand::IdRef(ptr)) = inst.operands.first() {
                if let Some(split) = plan.by_var.get(ptr) {
                    let result = inst.result_id.expect("OpLoad has a result id");
                    // Determine the original sampled-image type from the load's
                    // result type so we can rebuild combined values later.
                    let sampled_image_type = inst.result_type.expect("OpLoad has a result type");

                    // The image load reuses the original load's result id, retyped
                    // to the image type — so consumers that we *don't* rewrite by
                    // reconstruction (the image-domain ones) already reference it.
                    let mut image_load = inst.clone();
                    image_load.result_type = Some(split.image_type_id);
                    // Keep result_id = `result` (the original), still loads from the
                    // (now image-typed) original variable.
                    out.push(image_load);

                    // The sampler load gets a fresh id.
                    let sampler_load_id = ids.fresh();
                    out.push(Instruction::new(
                        Op::Load,
                        Some(split.sampler_type_id),
                        Some(sampler_load_id),
                        vec![Operand::IdRef(split.sampler_var_id)],
                    ));

                    split_values.insert(
                        result,
                        SplitValue {
                            image_id: result,
                            sampler_id: sampler_load_id,
                            sampled_image_type,
                        },
                    );
                    continue;
                }
            }
        }

        // (b) Resolve a forwarded OpImage operand back to its image source so the
        // classification below sees the underlying split value.
        let op = inst.class.opcode;

        // (c) Does operand 0 reference a split combined value (directly, or via a
        // forwarded OpImage)?
        let image_operand = inst.operands.first().and_then(|o| {
            if let Operand::IdRef(id) = o {
                Some(*id)
            } else {
                None
            }
        });

        if let Some(operand_id) = image_operand {
            // Map through OpImage forwarding first.
            let resolved = image_forward
                .get(&operand_id)
                .copied()
                .unwrap_or(operand_id);
            if let Some(split) = split_values.get(&resolved).copied() {
                match classify_consumer(op) {
                    Some(ConsumerKind::ImageExtract) => {
                        // OpImage %img %sampledimage: its operand is already the
                        // split image. Drop the OpImage and forward its result id to
                        // the image load, so later image-domain consumers resolve.
                        let result = inst.result_id.expect("OpImage has a result id");
                        image_forward.insert(result, split.image_id);
                        // Also expose the result as a split value? No — OpImage
                        // yields a plain image, used only by image-domain ops, which
                        // we handle via image_forward. Skip emitting the OpImage.
                        continue;
                    }
                    Some(ConsumerKind::Image) => {
                        // Image-domain consumer: repoint operand 0 at the image load.
                        let mut new_inst = inst.clone();
                        new_inst.operands[0] = Operand::IdRef(split.image_id);
                        out.push(new_inst);
                        continue;
                    }
                    Some(ConsumerKind::SampledImage) => {
                        // Reconstruct OpSampledImage %sit %img %smp, then repoint.
                        let combined_id = ids.fresh();
                        out.push(Instruction::new(
                            Op::SampledImage,
                            Some(split.sampled_image_type),
                            Some(combined_id),
                            vec![
                                Operand::IdRef(split.image_id),
                                Operand::IdRef(split.sampler_id),
                            ],
                        ));
                        let mut new_inst = inst.clone();
                        new_inst.operands[0] = Operand::IdRef(combined_id);
                        out.push(new_inst);
                        continue;
                    }
                    None => {
                        // A combined value reached an opcode we don't model. Refuse.
                        return Err(SplitError::UnsupportedConsumer { opcode: op });
                    }
                }
            }
        }

        // (d) Defensive: a split combined value must not appear in any *other*
        // operand position (e.g. stored to a Function pointer, passed to a call).
        // If it does, we can't guarantee correctness — abort rather than corrupt.
        for (i, operand) in inst.operands.iter().enumerate() {
            if i == 0 {
                continue; // operand 0 handled above
            }
            if let Operand::IdRef(id) = operand {
                let resolved = image_forward.get(id).copied().unwrap_or(*id);
                if split_values.contains_key(&resolved) {
                    return Err(SplitError::UnsupportedDeclaration(format!(
                        "a combined image-sampler value flows into operand {i} of {op:?}, \
                         which the transform does not handle (e.g. stored, copied, or \
                         passed to a function)"
                    )));
                }
            }
        }

        out.push(inst.clone());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A combined value is split only when present; no combined sampler → byte-for-
    /// byte identity.
    #[test]
    fn no_combined_sampler_is_identity() {
        // A trivial valid module: header + capability + memory model + void type.
        // Build it through rspirv so the bytes are well-formed.
        let mut b = rspirv::dr::Builder::new();
        b.set_version(1, 0);
        b.capability(rspirv::spirv::Capability::Shader);
        b.memory_model(
            rspirv::spirv::AddressingModel::Logical,
            rspirv::spirv::MemoryModel::GLSL450,
        );
        let module = b.module();
        let words = module.assemble();
        let out = split_combined_samplers(&words).expect("transform");
        assert_eq!(out, words, "no combined sampler must be a verbatim no-op");
    }

    /// Re-parsing the transform's own output must find no combined sampler left and
    /// be a verbatim identity (idempotence): once split, running it again is a
    /// no-op, which also proves the output has the separate form.
    #[test]
    fn split_output_has_no_combined_sampler_and_is_idempotent() {
        // Frag SPIR-V for a combined `sampler2D` passthrough, produced via glslang
        // through the normal compile path so the bytes are exactly what we ship.
        let shader = crate::compile_slang(
            "\
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
",
            None,
        )
        .expect("compile");
        // `compile_slang` already ran the split; re-running on the fragment SPIR-V
        // must be an identity (no combined sampler remains).
        let again = split_combined_samplers(&shader.fragment_spirv).expect("re-split");
        assert_eq!(
            again, shader.fragment_spirv,
            "the split output must contain no combined sampler (idempotent)"
        );

        // And the module must now contain an OpTypeSampler + a separate sampler
        // OpVariable that did not exist in the combined form.
        let module = dr::load_words(&shader.fragment_spirv).expect("parse split output");
        let has_sampler_type = module
            .types_global_values
            .iter()
            .any(|i| i.class.opcode == Op::TypeSampler);
        let has_sampled_image_type = module
            .types_global_values
            .iter()
            .any(|i| i.class.opcode == Op::TypeSampledImage);
        assert!(has_sampler_type, "split output declares an OpTypeSampler");
        // The OpTypeSampledImage type may survive (it's needed to rebuild combined
        // values at the sample site via OpSampledImage), but no *variable* may be of
        // a sampled-image pointer type anymore — checked by the no-combined-variable
        // re-split identity above. We only assert the sampler type was introduced.
        let _ = has_sampled_image_type;
    }

    /// #32 review finding 1: a combined sampler passed to a GLSL function (glslang
    /// passes the variable POINTER as an `OpFunctionCall` arg, never loading it in
    /// the caller) must be REJECTED with a clear error — not silently rewritten into
    /// corrupt SPIR-V (a caller/callee type mismatch that only fails far downstream).
    #[test]
    fn combined_sampler_passed_to_function_is_rejected() {
        let err = crate::compile_slang(
            "\
#version 450
layout(set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Source;
vec4 doit(sampler2D s, vec2 uv) { return texture(s, uv); }
void main() { FragColor = doit(Source, vTexCoord); }
",
            None,
        )
        .expect_err("a combined sampler passed to a function must be rejected");
        assert!(
            matches!(
                err,
                crate::CompileError::SplitSamplers {
                    source: SplitError::UnsupportedDeclaration(_),
                    ..
                }
            ),
            "expected SplitSamplers/UnsupportedDeclaration, got {err:?}"
        );
    }

    /// #32 review finding 2: an array of combined samplers (`sampler2D Tex[2]`) must
    /// be REJECTED with a clear error — not silently left untouched (which would ship
    /// combined SPIR-V naga can't parse, failing with an opaque `invalid id`).
    #[test]
    fn array_of_combined_samplers_is_rejected() {
        let err = crate::compile_slang(
            "\
#version 450
layout(set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Tex[2];
void main() { FragColor = texture(Tex[0], vTexCoord) + texture(Tex[1], vTexCoord); }
",
            None,
        )
        .expect_err("an array of combined samplers must be rejected");
        assert!(
            matches!(
                err,
                crate::CompileError::SplitSamplers {
                    source: SplitError::UnsupportedDeclaration(_),
                    ..
                }
            ),
            "expected SplitSamplers/UnsupportedDeclaration, got {err:?}"
        );
    }

    /// The transform's output must pass `spirv-val` (the canonical validator).
    /// Skipped — not failed — when `spirv-val` is not installed, so CI without the
    /// SPIRV-Tools binary still passes (naga's acceptance is the fallback gate).
    #[test]
    fn split_output_passes_spirv_val() {
        use std::io::Write;
        use std::process::Command;

        // Locate spirv-val; skip gracefully if absent.
        if Command::new("spirv-val").arg("--version").output().is_err() {
            eprintln!("split_output_passes_spirv_val: spirv-val not found, skipping");
            return;
        }

        for src in [
            // combined texture()
            "\
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
",
            // combined texelFetch() + textureSize()
            "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; vec4 SourceSize; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Source;
void main() {
    ivec2 c = ivec2(vTexCoord * global.SourceSize.xy);
    FragColor = texelFetch(Source, c, 0) + vec4(textureSize(Source, 0), 0.0, 0.0);
}
",
        ] {
            let shader = crate::compile_slang(src, None).expect("compile");
            for (label, words) in [
                ("vertex", &shader.vertex_spirv),
                ("fragment", &shader.fragment_spirv),
            ] {
                let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
                let mut tmp = tempfile::NamedTempFile::new().expect("tmp");
                tmp.write_all(&bytes).expect("write spv");
                let out = Command::new("spirv-val")
                    .arg(tmp.path())
                    .output()
                    .expect("run spirv-val");
                assert!(
                    out.status.success(),
                    "spirv-val rejected the {label} stage:\n{}\n{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr),
                );
            }
        }
    }
}
