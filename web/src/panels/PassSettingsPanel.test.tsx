import { fireEvent, render, screen, act } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { useDocumentStore } from "../store/documentStore";
import { resetIdsForTest } from "../store/ids";
import { PassSettingsPanel } from "./PassSettingsPanel";

function store() {
  return useDocumentStore.getState();
}

function activeSettings() {
  const s = store();
  return s.project.passes.find((p) => p.id === s.activePassId)!.settings;
}

beforeEach(() => {
  resetIdsForTest();
  store().reset();
});

describe("PassSettingsPanel", () => {
  it("writes a scale type + factor into Pass.settings", () => {
    render(<PassSettingsPanel />);
    fireEvent.change(screen.getByLabelText("scaleX scale type"), {
      target: { value: "absolute" },
    });
    expect(activeSettings().scaleX.scaleType).toBe("absolute");
    fireEvent.change(screen.getByLabelText("scaleX scale factor"), {
      target: { value: "320" },
    });
    expect(activeSettings().scaleX.scale).toBe(320);
  });

  it("maps the FBO format select onto the float/srgb flags", () => {
    render(<PassSettingsPanel />);
    const select = screen.getByLabelText("FBO format");
    fireEvent.change(select, { target: { value: "float16" } });
    expect(activeSettings().floatFramebuffer).toBe(true);
    expect(activeSettings().srgbFramebuffer).toBe(null);
    fireEvent.change(select, { target: { value: "srgb" } });
    expect(activeSettings().floatFramebuffer).toBe(null);
    expect(activeSettings().srgbFramebuffer).toBe(true);
    fireEvent.change(select, { target: { value: "rgba8" } });
    expect(activeSettings().floatFramebuffer).toBe(null);
    expect(activeSettings().srgbFramebuffer).toBe(null);
  });

  it("edits filter, wrap, mipmap and alias", () => {
    render(<PassSettingsPanel />);
    fireEvent.change(screen.getByLabelText("Filter linear"), { target: { value: "off" } });
    expect(activeSettings().filterLinear).toBe(false);
    fireEvent.change(screen.getByLabelText("Wrap mode"), { target: { value: "repeat" } });
    expect(activeSettings().wrapMode).toBe("repeat");
    fireEvent.click(screen.getByLabelText("Mipmap input"));
    expect(activeSettings().mipmapInput).toBe(true);
    fireEvent.change(screen.getByLabelText("Alias"), { target: { value: "blur" } });
    expect(activeSettings().alias).toBe("blur");
  });

  it("toggles the project global feedback pass for the active pass index", () => {
    render(<PassSettingsPanel />);
    fireEvent.click(screen.getByLabelText("Feedback pass"));
    expect(store().project.feedbackPass).toBe(0);
    fireEvent.click(screen.getByLabelText("Feedback pass"));
    expect(store().project.feedbackPass).toBe(null);
  });

  it("shows a placeholder when there is no active pass", () => {
    act(() => useDocumentStore.setState({ activePassId: "missing" }));
    render(<PassSettingsPanel />);
    expect(screen.getByText(/No active pass/i)).toBeInTheDocument();
  });
});
