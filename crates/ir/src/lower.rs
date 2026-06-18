//! **Lowering** a type-checked [`IrGraph`] into a linear, SSA-form
//! [`LoweredIr`] plus a [`PassManifest`] (#41).
//!
//! This is the bridge between the type checker (#40) and the slang emitter
//! (#42). Where [`check`](crate::check) validates a graph, [`lower`] consumes a
//! graph that type-checked **clean** and produces the codegen-facing linear
//! form:
//!
//! 1. **Topological sort** of the DAG, starting from the [`Output`](NodeOp::Output)
//!    node's transitive inputs. Only nodes *reachable* from `Output` are emitted —
//!    unreached (dead) nodes are dropped. Ties between independent nodes are broken
//!    by a stable key (the node id) so the order is reproducible across runs.
//! 2. **SSA temp allocation**: every producing op gets a unique [`TempId`] carrying
//!    its [`PortType`]. Statements are ordered so every operand temp is defined
//!    before it is used (a valid SSA linearization of the DAG).
//! 3. **Manifest collection**: the deduplicated, deterministically-ordered set of
//!    `#pragma parameter`s ([`Param`](NodeOp::Param) ops), builtin uniforms
//!    ([`Builtin`](NodeOp::Builtin) ops), and sampler bindings + sampled RetroArch
//!    textures ([`Sample`](NodeOp::Sample) ops) the pass needs.
//!
//! ## Precondition: a clean graph
//!
//! [`lower`] is **only** defined for a graph with no blocking
//! [`Diagnostic`](core_model::ir::Diagnostic)s. It enforces this by running the
//! checker itself and refusing (returning [`LowerError::TypeErrors`]) when the
//! graph has errors, so a type-invalid graph can never silently lower to garbage.
//!
//! ## These types are codegen-internal
//!
//! [`LoweredIr`], [`SsaStmt`], [`LoweredOp`], [`TempId`], and [`PassManifest`] are
//! plain Rust consumed by `codegen-slang` (#42) — they are deliberately **not**
//! `#[ts(export)]` (the editor never sees the SSA form; it sees the
//! [`IrGraph`] and the [`Diagnostics`](core_model::ir::Diagnostics)).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use core_model::ir::{
    BuiltinSemantic, ConstValue, ExprOp, IrEdge, IrGraph, IrNode, NodeOp, PortDecl, PortType,
    TextureSource,
};

use crate::{check, CheckContext};

/// A unique SSA temporary id. Each value-producing op in the lowered IR writes
/// exactly one temp; the emitter (#42) names them deterministically (e.g. `t0`,
/// `t1`, …). Ids are assigned in emission order, so a `TempId` is also a stable
/// index into [`LoweredIr::stmts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TempId(pub u32);

impl std::fmt::Display for TempId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t{}", self.0)
    }
}

/// The lowered counterpart of a value-producing [`NodeOp`]. Carries exactly the
/// intrinsic configuration the emitter needs; operands are referenced positionally
/// via [`SsaStmt::operands`] (the operand temp ids in op order), so the wiring
/// lives on the statement, not the op.
///
/// Note there is no `Output` variant — the final color write is represented by
/// [`LoweredIr::output`] (the temp feeding `FragColor`), not an SSA statement.
#[derive(Debug, Clone, PartialEq)]
pub enum LoweredOp {
    /// Sample a RetroArch texture (operand 0 is the `vec2` coord temp). The
    /// sampler this reads from is the [`SamplerBinding`] with this `texture`.
    Sample {
        /// Which RetroArch texture is sampled.
        texture: TextureSource,
    },
    /// Read a builtin-semantic uniform (no operands).
    Builtin {
        /// Which reserved RetroArch semantic.
        semantic: BuiltinSemantic,
    },
    /// Read a declared `#pragma parameter` (no operands).
    Param {
        /// The parameter identifier.
        name: String,
    },
    /// A typed literal constant (no operands).
    Const {
        /// The literal value (and its type).
        value: ConstValue,
    },
    /// An intrinsic expression over its operand temps (in op order).
    Expr {
        /// The intrinsic applied.
        op: ExprOp,
    },
    /// A verbatim GLSL snippet, lowered to a call of a generated **wrapper
    /// function** (#43). Operands are the temps feeding `inputs` (in the declared
    /// input-port order); `node_id`/`inputs`/`outputs`/`body` are carried verbatim
    /// for the emitter to (a) emit one `snippet_<node_id>(...)` function whose
    /// body is the snippet — so its locals live in their own scope and cannot
    /// collide with `main` or another snippet — and (b) call it from the SSA
    /// stream. Lowering emits one [`SsaStmt`] per declared output port (a snippet
    /// may assign several); this statement's result temp is its `result_port`.
    CustomSnippet {
        /// The stable id of the originating [`NodeOp::CustomSnippet`] node — the
        /// emitter derives the unique wrapper function name (`snippet_<node_id>`)
        /// from it so two snippets never share a function and their locals never
        /// collide.
        node_id: String,
        /// The GLSL statement body, verbatim. Reads its input ports by name (the
        /// wrapper's `in` parameters) and assigns its output ports by name (the
        /// wrapper's `out` parameters).
        body: String,
        /// Declared typed input ports, in the same order as [`SsaStmt::operands`]
        /// — the wrapper function's `in` parameters (name + type).
        inputs: Vec<PortDecl>,
        /// All declared typed output ports (a snippet may assign several) — the
        /// wrapper function's `out` parameters. This statement's result temp is
        /// the [`result_port`](LoweredOp::CustomSnippet::result_port) output.
        outputs: Vec<PortDecl>,
        /// The output port name this statement's result temp corresponds to.
        result_port: String,
    },
}

/// A single SSA statement in the lowered IR: a result temp of a known type,
/// produced by a [`LoweredOp`] applied to its operand temps (in op order).
///
/// Statements appear in [`LoweredIr::stmts`] in a valid topological order: every
/// temp in [`operands`](SsaStmt::operands) is the [`result`](SsaStmt::result) of
/// an earlier statement.
#[derive(Debug, Clone, PartialEq)]
pub struct SsaStmt {
    /// The temp this statement defines.
    pub result: TempId,
    /// The type of the produced value.
    pub ty: PortType,
    /// The operation producing the value.
    pub op: LoweredOp,
    /// The operand temps, in op order (empty for source ops).
    pub operands: Vec<TempId>,
}

/// A required `#pragma parameter`, in the manifest. Carries the name and, when the
/// declaring [`Parameter`](core_model::Parameter) could be resolved against the
/// [`CheckContext`]/[`LowerContext`], whether it was declared (so the emitter can
/// decide to emit a stub `#pragma parameter` line or trust an external decl).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamRequirement {
    /// The parameter identifier (matches a [`NodeOp::Param`] `name`).
    pub name: String,
}

/// A sampler the pass binds: the RetroArch [`TextureSource`] it reads and the
/// `layout(set=0, binding=N)` slot the generator assigns it. Bindings are assigned
/// deterministically (see [`PassManifest`] ordering) so the emitted layout is
/// reproducible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplerBinding {
    /// The RetroArch texture this sampler reads.
    pub texture: TextureSource,
    /// The assigned `layout(set=0, binding=N)` index.
    pub binding: u32,
}

/// The manifest of resources a lowered pass requires (#41): the ordered,
/// deduplicated `#pragma parameter`s, builtin uniforms, sampler bindings, and the
/// set of RetroArch textures sampled. Everything is **deterministically ordered**
/// so the emitter (#42) produces a reproducible binding layout.
///
/// ### Ordering rule (deterministic, documented)
///
/// All four collections are sorted by a **stable key independent of graph
/// traversal order**, so two runs over the same graph — and two graphs that differ
/// only in node ordering — produce byte-identical manifests:
///
/// - **`parameters`**: sorted by parameter name (`BTreeSet` insert).
/// - **`builtins`**: sorted by the semantic's discriminant order.
/// - **`textures`** (the sampled set): sorted by a canonical
///   [`texture_sort_key`] (kind, then index/name).
/// - **`samplers`**: one per distinct sampled `texture`, in the same canonical
///   texture order, with `binding` assigned `0, 1, 2, …` in that order.
///
/// Sorting by a content key (rather than first-use order) was chosen because it is
/// trivially reproducible and order-insensitive to how the hand-built graph
/// happens to list its nodes — the property the snapshot test asserts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PassManifest {
    /// Required `#pragma parameter`s, sorted by name.
    pub parameters: Vec<ParamRequirement>,
    /// Builtin-semantic uniforms read, sorted by semantic.
    pub builtins: Vec<BuiltinSemantic>,
    /// Sampler bindings, one per distinct sampled texture, in canonical texture
    /// order with sequential `binding` indices.
    pub samplers: Vec<SamplerBinding>,
    /// The deduplicated set of RetroArch textures sampled, in canonical order.
    /// (Mirrors the `texture` field of each [`SamplerBinding`]; kept as its own
    /// field because the "what textures does this pass sample" question drives
    /// future automatic pipeline wiring independent of binding assignment.)
    pub textures: Vec<TextureSource>,
}

/// The fully lowered, linear form of one pass: the SSA statements (in topological
/// order) plus the resource manifest. The emitter (#42) walks [`stmts`] in order,
/// emitting one slang statement per [`SsaStmt`], then writes
/// [`output`](LoweredIr::output) to `FragColor`.
#[derive(Debug, Clone, PartialEq)]
pub struct LoweredIr {
    /// The SSA statements, topologically ordered (operands precede uses).
    pub stmts: Vec<SsaStmt>,
    /// The temp whose value is written to `FragColor` (the `Output.color` source).
    pub output: TempId,
    /// The [`PortType`] of the [`output`](LoweredIr::output) temp. `FragColor` is
    /// a `vec4`, so when this is a scalar (`Float`/`Int`) the emitter broadcasts it
    /// with `FragColor = vec4(<temp>);` (the documented `Float→vecN` scalar-color
    /// shorthand the type checker honors on `Output.color`); when it is already
    /// `Vec4` the emitter writes it verbatim. Carried here so the emitter knows the
    /// source type at the write site without re-scanning the SSA stream.
    pub output_ty: PortType,
    /// The resource manifest (params/builtins/samplers/textures).
    pub manifest: PassManifest,
}

/// Why [`lower`] refused a graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// The graph did not type-check clean — lowering is undefined on a graph with
    /// blocking errors. Carries the offending diagnostic codes (in checker order)
    /// for diagnostics; the full set is available by calling [`check`] directly.
    TypeErrors(Vec<String>),
    /// The graph type-checked clean but had no reachable `Output` (defensive — the
    /// checker should have reported `missingOutput`, so this is unreachable in
    /// practice; kept so lowering never panics).
    NoOutput,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LowerError::TypeErrors(codes) => write!(
                f,
                "graph has {} blocking type error(s) and cannot be lowered: [{}]",
                codes.len(),
                codes.join(", ")
            ),
            LowerError::NoOutput => write!(f, "graph has no reachable `Output` node to lower from"),
        }
    }
}

impl std::error::Error for LowerError {}

/// The declared parameters/LUTs a graph's references resolve against, shared with
/// the type checker. Lowering reuses [`CheckContext`] verbatim — it is the same
/// precondition surface — so callers build one context and pass it to both
/// [`check`] and [`lower`].
pub type LowerContext = CheckContext;

/// Lower a **type-checked** `graph` into its linear [`LoweredIr`] + [`PassManifest`].
///
/// Lowering first runs [`check`] internally and **refuses** ([`LowerError::TypeErrors`])
/// if the graph has any blocking error, so a type-invalid graph never silently
/// lowers. On a clean graph it:
///
/// 1. topologically orders the nodes reachable from the `Output` sink (dead nodes
///    dropped; ties broken by node id for determinism),
/// 2. allocates one SSA [`TempId`] per producing op in that order, typed via the
///    same inference the checker uses,
/// 3. collects the deterministically-ordered [`PassManifest`].
pub fn lower(graph: &IrGraph, ctx: &LowerContext) -> Result<LoweredIr, LowerError> {
    // Precondition: the graph must type-check clean. We run the checker rather
    // than trusting the caller, so a type-invalid graph can never lower to garbage.
    let diags = check(graph, ctx);
    if diags.has_errors() {
        let codes = diags
            .iter()
            .filter(|d| d.severity == core_model::ir::DiagnosticSeverity::Error)
            .map(|d| d.code.clone())
            .collect();
        return Err(LowerError::TypeErrors(codes));
    }

    let index = GraphIndex::new(graph);
    let output_id = index.output_id().ok_or(LowerError::NoOutput)?;

    // Topologically order the nodes reachable from Output's transitive inputs.
    let order = topo_order_from_output(&index, output_id);

    // Allocate SSA temps + build statements in that order, inferring each result
    // type. A node's output temp is recorded so later operands resolve to it.
    let mut builder = Lowerer::new(&index);
    for node_id in &order {
        builder.emit_node(node_id);
    }

    // The Output's `color` input is fed by exactly one edge (the checker enforced
    // that); its source temp is what we write to FragColor.
    let output = builder
        .input_temp(output_id, "color")
        .ok_or(LowerError::NoOutput)?;
    // The type of that source temp, so the emitter can broadcast a scalar source
    // into the vec4 `FragColor` (the `Float→vecN` shorthand) and leave a vec4
    // source verbatim. The producing statement is guaranteed present in `stmts`.
    let output_ty = builder
        .stmts
        .iter()
        .find(|s| s.result == output)
        .map(|s| s.ty)
        .unwrap_or(PortType::Vec4);

    let manifest = collect_manifest(&builder);

    Ok(LoweredIr {
        stmts: builder.stmts,
        output,
        output_ty,
        manifest,
    })
}

// ----------------------------------------------------------------------------
// Graph indexing
// ----------------------------------------------------------------------------

/// A by-id node index + resolved incoming-edge map, computed once and reused by
/// the topo-sort, SSA emission, and type inference. Built only after the graph
/// type-checked clean, so every edge endpoint is known-valid and each input port
/// has at most one incoming edge.
struct GraphIndex<'g> {
    nodes: HashMap<&'g str, &'g IrNode>,
    /// `(target-node-id, target-port) -> source PortRef` for each wired input.
    incoming: BTreeMap<(&'g str, &'g str), &'g IrEdge>,
}

impl<'g> GraphIndex<'g> {
    fn new(graph: &'g IrGraph) -> Self {
        let mut nodes = HashMap::with_capacity(graph.nodes.len());
        for node in &graph.nodes {
            nodes.insert(node.id.as_str(), node);
        }
        let mut incoming = BTreeMap::new();
        for edge in &graph.edges {
            incoming.insert((edge.target.node.as_str(), edge.target.port.as_str()), edge);
        }
        Self { nodes, incoming }
    }

    fn node(&self, id: &str) -> Option<&'g IrNode> {
        self.nodes.get(id).copied()
    }

    /// The (single) reachable `Output` node's id. The checker guaranteed exactly
    /// one; if (defensively) several exist, the lexicographically-smallest id wins
    /// so the choice is deterministic.
    fn output_id(&self) -> Option<&'g str> {
        self.nodes
            .values()
            .filter(|n| matches!(n.op, NodeOp::Output))
            .map(|n| n.id.as_str())
            .min()
    }

    /// The source [`PortRef`] feeding `node_id`'s input `port`, if wired.
    fn input_source(&self, node_id: &str, port: &str) -> Option<&'g IrEdge> {
        self.incoming.get(&(node_id, port)).copied()
    }
}

/// The **ordered** input-port names a node consumes (operands, in op order). These
/// are the edges the topo-sort follows and the operand order SSA emission uses;
/// kept in sync with the type checker's `input_ports`.
fn ordered_input_ports(op: &NodeOp) -> Vec<String> {
    match op {
        NodeOp::Sample { .. } => vec!["coord".to_owned()],
        NodeOp::Builtin { .. } | NodeOp::Param { .. } | NodeOp::Const { .. } => Vec::new(),
        NodeOp::Expr { operands, .. } => operands.clone(),
        NodeOp::Output => vec!["color".to_owned()],
        NodeOp::CustomSnippet { inputs, .. } => inputs.iter().map(|p| p.name.clone()).collect(),
    }
}

// ----------------------------------------------------------------------------
// Topological sort (from Output, dead-node-dropping, deterministic)
// ----------------------------------------------------------------------------

/// Topologically order the nodes **reachable from `output_id`'s transitive
/// inputs**, with the `Output` node last. Nodes not reachable from Output are
/// dead and dropped (never appear in the result).
///
/// Determinism: at each node we visit its input edges in op order, and an
/// iterative post-order DFS appends a node only after all its dependencies. To
/// make the order independent of how the graph happens to list edges/nodes, the
/// DFS visits dependency nodes sorted by id. The result is a stable, reproducible
/// linearization in which every dependency precedes its dependents.
fn topo_order_from_output<'g>(index: &GraphIndex<'g>, output_id: &'g str) -> Vec<&'g str> {
    // Build the dependency adjacency for reachable nodes only: node -> the set of
    // nodes feeding its inputs (its data dependencies). Sorted (BTreeSet) so the
    // DFS child order is stable.
    //
    // Post-order DFS over dependencies yields dependencies-before-dependents.
    let mut order: Vec<&'g str> = Vec::new();
    let mut state: HashMap<&'g str, Visit> = HashMap::new();

    // Iterative post-order DFS. Stack frames carry the node and an index into its
    // (sorted) dependency list.
    let mut stack: Vec<(&'g str, Vec<&'g str>, usize)> = Vec::new();

    let deps_of = |node_id: &'g str| -> Vec<&'g str> {
        let Some(node) = index.node(node_id) else {
            return Vec::new();
        };
        let mut deps: BTreeSet<&'g str> = BTreeSet::new();
        for port in ordered_input_ports(&node.op) {
            if let Some(edge) = index.input_source(node_id, &port) {
                // The source node is a dependency (edges only ever reference real
                // nodes after a clean check, but be defensive via the index).
                if let Some(src) = index.node(edge.source.node.as_str()) {
                    deps.insert(src.id.as_str());
                }
            }
        }
        deps.into_iter().collect()
    };

    state.insert(output_id, Visit::Active);
    stack.push((output_id, deps_of(output_id), 0));

    while let Some(frame) = stack.last_mut() {
        let (node_id, deps, idx) = (frame.0, &frame.1, &mut frame.2);
        if *idx < deps.len() {
            let dep = deps[*idx];
            *idx += 1;
            match state.get(dep) {
                Some(Visit::Done) => {}
                Some(Visit::Active) => {
                    // A back edge would be a cycle — impossible on a clean graph
                    // (the checker rejected cycles), so skip defensively.
                }
                None => {
                    state.insert(dep, Visit::Active);
                    let dep_deps = deps_of(dep);
                    stack.push((dep, dep_deps, 0));
                }
            }
        } else {
            // All dependencies emitted: this node is ready (post-order append).
            state.insert(node_id, Visit::Done);
            order.push(node_id);
            stack.pop();
        }
    }

    // Sanity: the iterative DFS appends Output last (post-order from it).
    debug_assert_eq!(order.last().copied(), Some(output_id));
    order
}

#[derive(Clone, Copy, PartialEq)]
enum Visit {
    Active,
    Done,
}

// ----------------------------------------------------------------------------
// SSA emission
// ----------------------------------------------------------------------------

/// Walks the topologically-ordered nodes, allocating one SSA temp per value-
/// producing op and recording the `(node, port) -> temp` map so later operands
/// resolve. Also accumulates the raw manifest inputs (params/builtins/textures)
/// as it visits ops, which [`collect_manifest`] then orders.
struct Lowerer<'g> {
    index: &'g GraphIndex<'g>,
    next_temp: u32,
    /// The SSA temp produced by each node's output port.
    out_temps: HashMap<(&'g str, String), TempId>,
    /// Emitted statements, in topological (emission) order.
    stmts: Vec<SsaStmt>,
    // Raw manifest inputs, accumulated as a deduplicated set in visit order;
    // `collect_manifest` imposes the final deterministic ordering. `BuiltinSemantic`
    // and `TextureSource` are not `Ord`, so we dedup by membership over a `Vec`
    // rather than a `BTreeSet` (param names *are* `Ord`, so they use a `BTreeSet`).
    seen_params: BTreeSet<String>,
    seen_builtins: Vec<BuiltinSemantic>,
    seen_textures: Vec<TextureSource>,
}

impl<'g> Lowerer<'g> {
    fn new(index: &'g GraphIndex<'g>) -> Self {
        Self {
            index,
            next_temp: 0,
            out_temps: HashMap::new(),
            stmts: Vec::new(),
            seen_params: BTreeSet::new(),
            seen_builtins: Vec::new(),
            seen_textures: Vec::new(),
        }
    }

    fn fresh(&mut self) -> TempId {
        let id = TempId(self.next_temp);
        self.next_temp += 1;
        id
    }

    /// The temp feeding `node_id`'s input `port` (resolved through its incoming
    /// edge to the source op's output temp).
    fn input_temp(&self, node_id: &str, port: &str) -> Option<TempId> {
        let edge = self.index.input_source(node_id, port)?;
        self.out_temps
            .get(&(edge.source.node.as_str(), edge.source.port.clone()))
            .copied()
    }

    /// The result temp's type for a node's output port — recomputed from operand
    /// temps' recorded types (every operand was emitted earlier in topo order).
    fn output_type(&self, node: &IrNode, port: &str) -> PortType {
        match &node.op {
            NodeOp::Sample { .. } => PortType::Vec4,
            NodeOp::Builtin { semantic } => semantic.port_type().unwrap_or(PortType::Int),
            NodeOp::Param { .. } => PortType::Float,
            NodeOp::Const { value } => value.port_type(),
            NodeOp::CustomSnippet { outputs, .. } => outputs
                .iter()
                .find(|p| p.name == port)
                .map(|p| p.ty)
                .unwrap_or(PortType::Vec4),
            NodeOp::Output => PortType::Vec4,
            NodeOp::Expr { op, operands } => self.expr_result_type(node.id.as_str(), op, operands),
        }
    }

    /// The result type of an `Expr` from its operand temps' types (mirrors the
    /// checker's inference, but reading the already-allocated operand temp types).
    fn expr_result_type(&self, node_id: &str, op: &ExprOp, operands: &[String]) -> PortType {
        let operand_ty = |port: &str| -> Option<PortType> {
            let t = self.input_temp(node_id, port)?;
            self.stmts.iter().find(|s| s.result == t).map(|s| s.ty)
        };
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
                // Widest numeric operand (scalars broadcast).
                let mut best = 0u8;
                for port in operands {
                    if let Some(t) = operand_ty(port) {
                        best = best.max(t.component_count().unwrap_or(1));
                    }
                }
                PortType::float_with_components(best).unwrap_or(PortType::Float)
            }
            ExprOp::Dot | ExprOp::Length => PortType::Float,
            ExprOp::Sin
            | ExprOp::Cos
            | ExprOp::Abs
            | ExprOp::Floor
            | ExprOp::Fract
            | ExprOp::Normalize => operands
                .first()
                .and_then(|p| operand_ty(p))
                .unwrap_or(PortType::Float),
            ExprOp::Swizzle { mask } => operands
                .first()
                .and_then(|p| operand_ty(p))
                .and_then(|t| t.swizzle_result(mask))
                .unwrap_or(PortType::Float),
            ExprOp::Construct { ty } => *ty,
        }
    }

    /// Emit SSA statement(s) for the node and record its output temp(s).
    fn emit_node(&mut self, node_id: &'g str) {
        let Some(node) = self.index.node(node_id) else {
            return;
        };
        match &node.op {
            // Output is the sink — no statement, no temp.
            NodeOp::Output => {}
            NodeOp::Const { value } => {
                let result = self.fresh();
                let ty = value.port_type();
                self.record_out(node_id, "out", result);
                self.stmts.push(SsaStmt {
                    result,
                    ty,
                    op: LoweredOp::Const { value: *value },
                    operands: Vec::new(),
                });
            }
            NodeOp::Param { name } => {
                self.seen_params.insert(name.clone());
                let result = self.fresh();
                self.record_out(node_id, "out", result);
                self.stmts.push(SsaStmt {
                    result,
                    ty: PortType::Float,
                    op: LoweredOp::Param { name: name.clone() },
                    operands: Vec::new(),
                });
            }
            NodeOp::Builtin { semantic } => {
                if !self.seen_builtins.contains(semantic) {
                    self.seen_builtins.push(*semantic);
                }
                let result = self.fresh();
                let ty = semantic.port_type().unwrap_or(PortType::Int);
                self.record_out(node_id, "out", result);
                self.stmts.push(SsaStmt {
                    result,
                    ty,
                    op: LoweredOp::Builtin {
                        semantic: *semantic,
                    },
                    operands: Vec::new(),
                });
            }
            NodeOp::Sample { texture } => {
                if !self.seen_textures.contains(texture) {
                    self.seen_textures.push(texture.clone());
                }
                // Operand: the coord temp (must already be emitted).
                let operands = vec![self.input_temp(node_id, "coord").unwrap_or(TempId(0))];
                let result = self.fresh();
                self.record_out(node_id, "out", result);
                self.stmts.push(SsaStmt {
                    result,
                    ty: PortType::Vec4,
                    op: LoweredOp::Sample {
                        texture: texture.clone(),
                    },
                    operands,
                });
            }
            NodeOp::Expr { op, operands } => {
                let ty = self.output_type(node, "out");
                let operand_temps: Vec<TempId> = operands
                    .iter()
                    .map(|p| self.input_temp(node_id, p).unwrap_or(TempId(0)))
                    .collect();
                let result = self.fresh();
                self.record_out(node_id, "out", result);
                self.stmts.push(SsaStmt {
                    result,
                    ty,
                    op: LoweredOp::Expr { op: op.clone() },
                    operands: operand_temps,
                });
            }
            NodeOp::CustomSnippet {
                body,
                inputs,
                outputs,
            } => {
                let operand_temps: Vec<TempId> = inputs
                    .iter()
                    .map(|p| self.input_temp(node_id, &p.name).unwrap_or(TempId(0)))
                    .collect();
                // One result temp per declared output port (a snippet may produce
                // several values). Each statement carries the same node_id / body /
                // typed ports but a distinct result_port; the emitter emits the
                // wrapper function once and aliases each output-port temp to the
                // matching `out` argument.
                for out in outputs {
                    let result = self.fresh();
                    self.record_out(node_id, &out.name, result);
                    self.stmts.push(SsaStmt {
                        result,
                        ty: out.ty,
                        op: LoweredOp::CustomSnippet {
                            node_id: node_id.to_owned(),
                            body: body.clone(),
                            inputs: inputs.clone(),
                            outputs: outputs.clone(),
                            result_port: out.name.clone(),
                        },
                        operands: operand_temps.clone(),
                    });
                }
            }
        }
    }

    fn record_out(&mut self, node_id: &'g str, port: &str, temp: TempId) {
        self.out_temps.insert((node_id, port.to_owned()), temp);
    }
}

// ----------------------------------------------------------------------------
// Manifest collection (deterministic ordering)
// ----------------------------------------------------------------------------

/// A canonical, traversal-order-independent sort key for a [`TextureSource`].
/// Ordering: by kind (Source, Original, OriginalHistory, PassOutput, PassFeedback,
/// Lut), then by index (or name for LUTs). Stable across runs and node orderings.
fn texture_sort_key(t: &TextureSource) -> (u8, u32, String) {
    match t {
        TextureSource::Source => (0, 0, String::new()),
        TextureSource::Original => (1, 0, String::new()),
        TextureSource::OriginalHistory { index } => (2, *index, String::new()),
        TextureSource::PassOutput { index } => (3, *index, String::new()),
        TextureSource::PassFeedback { index } => (4, *index, String::new()),
        TextureSource::Lut { name } => (5, 0, name.clone()),
    }
}

/// Build the deterministically-ordered [`PassManifest`] from the lowerer's
/// accumulated raw inputs. See [`PassManifest`] for the ordering rule.
fn collect_manifest(builder: &Lowerer) -> PassManifest {
    // Parameters: sorted by name (BTreeSet already sorted).
    let parameters = builder
        .seen_params
        .iter()
        .map(|name| ParamRequirement { name: name.clone() })
        .collect();

    // Builtins: sorted by the reserved slang identifier (a stable content key,
    // since `BuiltinSemantic` is not `Ord`).
    let mut builtins: Vec<BuiltinSemantic> = builder.seen_builtins.clone();
    builtins.sort_by_key(|b| b.slang_name());

    // Textures: canonical content order, independent of traversal.
    let mut textures: Vec<TextureSource> = builder.seen_textures.to_vec();
    textures.sort_by_key(texture_sort_key);

    // Samplers: one per distinct texture, in the same canonical order, with
    // sequential binding indices `0, 1, 2, …`.
    let samplers = textures
        .iter()
        .enumerate()
        .map(|(i, texture)| SamplerBinding {
            texture: texture.clone(),
            binding: i as u32,
        })
        .collect();

    PassManifest {
        parameters,
        builtins,
        samplers,
        textures,
    }
}
