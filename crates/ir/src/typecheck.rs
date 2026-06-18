//! The pure per-pass graph **type checker** (#40).
//!
//! [`check`] validates a hand-built (Phase 4) or editor-lowered (Phase 5)
//! [`IrGraph`] against the port type system (Spec §8.2) and the op model
//! (Architecture §C), returning structured [`Diagnostics`] — each carrying the
//! **offending node id** so the Phase-5 editor can map it to an inline node
//! highlight. It is a pure function of the graph plus a [`CheckContext`] (the
//! declared parameter names and LUT names a `Param`/`Sample` reference must
//! resolve against); it touches no GPU, engine, or filesystem and runs in the
//! headless test suite.
//!
//! The checker is the gate before lowering (#41 only lowers a graph that
//! type-checked) and before the future `compile_graph` command (#42). Callers
//! key off [`Diagnostics::has_errors`] to tell "clean / lowerable" from
//! "has blocking errors".
//!
//! ## What it validates
//!
//! 1. **Edge type-compatibility** — for each edge, the source output port type
//!    must be [`assignable_to`](PortType::assignable_to) the sink input port
//!    type (exact, `Int→Float` widen, or `Float→vecN` broadcast). Mismatch →
//!    `typeMismatch` on the sink node + port.
//! 2. **Op arity & operand typing** — each [`ExprOp`] has a required operand
//!    count and operand-type constraints; a [`Swizzle`](ExprOp::Swizzle) mask
//!    must be legal for its input type. Violations → `wrongArity` /
//!    `illegalSwizzle` / `operandType` on the node.
//! 3. **Reference resolution** — a [`Param`](NodeOp::Param) name must match a
//!    declared parameter; a [`Sample`](NodeOp::Sample) `Lut` ref must name a
//!    declared LUT. Unresolved → `unknownParam` / `unknownTexture`.
//! 4. **Graph well-formedness** — acyclic (cycles → `cycle`), exactly one
//!    reachable [`Output`](NodeOp::Output) (`missingOutput` / `multipleOutputs`),
//!    no dangling required inputs (`danglingInput`), and edges referencing real
//!    nodes/ports (`unknownNode` / `unknownPort`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use core_model::ir::{
    Diagnostic, Diagnostics, ExprOp, IrEdge, IrGraph, IrNode, NodeOp, PortType, TextureSource,
};

/// Stable machine-readable [`Diagnostic::code`] tags the checker emits. Grouped
/// here so callers (and tests) match the exact spelling and the Phase-5 editor
/// can switch on them.
pub mod codes {
    /// An edge's source value type is not assignable to the sink input port type.
    pub const TYPE_MISMATCH: &str = "typeMismatch";
    /// An [`ExprOp`](core_model::ir::ExprOp) has the wrong number of operands.
    pub const WRONG_ARITY: &str = "wrongArity";
    /// An operand has a type the op forbids (e.g. a sampler into arithmetic).
    pub const OPERAND_TYPE: &str = "operandType";
    /// A swizzle mask is illegal for its input type.
    pub const ILLEGAL_SWIZZLE: &str = "illegalSwizzle";
    /// A `Param` node references an undeclared parameter name.
    pub const UNKNOWN_PARAM: &str = "unknownParam";
    /// A `Sample` node references an undeclared LUT name.
    pub const UNKNOWN_TEXTURE: &str = "unknownTexture";
    /// The graph contains a cycle.
    pub const CYCLE: &str = "cycle";
    /// The graph has no reachable `Output` node.
    pub const MISSING_OUTPUT: &str = "missingOutput";
    /// The graph has more than one `Output` node.
    pub const MULTIPLE_OUTPUTS: &str = "multipleOutputs";
    /// A required input port has no incoming edge.
    pub const DANGLING_INPUT: &str = "danglingInput";
    /// An edge references a node id that does not exist.
    pub const UNKNOWN_NODE: &str = "unknownNode";
    /// An edge references a port name that does not exist on its node.
    pub const UNKNOWN_PORT: &str = "unknownPort";
    /// Two edges feed the same input port.
    pub const DUPLICATE_INPUT: &str = "duplicateInput";
    /// Two nodes share the same id.
    pub const DUPLICATE_NODE_ID: &str = "duplicateNodeId";
}

/// The declared identifiers a graph's [`Param`](NodeOp::Param) and
/// [`Sample`](NodeOp::Sample)-of-LUT references must resolve against (Spec §4/§7).
///
/// In Phase 4 these are supplied by the hand-built test; in Phase 5 they come
/// from the pass's declared [`Parameter`](core_model::Parameter)s and the
/// project's [`Lut`](core_model::Lut)s. A graph that references a parameter or
/// LUT not listed here is rejected with `unknownParam` / `unknownTexture`.
#[derive(Debug, Clone, Default)]
pub struct CheckContext {
    /// Declared `#pragma parameter` names a `Param` node may reference.
    pub parameters: BTreeSet<String>,
    /// Declared LUT names a `Sample { texture: Lut { name } }` may reference.
    pub luts: BTreeSet<String>,
}

impl CheckContext {
    /// An empty context (no declared parameters or LUTs).
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a declared parameter name (builder style).
    #[must_use]
    pub fn with_parameter(mut self, name: impl Into<String>) -> Self {
        self.parameters.insert(name.into());
        self
    }

    /// Register a declared LUT name (builder style).
    #[must_use]
    pub fn with_lut(mut self, name: impl Into<String>) -> Self {
        self.luts.insert(name.into());
        self
    }
}

/// Type-check `graph` against `ctx`, returning all [`Diagnostics`] found (empty
/// when the graph is clean). Pure: no GPU/engine/filesystem.
///
/// See the module docs for the full list of checks. Diagnostics are emitted in a
/// deterministic order (node-structure checks, then edge checks, then per-op
/// checks, then well-formedness) so tests can assert on them reliably; every
/// diagnostic carries the offending [`Diagnostic::node`].
pub fn check(graph: &IrGraph, ctx: &CheckContext) -> Diagnostics {
    let mut diags = Diagnostics::new();

    // Index nodes by id; report duplicate ids (the first definition wins for
    // lookups). A duplicate id makes edge endpoints ambiguous, so flag it.
    let mut nodes: HashMap<&str, &IrNode> = HashMap::with_capacity(graph.nodes.len());
    let mut seen_ids: HashSet<&str> = HashSet::new();
    for node in &graph.nodes {
        if !seen_ids.insert(node.id.as_str()) {
            diags.push(Diagnostic::error(
                codes::DUPLICATE_NODE_ID,
                format!("duplicate node id `{}`", node.id),
                &node.id,
            ));
            continue;
        }
        nodes.insert(node.id.as_str(), node);
    }

    // Per-node reference & op-shape checks (parameter/LUT resolution, swizzle
    // legality, construct target). These are independent of wiring.
    for node in &graph.nodes {
        check_node_references(node, ctx, &mut diags);
    }

    // Edge structural validity: endpoints must name real nodes and real ports,
    // and an input port may be fed by at most one edge. We collect the resolved
    // incoming edge per (node, input-port) for the dangling-input and type
    // checks below.
    let incoming = check_edge_structure(graph, &nodes, &mut diags);

    // Dangling required inputs: every op input port with no default and no
    // incoming edge is an error.
    check_required_inputs(graph, &incoming, &mut diags);

    // Output cardinality: exactly one Output node.
    check_output_cardinality(graph, &mut diags);

    // Acyclicity: a cycle makes type inference and SSA-lowering undefined.
    let cycle_nodes = check_acyclic(graph, &nodes, &mut diags);

    // Edge type-compatibility & Expr operand typing both need inferred output
    // port types. Inference is only meaningful on an acyclic graph (a cycle has
    // no well-defined types), so skip the type-driven checks when a cycle was
    // found — the cycle diagnostic is the actionable one.
    if cycle_nodes.is_empty() {
        let mut inferer = TypeInferer::new(&nodes, &incoming);
        check_edge_types(graph, &nodes, &mut inferer, &mut diags);
        check_expr_operand_types(graph, &incoming, &mut inferer, &mut diags);
    }

    diags
}

// ----------------------------------------------------------------------------
// Port-name conventions (Architecture §C, NodeOp docs)
// ----------------------------------------------------------------------------

/// The declared **input** port names of a node, with whether each is *required*
/// (must have an incoming edge — there is no default value). Returns `None` ports
/// only for nodes with no inputs.
fn input_ports(op: &NodeOp) -> Vec<(String, bool)> {
    match op {
        // Sample reads a vec2 coord; required.
        NodeOp::Sample { .. } => vec![("coord".to_owned(), true)],
        // Source nodes: no inputs.
        NodeOp::Builtin { .. } | NodeOp::Param { .. } | NodeOp::Const { .. } => Vec::new(),
        // Expr: its ordered operand port names, all required.
        NodeOp::Expr { operands, .. } => operands.iter().map(|p| (p.clone(), true)).collect(),
        // Output: the single `color` sink; required.
        NodeOp::Output => vec![("color".to_owned(), true)],
        // CustomSnippet: its declared input ports, all required.
        NodeOp::CustomSnippet { inputs, .. } => {
            inputs.iter().map(|p| (p.name.clone(), true)).collect()
        }
    }
}

/// Whether `op` declares an **output** port named `port`. Every value-producing
/// op exposes `"out"`; a [`CustomSnippet`](NodeOp::CustomSnippet) exposes its
/// declared output port names; [`Output`](NodeOp::Output) has no output port.
fn has_output_port(op: &NodeOp, port: &str) -> bool {
    match op {
        NodeOp::Sample { .. }
        | NodeOp::Builtin { .. }
        | NodeOp::Param { .. }
        | NodeOp::Const { .. }
        | NodeOp::Expr { .. } => port == "out",
        NodeOp::Output => false,
        NodeOp::CustomSnippet { outputs, .. } => outputs.iter().any(|p| p.name == port),
    }
}

/// Whether `op` declares an **input** port named `port`.
fn has_input_port(op: &NodeOp, port: &str) -> bool {
    input_ports(op).iter().any(|(name, _)| name == port)
}

// ----------------------------------------------------------------------------
// Per-node reference & op-shape checks
// ----------------------------------------------------------------------------

fn check_node_references(node: &IrNode, ctx: &CheckContext, diags: &mut Diagnostics) {
    match &node.op {
        NodeOp::Param { name } => {
            if !ctx.parameters.contains(name) {
                diags.push(Diagnostic::error(
                    codes::UNKNOWN_PARAM,
                    format!("parameter `{name}` is not declared on this pass"),
                    &node.id,
                ));
            }
        }
        NodeOp::Sample { texture } => {
            if let TextureSource::Lut { name } = texture {
                if !ctx.luts.contains(name) {
                    diags.push(Diagnostic::error(
                        codes::UNKNOWN_TEXTURE,
                        format!("LUT `{name}` is not declared in the project textures"),
                        &node.id,
                    ));
                }
            }
        }
        NodeOp::Expr { op, operands } => {
            check_expr_op_shape(node.id.as_str(), op, operands, diags);
        }
        NodeOp::Builtin { .. }
        | NodeOp::Const { .. }
        | NodeOp::Output
        | NodeOp::CustomSnippet { .. } => {}
    }
}

/// Validate an [`ExprOp`]'s static shape that does not depend on operand types:
/// the operand **count** (arity), the swizzle **mask** legality envelope, and the
/// construct **target** type. (Operand-*type* checks that need wiring happen in
/// [`check_expr_operand_types`].)
fn check_expr_op_shape(node_id: &str, op: &ExprOp, operands: &[String], diags: &mut Diagnostics) {
    let arity = expr_arity(op);
    if let Some(n) = arity.exact {
        if operands.len() != n {
            diags.push(Diagnostic::error(
                codes::WRONG_ARITY,
                format!(
                    "`{}` takes {} operand(s) but {} were wired",
                    expr_name(op),
                    n,
                    operands.len()
                ),
                node_id,
            ));
        }
    } else if let Some(min) = arity.at_least {
        if operands.len() < min {
            diags.push(Diagnostic::error(
                codes::WRONG_ARITY,
                format!(
                    "`{}` takes at least {} operand(s) but {} were wired",
                    expr_name(op),
                    min,
                    operands.len()
                ),
                node_id,
            ));
        }
    }

    // A Construct's target must be a vector (Float construct is meaningless;
    // sampler is not constructible). The component-sum check needs operand types
    // and happens later.
    if let ExprOp::Construct { ty } = op {
        if !ty.is_vector() {
            diags.push(Diagnostic::error(
                codes::OPERAND_TYPE,
                format!("`construct` target must be a vector type, got {ty:?}"),
                node_id,
            ));
        }
    }
}

/// The arity constraint of an [`ExprOp`].
struct Arity {
    /// An exact operand count, when fixed.
    exact: Option<usize>,
    /// A minimum operand count, when variadic.
    at_least: Option<usize>,
}

const fn fixed(n: usize) -> Arity {
    Arity {
        exact: Some(n),
        at_least: None,
    }
}

fn expr_arity(op: &ExprOp) -> Arity {
    match op {
        // Binary arithmetic + dot/pow/min/max.
        ExprOp::Add
        | ExprOp::Sub
        | ExprOp::Mul
        | ExprOp::Div
        | ExprOp::Min
        | ExprOp::Max
        | ExprOp::Pow
        | ExprOp::Dot => fixed(2),
        // Ternary.
        ExprOp::Mix | ExprOp::Clamp => fixed(3),
        // Unary math + swizzle/normalize/length.
        ExprOp::Sin
        | ExprOp::Cos
        | ExprOp::Abs
        | ExprOp::Floor
        | ExprOp::Fract
        | ExprOp::Normalize
        | ExprOp::Length
        | ExprOp::Swizzle { .. } => fixed(1),
        // Construct is variadic: at least one operand.
        ExprOp::Construct { .. } => Arity {
            exact: None,
            at_least: Some(1),
        },
    }
}

/// A short human name for an [`ExprOp`] used in diagnostic messages.
fn expr_name(op: &ExprOp) -> &'static str {
    match op {
        ExprOp::Add => "add",
        ExprOp::Sub => "sub",
        ExprOp::Mul => "mul",
        ExprOp::Div => "div",
        ExprOp::Mix => "mix",
        ExprOp::Clamp => "clamp",
        ExprOp::Min => "min",
        ExprOp::Max => "max",
        ExprOp::Pow => "pow",
        ExprOp::Sin => "sin",
        ExprOp::Cos => "cos",
        ExprOp::Abs => "abs",
        ExprOp::Floor => "floor",
        ExprOp::Fract => "fract",
        ExprOp::Dot => "dot",
        ExprOp::Normalize => "normalize",
        ExprOp::Length => "length",
        ExprOp::Swizzle { .. } => "swizzle",
        ExprOp::Construct { .. } => "construct",
    }
}

// ----------------------------------------------------------------------------
// Edge structure
// ----------------------------------------------------------------------------

/// The resolved incoming edge feeding a `(node-id, input-port)` pair. Built by
/// [`check_edge_structure`] and reused for dangling-input, type, and operand
/// checks so the graph is walked once.
type Incoming<'g> = BTreeMap<(String, String), &'g IrEdge>;

/// Validate that every edge's endpoints name a real node + a real port (output on
/// the source, input on the target), and that no input port is fed twice. Returns
/// the resolved incoming-edge map for structurally-valid edges.
fn check_edge_structure<'g>(
    graph: &'g IrGraph,
    nodes: &HashMap<&str, &'g IrNode>,
    diags: &mut Diagnostics,
) -> Incoming<'g> {
    let mut incoming: Incoming<'g> = BTreeMap::new();

    for edge in &graph.edges {
        // Source endpoint.
        let source_ok = match nodes.get(edge.source.node.as_str()) {
            None => {
                diags.push(Diagnostic::error(
                    codes::UNKNOWN_NODE,
                    format!("edge source references unknown node `{}`", edge.source.node),
                    &edge.source.node,
                ));
                false
            }
            Some(src) => {
                if has_output_port(&src.op, &edge.source.port) {
                    true
                } else {
                    diags.push(
                        Diagnostic::error(
                            codes::UNKNOWN_PORT,
                            format!(
                                "node `{}` has no output port `{}`",
                                edge.source.node, edge.source.port
                            ),
                            &edge.source.node,
                        )
                        .with_port(&edge.source.port),
                    );
                    false
                }
            }
        };

        // Target endpoint.
        let target_ok = match nodes.get(edge.target.node.as_str()) {
            None => {
                diags.push(Diagnostic::error(
                    codes::UNKNOWN_NODE,
                    format!("edge target references unknown node `{}`", edge.target.node),
                    &edge.target.node,
                ));
                false
            }
            Some(tgt) => {
                if has_input_port(&tgt.op, &edge.target.port) {
                    true
                } else {
                    diags.push(
                        Diagnostic::error(
                            codes::UNKNOWN_PORT,
                            format!(
                                "node `{}` has no input port `{}`",
                                edge.target.node, edge.target.port
                            ),
                            &edge.target.node,
                        )
                        .with_port(&edge.target.port),
                    );
                    false
                }
            }
        };

        if source_ok && target_ok {
            let key = (edge.target.node.clone(), edge.target.port.clone());
            if incoming.insert(key, edge).is_some() {
                diags.push(
                    Diagnostic::error(
                        codes::DUPLICATE_INPUT,
                        format!(
                            "input port `{}` on node `{}` is fed by more than one edge",
                            edge.target.port, edge.target.node
                        ),
                        &edge.target.node,
                    )
                    .with_port(&edge.target.port),
                );
            }
        }
    }

    incoming
}

/// Report any required input port with no incoming edge as `danglingInput`.
fn check_required_inputs(graph: &IrGraph, incoming: &Incoming, diags: &mut Diagnostics) {
    for node in &graph.nodes {
        for (port, required) in input_ports(&node.op) {
            if required && !incoming.contains_key(&(node.id.clone(), port.clone())) {
                diags.push(
                    Diagnostic::error(
                        codes::DANGLING_INPUT,
                        format!(
                            "required input port `{port}` on node `{}` has no incoming edge",
                            node.id
                        ),
                        &node.id,
                    )
                    .with_port(&port),
                );
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Output cardinality & acyclicity
// ----------------------------------------------------------------------------

fn check_output_cardinality(graph: &IrGraph, diags: &mut Diagnostics) {
    let outputs: Vec<&IrNode> = graph
        .nodes
        .iter()
        .filter(|n| matches!(n.op, NodeOp::Output))
        .collect();
    match outputs.len() {
        1 => {}
        0 => diags.push(Diagnostic::error(
            codes::MISSING_OUTPUT,
            "graph has no `Output` node (the final color sink)".to_owned(),
            // No single offending node: attribute to the empty id so the field is
            // still present; the editor treats an empty node id as graph-level.
            String::new(),
        )),
        _ => {
            // Flag each surplus Output node so the editor highlights all of them.
            for node in &outputs {
                diags.push(Diagnostic::error(
                    codes::MULTIPLE_OUTPUTS,
                    format!(
                        "graph has {} `Output` nodes; exactly one is allowed",
                        outputs.len()
                    ),
                    &node.id,
                ));
            }
        }
    }
}

/// Detect cycles via DFS over the edge-implied node dependency graph. Returns the
/// set of node ids that participate in a cycle (empty if acyclic), and pushes a
/// `cycle` diagnostic naming one node on each detected cycle.
fn check_acyclic<'g>(
    graph: &'g IrGraph,
    nodes: &HashMap<&str, &'g IrNode>,
    diags: &mut Diagnostics,
) -> HashSet<String> {
    // Adjacency: source-node -> target-nodes (data flows source -> target).
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in &graph.edges {
        // Only edges with both endpoints present matter for the cycle walk.
        if nodes.contains_key(edge.source.node.as_str())
            && nodes.contains_key(edge.target.node.as_str())
        {
            adj.entry(edge.source.node.as_str())
                .or_default()
                .push(edge.target.node.as_str());
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    let mut marks: HashMap<&str, Mark> = HashMap::new();
    let mut in_cycle: HashSet<String> = HashSet::new();

    // Iterative DFS so deep graphs don't blow the stack.
    for start in graph.nodes.iter().map(|n| n.id.as_str()) {
        if marks.contains_key(start) {
            continue;
        }
        // Stack of (node, next-child-index).
        let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
        marks.insert(start, Mark::Visiting);
        while let Some(&(node, idx)) = stack.last() {
            let children = adj.get(node).map(Vec::as_slice).unwrap_or(&[]);
            if idx < children.len() {
                stack.last_mut().unwrap().1 += 1;
                let child = children[idx];
                match marks.get(child) {
                    Some(Mark::Visiting) => {
                        // Back edge: `child` is an ancestor on the stack → cycle.
                        in_cycle.insert(child.to_owned());
                    }
                    Some(Mark::Done) => {}
                    None => {
                        marks.insert(child, Mark::Visiting);
                        stack.push((child, 0));
                    }
                }
            } else {
                marks.insert(node, Mark::Done);
                stack.pop();
            }
        }
    }

    // Emit one diagnostic per node identified on a cycle (deterministic order).
    let mut sorted: Vec<&String> = in_cycle.iter().collect();
    sorted.sort();
    for id in sorted {
        diags.push(Diagnostic::error(
            codes::CYCLE,
            format!("node `{id}` participates in a cycle; the graph must be acyclic"),
            id,
        ));
    }

    in_cycle
}

// ----------------------------------------------------------------------------
// Type inference + edge/operand type checks
// ----------------------------------------------------------------------------

/// Memoized output-port type inference over an **acyclic** graph. The output type
/// of an [`Expr`](NodeOp::Expr) (and a [`Swizzle`](ExprOp::Swizzle)) depends on
/// its operand types, so inference recurses through incoming edges; results are
/// cached per `(node, port)`. Returns `None` for a port whose type cannot be
/// determined (an upstream error already reported it).
struct TypeInferer<'g> {
    nodes: &'g HashMap<&'g str, &'g IrNode>,
    incoming: &'g Incoming<'g>,
    cache: HashMap<(String, String), Option<PortType>>,
}

impl<'g> TypeInferer<'g> {
    fn new(nodes: &'g HashMap<&'g str, &'g IrNode>, incoming: &'g Incoming<'g>) -> Self {
        Self {
            nodes,
            incoming,
            cache: HashMap::new(),
        }
    }

    /// The type carried by `node_id`'s output port `port`, or `None` if it cannot
    /// be inferred.
    fn output_type(&mut self, node_id: &str, port: &str) -> Option<PortType> {
        let key = (node_id.to_owned(), port.to_owned());
        if let Some(cached) = self.cache.get(&key) {
            return *cached;
        }
        // Guard against unexpected re-entrancy (the graph is acyclic by the time
        // we infer, but be defensive): seed the cache with None first.
        self.cache.insert(key.clone(), None);
        let node = self.nodes.get(node_id)?;
        let ty = self.infer_output(node, port);
        self.cache.insert(key, ty);
        ty
    }

    /// The type flowing **into** `node_id`'s input port `port` (the source output
    /// type of the edge feeding it), or `None` if unfed / unresolvable.
    fn input_type(&mut self, node_id: &str, port: &str) -> Option<PortType> {
        let edge = self.incoming.get(&(node_id.to_owned(), port.to_owned()))?;
        self.output_type(&edge.source.node, &edge.source.port)
    }

    fn infer_output(&mut self, node: &IrNode, port: &str) -> Option<PortType> {
        match &node.op {
            // Sample always produces vec4 on `out`.
            NodeOp::Sample { .. } => (port == "out").then_some(PortType::Vec4),
            NodeOp::Builtin { semantic } => {
                if port == "out" {
                    semantic.port_type()
                } else {
                    None
                }
            }
            NodeOp::Param { .. } => (port == "out").then_some(PortType::Float),
            NodeOp::Const { value } => (port == "out").then_some(value.port_type()),
            NodeOp::Output => None,
            NodeOp::CustomSnippet { outputs, .. } => {
                outputs.iter().find(|p| p.name == port).map(|p| p.ty)
            }
            NodeOp::Expr { op, operands } => {
                if port != "out" {
                    return None;
                }
                self.infer_expr_result(node.id.as_str(), op, operands)
            }
        }
    }

    /// The result type of an [`Expr`](NodeOp::Expr) given its operands' inbound
    /// types (Spec §8.2).
    fn infer_expr_result(
        &mut self,
        node_id: &str,
        op: &ExprOp,
        operands: &[String],
    ) -> Option<PortType> {
        // Resolve operand types (in declared order).
        let types: Vec<Option<PortType>> = operands
            .iter()
            .map(|p| self.input_type(node_id, p))
            .collect();

        match op {
            // Component-wise binary/ternary arithmetic: result is the widest
            // (most-components) numeric operand type; scalars broadcast.
            ExprOp::Add
            | ExprOp::Sub
            | ExprOp::Mul
            | ExprOp::Div
            | ExprOp::Min
            | ExprOp::Max
            | ExprOp::Pow
            | ExprOp::Mix
            | ExprOp::Clamp => widest_numeric(&types),
            // dot -> float; length -> float.
            ExprOp::Dot | ExprOp::Length => Some(PortType::Float),
            // Unary component-wise math preserves the operand type.
            ExprOp::Sin | ExprOp::Cos | ExprOp::Abs | ExprOp::Floor | ExprOp::Fract => {
                types.first().copied().flatten()
            }
            // normalize preserves the (vector) operand type.
            ExprOp::Normalize => types.first().copied().flatten(),
            ExprOp::Swizzle { mask } => {
                let base = types.first().copied().flatten()?;
                base.swizzle_result(mask)
            }
            ExprOp::Construct { ty } => Some(*ty),
        }
    }
}

/// The widest float type among `types` (the one with the most components),
/// treating `Int` as `Float` for promotion. `None` if any operand is missing or
/// non-numeric (an error is reported elsewhere).
fn widest_numeric(types: &[Option<PortType>]) -> Option<PortType> {
    let mut best = 0u8;
    for t in types {
        let t = (*t)?;
        if !t.is_numeric() {
            return None;
        }
        best = best.max(t.component_count()?);
    }
    PortType::float_with_components(best)
}

/// Validate every structurally-valid edge: the source output type must be
/// [`assignable_to`](PortType::assignable_to) the sink input port type.
fn check_edge_types(
    graph: &IrGraph,
    nodes: &HashMap<&str, &IrNode>,
    inferer: &mut TypeInferer,
    diags: &mut Diagnostics,
) {
    for edge in &graph.edges {
        // Skip edges whose endpoints were already flagged as unknown.
        let Some(src) = nodes.get(edge.source.node.as_str()) else {
            continue;
        };
        let Some(tgt) = nodes.get(edge.target.node.as_str()) else {
            continue;
        };
        if !has_output_port(&src.op, &edge.source.port)
            || !has_input_port(&tgt.op, &edge.target.port)
        {
            continue;
        }
        let (Some(src_ty), Some(tgt_ty)) = (
            inferer.output_type(&edge.source.node, &edge.source.port),
            input_port_type(&tgt.op, &edge.target.port, inferer, &edge.target.node),
        ) else {
            continue;
        };
        if !src_ty.assignable_to(tgt_ty) {
            diags.push(
                Diagnostic::error(
                    codes::TYPE_MISMATCH,
                    format!(
                        "value of type {src_ty:?} is not assignable to input port `{}` of type \
                         {tgt_ty:?} on node `{}`",
                        edge.target.port, edge.target.node
                    ),
                    &edge.target.node,
                )
                .with_port(&edge.target.port),
            );
        }
    }
}

/// The declared type of an **input** port. For ops with fixed-typed inputs this
/// is statically known ([`Sample`](NodeOp::Sample) `coord` = `vec2`,
/// [`Output`](NodeOp::Output) `color` = `vec4`, a [`CustomSnippet`] port = its
/// declared type). For an [`Expr`](NodeOp::Expr) operand the "expected" type is
/// the operand's own inferred inbound type (the operator is polymorphic), so edge
/// type-checking against an Expr operand always passes the structural assignment
/// (the *operand-type constraints* are checked separately in
/// [`check_expr_operand_types`]); we return the inbound type so the assignment is
/// reflexive.
fn input_port_type(
    op: &NodeOp,
    port: &str,
    inferer: &mut TypeInferer,
    node_id: &str,
) -> Option<PortType> {
    match op {
        NodeOp::Sample { .. } if port == "coord" => Some(PortType::Vec2),
        NodeOp::Output if port == "color" => Some(PortType::Vec4),
        NodeOp::CustomSnippet { inputs, .. } => {
            inputs.iter().find(|p| p.name == port).map(|p| p.ty)
        }
        // Expr operands are polymorphic; the type *constraints* are validated by
        // check_expr_operand_types, not by the structural edge check. Return the
        // inbound type so the structural assignment is trivially satisfied.
        NodeOp::Expr { .. } => inferer.input_type(node_id, port),
        _ => None,
    }
}

/// Validate per-[`ExprOp`] operand **type** constraints that the structural edge
/// check cannot express: arithmetic operands must be numeric and component-count
/// compatible; `dot`/`length`/`normalize` need a vector; `swizzle` needs a legal
/// mask for the operand type; `construct`'s components must sum to the target.
fn check_expr_operand_types(
    graph: &IrGraph,
    incoming: &Incoming,
    inferer: &mut TypeInferer,
    diags: &mut Diagnostics,
) {
    for node in &graph.nodes {
        let NodeOp::Expr { op, operands } = &node.op else {
            continue;
        };
        // Resolve only the operands that are actually fed (a dangling operand is
        // reported elsewhere; don't double-report).
        let resolved: Vec<Option<PortType>> = operands
            .iter()
            .map(|p| {
                incoming
                    .get(&(node.id.clone(), p.clone()))
                    .and_then(|_| inferer.input_type(&node.id, p))
            })
            .collect();

        match op {
            ExprOp::Add
            | ExprOp::Sub
            | ExprOp::Mul
            | ExprOp::Div
            | ExprOp::Min
            | ExprOp::Max
            | ExprOp::Pow
            | ExprOp::Mix
            | ExprOp::Clamp => {
                check_componentwise_operands(&node.id, expr_name(op), &resolved, diags);
            }
            ExprOp::Dot => {
                // Both operands must be the same vector type.
                let present: Vec<PortType> = resolved.iter().filter_map(|t| *t).collect();
                if present.len() == 2 {
                    let (a, b) = (present[0], present[1]);
                    if !a.is_vector() || !b.is_vector() || a != b {
                        diags.push(Diagnostic::error(
                            codes::OPERAND_TYPE,
                            format!(
                                "`dot` requires two operands of the same vector type, got \
                                 {a:?} and {b:?}"
                            ),
                            &node.id,
                        ));
                    }
                }
            }
            ExprOp::Normalize | ExprOp::Length => {
                if let Some(t) = resolved.first().copied().flatten() {
                    if !t.is_vector() {
                        diags.push(Diagnostic::error(
                            codes::OPERAND_TYPE,
                            format!("`{}` requires a vector operand, got {t:?}", expr_name(op)),
                            &node.id,
                        ));
                    }
                }
            }
            ExprOp::Sin | ExprOp::Cos | ExprOp::Abs | ExprOp::Floor | ExprOp::Fract => {
                if let Some(t) = resolved.first().copied().flatten() {
                    if !t.is_numeric() {
                        diags.push(Diagnostic::error(
                            codes::OPERAND_TYPE,
                            format!("`{}` requires a numeric operand, got {t:?}", expr_name(op)),
                            &node.id,
                        ));
                    }
                }
            }
            ExprOp::Swizzle { mask } => {
                if let Some(t) = resolved.first().copied().flatten() {
                    if t.swizzle_result(mask).is_none() {
                        diags.push(Diagnostic::error(
                            codes::ILLEGAL_SWIZZLE,
                            format!("swizzle mask `.{mask}` is illegal for operand type {t:?}"),
                            &node.id,
                        ));
                    }
                }
            }
            ExprOp::Construct { ty } => {
                check_construct_operands(&node.id, *ty, &resolved, diags);
            }
        }
    }
}

/// Component-wise arithmetic: every operand must be numeric, and each must
/// broadcast to (or already match the component count of) the widest operand —
/// i.e. either a scalar or the same vector width. A `vec2` mixed with a `vec3` is
/// illegal (no implicit vector widening).
fn check_componentwise_operands(
    node_id: &str,
    name: &str,
    resolved: &[Option<PortType>],
    diags: &mut Diagnostics,
) {
    let present: Vec<PortType> = resolved.iter().filter_map(|t| *t).collect();
    // Non-numeric operand?
    if let Some(bad) = present.iter().find(|t| !t.is_numeric()) {
        diags.push(Diagnostic::error(
            codes::OPERAND_TYPE,
            format!("`{name}` operands must be numeric, got {bad:?}"),
            node_id,
        ));
        return;
    }
    // The set of distinct vector widths among operands (scalars excluded — they
    // broadcast). More than one distinct vector width is a mismatch.
    let mut vec_widths: Vec<u8> = present
        .iter()
        .filter(|t| t.is_vector())
        .filter_map(|t| t.component_count())
        .collect();
    vec_widths.sort_unstable();
    vec_widths.dedup();
    if vec_widths.len() > 1 {
        diags.push(Diagnostic::error(
            codes::OPERAND_TYPE,
            format!(
                "`{name}` operands have incompatible vector widths {vec_widths:?}; mixing \
                 different vector sizes needs an explicit construct"
            ),
            node_id,
        ));
    }
}

/// `construct { ty }`: every operand must be numeric, and the operands' component
/// counts must either **sum exactly** to the target vector's component count
/// (e.g. `vec4(vec3, float)`) **or** be a single scalar that broadcasts to fill
/// every component (GLSL `vec4(x)`).
fn check_construct_operands(
    node_id: &str,
    ty: PortType,
    resolved: &[Option<PortType>],
    diags: &mut Diagnostics,
) {
    // If the target isn't a vector, the shape check already reported it.
    let Some(target) = ty.component_count().filter(|_| ty.is_vector()) else {
        return;
    };
    // Only validate the sum when every operand resolved (a dangling operand is
    // reported separately).
    if resolved.iter().any(Option::is_none) {
        return;
    }
    let mut sum = 0u8;
    for t in resolved.iter().flatten() {
        if !t.is_numeric() {
            diags.push(Diagnostic::error(
                codes::OPERAND_TYPE,
                format!("`construct` operands must be numeric, got {t:?}"),
                node_id,
            ));
            return;
        }
        sum += t.component_count().unwrap_or(0);
    }
    // A single scalar operand broadcasts to fill all components (GLSL `vec4(x)`).
    let single_scalar_broadcast = resolved.len() == 1
        && resolved
            .first()
            .copied()
            .flatten()
            .is_some_and(PortType::is_scalar);
    if sum != target && !single_scalar_broadcast {
        diags.push(Diagnostic::error(
            codes::OPERAND_TYPE,
            format!(
                "`construct` of {ty:?} needs operands summing to {target} components (or a single \
                 broadcast scalar), got {sum}"
            ),
            node_id,
        ));
    }
}
