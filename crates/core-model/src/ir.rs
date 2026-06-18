//! The per-pass **typed IR op model** and **port type system** (Spec §8.2,
//! Architecture §C) — the frozen Phase-4 contract the type checker (#40),
//! lowering (#41), the slang emitter (#42/#43), and the codegen fixtures (#44)
//! all build on.
//!
//! ## Relationship to the skeletal [`Graph`](crate::Graph)
//!
//! `core-model` carries **two** graph representations, on purpose:
//!
//! - The skeletal [`Graph`](crate::Graph) / [`Node`](crate::Node) /
//!   [`Edge`](crate::Edge) is the **editor-canvas** model: free-form
//!   `Node { kind: String, data: Record<string, unknown> }`. It is what the
//!   React-Flow editor authors in Phase 5 and what [`PassSource::Graph`] still
//!   references. It deliberately carries no port types — it is presentation +
//!   loose authoring data.
//! - The [`IrGraph`] in this module is the **typed dataflow DAG**: every node
//!   is a concrete [`NodeOp`] with typed ports, edges connect
//!   `(node, port) → (node, port)`, and the whole thing type-checks and lowers
//!   to SSA. This is the codegen-facing model.
//!
//! The flow (Phase 5 onward) is: editor [`Graph`](crate::Graph) → *lower to*
//! [`IrGraph`] → type-check (#40) → SSA-lower (#41) → emit slang (#42). In
//! Phase 4 the [`IrGraph`] is **hand-built in Rust** for tests; the editor →
//! IR bridge is Phase 5. The two models are kept separate so the editor can
//! evolve its node taxonomy without touching the frozen codegen contract, and
//! so the existing skeletal-`Graph` round-trip tests stay valid.
//!
//! ## Whole-pass code
//!
//! A pass is **either** a typed [`IrGraph`] **or** verbatim whole-pass slang.
//! The verbatim path reuses the existing [`PassSource::WholePassCode`] variant
//! (an opaque, non-decomposable source string) — there is intentionally no IR
//! representation of whole-pass bodies; #43 handles that path by scanning the
//! source for parameters/textures rather than lowering it. This module is only
//! the typed-graph half.
//!
//! [`PassSource::Graph`]: crate::PassSource::Graph
//! [`PassSource::WholePassCode`]: crate::PassSource::WholePassCode

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The type of a value flowing along a port edge in an [`IrGraph`] (Spec §8.2).
///
/// These are the GLSL-ish scalar/vector/opaque types the node taxonomy traffics
/// in. The type checker (#40) uses the pure predicates below
/// ([`component_count`](PortType::component_count),
/// [`broadcast_to`](PortType::broadcast_to),
/// [`implicit_widen_to`](PortType::implicit_widen_to),
/// [`swizzle_result`](PortType::swizzle_result)) to validate edges and operator
/// operands; the emitter (#42) maps each to its slang spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PortType {
    /// A single `float`.
    Float,
    /// A `vec2` (two floats).
    Vec2,
    /// A `vec3` (three floats).
    Vec3,
    /// A `vec4` (four floats).
    Vec4,
    /// A single `int`.
    Int,
    /// A single `bool`.
    Bool,
    /// An opaque `sampler2D` handle — only produced by a texture-source port and
    /// only consumable by a [`NodeOp::Sample`]. Never participates in arithmetic.
    Sampler2D,
}

impl PortType {
    /// The number of float/scalar components, or `None` for the opaque
    /// [`PortType::Sampler2D`].
    ///
    /// `Float`/`Int`/`Bool` are 1 component; `Vec2`/`Vec3`/`Vec4` are 2/3/4.
    /// Used by the type checker to reason about broadcasting and widening.
    pub const fn component_count(self) -> Option<u8> {
        match self {
            PortType::Float | PortType::Int | PortType::Bool => Some(1),
            PortType::Vec2 => Some(2),
            PortType::Vec3 => Some(3),
            PortType::Vec4 => Some(4),
            PortType::Sampler2D => None,
        }
    }

    /// Whether this is a scalar type (`Float`/`Int`/`Bool`).
    pub const fn is_scalar(self) -> bool {
        matches!(self, PortType::Float | PortType::Int | PortType::Bool)
    }

    /// Whether this is a float vector type (`Vec2`/`Vec3`/`Vec4`).
    pub const fn is_vector(self) -> bool {
        matches!(self, PortType::Vec2 | PortType::Vec3 | PortType::Vec4)
    }

    /// Whether this is a numeric type that can take part in arithmetic — any
    /// scalar or vector (i.e. everything except [`PortType::Sampler2D`]).
    pub const fn is_numeric(self) -> bool {
        self.is_scalar() || self.is_vector()
    }

    /// Whether this is a **float-family** type — a `Float` or a `vecN`
    /// (`Vec2`/`Vec3`/`Vec4`). This is the operand class the component-wise math
    /// [`ExprOp`](ExprOp)s (`add`/`sub`/`mul`/… plus `dot`/`normalize`/`length`/
    /// the unary intrinsics and `construct`) require: an `Int` or `Bool` is
    /// [`is_numeric`](PortType::is_numeric) but does **not** participate in this
    /// arithmetic, because the lowering + emitter only ever produce float-typed
    /// result temps and GLSL forbids e.g. `vec4 * int` / `bool + bool`. Keeping
    /// the checker's operand gate aligned with what codegen can emit upholds the
    /// "clean-checks ⇒ compiles" invariant.
    pub const fn is_float_family(self) -> bool {
        matches!(
            self,
            PortType::Float | PortType::Vec2 | PortType::Vec3 | PortType::Vec4
        )
    }

    /// The float-vector type with the given component count (`1 → Float`,
    /// `2 → Vec2`, `3 → Vec3`, `4 → Vec4`), or `None` for any other count.
    ///
    /// The inverse of [`component_count`](PortType::component_count) for the
    /// float family; used to compute swizzle and operator result types.
    pub const fn float_with_components(n: u8) -> Option<PortType> {
        match n {
            1 => Some(PortType::Float),
            2 => Some(PortType::Vec2),
            3 => Some(PortType::Vec3),
            4 => Some(PortType::Vec4),
            _ => None,
        }
    }

    /// Whether a value of `self` may be **scalar-broadcast** to `target`
    /// (Spec §8.2): a `Float` broadcasts to any float vector (`float → vecN`),
    /// matching GLSL `vecN(x)` construction. A type always "broadcasts" to
    /// itself. No other broadcast is legal (an `Int`/`Bool` does not silently
    /// broadcast to a float vector).
    pub const fn broadcast_to(self, target: PortType) -> bool {
        if port_type_eq(self, target) {
            return true;
        }
        matches!(self, PortType::Float) && target.is_vector()
    }

    /// Whether a value of `self` **implicitly widens** to `target` (Spec §8.2).
    ///
    /// The only implicit widening allowed is the scalar promotion `Int → Float`
    /// (GLSL-style). A narrower vector does **not** implicitly widen to a wider
    /// one (`vec2 → vec3` is illegal); that requires an explicit constructor
    /// node. A type trivially widens to itself. `Float → vecN` is *broadcast*,
    /// not widening — see [`broadcast_to`](PortType::broadcast_to).
    pub const fn implicit_widen_to(self, target: PortType) -> bool {
        if port_type_eq(self, target) {
            return true;
        }
        matches!((self, target), (PortType::Int, PortType::Float))
    }

    /// Whether `self` is **assignable** to `target` accepting both the implicit
    /// widening (`Int → Float`) and scalar broadcast (`Float → vecN`) rules.
    ///
    /// This is the combined edge-compatibility predicate the type checker (#40)
    /// applies to a `source_port: self` → `target_port: target` connection.
    pub const fn assignable_to(self, target: PortType) -> bool {
        self.implicit_widen_to(target) || self.broadcast_to(target)
    }

    /// Compute the [`PortType`] that results from applying a **swizzle** `mask`
    /// to a value of `self`, or `None` if the swizzle is illegal (Spec §8.2).
    ///
    /// A swizzle selects components by name; the result is the float type whose
    /// component count equals the mask length (length 1 → [`PortType::Float`],
    /// 2 → `Vec2`, 3 → `Vec3`, 4 → `Vec4`). Rules:
    ///
    /// - The base must be a float scalar or vector ([`is_numeric`] and not
    ///   `Int`/`Bool` — an `Int`/`Bool` cannot be swizzled here).
    /// - Mask length must be 1..=4.
    /// - Every mask char must come from a single accessor set — `xyzw`,
    ///   `rgba`, or `stpq` — and the three sets may not be mixed in one mask
    ///   (GLSL rule).
    /// - Every selected component index must be `< self.component_count()`
    ///   (you cannot read `.z` of a `Vec2`). A `Float` exposes only `.x`/`.r`/`.s`.
    ///
    /// Returns the result [`PortType`] on success, else `None`.
    ///
    /// [`is_numeric`]: PortType::is_numeric
    pub fn swizzle_result(self, mask: &str) -> Option<PortType> {
        // Sampler / int / bool cannot be swizzled.
        if !matches!(
            self,
            PortType::Float | PortType::Vec2 | PortType::Vec3 | PortType::Vec4
        ) {
            return None;
        }
        let base_components = self.component_count()?;
        let len = mask.chars().count();
        if len == 0 || len > 4 {
            return None;
        }

        // Each accessor set, in canonical component order.
        const SETS: [&str; 3] = ["xyzw", "rgba", "stpq"];
        // Which set the first char belongs to; all chars must share it.
        let mut chosen_set: Option<usize> = None;
        for ch in mask.chars() {
            let mut found = None;
            for (set_idx, set) in SETS.iter().enumerate() {
                if let Some(comp_idx) = set.find(ch) {
                    found = Some((set_idx, comp_idx));
                    break;
                }
            }
            let (set_idx, comp_idx) = found?;
            match chosen_set {
                None => chosen_set = Some(set_idx),
                Some(s) if s == set_idx => {}
                Some(_) => return None, // mixed accessor sets
            }
            if (comp_idx as u8) >= base_components {
                return None; // selecting a component the base doesn't have
            }
        }

        PortType::float_with_components(len as u8)
    }
}

/// `const fn`-friendly equality for [`PortType`] (a `Copy` C-like enum). The
/// predicate bodies above are `const fn`, where `PartialEq::eq` is not callable,
/// so they compare via the enum's `u8` discriminant.
const fn port_type_eq(a: PortType, b: PortType) -> bool {
    a as u8 == b as u8
}

/// A concrete RetroArch texture a [`NodeOp::Sample`] reads from (Spec §7 binding
/// table). This is the **typed, resolved** reference (it carries its index/name),
/// distinct from the import-scan's coarse [`TextureRefKind`](crate::TextureRefKind)
/// classification: the emitter (#42) maps each variant to a concrete RetroArch
/// sampler identifier (`Source`, `Original`, `OriginalHistory2`, `PassOutput0`,
/// `PassFeedback1`, or a LUT name) and assigns its `layout(set=0, binding=N)`.
///
/// History/PassOutput/PassFeedback carry a `u32` index; a LUT carries its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum TextureSource {
    /// `Source` — the previous pass's output (`Original` for pass 0).
    Source,
    /// `Original` — the whole-chain input frame (≡ `OriginalHistory0`).
    Original,
    /// `OriginalHistoryN` — `Original` from `index` frames ago (§5). Index `0`
    /// is `Original` itself.
    OriginalHistory {
        /// How many frames back (`OriginalHistory{index}`).
        index: u32,
    },
    /// `PassOutputN` — pass `index`'s output **this frame** (causal, §7).
    PassOutput {
        /// The producing pass index (`PassOutput{index}`).
        index: u32,
    },
    /// `PassFeedbackN` — pass `index`'s output from the **previous** frame (§4).
    PassFeedback {
        /// The producing pass index (`PassFeedback{index}`).
        index: u32,
    },
    /// A LUT texture bound by its `textures` name (`<NAME>`, §7).
    Lut {
        /// The LUT name as declared in the project's `luts`.
        name: String,
    },
}

/// A reserved RetroArch builtin-semantic uniform a [`NodeOp::Builtin`] reads
/// (Spec §8.1; the worked-example header). These names are **reserved** — the
/// emitter spells each exactly as the RetroArch slang identifier (e.g.
/// `SourceSize`, `FrameCount`, `MVP`), reading them from the push-constant /
/// global UBO. Each variant's documented [`PortType`] is its value type, which
/// the type checker uses for the builtin node's output port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BuiltinSemantic {
    /// `SourceSize` — `vec4(w, h, 1/w, 1/h)` of this pass's input. [`PortType::Vec4`].
    SourceSize,
    /// `OriginalSize` — `vec4` of the whole-chain input frame. [`PortType::Vec4`].
    OriginalSize,
    /// `OutputSize` — `vec4` of this pass's render target. [`PortType::Vec4`].
    OutputSize,
    /// `FinalViewportSize` — `vec4` of the simulated final viewport. [`PortType::Vec4`].
    FinalViewportSize,
    /// `FrameCount` — the running frame counter (`uint`). [`PortType::Int`].
    FrameCount,
    /// `FrameDirection` — `+1` playing forward, `-1` rewinding (`int`). [`PortType::Int`].
    FrameDirection,
    /// `MVP` — the model-view-projection matrix used by the standard vertex
    /// stage. Has no scalar/vector [`PortType`] (it is a `mat4`); a graph node
    /// never consumes it directly — the vertex stage is the fixed `MVP*Position`
    /// passthrough — but the semantic is enumerated for completeness/round-trip.
    Mvp,
}

impl BuiltinSemantic {
    /// The [`PortType`] of this builtin's value, or `None` for [`BuiltinSemantic::Mvp`]
    /// (a `mat4`, which has no scalar/vector port type). The type checker (#40)
    /// uses this to type a [`NodeOp::Builtin`] node's output port.
    pub const fn port_type(self) -> Option<PortType> {
        match self {
            BuiltinSemantic::SourceSize
            | BuiltinSemantic::OriginalSize
            | BuiltinSemantic::OutputSize
            | BuiltinSemantic::FinalViewportSize => Some(PortType::Vec4),
            BuiltinSemantic::FrameCount | BuiltinSemantic::FrameDirection => Some(PortType::Int),
            BuiltinSemantic::Mvp => None,
        }
    }

    /// The exact RetroArch slang identifier this semantic emits as (e.g.
    /// `"SourceSize"`, `"FrameCount"`, `"MVP"`). Reserved names — the emitter
    /// (#42) writes these verbatim into the generated shader.
    pub const fn slang_name(self) -> &'static str {
        match self {
            BuiltinSemantic::SourceSize => "SourceSize",
            BuiltinSemantic::OriginalSize => "OriginalSize",
            BuiltinSemantic::OutputSize => "OutputSize",
            BuiltinSemantic::FinalViewportSize => "FinalViewportSize",
            BuiltinSemantic::FrameCount => "FrameCount",
            BuiltinSemantic::FrameDirection => "FrameDirection",
            BuiltinSemantic::Mvp => "MVP",
        }
    }
}

/// A typed literal value produced by a [`NodeOp::Const`] (Spec §8.2). Each
/// variant pins both the value and its [`PortType`] — the type checker (#40)
/// reads [`const_type`](ConstValue::port_type) for the const node's output port,
/// and the emitter (#42) writes the matching slang literal/constructor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ConstValue {
    /// A `float` literal. [`PortType::Float`].
    Float {
        /// The scalar value.
        value: f32,
    },
    /// A `vec2` literal. [`PortType::Vec2`].
    Vec2 {
        /// The two components `[x, y]`.
        value: [f32; 2],
    },
    /// A `vec3` literal. [`PortType::Vec3`].
    Vec3 {
        /// The three components `[x, y, z]`.
        value: [f32; 3],
    },
    /// A `vec4` literal. [`PortType::Vec4`].
    Vec4 {
        /// The four components `[x, y, z, w]`.
        value: [f32; 4],
    },
    /// An `int` literal. [`PortType::Int`].
    Int {
        /// The integer value.
        value: i32,
    },
    /// A `bool` literal. [`PortType::Bool`].
    Bool {
        /// The boolean value.
        value: bool,
    },
}

impl ConstValue {
    /// The [`PortType`] of this literal's value.
    pub const fn port_type(&self) -> PortType {
        match self {
            ConstValue::Float { .. } => PortType::Float,
            ConstValue::Vec2 { .. } => PortType::Vec2,
            ConstValue::Vec3 { .. } => PortType::Vec3,
            ConstValue::Vec4 { .. } => PortType::Vec4,
            ConstValue::Int { .. } => PortType::Int,
            ConstValue::Bool { .. } => PortType::Bool,
        }
    }
}

/// The intrinsic operation an [`NodeOp::Expr`] performs over its operand ports
/// (Spec §8.3 node taxonomy). The set is deliberately the intrinsics #44's
/// fixtures exercise (color transform, contrast/gamma, UV warp, blur taps) plus
/// the common GLSL math the node taxonomy needs — each maps 1:1 to a slang
/// intrinsic or operator the emitter (#42) writes, and each is buildable by #44.
///
/// **Arity / typing** (used by the #40 type checker): the binary arithmetic ops
/// are component-wise with scalar broadcast; `mix`/`clamp` are ternary; the unary
/// math ops are one operand; `dot` is binary → `float`; `normalize`/`length` act
/// on a vector; `swizzle` carries the selecting [`mask`](ExprOp::Swizzle::mask).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ExprOp {
    /// `a + b` (component-wise, scalar-broadcasting). Binary.
    Add,
    /// `a - b` (component-wise, scalar-broadcasting). Binary.
    Sub,
    /// `a * b` (component-wise, scalar-broadcasting). Binary.
    Mul,
    /// `a / b` (component-wise, scalar-broadcasting). Binary.
    Div,
    /// `mix(a, b, t)` — linear interpolation. Ternary.
    Mix,
    /// `clamp(x, lo, hi)`. Ternary.
    Clamp,
    /// `min(a, b)` (component-wise). Binary.
    Min,
    /// `max(a, b)` (component-wise). Binary.
    Max,
    /// `pow(a, b)` (component-wise). Binary.
    Pow,
    /// `sin(x)` (component-wise). Unary.
    Sin,
    /// `cos(x)` (component-wise). Unary.
    Cos,
    /// `abs(x)` (component-wise). Unary.
    Abs,
    /// `floor(x)` (component-wise). Unary.
    Floor,
    /// `fract(x)` (component-wise). Unary.
    Fract,
    /// `dot(a, b)` → `float`. Binary; operands must share a vector type.
    Dot,
    /// `normalize(v)` → same vector type. Unary.
    Normalize,
    /// `length(v)` → `float`. Unary.
    Length,
    /// A component **swizzle** `v.<mask>` (e.g. `.rgb`, `.xy`, `.x`). Unary; the
    /// result type is [`PortType::swizzle_result`] of the operand and `mask`.
    Swizzle {
        /// The swizzle accessor mask (`xyzw` / `rgba` / `stpq`, length 1..=4).
        mask: String,
    },
    /// Construct a `vecN` from its scalar/sub-vector operands (e.g. build a
    /// `vec4` from a `vec3` + a `float`). The target type is [`ty`](ExprOp::Construct::ty);
    /// this is the explicit widening the implicit rules forbid. Variadic.
    Construct {
        /// The vector type to construct.
        ty: PortType,
    },
}

/// A reference to one **port** on a node in an [`IrGraph`] — the endpoint an
/// [`IrEdge`] connects (Architecture §C). Ports are named by string identifiers
/// declared by the node's [`NodeOp`] (its input/output port names); an edge wires
/// a `(source.node, source.port)` output to a `(target.node, target.port)` input.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PortRef {
    /// The [`IrNode::id`] this port belongs to.
    pub node: String,
    /// The port identifier on that node (e.g. `"out"`, `"coord"`, `"a"`).
    pub port: String,
}

impl PortRef {
    /// Construct a [`PortRef`] from a node id and a port name.
    pub fn new(node: impl Into<String>, port: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            port: port.into(),
        }
    }
}

/// A typed input/output port declaration on a [`NodeOp::CustomSnippet`]
/// (Architecture §C). The snippet's GLSL `body` reads its declared `inputs` and
/// writes its declared `outputs` by name; the type checker validates the wired
/// edges against these types and the emitter substitutes them into the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PortDecl {
    /// The port identifier (the GLSL variable name the snippet body uses).
    pub name: String,
    /// The port's type.
    #[serde(rename = "type")]
    pub ty: PortType,
}

impl PortDecl {
    /// Construct a [`PortDecl`] from a name and type.
    pub fn new(name: impl Into<String>, ty: PortType) -> Self {
        Self {
            name: name.into(),
            ty,
        }
    }
}

/// The operation a single [`IrNode`] performs — the per-node op model
/// (Architecture §C). A node's **inputs** are wired by [`IrEdge`]s targeting its
/// input port names; its **output(s)** are referenced by edges sourcing its
/// output port names. The op carries only its intrinsic configuration (which
/// texture, which semantic, which literal, which intrinsic, the snippet body) —
/// never the wiring, which lives on the edges.
///
/// ## Port-name conventions
///
/// Each variant has a fixed set of port names the lowering pass (#41) and type
/// checker (#40) agree on:
///
/// - [`Sample`](NodeOp::Sample): input `"coord"` (a `vec2`); output `"out"` (a `vec4`).
/// - [`Builtin`](NodeOp::Builtin): no inputs; output `"out"` (the semantic's type).
/// - [`Param`](NodeOp::Param): no inputs; output `"out"` (`float`).
/// - [`Const`](NodeOp::Const): no inputs; output `"out"` (the literal's type).
/// - [`Expr`](NodeOp::Expr): inputs are the entries of `operands` (in order);
///   output `"out"`.
/// - [`Output`](NodeOp::Output): input `"color"` (a `vec4`); no output. Exactly
///   one reachable [`Output`] per graph (the final color sink).
/// - [`CustomSnippet`](NodeOp::CustomSnippet): inputs/outputs are the declared
///   [`PortDecl`]s, addressed by their `name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum NodeOp {
    /// Sample a RetroArch texture at a coordinate. Reads its `"coord"` input port
    /// (`vec2`) and produces a `vec4` on `"out"`.
    Sample {
        /// Which RetroArch texture to sample.
        texture: TextureSource,
    },
    /// Read a builtin-semantic uniform; produces the semantic's type on `"out"`.
    Builtin {
        /// Which reserved RetroArch semantic to read.
        semantic: BuiltinSemantic,
    },
    /// Read a declared `#pragma parameter` value; produces a `float` on `"out"`.
    Param {
        /// The parameter identifier (must match a declared [`Parameter`](crate::Parameter)).
        name: String,
    },
    /// A typed literal constant; produces the literal's type on `"out"`.
    Const {
        /// The literal value (and its type).
        value: ConstValue,
    },
    /// An intrinsic expression over the named `operands` input ports; produces
    /// its result on `"out"`. The result type is determined by `op` and the
    /// operand types (Spec §8.2 rules).
    Expr {
        /// The intrinsic to apply.
        op: ExprOp,
        /// The **ordered** input port names this expression consumes. Each name
        /// is an input port of this node that an [`IrEdge`] wires a value into;
        /// the order is the operand order of `op` (e.g. `["a", "b"]` for `add`,
        /// `["x", "lo", "hi"]` for `clamp`).
        operands: Vec<String>,
    },
    /// The final color sink — the one reachable per graph. Reads its `"color"`
    /// input port (`vec4`) and writes it to `FragColor`. Produces no output port.
    Output,
    /// A verbatim GLSL snippet with typed in/out ports (the escape hatch inside a
    /// graph). The `body` reads its declared `inputs` by name and assigns its
    /// declared `outputs` by name; the emitter inlines it with those substitutions.
    CustomSnippet {
        /// The GLSL statements. References its input port names as in-scope
        /// values and assigns its output port names.
        body: String,
        /// Typed input ports (the snippet's free variables).
        inputs: Vec<PortDecl>,
        /// Typed output ports (the values the snippet assigns).
        outputs: Vec<PortDecl>,
    },
}

/// A directed, port-to-port connection in an [`IrGraph`] (Architecture §C): the
/// `source` node's **output** port feeds the `target` node's **input** port. The
/// value's type must be [`assignable_to`](PortType::assignable_to) the target
/// port's type (validated by the #40 type checker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IrEdge {
    /// The output port the value comes from.
    pub source: PortRef,
    /// The input port the value flows into.
    pub target: PortRef,
}

impl IrEdge {
    /// Wire `source_node.source_port` → `target_node.target_port`.
    pub fn new(
        source_node: impl Into<String>,
        source_port: impl Into<String>,
        target_node: impl Into<String>,
        target_port: impl Into<String>,
    ) -> Self {
        Self {
            source: PortRef::new(source_node, source_port),
            target: PortRef::new(target_node, target_port),
        }
    }
}

/// A single typed node in an [`IrGraph`]: a stable `id` plus its [`NodeOp`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IrNode {
    /// Stable unique id, referenced by [`IrEdge`]/[`PortRef`] and diagnostics.
    pub id: String,
    /// The operation this node performs.
    pub op: NodeOp,
}

impl IrNode {
    /// Construct an [`IrNode`] from an id and an op.
    pub fn new(id: impl Into<String>, op: NodeOp) -> Self {
        Self { id: id.into(), op }
    }
}

/// The per-pass **typed dataflow DAG** (Architecture §C) — the codegen-facing
/// model the type checker (#40), lowering (#41), and emitter (#42) consume.
///
/// This is the typed counterpart to the skeletal editor [`Graph`](crate::Graph)
/// (see the module docs): nodes are concrete [`NodeOp`]s with typed ports, edges
/// wire output ports to input ports. A well-formed graph has exactly one
/// reachable [`NodeOp::Output`] sink; the lowering pass topologically sorts from
/// it. Hand-built in Rust for Phase-4 tests; the editor → IR bridge is Phase 5.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IrGraph {
    /// The typed nodes.
    pub nodes: Vec<IrNode>,
    /// The directed, port-to-port connections between nodes.
    pub edges: Vec<IrEdge>,
}

/// The severity of a [`Diagnostic`] emitted by the type checker (#40).
///
/// An [`Error`](DiagnosticSeverity::Error) means the graph is **not** lowerable /
/// emittable (it would produce invalid slang or has no defined meaning); a
/// [`Warning`](DiagnosticSeverity::Warning) is advisory and does not block
/// lowering. [`Diagnostics::has_errors`] keys off this distinction so callers
/// (lowering #41, `compile_graph` #42) can cleanly tell "clean" from "broken".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DiagnosticSeverity {
    /// A blocking problem — the graph cannot be lowered or emitted.
    Error,
    /// An advisory problem — does not block lowering.
    Warning,
}

/// A single structured diagnostic produced when type-checking an [`IrGraph`]
/// (#40). Every diagnostic carries the **offending node id** (and, where it
/// applies, the offending **port id** on that node) so the Phase-5 editor can map
/// it to an inline highlight on the exact node/port. It is `serde` +
/// `#[ts(export)]` because the future `compile_graph` IPC command returns these
/// to the webview (module doc §A).
///
/// The `code` is a short stable machine-readable tag (e.g. `"typeMismatch"`,
/// `"cycle"`, `"missingOutput"`) the UI can switch on without parsing `message`;
/// `message` is the human-readable explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Diagnostic {
    /// Whether this blocks lowering ([`Error`](DiagnosticSeverity::Error)) or is
    /// advisory ([`Warning`](DiagnosticSeverity::Warning)).
    pub severity: DiagnosticSeverity,
    /// A short, stable, machine-readable category tag (e.g. `"typeMismatch"`).
    pub code: String,
    /// The human-readable explanation of the problem.
    pub message: String,
    /// The id of the offending [`IrNode`] — always present so the editor can map
    /// the diagnostic to a node.
    pub node: String,
    /// The offending port name on [`node`](Diagnostic::node), when the diagnostic
    /// is about a specific port (e.g. a type mismatch on an input port); `None`
    /// for node-level problems (e.g. a cycle, an unknown parameter).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub port: Option<String>,
}

impl Diagnostic {
    /// Build an [`Error`](DiagnosticSeverity::Error)-severity diagnostic on a node.
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        node: impl Into<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            code: code.into(),
            message: message.into(),
            node: node.into(),
            port: None,
        }
    }

    /// Build a [`Warning`](DiagnosticSeverity::Warning)-severity diagnostic on a node.
    pub fn warning(
        code: impl Into<String>,
        message: impl Into<String>,
        node: impl Into<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            code: code.into(),
            message: message.into(),
            node: node.into(),
            port: None,
        }
    }

    /// Attach an offending port name to this diagnostic (builder style).
    #[must_use]
    pub fn with_port(mut self, port: impl Into<String>) -> Self {
        self.port = Some(port.into());
        self
    }
}

/// The result of type-checking an [`IrGraph`] (#40): the ordered list of
/// [`Diagnostic`]s found. A clean graph yields an empty collection.
///
/// This is the value lowering (#41) and the `compile_graph` command (#42) consume
/// to decide whether to proceed: [`has_errors`](Diagnostics::has_errors)
/// distinguishes "clean" (lowerable) from "has blocking errors" cleanly, without
/// the caller re-scanning severities. It is `serde` + `#[ts(export)]` so the
/// whole collection round-trips over IPC to the editor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Diagnostics {
    /// The diagnostics found, in the order the checker emitted them.
    pub items: Vec<Diagnostic>,
}

impl Diagnostics {
    /// An empty collection (no diagnostics).
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a diagnostic.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.items.push(diagnostic);
    }

    /// Whether any diagnostic is an [`Error`](DiagnosticSeverity::Error). When
    /// this is `false`, the graph type-checked clean enough to lower (#41).
    pub fn has_errors(&self) -> bool {
        self.items
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error)
    }

    /// Whether the collection is empty (no diagnostics at all — fully clean).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The number of diagnostics.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Iterate over the diagnostics.
    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.items.iter()
    }
}

/// The result of compiling a typed [`IrGraph`] to slang — the payload the
/// `compile_graph` IPC command (#42) returns to the webview.
///
/// It bundles the two things the Phase-5 editor's debounced edit loop needs after
/// every graph change: the [`Diagnostics`] to surface inline (always present —
/// empty when the graph is clean) and the emitted slang `source` (present only
/// when the graph type-checked clean and lowered + emitted successfully, `None`
/// otherwise). A caller renders the diagnostics regardless, and previews/compiles
/// the `source` only when it is `Some`.
///
/// `serde` + `#[ts(export)]` so it round-trips over IPC as a single typed shape.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CompileGraphResult {
    /// The emitted `.slang` source, or `None` when the graph had blocking errors
    /// (and so was never lowered/emitted).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    /// The type-checker diagnostics (empty when the graph is fully clean). Always
    /// present so the editor can render them whether or not `source` was produced.
    pub diagnostics: Diagnostics,
}

#[cfg(test)]
mod port_type_tests {
    use super::*;

    #[test]
    fn component_counts() {
        assert_eq!(PortType::Float.component_count(), Some(1));
        assert_eq!(PortType::Int.component_count(), Some(1));
        assert_eq!(PortType::Bool.component_count(), Some(1));
        assert_eq!(PortType::Vec2.component_count(), Some(2));
        assert_eq!(PortType::Vec3.component_count(), Some(3));
        assert_eq!(PortType::Vec4.component_count(), Some(4));
        assert_eq!(PortType::Sampler2D.component_count(), None);
    }

    #[test]
    fn scalar_vector_numeric_classification() {
        assert!(PortType::Float.is_scalar());
        assert!(PortType::Int.is_scalar());
        assert!(PortType::Bool.is_scalar());
        assert!(!PortType::Vec3.is_scalar());

        assert!(PortType::Vec2.is_vector());
        assert!(PortType::Vec4.is_vector());
        assert!(!PortType::Float.is_vector());
        assert!(!PortType::Sampler2D.is_vector());

        assert!(PortType::Float.is_numeric());
        assert!(PortType::Vec4.is_numeric());
        assert!(!PortType::Sampler2D.is_numeric());
    }

    #[test]
    fn float_family_excludes_int_bool_and_sampler() {
        // The float family is exactly Float + the vecN types.
        assert!(PortType::Float.is_float_family());
        assert!(PortType::Vec2.is_float_family());
        assert!(PortType::Vec3.is_float_family());
        assert!(PortType::Vec4.is_float_family());
        // Int/Bool are numeric/scalar but NOT float-family (they cannot take
        // part in the component-wise math the emitter lowers to GLSL operators).
        assert!(!PortType::Int.is_float_family());
        assert!(!PortType::Bool.is_float_family());
        // Samplers are never float-family.
        assert!(!PortType::Sampler2D.is_float_family());
    }

    #[test]
    fn float_with_components_inverts_count() {
        assert_eq!(PortType::float_with_components(1), Some(PortType::Float));
        assert_eq!(PortType::float_with_components(2), Some(PortType::Vec2));
        assert_eq!(PortType::float_with_components(3), Some(PortType::Vec3));
        assert_eq!(PortType::float_with_components(4), Some(PortType::Vec4));
        assert_eq!(PortType::float_with_components(0), None);
        assert_eq!(PortType::float_with_components(5), None);
    }

    #[test]
    fn scalar_broadcast_rules() {
        // float broadcasts to any float vector.
        assert!(PortType::Float.broadcast_to(PortType::Vec2));
        assert!(PortType::Float.broadcast_to(PortType::Vec3));
        assert!(PortType::Float.broadcast_to(PortType::Vec4));
        // Identity always broadcasts.
        assert!(PortType::Vec3.broadcast_to(PortType::Vec3));
        assert!(PortType::Float.broadcast_to(PortType::Float));
        // A vector does not broadcast to a scalar or another vector.
        assert!(!PortType::Vec2.broadcast_to(PortType::Vec3));
        assert!(!PortType::Vec3.broadcast_to(PortType::Float));
        // Int/Bool do not silently broadcast to a float vector.
        assert!(!PortType::Int.broadcast_to(PortType::Vec3));
        assert!(!PortType::Bool.broadcast_to(PortType::Vec4));
        // Samplers never broadcast.
        assert!(!PortType::Float.broadcast_to(PortType::Sampler2D));
    }

    #[test]
    fn implicit_widening_rules() {
        // Only Int -> Float is an implicit widen.
        assert!(PortType::Int.implicit_widen_to(PortType::Float));
        // Identity widens trivially.
        assert!(PortType::Float.implicit_widen_to(PortType::Float));
        assert!(PortType::Vec3.implicit_widen_to(PortType::Vec3));
        // A narrower vector does NOT implicitly widen to a wider one.
        assert!(!PortType::Vec2.implicit_widen_to(PortType::Vec3));
        assert!(!PortType::Vec3.implicit_widen_to(PortType::Vec4));
        // Float -> vecN is broadcast, not widening.
        assert!(!PortType::Float.implicit_widen_to(PortType::Vec3));
        // Float does not narrow to Int.
        assert!(!PortType::Float.implicit_widen_to(PortType::Int));
    }

    #[test]
    fn assignable_combines_widen_and_broadcast() {
        // Int widens to Float.
        assert!(PortType::Int.assignable_to(PortType::Float));
        // Float broadcasts to a vector.
        assert!(PortType::Float.assignable_to(PortType::Vec4));
        // Same type is assignable.
        assert!(PortType::Vec2.assignable_to(PortType::Vec2));
        // vec2 -> vec3 is neither widen nor broadcast.
        assert!(!PortType::Vec2.assignable_to(PortType::Vec3));
    }

    #[test]
    fn swizzle_result_typing() {
        // Single-component swizzle yields Float.
        assert_eq!(PortType::Vec4.swizzle_result("x"), Some(PortType::Float));
        assert_eq!(PortType::Vec3.swizzle_result("r"), Some(PortType::Float));
        // Two/three/four components.
        assert_eq!(PortType::Vec4.swizzle_result("xy"), Some(PortType::Vec2));
        assert_eq!(PortType::Vec4.swizzle_result("rgb"), Some(PortType::Vec3));
        assert_eq!(PortType::Vec4.swizzle_result("xyzw"), Some(PortType::Vec4));
        // Repeats/reorders are allowed (GLSL): vec2.yx -> Vec2, vec3.xxxx -> Vec4.
        assert_eq!(PortType::Vec2.swizzle_result("yx"), Some(PortType::Vec2));
        assert_eq!(PortType::Vec3.swizzle_result("xxxx"), Some(PortType::Vec4));
        // A float exposes only .x/.r/.s.
        assert_eq!(PortType::Float.swizzle_result("x"), Some(PortType::Float));
        assert_eq!(PortType::Float.swizzle_result("r"), Some(PortType::Float));
        assert_eq!(PortType::Float.swizzle_result("xx"), Some(PortType::Vec2));
    }

    #[test]
    fn swizzle_result_rejects_illegal() {
        // Selecting a component the base doesn't have.
        assert_eq!(PortType::Vec2.swizzle_result("z"), None);
        assert_eq!(PortType::Float.swizzle_result("y"), None);
        assert_eq!(PortType::Vec3.swizzle_result("w"), None);
        // Empty / too-long masks.
        assert_eq!(PortType::Vec4.swizzle_result(""), None);
        assert_eq!(PortType::Vec4.swizzle_result("xyzwx"), None);
        // Mixed accessor sets.
        assert_eq!(PortType::Vec4.swizzle_result("xr"), None);
        assert_eq!(PortType::Vec4.swizzle_result("rs"), None);
        // Unknown char.
        assert_eq!(PortType::Vec4.swizzle_result("q1"), None);
        // Non-float bases cannot be swizzled.
        assert_eq!(PortType::Int.swizzle_result("x"), None);
        assert_eq!(PortType::Bool.swizzle_result("x"), None);
        assert_eq!(PortType::Sampler2D.swizzle_result("x"), None);
    }

    #[test]
    fn port_type_serializes_camel_case() {
        assert_eq!(
            serde_json::to_value(PortType::Float).unwrap(),
            serde_json::json!("float")
        );
        assert_eq!(
            serde_json::to_value(PortType::Vec2).unwrap(),
            serde_json::json!("vec2")
        );
        assert_eq!(
            serde_json::to_value(PortType::Sampler2D).unwrap(),
            serde_json::json!("sampler2D")
        );
    }
}

#[cfg(test)]
mod texture_and_builtin_tests {
    use super::*;

    #[test]
    fn texture_source_round_trips_and_tags() {
        for ts in [
            TextureSource::Source,
            TextureSource::Original,
            TextureSource::OriginalHistory { index: 2 },
            TextureSource::PassOutput { index: 0 },
            TextureSource::PassFeedback { index: 1 },
            TextureSource::Lut {
                name: "BORDER".to_owned(),
            },
        ] {
            let v = serde_json::to_value(&ts).expect("serializes");
            assert!(
                v.get("kind").and_then(|k| k.as_str()).is_some(),
                "carries a `kind` discriminator: {v}"
            );
            let back: TextureSource = serde_json::from_value(v).expect("round-trips");
            assert_eq!(ts, back);
        }
    }

    #[test]
    fn texture_source_tags_are_camel_case() {
        assert_eq!(
            serde_json::to_value(TextureSource::OriginalHistory { index: 3 }).unwrap(),
            serde_json::json!({ "kind": "originalHistory", "index": 3 })
        );
        assert_eq!(
            serde_json::to_value(TextureSource::PassFeedback { index: 1 }).unwrap(),
            serde_json::json!({ "kind": "passFeedback", "index": 1 })
        );
        assert_eq!(
            serde_json::to_value(TextureSource::Lut {
                name: "OVERLAY".to_owned()
            })
            .unwrap(),
            serde_json::json!({ "kind": "lut", "name": "OVERLAY" })
        );
    }

    #[test]
    fn builtin_semantic_port_types() {
        assert_eq!(
            BuiltinSemantic::SourceSize.port_type(),
            Some(PortType::Vec4)
        );
        assert_eq!(
            BuiltinSemantic::OutputSize.port_type(),
            Some(PortType::Vec4)
        );
        assert_eq!(
            BuiltinSemantic::FinalViewportSize.port_type(),
            Some(PortType::Vec4)
        );
        assert_eq!(BuiltinSemantic::FrameCount.port_type(), Some(PortType::Int));
        assert_eq!(
            BuiltinSemantic::FrameDirection.port_type(),
            Some(PortType::Int)
        );
        assert_eq!(BuiltinSemantic::Mvp.port_type(), None);
    }

    #[test]
    fn builtin_semantic_slang_names_are_reserved_spellings() {
        assert_eq!(BuiltinSemantic::SourceSize.slang_name(), "SourceSize");
        assert_eq!(BuiltinSemantic::OriginalSize.slang_name(), "OriginalSize");
        assert_eq!(BuiltinSemantic::OutputSize.slang_name(), "OutputSize");
        assert_eq!(
            BuiltinSemantic::FinalViewportSize.slang_name(),
            "FinalViewportSize"
        );
        assert_eq!(BuiltinSemantic::FrameCount.slang_name(), "FrameCount");
        assert_eq!(
            BuiltinSemantic::FrameDirection.slang_name(),
            "FrameDirection"
        );
        assert_eq!(BuiltinSemantic::Mvp.slang_name(), "MVP");
    }

    #[test]
    fn builtin_semantic_serializes_camel_case() {
        assert_eq!(
            serde_json::to_value(BuiltinSemantic::SourceSize).unwrap(),
            serde_json::json!("sourceSize")
        );
        assert_eq!(
            serde_json::to_value(BuiltinSemantic::Mvp).unwrap(),
            serde_json::json!("mvp")
        );
        let back: BuiltinSemantic =
            serde_json::from_value(serde_json::json!("frameDirection")).unwrap();
        assert_eq!(back, BuiltinSemantic::FrameDirection);
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;

    /// Build the canonical demo graph the acceptance criterion names:
    /// `Sample(Source) → Expr(color transform) → Output`.
    ///
    /// Wiring:
    /// - `coord` reads the builtin texcoord via a `Const` `vec2` placeholder
    ///   (Phase-4 graphs are hand-built; the real vTexCoord plumbing is the
    ///   emitter's job). Here `uv` feeds `sample.coord`.
    /// - `sample.out` (vec4) feeds the color-transform expression's first operand.
    /// - a `Const` brightness scalar feeds the second operand.
    /// - `mul.out` feeds `output.color`.
    fn demo_graph() -> IrGraph {
        IrGraph {
            nodes: vec![
                IrNode::new(
                    "uv",
                    NodeOp::Const {
                        value: ConstValue::Vec2 { value: [0.5, 0.5] },
                    },
                ),
                IrNode::new(
                    "sample",
                    NodeOp::Sample {
                        texture: TextureSource::Source,
                    },
                ),
                IrNode::new(
                    "bright",
                    NodeOp::Const {
                        value: ConstValue::Float { value: 1.5 },
                    },
                ),
                IrNode::new(
                    "mul",
                    NodeOp::Expr {
                        op: ExprOp::Mul,
                        operands: vec!["a".to_owned(), "b".to_owned()],
                    },
                ),
                IrNode::new("output", NodeOp::Output),
            ],
            edges: vec![
                IrEdge::new("uv", "out", "sample", "coord"),
                IrEdge::new("sample", "out", "mul", "a"),
                IrEdge::new("bright", "out", "mul", "b"),
                IrEdge::new("mul", "out", "output", "color"),
            ],
        }
    }

    #[test]
    fn hand_built_graph_constructs_and_round_trips_identically() {
        let graph = demo_graph();
        // Rust -> JSON -> Rust is identical (the acceptance criterion).
        let json = serde_json::to_string_pretty(&graph).expect("serialize");
        let back: IrGraph = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(graph, back);
    }

    #[test]
    fn demo_graph_has_exactly_one_output_sink() {
        let graph = demo_graph();
        let outputs = graph
            .nodes
            .iter()
            .filter(|n| matches!(n.op, NodeOp::Output))
            .count();
        assert_eq!(outputs, 1, "exactly one reachable Output sink");
    }

    #[test]
    fn node_op_tags_are_camel_case() {
        let sample = serde_json::to_value(NodeOp::Sample {
            texture: TextureSource::Source,
        })
        .unwrap();
        assert_eq!(sample["kind"], "sample");
        assert_eq!(sample["texture"]["kind"], "source");

        let expr = serde_json::to_value(NodeOp::Expr {
            op: ExprOp::Mix,
            operands: vec!["a".to_owned(), "b".to_owned(), "t".to_owned()],
        })
        .unwrap();
        assert_eq!(expr["kind"], "expr");
        assert_eq!(expr["op"]["op"], "mix");
        assert_eq!(expr["operands"], serde_json::json!(["a", "b", "t"]));

        let output = serde_json::to_value(NodeOp::Output).unwrap();
        assert_eq!(output["kind"], "output");
    }

    #[test]
    fn const_value_round_trips_each_variant() {
        for cv in [
            ConstValue::Float { value: 0.25 },
            ConstValue::Vec2 { value: [1.0, 2.0] },
            ConstValue::Vec3 {
                value: [1.0, 2.0, 3.0],
            },
            ConstValue::Vec4 {
                value: [1.0, 2.0, 3.0, 4.0],
            },
            ConstValue::Int { value: -7 },
            ConstValue::Bool { value: true },
        ] {
            let v = serde_json::to_value(cv).expect("serialize");
            let back: ConstValue = serde_json::from_value(v).expect("deserialize");
            assert_eq!(cv, back);
        }
        assert_eq!(
            ConstValue::Vec3 {
                value: [0.0, 0.0, 0.0]
            }
            .port_type(),
            PortType::Vec3
        );
        assert_eq!(ConstValue::Int { value: 1 }.port_type(), PortType::Int);
    }

    #[test]
    fn expr_op_swizzle_and_construct_round_trip() {
        let sw = ExprOp::Swizzle {
            mask: "rgb".to_owned(),
        };
        let v = serde_json::to_value(&sw).unwrap();
        assert_eq!(v["op"], "swizzle");
        assert_eq!(v["mask"], "rgb");
        let back: ExprOp = serde_json::from_value(v).unwrap();
        assert_eq!(sw, back);

        let ctor = ExprOp::Construct { ty: PortType::Vec4 };
        let v = serde_json::to_value(&ctor).unwrap();
        assert_eq!(v["op"], "construct");
        assert_eq!(v["ty"], "vec4");
        let back: ExprOp = serde_json::from_value(v).unwrap();
        assert_eq!(ctor, back);
    }

    #[test]
    fn custom_snippet_round_trips_with_typed_ports() {
        let node = IrNode::new(
            "snippet",
            NodeOp::CustomSnippet {
                body: "out_color = vec4(in_color.rgb * gain, in_color.a);".to_owned(),
                inputs: vec![
                    PortDecl::new("in_color", PortType::Vec4),
                    PortDecl::new("gain", PortType::Float),
                ],
                outputs: vec![PortDecl::new("out_color", PortType::Vec4)],
            },
        );
        let v = serde_json::to_value(&node).expect("serialize");
        assert_eq!(v["op"]["kind"], "customSnippet");
        // PortDecl renames `ty` to `type` on the wire.
        assert_eq!(v["op"]["inputs"][0]["type"], "vec4");
        assert_eq!(v["op"]["inputs"][0]["name"], "in_color");
        let back: IrNode = serde_json::from_value(v).expect("deserialize");
        assert_eq!(node, back);
    }

    #[test]
    fn ir_edge_and_port_ref_round_trip_camel_case() {
        let edge = IrEdge::new("sample", "out", "output", "color");
        let v = serde_json::to_value(&edge).unwrap();
        assert_eq!(v["source"]["node"], "sample");
        assert_eq!(v["source"]["port"], "out");
        assert_eq!(v["target"]["node"], "output");
        assert_eq!(v["target"]["port"], "color");
        let back: IrEdge = serde_json::from_value(v).unwrap();
        assert_eq!(edge, back);
    }

    #[test]
    fn diagnostic_round_trips_and_serializes_camel_case() {
        let d = Diagnostic::error("typeMismatch", "vec3 is not assignable to vec4", "mul")
            .with_port("color");
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["severity"], "error");
        assert_eq!(v["code"], "typeMismatch");
        assert_eq!(v["node"], "mul");
        assert_eq!(v["port"], "color");
        let back: Diagnostic = serde_json::from_value(v).unwrap();
        assert_eq!(d, back);

        // A node-level (no-port) diagnostic omits `port` on the wire (skip-if-none).
        let warn = Diagnostic::warning("unusedNode", "node has no path to Output", "stray");
        let v = serde_json::to_value(&warn).unwrap();
        assert_eq!(v["severity"], "warning");
        assert!(
            v.get("port").is_none(),
            "no-port diagnostic omits `port`: {v}"
        );
        // ...and a payload that omitted `port` still deserializes (serde default).
        let back: Diagnostic = serde_json::from_value(v).unwrap();
        assert_eq!(warn, back);
        assert_eq!(back.port, None);
    }

    #[test]
    fn diagnostics_collection_distinguishes_clean_from_errors() {
        let mut diags = Diagnostics::new();
        assert!(diags.is_empty());
        assert!(!diags.has_errors());

        diags.push(Diagnostic::warning("w", "advisory", "n1"));
        assert!(!diags.is_empty());
        assert!(
            !diags.has_errors(),
            "a warning alone is not a blocking error"
        );
        assert_eq!(diags.len(), 1);

        diags.push(Diagnostic::error("e", "blocking", "n2"));
        assert!(diags.has_errors());

        // Round-trips as an IPC payload.
        let v = serde_json::to_value(&diags).unwrap();
        let back: Diagnostics = serde_json::from_value(v).unwrap();
        assert_eq!(diags, back);
    }

    #[test]
    fn compile_graph_result_round_trips_clean_and_errored() {
        // Clean: a source and no diagnostics — `diagnostics` is always present,
        // `source` is omitted on the wire only when it is `None`.
        let ok = CompileGraphResult {
            source: Some("#version 450\n".to_owned()),
            diagnostics: Diagnostics::new(),
        };
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["source"], "#version 450\n");
        assert!(
            v.get("diagnostics").is_some(),
            "diagnostics always present: {v}"
        );
        let back: CompileGraphResult = serde_json::from_value(v).unwrap();
        assert_eq!(ok, back);

        // Errored: no source, carries diagnostics.
        let mut diags = Diagnostics::new();
        diags.push(Diagnostic::error("cycle", "graph has a cycle", "n1"));
        let err = CompileGraphResult {
            source: None,
            diagnostics: diags,
        };
        let v = serde_json::to_value(&err).unwrap();
        assert!(v.get("source").is_none(), "no source on error: {v}");
        let back: CompileGraphResult = serde_json::from_value(v).unwrap();
        assert_eq!(err, back);
    }

    #[test]
    fn pass_is_either_typed_graph_or_verbatim_whole_pass_code() {
        // The typed model and the verbatim-source path are both reachable: a
        // pass authored as a typed IrGraph lowers to slang (#42), while
        // whole-pass code reuses the existing PassSource::WholePassCode escape
        // hatch (#43) — confirming the "either/or" documented in the module docs.
        use crate::PassSource;
        let verbatim = PassSource::WholePassCode {
            source: "#version 450\n// verbatim".to_owned(),
            filename: Some("imported.slang".to_owned()),
            opaque: true,
        };
        match verbatim {
            PassSource::WholePassCode { opaque, .. } => {
                assert!(opaque, "whole-pass code is the opaque, non-IR path");
            }
            _ => panic!("expected whole-pass code"),
        }
        // And a typed graph constructs independently.
        let _typed = demo_graph();
    }
}
