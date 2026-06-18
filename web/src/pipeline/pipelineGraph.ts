// Derive the PIPELINE-VIEW React Flow graph (#46) from Project.passes. The
// pipeline view is a PROJECTION of the document, never a second source of truth:
// one Pass node per pass (in pass-index order), and an edge for every cross-pass
// texture binding a pass's own graph declares (PassOutputN / PassFeedbackN).
//
// Binding extraction scans each pass's SKELETAL graph for sampler nodes whose
// kind binds another pass by index (passOutput / passFeedback) and records the
// referenced pass index + binding kind. OriginalHistory / Source / Original /
// LUT bind the chain input or a project texture, not a producing pass, so they
// contribute a self/boundary annotation rather than a pass→pass edge.
import type { Edge as RfEdge, Node as RfNode } from "@xyflow/react";

import type { Pass } from "../bindings/Pass";
import type { Project } from "../bindings/Project";

/** How one pass consumes another pass's texture — drives edge styling. */
export type PipelineBindingKind = "passOutput" | "passFeedback";

/** The data carried on a pipeline (Pass) node. */
export interface PipelineNodeData extends Record<string, unknown> {
  passId: string;
  /** 0-based authoritative pass index (== Project.passes order == .slangp index). */
  index: number;
  label: string;
  /** Whether this pass is graph-authored (drill-in enabled) or opaque code. */
  isGraph: boolean;
  /** Distinct boundary inputs this pass samples (Source/Original/History/LUT). */
  boundaryInputs: PipelineBoundaryInput[];
}

/** A non-pass texture a pass samples (chain input / project LUT). */
export interface PipelineBoundaryInput {
  kind: "source" | "original" | "originalHistory" | "lut";
  /** History depth (originalHistory) or LUT name (lut); else undefined. */
  detail?: string;
}

/** The data carried on a pipeline edge (which texture the target consumes). */
export interface PipelineEdgeData extends Record<string, unknown> {
  binding: PipelineBindingKind;
}

export type PipelineRfNode = RfNode<PipelineNodeData>;
export type PipelineRfEdge = RfEdge<PipelineEdgeData>;

/** Horizontal spacing between auto-laid pipeline nodes. */
const NODE_DX = 240;
/** Y position of the auto-laid pipeline row. */
const NODE_Y = 80;

/** One cross-pass binding discovered inside a pass's graph. */
interface PassBinding {
  /** Index of the producing pass this binding references. */
  sourceIndex: number;
  kind: PipelineBindingKind;
}

/**
 * Scan a pass's skeletal graph for its cross-pass + boundary sampler bindings.
 * `passOutput`/`passFeedback` yield pass→pass bindings; the boundary samplers
 * (source/original/originalHistory/lut) are recorded for the node annotation.
 */
export function extractPassBindings(pass: Pass): {
  bindings: PassBinding[];
  boundaryInputs: PipelineBoundaryInput[];
} {
  const bindings: PassBinding[] = [];
  const boundaryInputs: PipelineBoundaryInput[] = [];
  if (pass.source.kind !== "graph") {
    return { bindings, boundaryInputs };
  }
  const seenBindings = new Set<string>();
  const seenBoundary = new Set<string>();
  for (const node of pass.source.graph.nodes) {
    const data = node.data as { index?: unknown; name?: unknown };
    switch (node.kind) {
      case "passOutput":
      case "passFeedback": {
        const sourceIndex = typeof data.index === "number" ? data.index : 0;
        const key = `${node.kind}:${sourceIndex}`;
        if (!seenBindings.has(key)) {
          seenBindings.add(key);
          bindings.push({ sourceIndex, kind: node.kind });
        }
        break;
      }
      case "source":
      case "original": {
        if (!seenBoundary.has(node.kind)) {
          seenBoundary.add(node.kind);
          boundaryInputs.push({ kind: node.kind });
        }
        break;
      }
      case "originalHistory": {
        const detail = String(typeof data.index === "number" ? data.index : 0);
        const key = `originalHistory:${detail}`;
        if (!seenBoundary.has(key)) {
          seenBoundary.add(key);
          boundaryInputs.push({ kind: "originalHistory", detail });
        }
        break;
      }
      case "lut": {
        const detail = typeof data.name === "string" ? data.name : "";
        const key = `lut:${detail}`;
        if (!seenBoundary.has(key)) {
          seenBoundary.add(key);
          boundaryInputs.push({ kind: "lut", detail });
        }
        break;
      }
      default:
        break;
    }
  }
  return { bindings, boundaryInputs };
}

/**
 * Build the React Flow node/edge arrays for the pipeline view from a project.
 *
 * Each pass becomes a node positioned in a left→right row (manual positions are
 * not persisted — the pipeline is derived, so a deterministic auto-layout keeps
 * it stable). Each pass's `passOutput`/`passFeedback` bindings become an edge
 * FROM the referenced producing pass node TO the consuming pass node, carrying
 * the binding kind so the view can style feedback vs pass-output distinctly. An
 * out-of-range / dangling reference (DANGLING_INDEX) produces no edge.
 */
export function toPipelineGraph(
  project: Project,
  selectedPassId: string | null,
): { nodes: PipelineRfNode[]; edges: PipelineRfEdge[] } {
  const passes = project.passes;
  const idByIndex = passes.map((p) => p.id);

  const nodes: PipelineRfNode[] = passes.map((pass, index) => {
    const { boundaryInputs } = extractPassBindings(pass);
    return {
      id: pass.id,
      type: "pipelinePass",
      position: { x: index * NODE_DX, y: NODE_Y },
      selected: pass.id === selectedPassId,
      data: {
        passId: pass.id,
        index,
        label: pass.name,
        isGraph: pass.source.kind === "graph",
        boundaryInputs,
      },
    };
  });

  const edges: PipelineRfEdge[] = [];
  for (const pass of passes) {
    const { bindings } = extractPassBindings(pass);
    for (const binding of bindings) {
      const producerId = idByIndex[binding.sourceIndex];
      if (producerId === undefined) {
        continue; // dangling / out-of-range index — no edge
      }
      edges.push({
        id: `pl-${pass.id}-${binding.kind}-${binding.sourceIndex}`,
        source: producerId,
        target: pass.id,
        sourceHandle: "out",
        targetHandle: binding.kind,
        data: { binding: binding.kind },
        className: `pipeline-edge pipeline-edge--${binding.kind}`,
        animated: binding.kind === "passFeedback",
      });
    }
  }

  return { nodes, edges };
}
