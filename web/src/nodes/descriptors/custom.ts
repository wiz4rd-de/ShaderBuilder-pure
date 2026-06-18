// Custom snippet (#52) — the raw-GLSL escape hatch INSIDE a per-pass graph. The
// only node whose ports are USER-EDITABLE: the author declares typed input/output
// ports and types a slang-dialect body that reads its inputs by name and assigns
// its outputs by name. It lowers verbatim to `NodeOp::CustomSnippet{body, inputs,
// outputs}`, which the Phase-4 emitter inlines as a `void snippet_<id>(in …, out …)`
// wrapper (see crates/codegen-slang/src/emit.rs) whose body is taken byte-for-byte.
//
// A sampler2D input port lets the snippet `texture()` a bound texture; the typed
// ports are addressed by graphToIr edges exactly like any other node's ports.
//
// Validation: the AUTHORITATIVE check is `compile_graph`'s diagnostics (#54). This
// descriptor adds only a CHEAP pre-check — `unresolvedSnippetPorts` flags declared
// port names that never appear as identifiers in the body — surfaced as a nicety,
// never a hard gate.
import type { PortType } from "../../bindings/PortType";
import { readString } from "../data";
import type {
  InspectorField,
  NodeData,
  NodeDescriptor,
  PortSignature,
  PortSpec,
} from "../types";
import { NodeLoweringError } from "../types";

/** The `data` keys the snippet stores its editable port lists under. */
const INPUTS_KEY = "inputs";
const OUTPUTS_KEY = "outputs";
/** The `data` key the snippet stores its GLSL body under. */
const BODY_KEY = "body";

/** A starter snippet that compiles on its own — a colour pass-through clamp. */
const DEFAULT_BODY = "result = clamp(color, vec4(0.0), vec4(1.0));";
const DEFAULT_INPUTS: PortSpec[] = [{ name: "color", type: "vec4" }];
const DEFAULT_OUTPUTS: PortSpec[] = [{ name: "result", type: "vec4" }];

/** The full port-type set a snippet port may take (matches the IR PortType set). */
const PORT_TYPES: ReadonlyArray<PortType> = [
  "float",
  "vec2",
  "vec3",
  "vec4",
  "int",
  "bool",
  "sampler2D",
];

/** Coerce a stored value into a clean PortSpec[] (drops malformed entries). */
function readPorts(data: NodeData, key: string, fallback: PortSpec[]): PortSpec[] {
  const raw = data[key];
  if (!Array.isArray(raw)) {
    return fallback.map((p) => ({ ...p }));
  }
  const ports: PortSpec[] = [];
  for (const entry of raw) {
    if (entry === null || typeof entry !== "object") {
      continue;
    }
    const rec = entry as Record<string, unknown>;
    const name = typeof rec.name === "string" ? rec.name : "";
    const type = PORT_TYPES.includes(rec.type as PortType) ? (rec.type as PortType) : "vec4";
    if (name.length === 0) {
      continue;
    }
    ports.push({ name, type });
  }
  return ports;
}

/** The snippet's declared input ports (free variables the body reads). */
function snippetInputs(data: NodeData): PortSpec[] {
  return readPorts(data, INPUTS_KEY, DEFAULT_INPUTS);
}

/** The snippet's declared output ports (values the body assigns). */
function snippetOutputs(data: NodeData): PortSpec[] {
  return readPorts(data, OUTPUTS_KEY, DEFAULT_OUTPUTS);
}

/**
 * A cheap, NON-authoritative pre-check: which declared port names never appear as
 * a standalone identifier in the body. `compile_graph` is the real validator
 * (#54); this only flags an obvious typo (a renamed port the body still misses).
 * Returns the offending port names (inputs + outputs), empty when all resolve.
 */
export function unresolvedSnippetPorts(body: string, signature: PortSignature): string[] {
  // Strip line + block comments so a name mentioned only in a comment doesn't
  // count as "referenced" (and a commented-out body still flags its ports).
  const code = body
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\/\/[^\n]*/g, " ");
  const idents = new Set(code.match(/[A-Za-z_][A-Za-z0-9_]*/g) ?? []);
  const unresolved: string[] = [];
  for (const port of [...signature.inputs, ...signature.outputs]) {
    if (!idents.has(port.name)) {
      unresolved.push(port.name);
    }
  }
  return unresolved;
}

/**
 * The custom-snippet node. Its ports are editable (the inspector renders the
 * generic port editor, #47), its body is a code field, and it lowers verbatim to
 * a CustomSnippet IR op.
 */
export const customSnippetDescriptor: NodeDescriptor = {
  kind: "customSnippet",
  category: "custom",
  label: "Custom Snippet",
  description: "A raw slang/GLSL body with typed in/out ports (inlined verbatim).",
  inputs: (data) => snippetInputs(data),
  outputs: (data) => snippetOutputs(data),
  defaultData: () => ({
    [BODY_KEY]: DEFAULT_BODY,
    [INPUTS_KEY]: DEFAULT_INPUTS.map((p) => ({ ...p })),
    [OUTPUTS_KEY]: DEFAULT_OUTPUTS.map((p) => ({ ...p })),
  }),
  inspector: (): InspectorField[] => [
    { key: BODY_KEY, label: "Body (slang)", kind: "code" },
  ],
  editablePorts: {
    setPorts: (_data: NodeData, signature: PortSignature) => ({
      [INPUTS_KEY]: signature.inputs.map((p) => ({ name: p.name, type: p.type })),
      [OUTPUTS_KEY]: signature.outputs.map((p) => ({ name: p.name, type: p.type })),
    }),
  },
  toNodeOp: (data: NodeData) => {
    const inputs = snippetInputs(data);
    const outputs = snippetOutputs(data);
    if (outputs.length === 0) {
      // A snippet with no output produces no value any edge can consume — the
      // checker would flag a dangling consumer, but lowering it is meaningless.
      throw new NodeLoweringError("customSnippet", "a snippet must declare at least one output");
    }
    return {
      kind: "customSnippet",
      body: readString(data, BODY_KEY, ""),
      inputs: inputs.map((p) => ({ name: p.name, type: p.type })),
      outputs: outputs.map((p) => ({ name: p.name, type: p.type })),
    };
  },
};

/** All custom (#52) descriptors, in palette order. */
export const customDescriptors: NodeDescriptor[] = [customSnippetDescriptor];
