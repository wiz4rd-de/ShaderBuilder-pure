import { beforeEach, describe, expect, it } from "vitest";

import { useConfirmStore } from "./confirmStore";

beforeEach(() => {
  useConfirmStore.setState({ prompt: null });
});

describe("confirmStore", () => {
  it("ask opens a prompt and answer resolves it with the choice", async () => {
    const store = useConfirmStore.getState();
    const p = store.ask("Discard?", { confirm: "Save", discard: "Discard", cancel: "Cancel" });
    expect(useConfirmStore.getState().prompt?.message).toBe("Discard?");

    useConfirmStore.getState().answer("discard");
    await expect(p).resolves.toBe("discard");
    expect(useConfirmStore.getState().prompt).toBeNull();
  });

  it("a second ask auto-cancels the first awaiter", async () => {
    const store = useConfirmStore.getState();
    const first = store.ask("A", { confirm: "Yes", discard: "No" });
    const second = store.ask("B", { confirm: "Yes", discard: "No" });

    await expect(first).resolves.toBe("cancel");
    // Only the second prompt is live.
    expect(useConfirmStore.getState().prompt?.message).toBe("B");

    useConfirmStore.getState().answer("confirm");
    await expect(second).resolves.toBe("confirm");
  });

  it("answer with no live prompt is a no-op", () => {
    expect(() => useConfirmStore.getState().answer("confirm")).not.toThrow();
  });
});
