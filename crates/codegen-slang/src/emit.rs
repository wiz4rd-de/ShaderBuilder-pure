//! The IR → Vulkan-GLSL slang **emitter** (#42).
//!
//! [`emit_pass`] turns a lowered, type-checked pass ([`ir::LoweredIr`] +
//! [`ir::PassManifest`], #41) into a complete `.slang` source string in
//! RetroArch's Vulkan-GLSL conventions — the exact shape the Phase-1
//! `slang-compile` path accepts (preprocess → glslang → SPIR-V) and the preview
//! engine renders (Architecture §C). The acceptance bar (#42) is that the emitted
//! string compiles through `slang_compile::compile_slang` with no errors.
//!
//! ## The emitted shape (matches the worked example in the brief)
//!
//! ```glsl
//! #version 450
//! layout(push_constant) uniform Push {
//!     vec4 SourceSize; vec4 OriginalSize; /* ... builtins used ... */
//!     uint FrameCount;
//!     float MY_PARAM;                    // one field per #pragma parameter
//! } params;
//! #pragma parameter MY_PARAM "Label" 0.5 0.0 1.0 0.01
//! #pragma name <alias>
//! #pragma format <fbo-format>
//! layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
//! #pragma stage vertex
//! layout(location = 0) in vec4 Position;
//! layout(location = 1) in vec2 TexCoord;
//! layout(location = 0) out vec2 vTexCoord;
//! void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
//! #pragma stage fragment
//! layout(location = 0) in vec2 vTexCoord;
//! layout(location = 0) out vec4 FragColor;
//! layout(set = 0, binding = N) uniform sampler2D <Name>;   // one per sampler
//! void main() {
//!     <type> t0 = <expr>;                 // one statement per SSA temp
//!     /* ... */
//!     FragColor = <output-temp>;
//! }
//! ```
//!
//! ## What the emitter owns (Spec §8.1)
//!
//! - **Binding assignment**: sampler `layout(set=0, binding=N)` indices come
//!   straight from the deterministic [`ir::PassManifest`] order — the emitter is
//!   the single source of truth for binding layout. The `global` UBO (holding
//!   `MVP`) is `set=0, binding=0`; the params block is `binding=1` when emitted as
//!   a UBO (here it is a `push_constant`, which `slang-compile` rewrites). Sampler
//!   bindings start at `2` so they never collide with those two reserved blocks —
//!   matching the worked example (`Source` at `binding = 2`).
//! - **Builtin uniform names** are the exact reserved RetroArch spellings
//!   ([`core_model::ir::BuiltinSemantic::slang_name`]).
//! - **The vertex stage** is the fixed `MVP * Position` passthrough emitting
//!   `vTexCoord` — never generated from the graph (the graph is fragment-only).

use std::fmt::Write as _;

use core_model::ir::{BuiltinSemantic, ConstValue, ExprOp, PortDecl, PortType, TextureSource};
use core_model::Parameter;
use ir::{LoweredIr, LoweredOp, SamplerBinding, SsaStmt, TempId};

/// The RetroArch FBO format the emitter defaults to when a pass declares none.
/// `R8G8B8A8_UNORM` is RetroArch's standard 8-bit-per-channel render target and
/// the format the engine's golden fixtures use.
pub const DEFAULT_FORMAT: &str = "R8G8B8A8_UNORM";

/// The `layout(set=0, binding=N)` index of the `global` UBO holding `MVP`.
const GLOBAL_UBO_BINDING: u32 = 0;
/// The first sampler binding index. `0` is the `global` UBO, `1` is reserved for
/// the params block (when emitted as a UBO); samplers start at `2`, matching the
/// worked example where `Source` lands at `binding = 2`.
pub const FIRST_SAMPLER_BINDING: u32 = 2;

/// Inputs to [`emit_pass`] that the lowered IR does not itself carry: the pass
/// alias/format (from [`core_model::PassSettings`]) and the full
/// [`Parameter`] declarations (the manifest only names the parameters a graph
/// reads; the `#pragma parameter` line needs the label/default/min/max/step).
///
/// All fields have sane defaults so a caller can `EmitOptions::default()` for a
/// nameless, default-format pass with no parameters and only fill in what it has.
#[derive(Debug, Clone, Default)]
pub struct EmitOptions {
    /// The pass alias emitted as `#pragma name <alias>` (and usable as a
    /// `<alias>` texture by later passes). `None` ⇒ no `#pragma name` line.
    pub name: Option<String>,
    /// The FBO format emitted as `#pragma format <format>`. `None` ⇒
    /// [`DEFAULT_FORMAT`].
    pub format: Option<String>,
    /// The full `#pragma parameter` declarations for the parameters the pass may
    /// read. The emitter emits a `#pragma parameter` line and a params-block
    /// field for each parameter the [`ir::PassManifest`] lists; the value comes
    /// from the matching entry here (by `name`). A manifest parameter with no
    /// matching declaration falls back to a neutral `0 0 1 0.01` stub so the
    /// emitted shader still compiles.
    pub parameters: Vec<Parameter>,
}

impl EmitOptions {
    /// Resolve the FBO format to emit (`format` or [`DEFAULT_FORMAT`]).
    fn format(&self) -> &str {
        self.format.as_deref().unwrap_or(DEFAULT_FORMAT)
    }

    /// The declared [`Parameter`] for `name`, if the caller supplied one.
    fn parameter(&self, name: &str) -> Option<&Parameter> {
        self.parameters.iter().find(|p| p.name == name)
    }
}

/// The GLSL type spelling for a [`PortType`] (`float`, `vec2`, …, `int`, `bool`).
/// [`PortType::Sampler2D`] has no value-type spelling (it is never an SSA temp's
/// type — samplers are opaque bindings, not values), so it maps to `sampler2D`
/// defensively; it never appears in a fragment-body temp declaration.
fn glsl_type(ty: PortType) -> &'static str {
    match ty {
        PortType::Float => "float",
        PortType::Vec2 => "vec2",
        PortType::Vec3 => "vec3",
        PortType::Vec4 => "vec4",
        PortType::Int => "int",
        PortType::Bool => "bool",
        PortType::Sampler2D => "sampler2D",
    }
}

/// The RetroArch slang identifier a [`TextureSource`] binds as — the reserved
/// sampler name the emitter writes into `layout(...) uniform sampler2D <name>`
/// and reads in `texture(<name>, …)`. The emitter owns this mapping (Spec §7/§8).
pub fn texture_slang_name(t: &TextureSource) -> String {
    match t {
        TextureSource::Source => "Source".to_owned(),
        TextureSource::Original => "Original".to_owned(),
        // OriginalHistory0 is `Original` itself (§5); higher indices spell
        // `OriginalHistoryN`.
        TextureSource::OriginalHistory { index: 0 } => "Original".to_owned(),
        TextureSource::OriginalHistory { index } => format!("OriginalHistory{index}"),
        TextureSource::PassOutput { index } => format!("PassOutput{index}"),
        TextureSource::PassFeedback { index } => format!("PassFeedback{index}"),
        TextureSource::Lut { name } => name.clone(),
    }
}

/// The GLSL `params`-block field type for a builtin semantic. The `*Size` family
/// is `vec4`; `FrameCount`/`FrameDirection` are `uint`/`int`. `MVP` is **not** a
/// params field (it lives in the `global` UBO), so it returns `None`.
fn builtin_field_type(semantic: BuiltinSemantic) -> Option<&'static str> {
    match semantic {
        BuiltinSemantic::SourceSize
        | BuiltinSemantic::OriginalSize
        | BuiltinSemantic::OutputSize
        | BuiltinSemantic::FinalViewportSize => Some("vec4"),
        BuiltinSemantic::FrameCount => Some("uint"),
        BuiltinSemantic::FrameDirection => Some("int"),
        BuiltinSemantic::Mvp => None,
    }
}

/// Emit a complete RetroArch Vulkan-GLSL `.slang` source for a lowered pass.
///
/// The result is the full single-pass shader (preamble + params block +
/// `#pragma parameter` lines + the standard vertex stage + the fragment stage
/// walking the SSA statements to the `FragColor` write). It is designed to
/// compile through `slang_compile::compile_slang(&emitted, None)` with no errors
/// (the #42 acceptance bar) and render identically to a hand-written equivalent
/// through the proven engine (#44).
pub fn emit_pass(lowered: &LoweredIr, opts: &EmitOptions) -> String {
    let mut out = String::new();

    emit_preamble(&mut out, lowered, opts);
    emit_vertex_stage(&mut out);
    emit_fragment_stage(&mut out, lowered, opts);

    out
}

/// `#version`, the `push_constant` params block (builtins used + parameters),
/// `#pragma parameter` lines, `#pragma name`/`#pragma format`, and the `global`
/// MVP UBO — everything before `#pragma stage vertex`.
fn emit_preamble(out: &mut String, lowered: &LoweredIr, opts: &EmitOptions) {
    let manifest = &lowered.manifest;

    out.push_str("#version 450\n\n");

    // --- Params block (push_constant; slang-compile rewrites it to a UBO) ------
    //
    // RetroArch convention: the *Size family + FrameCount + FrameDirection +
    // FinalViewportSize and the declared #pragma parameters live in `Push`.
    // `MVP` is the one builtin that lives in the separate `global` UBO instead.
    //
    // GLSL forbids an empty struct, so the block is only emitted when the pass
    // actually reads a builtin or a parameter (a pure passthrough that reads
    // neither omits it entirely — nothing references `params` then).
    let builtin_fields: Vec<(&'static str, &'static str)> = manifest
        .builtins
        .iter()
        .filter_map(|s| builtin_field_type(*s).map(|ty| (ty, s.slang_name())))
        .collect();
    if !builtin_fields.is_empty() || !manifest.parameters.is_empty() {
        out.push_str("layout(push_constant) uniform Push\n{\n");
        for (field_ty, name) in &builtin_fields {
            let _ = writeln!(out, "    {field_ty} {name};");
        }
        for param in &manifest.parameters {
            let _ = writeln!(out, "    float {};", param.name);
        }
        out.push_str("} params;\n\n");
    }

    // --- #pragma parameter lines ----------------------------------------------
    for param in &manifest.parameters {
        let decl = opts.parameter(&param.name);
        emit_pragma_parameter(out, &param.name, decl);
    }
    if !manifest.parameters.is_empty() {
        out.push('\n');
    }

    // --- #pragma name / #pragma format ----------------------------------------
    if let Some(name) = &opts.name {
        let _ = writeln!(out, "#pragma name {name}");
    }
    let _ = writeln!(out, "#pragma format {}", opts.format());
    out.push('\n');

    // --- The global MVP UBO (set=0, binding=0) --------------------------------
    let _ = writeln!(
        out,
        "layout(std140, set = 0, binding = {GLOBAL_UBO_BINDING}) uniform UBO\n{{\n    mat4 MVP;\n}} global;\n"
    );
}

/// Emit one `#pragma parameter <name> "<label>" <default> <min> <max> <step>`
/// line. When the caller supplied a [`Parameter`] declaration its label/range is
/// used; otherwise a neutral `"<name>" 0 0 1 0.01` stub keeps the shader valid.
fn emit_pragma_parameter(out: &mut String, name: &str, decl: Option<&Parameter>) {
    match decl {
        Some(p) => {
            let _ = writeln!(
                out,
                "#pragma parameter {} \"{}\" {} {} {} {}",
                p.name,
                p.label,
                fmt_f32(p.default),
                fmt_f32(p.min),
                fmt_f32(p.max),
                fmt_f32(p.step),
            );
        }
        None => {
            let _ = writeln!(out, "#pragma parameter {name} \"{name}\" 0.0 0.0 1.0 0.01");
        }
    }
}

/// The fixed RetroArch passthrough vertex stage (`MVP * Position`, forwarding
/// `TexCoord` to `vTexCoord`). Never generated from the graph — the graph is
/// fragment-only; this matches the worked example verbatim.
fn emit_vertex_stage(out: &mut String) {
    out.push_str(
        "#pragma stage vertex\n\
         layout(location = 0) in vec4 Position;\n\
         layout(location = 1) in vec2 TexCoord;\n\
         layout(location = 0) out vec2 vTexCoord;\n\
         void main()\n\
         {\n\
         \x20   gl_Position = global.MVP * Position;\n\
         \x20   vTexCoord = TexCoord;\n\
         }\n\n",
    );
}

/// The fragment stage: the `vTexCoord` in / `FragColor` out plumbing, the sampler
/// bindings (deterministic manifest order), then one GLSL statement per SSA temp
/// ending in the `FragColor = <output>` write.
fn emit_fragment_stage(out: &mut String, lowered: &LoweredIr, opts: &EmitOptions) {
    out.push_str(
        "#pragma stage fragment\n\
         layout(location = 0) in vec2 vTexCoord;\n\
         layout(location = 0) out vec4 FragColor;\n",
    );

    // Sampler bindings, one per manifest sampler, in deterministic order with the
    // manifest's assigned binding offset past the two reserved blocks.
    for SamplerBinding { texture, binding } in &lowered.manifest.samplers {
        let _ = writeln!(
            out,
            "layout(set = 0, binding = {}) uniform sampler2D {};",
            FIRST_SAMPLER_BINDING + binding,
            texture_slang_name(texture),
        );
    }
    out.push('\n');

    // Lowering emits one SSA statement per snippet *output port*, all carrying
    // the same body/operands. Resolve each output port's GLSL type from the
    // statements (each statement's `ty` is its `result_port`'s type), keyed by the
    // snippet instance (operand tuple + body), so the snippet block — which
    // declares *all* output locals — can be emitted exactly once.
    let snippet_output_types = collect_snippet_output_types(lowered);

    out.push_str("void main()\n{\n");
    // Track which CustomSnippet bodies have already been inlined so the body runs
    // exactly once; each output-port temp then aliases the matching output local.
    let mut emitted_snippets: Vec<(Vec<TempId>, String)> = Vec::new();
    for stmt in &lowered.stmts {
        if let LoweredOp::CustomSnippet {
            body,
            inputs,
            result_port,
            ..
        } = &stmt.op
        {
            let key = (stmt.operands.clone(), body.clone());
            if !emitted_snippets.contains(&key) {
                let outputs = snippet_output_types.get(&key).cloned().unwrap_or_default();
                emit_snippet_block(out, body, inputs, &outputs, &stmt.operands);
                emitted_snippets.push(key);
            }
            // This statement's result temp aliases the snippet's `result_port`
            // output local (declared by the block above).
            let _ = writeln!(
                out,
                "    {} {} = {};",
                glsl_type(stmt.ty),
                stmt.result,
                result_port
            );
            continue;
        }
        let expr = emit_stmt_expr(stmt, opts);
        let _ = writeln!(
            out,
            "    {} {} = {};",
            glsl_type(stmt.ty),
            stmt.result,
            expr
        );
    }
    let _ = writeln!(out, "    FragColor = {};", lowered.output);
    out.push_str("}\n");
}

/// For each CustomSnippet instance (keyed by its operand tuple + body), the
/// `(output-port-name, type)` of every output port it produces — gathered from
/// the per-output-port SSA statements (each statement's `ty` is its
/// `result_port`'s type). Preserves the statement order so the declared output
/// locals come out deterministically.
type SnippetKey = (Vec<TempId>, String);
fn collect_snippet_output_types(
    lowered: &LoweredIr,
) -> std::collections::HashMap<SnippetKey, Vec<PortDecl>> {
    let mut map: std::collections::HashMap<SnippetKey, Vec<PortDecl>> =
        std::collections::HashMap::new();
    for stmt in &lowered.stmts {
        if let LoweredOp::CustomSnippet {
            body, result_port, ..
        } = &stmt.op
        {
            let key = (stmt.operands.clone(), body.clone());
            let entry = map.entry(key).or_default();
            if !entry.iter().any(|d| &d.name == result_port) {
                entry.push(PortDecl::new(result_port.clone(), stmt.ty));
            }
        }
    }
    map
}

/// Inline a [`LoweredOp::CustomSnippet`] body once: bind each input port to its
/// operand temp, declare each output port local, then emit the snippet `body`
/// verbatim (it reads inputs and assigns outputs by name). The caller then binds
/// each output-port SSA temp to the matching output local.
fn emit_snippet_block(
    out: &mut String,
    body: &str,
    inputs: &[String],
    outputs: &[PortDecl],
    operands: &[TempId],
) {
    // Bind each input port name to its operand temp with a `#define`. The lowered
    // `CustomSnippet` op does not carry the input port types (only the names), so
    // an aliasing macro — rather than a typed local copy — is how we wire inputs
    // without re-deriving their types; the body then reads `port` and it expands
    // to `tN`. `#define`/`#undef` must start at column 0 (preprocessor lines).
    for (port, operand) in inputs.iter().zip(operands.iter()) {
        let _ = writeln!(out, "#define {port} {operand}");
    }
    // Declare the output port locals the body assigns into.
    for decl in outputs {
        let _ = writeln!(out, "    {} {};", glsl_type(decl.ty), decl.name);
    }
    // The body verbatim — reads the input #defines, assigns the output locals.
    for line in body.lines() {
        let _ = writeln!(out, "    {}", line.trim_end());
    }
    for port in inputs {
        let _ = writeln!(out, "#undef {port}");
    }
}

/// The right-hand-side GLSL expression for one SSA statement (the left-hand side
/// `<type> tN =` is emitted by the caller). Operand temps are referenced by their
/// `tN` names ([`TempId`]'s `Display`).
fn emit_stmt_expr(stmt: &SsaStmt, opts: &EmitOptions) -> String {
    match &stmt.op {
        LoweredOp::Sample { texture } => {
            // The Phase-4 graphs feed the coord via operand 0. The worked example
            // samples at `vTexCoord`; a coord temp (e.g. a warped UV) is honored
            // when present, otherwise we fall back to `vTexCoord`.
            let coord = stmt
                .operands
                .first()
                .map(|t| t.to_string())
                .unwrap_or_else(|| "vTexCoord".to_owned());
            format!("texture({}, {})", texture_slang_name(texture), coord)
        }
        LoweredOp::Builtin { semantic } => emit_builtin_read(*semantic),
        LoweredOp::Param { name } => {
            let _ = opts; // declarations are emitted in the preamble, not here.
            format!("params.{name}")
        }
        LoweredOp::Const { value } => emit_const(value),
        LoweredOp::Expr { op } => emit_expr_op(op, &stmt.operands),
        // CustomSnippet is inlined as a multi-statement block by
        // `emit_fragment_stage`, not as a single RHS expression — it never
        // reaches here.
        LoweredOp::CustomSnippet { result_port, .. } => result_port.clone(),
    }
}

/// Read a builtin-semantic uniform. The `*Size` family + frame counters live in
/// the `params` push-constant block; `MVP` lives in `global`. `FrameCount` is a
/// `uint` in the block but typed `Int` in the IR — read it back through a cast so
/// the emitted temp's declared `int` type matches.
fn emit_builtin_read(semantic: BuiltinSemantic) -> String {
    match semantic {
        BuiltinSemantic::Mvp => "global.MVP".to_owned(),
        BuiltinSemantic::FrameCount => "int(params.FrameCount)".to_owned(),
        other => format!("params.{}", other.slang_name()),
    }
}

/// The GLSL literal / constructor for a [`ConstValue`].
fn emit_const(value: &ConstValue) -> String {
    match value {
        ConstValue::Float { value } => fmt_f32(*value),
        ConstValue::Vec2 { value } => format!("vec2({}, {})", fmt_f32(value[0]), fmt_f32(value[1])),
        ConstValue::Vec3 { value } => format!(
            "vec3({}, {}, {})",
            fmt_f32(value[0]),
            fmt_f32(value[1]),
            fmt_f32(value[2])
        ),
        ConstValue::Vec4 { value } => format!(
            "vec4({}, {}, {}, {})",
            fmt_f32(value[0]),
            fmt_f32(value[1]),
            fmt_f32(value[2]),
            fmt_f32(value[3])
        ),
        ConstValue::Int { value } => value.to_string(),
        ConstValue::Bool { value } => value.to_string(),
    }
}

/// Map an [`ExprOp`] over its operand temps to the matching GLSL expression.
/// Binary arithmetic → operators; the math intrinsics → their GLSL builtins;
/// `Swizzle` → `.mask`; `Construct` → a `vecN(...)` constructor.
fn emit_expr_op(op: &ExprOp, operands: &[TempId]) -> String {
    let arg = |i: usize| -> String {
        operands
            .get(i)
            .map(|t| t.to_string())
            // Defensive: a clean (type-checked) graph always supplies the
            // required operands; fall back to `0.0` rather than panicking.
            .unwrap_or_else(|| "0.0".to_owned())
    };
    match op {
        ExprOp::Add => format!("({} + {})", arg(0), arg(1)),
        ExprOp::Sub => format!("({} - {})", arg(0), arg(1)),
        ExprOp::Mul => format!("({} * {})", arg(0), arg(1)),
        ExprOp::Div => format!("({} / {})", arg(0), arg(1)),
        ExprOp::Mix => format!("mix({}, {}, {})", arg(0), arg(1), arg(2)),
        ExprOp::Clamp => format!("clamp({}, {}, {})", arg(0), arg(1), arg(2)),
        ExprOp::Min => format!("min({}, {})", arg(0), arg(1)),
        ExprOp::Max => format!("max({}, {})", arg(0), arg(1)),
        ExprOp::Pow => format!("pow({}, {})", arg(0), arg(1)),
        ExprOp::Sin => format!("sin({})", arg(0)),
        ExprOp::Cos => format!("cos({})", arg(0)),
        ExprOp::Abs => format!("abs({})", arg(0)),
        ExprOp::Floor => format!("floor({})", arg(0)),
        ExprOp::Fract => format!("fract({})", arg(0)),
        ExprOp::Dot => format!("dot({}, {})", arg(0), arg(1)),
        ExprOp::Normalize => format!("normalize({})", arg(0)),
        ExprOp::Length => format!("length({})", arg(0)),
        ExprOp::Swizzle { mask } => format!("{}.{mask}", arg(0)),
        ExprOp::Construct { ty } => {
            let args = (0..operands.len()).map(arg).collect::<Vec<_>>().join(", ");
            format!("{}({args})", glsl_type(*ty))
        }
    }
}

/// Format an `f32` as a GLSL float literal that always carries a decimal point
/// (so `1` becomes `1.0`, a valid `float`, not an `int`). Trims to a stable,
/// round-trippable shortest representation.
fn fmt_f32(v: f32) -> String {
    if v.is_nan() {
        return "0.0".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "1e30" } else { "-1e30" }.to_owned();
    }
    // `{}` on f32 yields the shortest round-trippable form but drops the `.0`
    // for integers; add it back so the literal is unambiguously a float.
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}
