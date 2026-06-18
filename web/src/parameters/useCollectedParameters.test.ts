import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Parameter } from "../bindings/Parameter";
import type { Project } from "../bindings/Project";
import { makeProject } from "../store/factories";
import { resetIdsForTest } from "../store/ids";
import { useDocumentStore } from "../store/documentStore";

// A controllable mock of the async whole-pass scanner so we can drive the exact
// cancel-before-resolve race that drops parameters (#11).
const deferreds: { resolve: (params: Parameter[]) => void }[] = [];
vi.mock("../wholePass/scanPassSource", () => ({
  scanPassSource: vi.fn(
    () =>
      new Promise((resolve) => {
        deferreds.push({
          resolve: (params: Parameter[]) => resolve({ parameters: params, references: [] }),
        });
      }),
  ),
}));

import { useCollectedParameters } from "./useCollectedParameters";

const PASS_ID = "wp-1";

/** A project with a single whole-pass code pass that declares one parameter. */
function wholePassProject(aliases: string[]): Project {
  const base = makeProject();
  return {
    ...base,
    passes: [
      {
        id: PASS_ID,
        name: "WP",
        source: {
          kind: "wholePassCode",
          source: "#pragma parameter foo \"Foo\" 0.5 0.0 1.0 0.1\n",
          filename: null,
          opaque: false,
        },
        parameters: [],
        settings: base.passes[0]!.settings,
        references: [],
      },
    ],
    pipeline: {
      ...base.pipeline,
      aliases: aliases.map((alias, i) => ({ alias, passIndex: i })),
    },
  };
}

const FOO: Parameter = {
  name: "foo",
  label: "Foo",
  default: 0.5,
  min: 0,
  max: 1,
  step: 0.1,
};

beforeEach(() => {
  resetIdsForTest();
  deferreds.length = 0;
  useDocumentStore.getState().reset();
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("useCollectedParameters (#11) — cancel-before-resolve retry", () => {
  it("retries a scan cancelled before it resolved, so its params still appear", async () => {
    act(() => useDocumentStore.getState().loadProject(wholePassProject([]), PASS_ID));

    const { result } = renderHook(() => useCollectedParameters());

    // First effect run kicked off scan #0; it has NOT resolved yet.
    expect(deferreds).toHaveLength(1);

    // A dep changes (an alias is added) BEFORE the first scan resolves — this
    // re-runs the effect and cancels the in-flight first scan.
    act(() => useDocumentStore.getState().loadProject(wholePassProject(["alias0"]), PASS_ID));

    // The re-run must retry the (same-source) scan rather than skip it as
    // already-scanned. Resolving the FIRST (cancelled) scan does nothing.
    expect(deferreds.length).toBeGreaterThanOrEqual(2);
    act(() => deferreds[0]!.resolve([FOO])); // cancelled — ignored
    expect(result.current.find((p) => p.name === "foo")).toBeUndefined();

    // Resolving the retry surfaces the parameter.
    act(() => deferreds[deferreds.length - 1]!.resolve([FOO]));
    await waitFor(() => {
      expect(result.current.find((p) => p.name === "foo")).toBeDefined();
    });
  });
});
