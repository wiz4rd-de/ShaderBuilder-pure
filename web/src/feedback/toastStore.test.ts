import { act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useToastStore } from "./toastStore";

function store() {
  return useToastStore.getState();
}

beforeEach(() => {
  vi.useFakeTimers();
  store().clear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("toastStore", () => {
  it("pushes a toast and returns its id", () => {
    const id = store().push("error", "boom");
    expect(store().toasts).toHaveLength(1);
    expect(store().toasts[0]).toMatchObject({ id, severity: "error", message: "boom" });
  });

  it("de-dupes an identical live toast (severity + message)", () => {
    const a = store().push("error", "boom");
    const b = store().push("error", "boom");
    expect(a).toBe(b);
    expect(store().toasts).toHaveLength(1);
  });

  it("keeps distinct toasts that differ in severity or message", () => {
    store().push("error", "boom");
    store().push("warning", "boom");
    store().push("error", "other");
    expect(store().toasts).toHaveLength(3);
  });

  it("auto-dismisses after the severity timeout", () => {
    store().push("info", "hi");
    expect(store().toasts).toHaveLength(1);
    act(() => {
      vi.advanceTimersByTime(4000);
    });
    expect(store().toasts).toHaveLength(0);
  });

  it("dismisses a toast manually by id", () => {
    const id = store().push("error", "boom");
    store().dismiss(id);
    expect(store().toasts).toHaveLength(0);
  });
});
