import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createParameterSender } from "./setParameter";

describe("createParameterSender", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // Force the setTimeout fallback path so we can advance time deterministically.
    vi.stubGlobal("requestAnimationFrame", undefined);
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("coalesces many sets per frame into the latest value per name", () => {
    const send = vi.fn();
    const sender = createParameterSender(send);
    sender.set("gamma", 0.1);
    sender.set("gamma", 0.2);
    sender.set("gamma", 0.3);
    expect(send).not.toHaveBeenCalled(); // nothing flushed yet
    vi.advanceTimersByTime(16);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith("gamma", 0.3);
  });

  it("sends the latest value per distinct name in one flush", () => {
    const send = vi.fn();
    const sender = createParameterSender(send);
    sender.set("a", 1);
    sender.set("b", 2);
    sender.set("a", 3);
    vi.advanceTimersByTime(16);
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenCalledWith("a", 3);
    expect(send).toHaveBeenCalledWith("b", 2);
  });

  it("flush() sends pending values synchronously", () => {
    const send = vi.fn();
    const sender = createParameterSender(send);
    sender.set("x", 5);
    sender.flush();
    expect(send).toHaveBeenCalledWith("x", 5);
  });

  it("cancel() drops pending values without sending", () => {
    const send = vi.fn();
    const sender = createParameterSender(send);
    sender.set("x", 5);
    sender.cancel();
    vi.advanceTimersByTime(100);
    expect(send).not.toHaveBeenCalled();
  });
});
