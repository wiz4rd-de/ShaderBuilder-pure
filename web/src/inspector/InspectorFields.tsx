// Field renderers (#47) — one small widget per InspectorFieldKind, driven ENTIRELY
// by the descriptor's field schema (never hand-coded per node kind). A field reads
// its current value out of `node.data[field.key]` and writes edits back through the
// NodeDataEditor: text/number inputs use the coalesced `live` path (one undo entry
// per typing burst); discrete select/boolean inputs `commit` one entry per change.
import { useEffect, useState } from "react";

import type { InspectorField } from "../nodes/types";
import type { NodeDataEditor } from "./useNodeDataEditor";

/** The numeric component count of a vecN field kind. */
const VEC_LEN: Record<string, number> = { vec2: 2, vec3: 3, vec4: 4 };
/** Per-component axis labels for vec editors. */
const VEC_AXES = ["x", "y", "z", "w"] as const;

interface FieldProps {
  field: InspectorField;
  /** The node's `data` (read the field's current value from here). */
  data: Record<string, unknown>;
  editor: NodeDataEditor;
}

/** Render the right widget for a field's kind. */
export function InspectorFieldRow({ field, data, editor }: FieldProps): React.JSX.Element {
  // Code fields are multi-line: stack the label above a full-width editor.
  const className =
    "inspector__field" + (field.kind === "code" ? " inspector__field--code" : "");
  return (
    <label className={className}>
      <span className="inspector__field-label">{field.label}</span>
      <FieldWidget field={field} data={data} editor={editor} />
    </label>
  );
}

function FieldWidget({ field, data, editor }: FieldProps): React.JSX.Element {
  switch (field.kind) {
    case "text":
      return <TextField field={field} data={data} editor={editor} />;
    case "code":
      return <CodeField field={field} data={data} editor={editor} />;
    case "number":
    case "integer":
      return <NumberField field={field} data={data} editor={editor} />;
    case "boolean":
      return <BooleanField field={field} data={data} editor={editor} />;
    case "select":
      return <SelectField field={field} data={data} editor={editor} />;
    case "vec2":
    case "vec3":
    case "vec4":
      return <VecField field={field} data={data} editor={editor} />;
  }
}

// ---- text -----------------------------------------------------------------

function TextField({ field, data, editor }: FieldProps): React.JSX.Element {
  const stored = typeof data[field.key] === "string" ? (data[field.key] as string) : "";
  const [value, setValue] = useState(stored);
  // Resync when the underlying value changes externally (undo/redo, reselect).
  useEffect(() => setValue(stored), [stored]);
  return (
    <input
      type="text"
      className="inspector__input"
      value={value}
      onChange={(e) => {
        setValue(e.target.value);
        editor.live({ [field.key]: e.target.value });
      }}
      onBlur={() => editor.flush()}
    />
  );
}

// ---- code (multi-line GLSL/slang body) ------------------------------------

function CodeField({ field, data, editor }: FieldProps): React.JSX.Element {
  const stored = typeof data[field.key] === "string" ? (data[field.key] as string) : "";
  const [value, setValue] = useState(stored);
  useEffect(() => setValue(stored), [stored]);
  return (
    <textarea
      className="inspector__input inspector__code"
      spellCheck={false}
      rows={8}
      value={value}
      onChange={(e) => {
        setValue(e.target.value);
        editor.live({ [field.key]: e.target.value });
      }}
      onBlur={() => editor.flush()}
    />
  );
}

// ---- number / integer -----------------------------------------------------

function NumberField({ field, data, editor }: FieldProps): React.JSX.Element {
  const isInt = field.kind === "integer";
  const stored =
    typeof data[field.key] === "number" && Number.isFinite(data[field.key] as number)
      ? (data[field.key] as number)
      : 0;
  const [value, setValue] = useState(String(stored));
  useEffect(() => setValue(String(stored)), [stored]);
  return (
    <input
      type="number"
      className="inspector__input"
      value={value}
      min={field.min}
      max={field.max}
      step={field.step ?? (isInt ? 1 : "any")}
      onChange={(e) => {
        setValue(e.target.value);
        const n = parseNumber(e.target.value, isInt);
        if (n !== null) {
          editor.live({ [field.key]: n });
        }
      }}
      onBlur={() => editor.flush()}
    />
  );
}

/** Parse an input string to a finite number (truncated for integers), or null. */
function parseNumber(raw: string, isInt: boolean): number | null {
  if (raw.trim() === "") {
    return null;
  }
  const n = Number(raw);
  if (!Number.isFinite(n)) {
    return null;
  }
  return isInt ? Math.trunc(n) : n;
}

// ---- boolean --------------------------------------------------------------

function BooleanField({ field, data, editor }: FieldProps): React.JSX.Element {
  const checked = data[field.key] === true;
  return (
    <input
      type="checkbox"
      className="inspector__checkbox"
      checked={checked}
      onChange={(e) => editor.commit({ [field.key]: e.target.checked })}
    />
  );
}

// ---- select ---------------------------------------------------------------

function SelectField({ field, data, editor }: FieldProps): React.JSX.Element {
  const options = field.options ?? [];
  const stored = typeof data[field.key] === "string" ? (data[field.key] as string) : "";
  const value = options.some((o) => o.value === stored)
    ? stored
    : (options[0]?.value ?? "");
  return (
    <select
      className="inspector__input"
      value={value}
      onChange={(e) => editor.commit({ [field.key]: e.target.value })}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

// ---- vecN -----------------------------------------------------------------

function VecField({ field, data, editor }: FieldProps): React.JSX.Element {
  const len = VEC_LEN[field.kind] ?? 0;
  const stored = Array.isArray(data[field.key]) ? (data[field.key] as unknown[]) : [];
  const current: number[] = Array.from({ length: len }, (_, i) => {
    const c = stored[i];
    return typeof c === "number" && Number.isFinite(c) ? c : 0;
  });
  const [text, setText] = useState(current.map(String));
  useEffect(() => setText(current.map(String)), [stored.join(",")]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="inspector__vec">
      {Array.from({ length: len }, (_, i) => (
        <input
          key={i}
          type="number"
          aria-label={VEC_AXES[i]}
          className="inspector__input inspector__vec-component"
          step={field.step ?? "any"}
          value={text[i] ?? ""}
          onChange={(e) => {
            const nextText = text.slice();
            nextText[i] = e.target.value;
            setText(nextText);
            const n = parseNumber(e.target.value, false);
            if (n !== null) {
              const next = current.slice();
              next[i] = n;
              editor.live({ [field.key]: next });
            }
          }}
          onBlur={() => editor.flush()}
        />
      ))}
    </div>
  );
}
