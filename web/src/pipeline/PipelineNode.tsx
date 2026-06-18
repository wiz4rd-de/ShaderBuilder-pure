// The PIPELINE-view node component (#46) — renders ONE pass as a card with its
// input/output handles. The pipeline graph is derived (see pipelineGraph.ts), so
// this component is purely presentational: it reads the PipelineNodeData the
// projection put on the node.
//
// Handles:
//   * output "out" (right)            — this pass's PassOutput, consumed downstream.
//   * input  "passOutput" (left)      — incoming PassOutputN bindings.
//   * input  "passFeedback" (left)    — incoming PassFeedbackN bindings (feedback).
// Boundary inputs (Source/Original/History/LUT) are shown as annotation chips —
// they bind the chain input / a project texture, not a producing pass, so they
// are not pass→pass handles.
import { Handle, Position, type NodeProps } from "@xyflow/react";

import type { PipelineBoundaryInput, PipelineNodeData } from "./pipelineGraph";

/** Short human label for a boundary input chip. */
function boundaryLabel(input: PipelineBoundaryInput): string {
  switch (input.kind) {
    case "source":
      return "Source";
    case "original":
      return "Original";
    case "originalHistory":
      return `History-${input.detail ?? "0"}`;
    case "lut":
      return `LUT:${input.detail || "?"}`;
    default:
      return input.kind;
  }
}

export function PipelineNode(props: NodeProps): React.JSX.Element {
  const data = props.data as PipelineNodeData;

  return (
    <div className="pipeline-node" data-pass-index={data.index} data-testid="pipeline-node">
      {/* Pass-output consumers connect here (left); feedback styled distinctly. */}
      <Handle
        type="target"
        position={Position.Left}
        id="passOutput"
        className="pipeline-node__handle pipeline-node__handle--passOutput"
        style={{ top: "38%" }}
      />
      <Handle
        type="target"
        position={Position.Left}
        id="passFeedback"
        className="pipeline-node__handle pipeline-node__handle--passFeedback"
        style={{ top: "70%" }}
      />

      <div className="pipeline-node__title">
        <span className="pipeline-node__index">#{data.index}</span>
        <span className="pipeline-node__name">{data.label}</span>
      </div>
      {!data.isGraph ? (
        <div className="pipeline-node__subtitle">opaque code</div>
      ) : null}
      {data.boundaryInputs.length > 0 ? (
        <div className="pipeline-node__chips">
          {data.boundaryInputs.map((input) => (
            <span
              key={`${input.kind}:${input.detail ?? ""}`}
              className={`pipeline-node__chip pipeline-node__chip--${input.kind}`}
            >
              {boundaryLabel(input)}
            </span>
          ))}
        </div>
      ) : null}

      {/* This pass's output, consumed by later passes. */}
      <Handle
        type="source"
        position={Position.Right}
        id="out"
        className="pipeline-node__handle pipeline-node__handle--out"
      />
    </div>
  );
}
