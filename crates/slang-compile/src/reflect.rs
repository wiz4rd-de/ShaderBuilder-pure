//! Binary SPIR-V reflection (#28) — the reusable layout-discovery infrastructure
//! that the engine's builtin-uniform packing (#28), live parameters (#29), and
//! texture bind tables (#26) all consume.
//!
//! [`Reflection::parameters`] (from `#pragma`) describes the *author's intent*;
//! this module describes the *compiled binary's truth*: where each uniform-block
//! member actually lands in memory and which `(set, binding)` every texture and
//! sampler occupies. Those are the numbers the GPU upload path needs, and the
//! only reliable source for them is the SPIR-V itself — a shader may declare the
//! builtin block members in any order or subset, so a fixed CPU-side layout
//! can't be assumed (see [`crate::uniforms`] in `preview-engine`).
//!
//! We reflect **both** stages of a [`CompiledShader`] and **merge** them: a UBO
//! block typically appears in both the vertex and fragment modules (e.g. `MVP`
//! is read in the VS, the `*Size` family in the FS), and a texture/sampler in
//! one stage only. Merging by `(set, binding)` yields one [`SpirvReflection`]
//! describing the whole pass.
//!
//! Backend: [`naga`] (already in the tree via wgpu). `naga::front::spv` parses a
//! SPIR-V word stream into a `naga::Module`; struct members carry `name` +
//! byte `offset`, and global variables carry an `AddressSpace`
//! (`Uniform`/`Immediate` for blocks, `Handle` for textures/samplers) and a
//! `ResourceBinding { group, binding }`.

use naga::{AddressSpace, ImageClass, ScalarKind, TypeInner};

use crate::CompiledShader;

/// The scalar/vector/matrix shape of a uniform-block member. Enough to drive
/// value packing (#28/#29 write at a member's `offset`; the kind tells a
/// consumer how many components to write and how to interpret them) without
/// re-deriving it from the SPIR-V type graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    /// A single scalar (`float`/`uint`/`int`/`bool`), e.g. `FrameCount`.
    Scalar(ScalarType),
    /// A vector of `len` components (2/3/4), e.g. the `*Size` family is
    /// `Vector { scalar: Float, len: 4 }`.
    Vector { scalar: ScalarType, len: u8 },
    /// A `cols`×`rows` matrix of floats, e.g. `MVP` is `Matrix { cols: 4, rows: 4 }`.
    Matrix { cols: u8, rows: u8 },
    /// A type we don't classify further (array/struct/etc.). Its `size`/`offset`
    /// are still reflected; consumers that only handle the scalar/vector/matrix
    /// builtins should skip it.
    Other,
}

/// The element scalar of a member (mirrors `naga::ScalarKind`, minus the WGSL
/// abstract kinds that never reach a SPIR-V binary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    Float,
    Uint,
    Sint,
    Bool,
}

/// One member of a reflected uniform block: its name, byte offset within the
/// block, byte size, and kind. This is the unit #28/#29 match by `name` and
/// write at `offset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniformMember {
    /// The member's GLSL name (e.g. `MVP`, `SourceSize`, `FrameCount`, or a
    /// `#pragma parameter` identifier). Matched by exact name.
    pub name: String,
    /// Byte offset of this member from the start of its block (the value a
    /// consumer writes the packed bytes at).
    pub offset: u32,
    /// Byte size of this member's type (e.g. 64 for a `mat4`, 16 for a `vec4`,
    /// 4 for a scalar — the std140 *base* size, not including trailing block
    /// padding).
    pub size: u32,
    /// Scalar/vector/matrix classification.
    pub kind: MemberKind,
}

/// Where a uniform block lives in the binding model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockBinding {
    /// A descriptor-bound uniform buffer at `(set, binding)`.
    Uniform { set: u32, binding: u32 },
    /// A push-constant / immediate block (no set/binding; at most one per
    /// module). RetroArch's slang allows the builtin block as either a UBO or a
    /// push-constant block, so #28's packing handles both.
    PushConstant,
}

/// A reflected uniform block (a UBO or push-constant block) and its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniformBlock {
    /// The block's GLSL instance/type name, if the binary preserved one.
    pub name: Option<String>,
    /// Where it binds.
    pub binding: BlockBinding,
    /// Total block size in bytes (std140, padded to a 16-byte multiple).
    pub size: u32,
    /// Members in declaration order (also sorted-stable by offset is fine; the
    /// consumer matches by name, so order is informational).
    pub members: Vec<UniformMember>,
}

impl UniformBlock {
    /// Find a member by exact name. The primitive #28/#29 build on: "does this
    /// block declare `OutputSize`, and at what offset?".
    pub fn member(&self, name: &str) -> Option<&UniformMember> {
        self.members.iter().find(|m| m.name == name)
    }
}

/// A reflected sampled-texture or sampler global. (#26 consumes these to build
/// per-pass texture bind tables; #28 only exposes them.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceBinding {
    /// The global's GLSL name (e.g. `Source`, `Original`, `PassOutput0`, a LUT
    /// alias). #26 maps this name → a runtime resource (doc §7).
    pub name: String,
    /// Descriptor set.
    pub set: u32,
    /// Binding number within the set.
    pub binding: u32,
}

/// The full binary reflection of a compiled pass: every uniform block (with its
/// members' offsets/sizes/kinds) plus every texture and sampler binding, merged
/// across both stages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpirvReflection {
    /// Uniform + push-constant blocks declared by either stage, merged by
    /// `(set, binding)` (a block shared by both stages appears once).
    pub blocks: Vec<UniformBlock>,
    /// Sampled-texture globals. A combined GLSL `sampler2D` is **not** yet split
    /// into a separate texture + sampler (tracked as a separate task); current
    /// fixtures use the separate Vulkan `texture2D` + `sampler` form, so these are
    /// pure `texture2D`s.
    pub textures: Vec<ResourceBinding>,
    /// Sampler globals.
    pub samplers: Vec<ResourceBinding>,
}

impl SpirvReflection {
    /// Find a uniform-block member by name across **all** blocks, returning the
    /// containing block and the member. The exact primitive #28's packing uses:
    /// "which block/offset does the semantic `name` land at?". Returns the first
    /// match (a name should be unique across the pass's blocks).
    pub fn find_member(&self, name: &str) -> Option<(&UniformBlock, &UniformMember)> {
        self.blocks
            .iter()
            .find_map(|b| b.member(name).map(|m| (b, m)))
    }
}

/// Errors from SPIR-V reflection.
#[derive(Debug)]
pub enum ReflectError {
    /// naga could not parse a stage's SPIR-V word stream.
    Parse {
        /// Which stage failed (`"vertex"` / `"fragment"`).
        stage: &'static str,
        /// naga's error, rendered.
        message: String,
    },
}

impl std::fmt::Display for ReflectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReflectError::Parse { stage, message } => {
                write!(
                    f,
                    "SPIR-V reflection failed for the {stage} stage: {message}"
                )
            }
        }
    }
}

impl std::error::Error for ReflectError {}

/// Reflect a compiled pass's SPIR-V into a [`SpirvReflection`]: parse both
/// stages with naga, collect each stage's uniform blocks + texture/sampler
/// globals, and merge them by `(set, binding)`.
///
/// This is pure and side-effect free; the engine calls it once per pass when a
/// chain is built and caches the result on its per-pass resources.
pub fn reflect(shader: &CompiledShader) -> Result<SpirvReflection, ReflectError> {
    let mut acc = SpirvReflection::default();
    reflect_stage(&shader.vertex_spirv, "vertex", &mut acc)?;
    reflect_stage(&shader.fragment_spirv, "fragment", &mut acc)?;
    Ok(acc)
}

/// Reflect one stage's SPIR-V words and merge its globals into `acc`.
fn reflect_stage(
    spirv: &[u32],
    stage: &'static str,
    acc: &mut SpirvReflection,
) -> Result<(), ReflectError> {
    // naga consumes a byte slice; the words are little-endian on every target we
    // ship to (the SPIR-V endianness convention).
    let bytes: Vec<u8> = spirv.iter().flat_map(|w| w.to_le_bytes()).collect();
    let module = naga::front::spv::parse_u8_slice(&bytes, &naga::front::spv::Options::default())
        .map_err(|e| ReflectError::Parse {
            stage,
            message: e.to_string(),
        })?;

    for (_, var) in module.global_variables.iter() {
        let ty = &module.types[var.ty];
        match var.space {
            AddressSpace::Uniform | AddressSpace::Immediate => {
                if let TypeInner::Struct { members, span } = &ty.inner {
                    let binding = match (var.space, var.binding) {
                        (AddressSpace::Immediate, _) => BlockBinding::PushConstant,
                        (_, Some(rb)) => BlockBinding::Uniform {
                            set: rb.group,
                            binding: rb.binding,
                        },
                        // A uniform block with no binding decoration is
                        // malformed for our pipeline; skip rather than guess.
                        _ => continue,
                    };
                    merge_block(acc, &module, binding, var, members, *span);
                }
            }
            AddressSpace::Handle => {
                let Some(rb) = var.binding else { continue };
                let res = ResourceBinding {
                    name: var.name.clone().unwrap_or_default(),
                    set: rb.group,
                    binding: rb.binding,
                };
                match &ty.inner {
                    TypeInner::Image {
                        class: ImageClass::Sampled { .. } | ImageClass::Depth { .. },
                        ..
                    } => push_unique_resource(&mut acc.textures, res),
                    TypeInner::Sampler { .. } => push_unique_resource(&mut acc.samplers, res),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Merge a freshly-reflected block into the accumulator, deduplicating by
/// binding (the same block declared in both stages reflects twice). If a block
/// at this binding already exists, we keep the one with more members (a stage
/// may strip members it doesn't reference — the fuller view is the truthful
/// layout), so the merged block is the union of what both stages saw.
fn merge_block(
    acc: &mut SpirvReflection,
    module: &naga::Module,
    binding: BlockBinding,
    var: &naga::GlobalVariable,
    members: &[naga::StructMember],
    span: u32,
) {
    let block = UniformBlock {
        name: var.name.clone(),
        binding,
        size: span,
        members: members.iter().map(|m| reflect_member(module, m)).collect(),
    };
    if let Some(existing) = acc.blocks.iter_mut().find(|b| b.binding == binding) {
        // Keep whichever stage reflected the larger/fuller block (more members,
        // or — tie — the larger span). naga drops members a stage never reads,
        // so the bigger one is the complete layout.
        let replace = block.members.len() > existing.members.len()
            || (block.members.len() == existing.members.len() && block.size > existing.size);
        if replace {
            *existing = block;
        }
    } else {
        acc.blocks.push(block);
    }
}

/// Reflect one struct member into a [`UniformMember`].
fn reflect_member(module: &naga::Module, m: &naga::StructMember) -> UniformMember {
    let inner = &module.types[m.ty].inner;
    UniformMember {
        name: m.name.clone().unwrap_or_default(),
        offset: m.offset,
        size: type_size(module, inner),
        kind: classify(inner),
    }
}

/// Map a `naga::ScalarKind` to our [`ScalarType`] (abstract kinds never appear
/// in a SPIR-V binary; treat them as float defensively).
fn scalar_type(kind: ScalarKind) -> ScalarType {
    match kind {
        ScalarKind::Float => ScalarType::Float,
        ScalarKind::Uint => ScalarType::Uint,
        ScalarKind::Sint => ScalarType::Sint,
        ScalarKind::Bool => ScalarType::Bool,
        ScalarKind::AbstractInt | ScalarKind::AbstractFloat => ScalarType::Float,
    }
}

/// Classify a member's type into the scalar/vector/matrix taxonomy.
fn classify(inner: &TypeInner) -> MemberKind {
    match inner {
        TypeInner::Scalar(s) => MemberKind::Scalar(scalar_type(s.kind)),
        TypeInner::Vector { size, scalar } => MemberKind::Vector {
            scalar: scalar_type(scalar.kind),
            len: *size as u8,
        },
        TypeInner::Matrix { columns, rows, .. } => MemberKind::Matrix {
            cols: *columns as u8,
            rows: *rows as u8,
        },
        _ => MemberKind::Other,
    }
}

/// Byte size of a member's type (the std140 *base* size: a `mat4` is 64, a
/// `vec4` 16, a scalar `width`). Used as [`UniformMember::size`]; the packing
/// path also derives how many bytes to write from the kind, so this is mainly
/// informational/validation.
fn type_size(module: &naga::Module, inner: &TypeInner) -> u32 {
    match inner {
        TypeInner::Scalar(s) => s.width as u32,
        TypeInner::Vector { size, scalar } => *size as u32 * scalar.width as u32,
        // A matrix's std140 size is column-aligned, NOT `cols*rows*width`: each
        // column is padded up to a vec4, so `mat3` is `3*16 = 48` (not 36) and
        // `mat4` is `4*16 = 64`. Defer to naga's own size accounting (which knows
        // the column stride) rather than a hand-rolled formula (#28).
        other => other.size(module.to_ctx()),
    }
}

/// Push a resource binding only if no binding with the same `(set, binding)`
/// already exists (the same texture/sampler can appear in multiple stages).
fn push_unique_resource(list: &mut Vec<ResourceBinding>, res: ResourceBinding) {
    if !list
        .iter()
        .any(|r| r.set == res.set && r.binding == res.binding)
    {
        list.push(res);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_slang;

    // A shader declaring the canonical builtin UBO at set 0 / binding 0, a
    // texture2D `Source` (binding 1) + sampler `Smp` (binding 2), and a
    // single-member parameter UBO at binding 3 — the exact bind set the engine
    // uses. Members are in canonical std140 order so offsets are predictable.
    const FIXTURE: &str = "\
#version 450
#pragma parameter LEVEL \"Level\" 0.5 0.0 1.0 0.01
layout(std140, set = 0, binding = 0) uniform UBO {
    mat4 MVP;          // offset 0,   size 64
    vec4 SourceSize;   // offset 64,  size 16
    vec4 OriginalSize; // offset 80,  size 16
    vec4 OutputSize;   // offset 96,  size 16
    uint FrameCount;   // offset 112, size 4
} global;
layout(std140, set = 0, binding = 3) uniform Params { float LEVEL; } params;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D Source;
layout(set = 0, binding = 2) uniform sampler Smp;
void main() {
    vec4 c = texture(sampler2D(Source, Smp), vTexCoord) * global.OutputSize.x * params.LEVEL;
    FragColor = vec4(c.rgb, float(global.FrameCount));
}
";

    fn reflect_fixture() -> SpirvReflection {
        let shader = compile_slang(FIXTURE, None).expect("compile fixture");
        reflect(&shader).expect("reflect")
    }

    #[test]
    fn reflects_builtin_member_names_offsets_and_kinds() {
        let r = reflect_fixture();
        let (block, mvp) = r.find_member("MVP").expect("MVP reflected");
        assert!(matches!(
            block.binding,
            BlockBinding::Uniform { set: 0, binding: 0 }
        ));
        assert_eq!(mvp.offset, 0);
        assert_eq!(mvp.size, 64);
        assert_eq!(mvp.kind, MemberKind::Matrix { cols: 4, rows: 4 });

        for (name, offset) in [
            ("SourceSize", 64u32),
            ("OriginalSize", 80),
            ("OutputSize", 96),
        ] {
            let (_, m) = r.find_member(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(m.offset, offset, "{name} offset");
            assert_eq!(m.size, 16, "{name} size");
            assert_eq!(
                m.kind,
                MemberKind::Vector {
                    scalar: ScalarType::Float,
                    len: 4
                },
                "{name} kind"
            );
        }

        let (_, fc) = r.find_member("FrameCount").expect("FrameCount reflected");
        assert_eq!(fc.offset, 112);
        assert_eq!(fc.kind, MemberKind::Scalar(ScalarType::Uint));
    }

    #[test]
    fn reflects_parameter_block_separately() {
        let r = reflect_fixture();
        let (block, level) = r.find_member("LEVEL").expect("LEVEL reflected");
        assert!(matches!(
            block.binding,
            BlockBinding::Uniform { set: 0, binding: 3 }
        ));
        assert_eq!(level.offset, 0);
        assert_eq!(level.kind, MemberKind::Scalar(ScalarType::Float));
    }

    #[test]
    fn reflects_texture_and_sampler_bindings_by_name() {
        let r = reflect_fixture();
        assert_eq!(r.textures.len(), 1, "one texture: {:?}", r.textures);
        let src = &r.textures[0];
        assert_eq!(src.name, "Source");
        assert_eq!((src.set, src.binding), (0, 1));

        assert_eq!(r.samplers.len(), 1, "one sampler: {:?}", r.samplers);
        let smp = &r.samplers[0];
        assert_eq!(smp.name, "Smp");
        assert_eq!((smp.set, smp.binding), (0, 2));
    }

    #[test]
    fn merges_stages_into_one_block_per_binding() {
        // The UBO is read in BOTH stages (MVP in VS, the rest in FS) yet must
        // reflect as a single block carrying every member.
        let r = reflect_fixture();
        let builtin_blocks = r
            .blocks
            .iter()
            .filter(|b| matches!(b.binding, BlockBinding::Uniform { set: 0, binding: 0 }))
            .count();
        assert_eq!(builtin_blocks, 1, "builtin block must merge to one");
        let (block, _) = r.find_member("MVP").unwrap();
        // All five canonical members survive the merge.
        for name in [
            "MVP",
            "SourceSize",
            "OriginalSize",
            "OutputSize",
            "FrameCount",
        ] {
            assert!(block.member(name).is_some(), "{name} present after merge");
        }
    }

    #[test]
    fn mat3_reports_column_aligned_std140_size() {
        // std140: a `mat3` is three column vectors each padded to a vec4, so its
        // size is `3*16 = 48` (NOT `3*3*4 = 36`), and the following member is
        // column-aligned (#28). A hand-rolled `cols*rows*width` would report 36
        // and mis-place everything after it.
        let src = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO {
    mat3 ColorMat;   // offset 0,  std140 size 48
    vec4 Tint;       // offset 48 (column-aligned right after the mat3)
} global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.ColorMat[0].xyzx + Position; }
#pragma stage fragment
layout(location = 0) out vec4 FragColor;
void main() { FragColor = global.Tint; }
";
        let shader = compile_slang(src, None).expect("compile");
        let r = reflect(&shader).expect("reflect");
        let (_, m) = r.find_member("ColorMat").expect("ColorMat reflected");
        assert_eq!(m.kind, MemberKind::Matrix { cols: 3, rows: 3 });
        assert_eq!(m.size, 48, "mat3 std140 size is column-aligned (3*16)");
        let (_, tint) = r.find_member("Tint").expect("Tint reflected");
        assert_eq!(tint.offset, 48, "member after a mat3 is column-aligned");
    }

    #[test]
    fn reflects_members_in_non_canonical_order() {
        // A subset declared OUT of canonical order — proves reflection reads the
        // real binary offsets, not an assumed layout. std140: mat4@0 (64),
        // uint@64 (rounds to 16-stride? no — scalar after mat4 sits at 64), then
        // vec4 must be 16-aligned -> 80.
        let src = "\
#version 450
layout(std140, set = 0, binding = 0) uniform UBO {
    mat4 MVP;
    uint FrameCount;
    vec4 OutputSize;
} global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
void main() { gl_Position = global.MVP * Position; }
#pragma stage fragment
layout(location = 0) out vec4 FragColor;
void main() { FragColor = vec4(global.OutputSize.x, float(global.FrameCount), 0.0, 1.0); }
";
        let shader = compile_slang(src, None).expect("compile");
        let r = reflect(&shader).expect("reflect");
        let (_, mvp) = r.find_member("MVP").unwrap();
        let (_, fc) = r.find_member("FrameCount").unwrap();
        let (_, os) = r.find_member("OutputSize").unwrap();
        assert_eq!(mvp.offset, 0);
        assert_eq!(fc.offset, 64); // scalar packs right after the mat4
        assert_eq!(os.offset, 80); // vec4 is 16-aligned -> rounds 68 up to 80
    }
}
