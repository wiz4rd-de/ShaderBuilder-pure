import { describe, expect, it } from "vitest";

import { tokenizeSlang, type SlangToken } from "./highlightSlang";

/** Concatenate the token texts (must round-trip the input exactly). */
function joined(tokens: SlangToken[]): string {
  return tokens.map((t) => t.text).join("");
}

/** The token type covering character index `at` in `source`. */
function typeAt(source: string, at: number): SlangToken["type"] {
  let pos = 0;
  for (const t of tokenizeSlang(source)) {
    if (at < pos + t.text.length) {
      return t.type;
    }
    pos += t.text.length;
  }
  throw new Error(`index ${at} out of range`);
}

describe("tokenizeSlang", () => {
  it("round-trips arbitrary source byte-for-byte", () => {
    const src = `#version 450\n// a comment\nvec4 main() { return texture(s, vUv) * 0.5; }\n`;
    expect(joined(tokenizeSlang(src))).toBe(src);
  });

  it("preserves whitespace and newlines as plain tokens", () => {
    const src = "a   b\n\tc";
    expect(joined(tokenizeSlang(src))).toBe(src);
  });

  it("classifies a preprocessor directive", () => {
    expect(typeAt("#version 450", 1)).toBe("directive");
  });

  it("classifies a line comment to end of line, not past it", () => {
    const src = "// hi\nfloat";
    expect(typeAt(src, 2)).toBe("comment");
    expect(typeAt(src, src.indexOf("float"))).toBe("type");
  });

  it("classifies a block comment", () => {
    expect(typeAt("/* x */", 3)).toBe("comment");
  });

  it("classifies keywords, types, and builtins distinctly", () => {
    expect(typeAt("return", 0)).toBe("keyword");
    expect(typeAt("vec4", 0)).toBe("type");
    expect(typeAt("texture", 0)).toBe("builtin");
  });

  it("classifies numbers including floats and suffixes", () => {
    expect(typeAt("0.5", 1)).toBe("number");
    expect(typeAt("1.0e-3", 2)).toBe("number");
    expect(typeAt(".25", 0)).toBe("number");
  });

  it("treats an unknown identifier as plain", () => {
    expect(typeAt("myVar", 0)).toBe("plain");
  });

  it("classifies punctuation", () => {
    expect(typeAt("a;", 1)).toBe("punctuation");
  });

  it("handles an unterminated block comment without looping", () => {
    const src = "/* never closed";
    const tokens = tokenizeSlang(src);
    expect(joined(tokens)).toBe(src);
    expect(tokens[0]!.type).toBe("comment");
  });

  it("handles empty source", () => {
    expect(tokenizeSlang("")).toEqual([]);
  });
});
