import { describe, expect, it } from "vitest";

import { basename } from "./paths";

describe("basename", () => {
  it("returns the final POSIX path component", () => {
    expect(basename("/home/user/projects/foo.json")).toBe("foo.json");
  });

  it("returns the final Windows path component", () => {
    expect(basename("C:\\Users\\me\\bar.json")).toBe("bar.json");
  });

  it("handles a bare filename", () => {
    expect(basename("baz.json")).toBe("baz.json");
  });

  it("ignores a trailing separator", () => {
    expect(basename("/a/b/")).toBe("b");
  });
});
