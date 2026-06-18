// A tiny, dependency-free slang/GLSL tokenizer for the READ-ONLY generated-code
// viewer (#55). The generated source is OUTPUT-ONLY (Decision Log #5) — it is
// never re-parsed back into nodes — so this need only colour tokens for display,
// not produce an editable/semantic model. A heavyweight editor would be overkill.
//
// It splits source into a flat list of typed tokens; the viewer maps each token
// class to a CSS class. The tokenizer is greedy + linear (single left-to-right
// pass), preserving every character (including whitespace + newlines) so the
// joined token text is byte-identical to the input — the viewer can render it in
// a <pre> verbatim.

/** A classified slice of source. `text` preserves the exact characters. */
export interface SlangToken {
  type:
    | "comment"
    | "directive"
    | "keyword"
    | "type"
    | "builtin"
    | "number"
    | "string"
    | "punctuation"
    | "plain";
  text: string;
}

// GLSL/slang control + qualifier keywords.
const KEYWORDS = new Set([
  "if", "else", "for", "while", "do", "return", "break", "continue", "discard",
  "switch", "case", "default", "struct", "const", "in", "out", "inout",
  "uniform", "layout", "flat", "smooth", "noperspective", "centroid", "precision",
  "highp", "mediump", "lowp", "void", "true", "false",
]);

// GLSL/slang built-in scalar/vector/matrix/sampler types.
const TYPES = new Set([
  "float", "double", "int", "uint", "bool",
  "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4", "uvec2", "uvec3", "uvec4",
  "bvec2", "bvec3", "bvec4", "dvec2", "dvec3", "dvec4",
  "mat2", "mat3", "mat4", "mat2x2", "mat3x3", "mat4x4",
  "sampler2D", "sampler3D", "samplerCube", "texture2D",
]);

// A representative set of GLSL built-in intrinsics the codegen emits.
const BUILTINS = new Set([
  "texture", "textureLod", "texelFetch", "mix", "clamp", "min", "max", "pow",
  "sin", "cos", "tan", "abs", "floor", "ceil", "fract", "mod", "sqrt",
  "inversesqrt", "exp", "log", "exp2", "log2", "sign", "step", "smoothstep",
  "normalize", "dot", "cross", "length", "distance", "reflect", "refract",
  "main",
]);

const IDENT_START = /[A-Za-z_]/;
const IDENT_PART = /[A-Za-z0-9_]/;
const DIGIT = /[0-9]/;
const SPACE = /\s/;
const PUNCT = /[{}()[\];,.<>+\-*/%=&|^!~?:]/;

/**
 * Tokenize `source` into a flat token list (no allocation per character beyond the
 * tokens). Whitespace is emitted as `plain` tokens so the joined text round-trips.
 * Unterminated comments/strings consume to end-of-input (the viewer never feeds
 * partial source — generated slang is complete — but be robust regardless).
 */
export function tokenizeSlang(source: string): SlangToken[] {
  const tokens: SlangToken[] = [];
  const n = source.length;
  let i = 0;

  const push = (type: SlangToken["type"], text: string): void => {
    if (text.length > 0) {
      tokens.push({ type, text });
    }
  };

  while (i < n) {
    const c = source[i]!;

    // Line comment.
    if (c === "/" && source[i + 1] === "/") {
      let j = i + 2;
      while (j < n && source[j] !== "\n") {
        j += 1;
      }
      push("comment", source.slice(i, j));
      i = j;
      continue;
    }

    // Block comment.
    if (c === "/" && source[i + 1] === "*") {
      let j = i + 2;
      while (j < n && !(source[j] === "*" && source[j + 1] === "/")) {
        j += 1;
      }
      j = Math.min(j + 2, n);
      push("comment", source.slice(i, j));
      i = j;
      continue;
    }

    // Preprocessor directive: a `#` that begins a line (after optional spaces).
    if (c === "#") {
      let j = i + 1;
      while (j < n && source[j] !== "\n") {
        j += 1;
      }
      push("directive", source.slice(i, j));
      i = j;
      continue;
    }

    // String literal (slang/GLSL rarely use these, but #include paths do).
    if (c === '"') {
      let j = i + 1;
      while (j < n && source[j] !== '"' && source[j] !== "\n") {
        j += 1;
      }
      j = Math.min(j + 1, n);
      push("string", source.slice(i, j));
      i = j;
      continue;
    }

    // Number: digits with optional fraction / exponent / float suffix. Also a
    // leading-dot float like `.5`.
    if (DIGIT.test(c) || (c === "." && i + 1 < n && DIGIT.test(source[i + 1]!))) {
      let j = i;
      while (j < n && /[0-9.eExXa-fA-F+\-uUfF]/.test(source[j]!)) {
        // Stop a `+`/`-` that is not an exponent sign.
        if ((source[j] === "+" || source[j] === "-") && !/[eE]/.test(source[j - 1] ?? "")) {
          break;
        }
        j += 1;
      }
      push("number", source.slice(i, j));
      i = j;
      continue;
    }

    // Identifier / keyword / type / builtin.
    if (IDENT_START.test(c)) {
      let j = i + 1;
      while (j < n && IDENT_PART.test(source[j]!)) {
        j += 1;
      }
      const word = source.slice(i, j);
      if (KEYWORDS.has(word)) {
        push("keyword", word);
      } else if (TYPES.has(word)) {
        push("type", word);
      } else if (BUILTINS.has(word)) {
        push("builtin", word);
      } else {
        push("plain", word);
      }
      i = j;
      continue;
    }

    // Whitespace run (kept as plain to round-trip exactly).
    if (SPACE.test(c)) {
      let j = i + 1;
      while (j < n && SPACE.test(source[j]!)) {
        j += 1;
      }
      push("plain", source.slice(i, j));
      i = j;
      continue;
    }

    // Punctuation / operator run.
    if (PUNCT.test(c)) {
      let j = i + 1;
      while (j < n && PUNCT.test(source[j]!)) {
        j += 1;
      }
      push("punctuation", source.slice(i, j));
      i = j;
      continue;
    }

    // Anything else: a single plain character (keeps the pass linear + total).
    push("plain", c);
    i += 1;
  }

  return tokens;
}
