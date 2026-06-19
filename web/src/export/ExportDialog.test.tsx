import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Avoid pulling the real Tauri plugins (dialog/opener) into jsdom — the dialog
// reads everything it needs from the store, which the tests drive directly.
vi.mock("./exportApi", () => ({
  validateExport: vi.fn(async () => ({ blockers: [] })),
  pickExportDir: vi.fn(async () => "/dest"),
  revealPath: vi.fn(async () => undefined),
}));

import { useDocumentStore } from "../store/documentStore";
import { ExportDialog } from "./ExportDialog";
import { useExportStore } from "./exportStore";

beforeEach(() => {
  useExportStore.getState().closeDialog();
  useDocumentStore.getState().reset();
});

describe("ExportDialog", () => {
  it("renders nothing when closed", () => {
    const { container } = render(<ExportDialog />);
    expect(container.firstChild).toBeNull();
  });

  it("disables Export and lists blocking reasons while the graph is invalid", () => {
    // Mark the pipeline invalid + supply a structural blocker.
    useDocumentStore.setState({ pipelineValid: false });
    useExportStore.setState({
      open: true,
      phase: "form",
      destDir: "/dest",
      bundleName: "p",
      validating: false,
      validation: {
        blockers: [{ kind: "uncompiledGraphPass", passId: "g0", passName: "Graph" }],
      },
    });
    render(<ExportDialog />);

    const exportBtn = screen.getByTestId("export-confirm");
    expect(exportBtn).toBeDisabled();
    // The exact reason is shown + linked into the editor.
    expect(screen.getByTestId("export-blockers")).toHaveTextContent("Graph");
    expect(screen.getByRole("button", { name: "Go to pass" })).toBeInTheDocument();
  });

  it("enables Export for a valid pipeline with a destination chosen", () => {
    useDocumentStore.setState({ pipelineValid: true });
    useExportStore.setState({
      open: true,
      phase: "form",
      destDir: "/dest",
      bundleName: "MyPreset",
      validating: false,
      validation: { blockers: [] },
    });
    render(<ExportDialog />);
    expect(screen.getByTestId("export-confirm")).not.toBeDisabled();
    expect(screen.queryByTestId("export-blockers")).toBeNull();
  });

  it("shows the written bundle path + a reveal action on success", () => {
    useExportStore.setState({
      open: true,
      phase: "done",
      result: {
        presetPath: "/dest/MyPreset/preset.slangp",
        passFiles: ["a.slang"],
        textureFiles: [],
        warnings: [],
      },
    });
    render(<ExportDialog />);
    expect(screen.getByTestId("export-done")).toHaveTextContent(
      "/dest/MyPreset/preset.slangp",
    );
    expect(
      screen.getByRole("button", { name: "Reveal in file manager" }),
    ).toBeInTheDocument();
  });

  it("surfaces a non-fatal write failure", () => {
    useDocumentStore.setState({ pipelineValid: true });
    useExportStore.setState({
      open: true,
      phase: "error",
      destDir: "/dest",
      bundleName: "p",
      validating: false,
      validation: { blockers: [] },
      errorMessage: "Could not write the bundle: permission denied",
    });
    render(<ExportDialog />);
    expect(screen.getByTestId("export-error")).toHaveTextContent("permission denied");
  });

  it("links a blocking reason into the editor (opens the offending pass)", () => {
    const openPass = vi.fn();
    useDocumentStore.setState({ pipelineValid: false, openPass });
    useExportStore.setState({
      open: true,
      phase: "form",
      destDir: null,
      bundleName: "p",
      validating: false,
      validation: {
        blockers: [{ kind: "uncompiledGraphPass", passId: "g0", passName: "Graph" }],
      },
    });
    render(<ExportDialog />);
    fireEvent.click(screen.getByRole("button", { name: "Go to pass" }));
    expect(openPass).toHaveBeenCalledWith("g0");
    // Jumping closes the dialog.
    expect(useExportStore.getState().open).toBe(false);
  });
});
