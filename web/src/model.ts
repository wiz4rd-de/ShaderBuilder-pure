// Re-exports and helpers over the generated core-model bindings.
//
// The types in ./bindings/ are generated from the Rust `core-model` crate (see
// crates/core-model). Importing them here means any drift between the Rust
// schema and the committed bindings surfaces as a TypeScript compile error.
import type { Project } from "./bindings/Project";

export type { Project } from "./bindings/Project";
export type { Pass } from "./bindings/Pass";
export type { PassSource } from "./bindings/PassSource";
export type { PassSettings } from "./bindings/PassSettings";
export type { ScaleAxis } from "./bindings/ScaleAxis";
export type { ScaleType } from "./bindings/ScaleType";
export type { WrapMode } from "./bindings/WrapMode";
export type { Graph } from "./bindings/Graph";
export type { Node } from "./bindings/Node";
export type { Edge } from "./bindings/Edge";
export type { Parameter } from "./bindings/Parameter";
export type { Lut } from "./bindings/Lut";
export type { Vec2 } from "./bindings/Vec2";
export type { TextureRef } from "./bindings/TextureRef";
export type { TextureRefKind } from "./bindings/TextureRefKind";
export type { PipelineMetadata } from "./bindings/PipelineMetadata";
export type { AliasBinding } from "./bindings/AliasBinding";
export type { PassAvailability } from "./bindings/PassAvailability";
export type { ProjectMetadata } from "./bindings/ProjectMetadata";
export type { LibraryRef } from "./bindings/LibraryRef";
// Typed save/load diagnostics returned by the save_project/load_project commands
// (#38) — re-exported so the Phase-7 save/load UX can match on the variants.
export type { ProjectLoadError } from "./bindings/ProjectLoadError";
export type { ProjectSaveError } from "./bindings/ProjectSaveError";

/** Schema version the frontend was built against (mirrors PROJECT_SCHEMA_VERSION). */
export const PROJECT_SCHEMA_VERSION = 1;

/** An empty in-memory project, typed against the generated schema. */
export const EMPTY_PROJECT: Project = {
  schemaVersion: PROJECT_SCHEMA_VERSION,
  name: "Untitled",
  passes: [],
  feedbackPass: null,
  pipeline: { aliases: [], availability: [] },
  parameters: [],
  luts: [],
  metadata: {
    description: null,
    author: null,
    createdAt: null,
    modifiedAt: null,
  },
  libraryRefs: [],
};
