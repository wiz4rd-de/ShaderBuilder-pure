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
