// The NODE-DESCRIPTOR REGISTRY contract (#49) — the single cross-cutting seam the
// rest of Phase 5 builds on. A descriptor is the complete, declarative spec of one
// editor node `kind`: how it looks (category/label), what ports it carries (typed
// inputs/outputs the canvas handles + graphToIr read), how it lowers to the typed
// IR (`toNodeOp`), what authoring data it defaults to, and how the inspector (#47)
// edits that data (the field schema).
//
// Adding a node type = adding a descriptor. The palette, the inspector, the canvas
// node component, and graphToIr ALL read these — never special-case a kind anywhere
// else. The taxonomy knowledge lives HERE (TS), keeping the Rust IR frozen.
import type { NodeOp } from "../bindings/NodeOp";
import type { Parameter } from "../bindings/Parameter";
import type { PortType } from "../bindings/PortType";

/** A typed port on a node — the unit the canvas handles + graphToIr edges address. */
export interface PortSpec {
  /** The port identifier. MUST match the IR port-name convention the checker
   *  expects (e.g. Sample's `"coord"`, Output's `"color"`, the universal `"out"`,
   *  Expr operand names). A mismatch surfaces as a danglingInput / unknown-port. */
  name: string;
  /** The value type flowing through this port (drives handle typing + validation). */
  type: PortType;
  /** Optional human label shown next to the handle (defaults to `name`). */
  label?: string;
}

/** The broad grouping a node belongs to — drives palette sectioning + node accent. */
export type NodeCategory =
  | "input" // samplers: Source/Original/History/PassOutput/PassFeedback/LUT
  | "coordinate" // Texcoord + UV transforms
  | "constant" // Const literals
  | "parameter" // #pragma parameter knobs
  | "builtin" // reserved RetroArch semantics
  | "math" // Expr intrinsics (#50)
  | "color" // color transforms (#51)
  | "custom" // raw CustomSnippet (#52)
  | "output"; // the final color sink

/** The widget an inspector field renders as (#47 consumes this schema). */
export type InspectorFieldKind =
  | "number"
  | "integer"
  | "text"
  | "boolean"
  | "select"
  | "vec2"
  | "vec3"
  | "vec4";

/** One editable field in a node's inspector — a typed view onto a `data` key. */
export interface InspectorField {
  /** The `node.data` key this field reads/writes. */
  key: string;
  /** Human label shown in the inspector. */
  label: string;
  /** Which widget to render. */
  kind: InspectorFieldKind;
  /** For `select`: the allowed options (value + display label). */
  options?: ReadonlyArray<{ value: string; label: string }>;
  /** For `number`/`integer`: optional bounds + step hints for the widget. */
  min?: number;
  max?: number;
  step?: number;
}

/** The free-form authoring data a node carries (the editable `node.data`). */
export type NodeData = Record<string, unknown>;

/**
 * The complete spec of one node `kind`. Generic over the data shape so a
 * descriptor's `toNodeOp`/`defaultData` are typed against its own data.
 */
export interface NodeDescriptor<D extends NodeData = NodeData> {
  /** The stable `node.kind` string this descriptor is keyed by. */
  kind: string;
  /** The palette grouping. */
  category: NodeCategory;
  /** Human label shown in the palette + as the node title. */
  label: string;
  /** A one-line palette/inspector description. */
  description?: string;
  /**
   * The node's input ports. May depend on `data` (e.g. an Expr's operand ports
   * are derived from its op). Static descriptors return a constant array.
   */
  inputs: (data: D) => PortSpec[];
  /** The node's output ports (likewise may depend on `data`). */
  outputs: (data: D) => PortSpec[];
  /** The default `data` a freshly-placed node of this kind carries. */
  defaultData: () => D;
  /** The inspector field schema (#47). May depend on `data` for dynamic forms. */
  inspector: (data: D) => InspectorField[];
  /**
   * Lower this node's `data` to its typed IR op. Pure — no edge/wiring knowledge
   * (that lives on the edges graphToIr derives). Throws `NodeLoweringError` only
   * for genuinely malformed data; well-formed defaults always lower.
   */
  toNodeOp: (data: D) => NodeOp;
  /**
   * If this node contributes a pass-level `Parameter` (only `param` does), return
   * it so graphToIr can collect the pass's declared parameters (the checker errors
   * `unknownParam` otherwise). Most descriptors omit this.
   */
  toParameter?: (data: D) => Parameter | null;
  /**
   * If this node references a LUT by name (only `lut` does), return that name so
   * graphToIr can collect the pass's declared LUTs (the checker errors on an
   * unknown LUT otherwise). Most descriptors omit this.
   */
  toLutName?: (data: D) => string | null;
}

/** Thrown by a descriptor's `toNodeOp` when `node.data` is structurally invalid. */
export class NodeLoweringError extends Error {
  constructor(
    /** The node kind whose data failed to lower. */
    readonly kind: string,
    message: string,
  ) {
    super(message);
    this.name = "NodeLoweringError";
  }
}
