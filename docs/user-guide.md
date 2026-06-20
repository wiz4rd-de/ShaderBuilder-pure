# ShaderBuilder user guide

ShaderBuilder is a desktop studio for building, previewing, and exporting
[RetroArch](https://www.retroarch.com/) **slang** shaders as a node graph. You
assemble a multi-pass shader pipeline visually, watch a live GPU preview update as
you edit, and export a ready-to-run `.slangp` bundle.

This guide covers every major surface. New here? Launch the app and click **Open
example** on the start screen for a working "CRT scanlines + curvature" preset to
explore, then come back to this guide. A focused import walkthrough lives in
[`import-walkthrough.md`](./import-walkthrough.md).

---

## 1. The start screen

On first launch (before any project is loaded) ShaderBuilder shows a welcome
screen with four ways in:

- **New project** — start from an empty single-pass graph.
- **Open example** — load the bundled CRT scanlines + curvature preset. It
  previews live immediately and exports cleanly, so it is the fastest way to see
  the whole loop.
- **Open project…** — reopen a saved `.json` project.
- **Import preset…** — bring in an existing RetroArch `.slangp` (see the import
  walkthrough).

Recent projects appear below the actions. The start screen also links to in-app
**Help** (the same dialog reachable from the title-bar **Help** button), which
holds the keyboard-shortcut reference.

---

## 2. The editing model

A project is a **pipeline** of one or more **passes**. Each pass produces exactly
one fragment shader; passes run in order, each reading the previous pass's output.
This mirrors RetroArch's `.slangp` model 1:1, so what you build is what exports.

There are two zoom levels, and you move between them on the canvas:

### Pipeline view

The top-level view shows the passes as a left-to-right chain. Here you:

- **Add / remove** passes.
- **Reorder** passes (pass order *is* the `.slangp` `shaderN` index).
- **Drill into** a pass to edit its graph.
- See each pass's referenced textures (`Source`, `PassOutputN`, history,
  feedback, LUTs) wired between passes.

### Per-pass graph

Drilling into a pass opens its **node graph** — the visual program for that pass's
fragment shader. You wire nodes from inputs (the source texture, UV coordinates,
parameters) through math/color/sampling operations to the pass **Output** node.

A pass can instead be **whole-pass code**: an opaque `.slang` body taken verbatim
(this is how imported presets and the bundled example are stored). Whole-pass code
is not decomposed into nodes; you edit its source directly in the code panel. You
can switch a pass between *graph* and *whole-pass code* in the pass settings.

### Editing, history, and selection

- **Add nodes** from the palette (toolbar **Add node**, or the palette menu).
- **Connect** ports by dragging from an output to an input. Connections are
  **type-checked while you drag**: only legal targets are offered, and a wire that
  needs a widen/broadcast is marked so you can see the conversion.
- **Undo / redo**, **copy / paste**, **duplicate**, **multi-select**, and
  **delete** all work as you would expect — see the shortcut reference in Help.
- **Subgraphs**: select several nodes and **Collapse** them into a reusable
  subgraph node; **Expand** to inline it back. Subgraphs can be saved to a
  library and reused across projects.

---

## 3. The node taxonomy

Nodes are grouped into categories (the palette is sectioned the same way):

- **Inputs / samplers** — the source texture and other sampler inputs (original,
  history, feedback, pass outputs, LUTs). These are the texture taps a pass reads.
- **Coordinates / UV** — the screen/texture coordinate and UV transforms.
- **Constants / parameters / builtins** — literal values, `#pragma parameter`
  knobs surfaced as live sliders, and runtime builtins (e.g. frame count, the
  various `*Size` vectors).
- **Math** — arithmetic, common functions (mix, clamp, dot, sin…), and the like,
  operating on the float/vector types.
- **Vector** — construct/swizzle/split vectors (e.g. build a `vec4`, take `.rgb`).
- **Color** — color-space and tone operations.
- **Custom (GLSL)** — drop to hand-written GLSL where the built-in nodes don't
  reach (see §5).
- **Output** — the pass's final color. Every graph pass terminates here.

Every node declares **typed** input and output ports (`Float`, `Vec2…Vec4`, `Int`,
`Bool`, `Sampler2D`). The connection rules (assignability, implicit widening,
broadcasting, swizzles) are enforced on connect, so an illegal wire is rejected
before it can produce a bad shader.

When a node has a problem (a type error, a cycle, an unbound input), it shows an
error/warning badge, and the problem is listed in the **Problems** panel with a
click-to-jump back to the offending node.

---

## 4. Preview controls

The preview pane shows a live GPU render of the current pipeline. It updates
automatically as you edit (a debounced compile → preview loop). When the pipeline
has a blocking error the preview holds the **last good** frame rather than going
blank, and the Problems panel flags that the preview is stale.

### Source and viewport

- **Source** — choose what feeds the first pass: a test pattern, a still image, or
  an image sequence. Playback controls (play / pause / step / seek / fps) drive a
  sequence.
- **Viewport** — set the output size and integer-scale behavior. You can also set
  a *simulated* viewport to preview at a target resolution.

### Parameters

Every `#pragma parameter` your passes declare appears as a **live slider**.
Dragging a slider re-renders immediately, so you tune the shader by feel. Project-
and pass-level parameters are reconciled into one set, the same way RetroArch
resolves them.

### Compare (A/B)

The preview can hold a **reference** frame and compare against it:

- **Set reference** captures the current output.
- Toggle **A/B** to flip between the live output and the reference, or use the
  **split** divider to show both halves at once and drag the seam.

Use this to judge a change against a known-good baseline.

### Pixel inspector

Hover the preview to **probe** the pixel under the cursor; click to **pin** a
sample. The inspector reads back the exact rendered pixel and shows its value
(the geometry maps the on-screen pane position back to the engine viewport), so
you can verify precise colors rather than eyeballing them.

---

## 5. Custom-GLSL nodes

Two escape hatches let you write GLSL directly when the node taxonomy isn't
enough:

- **Snippet node** — a small GLSL expression/statement node with typed input and
  output ports. It slots into a graph like any other node, so you can mix
  hand-written GLSL with visual nodes in the same pass.
- **Whole-pass code** — the entire pass body as opaque `.slang`. Switch a pass to
  whole-pass code in the pass settings and edit the source in the code panel.
  ShaderBuilder still recovers the pass's `#pragma parameter` sliders and the
  textures it references by a light textual scan, so parameters and pipeline
  wiring keep working even though the body is opaque.

Whole-pass code is exactly how imported presets and the bundled example are
stored, which is why they export byte-for-byte losslessly.

---

## 6. Importing a RetroArch preset

Use **Import preset…** (start screen) to bring an existing `.slangp` into the
editor. Each pass becomes a **whole-pass code** pass with its `.slang` source read
in verbatim; the pass's parameters and texture references are recovered, LUTs are
mapped, and the pipeline wiring is reconstructed. From there you can preview it,
tweak parameters, and re-export. See
[`import-walkthrough.md`](./import-walkthrough.md) for a step-by-step.

---

## 7. Exporting a bundle

**File ▸ Export Bundle…** writes a RetroArch-conventional `.slangp` bundle to a
folder you choose:

- a `preset.slangp` with **relative** paths,
- one `.slang` file per pass,
- a `textures/` folder for any LUT PNGs,
- inline parameter defaults.

Export is **gated**: ShaderBuilder validates the project first and refuses to
write an un-representable one (for example, a graph pass that doesn't compile, an
empty pass body, or a project with no passes). The export dialog lists the exact
blockers, each linking back into the Problems panel, so you know what to fix. A
clean project exports a bundle that re-imports and runs in RetroArch unchanged.

> The native **project file** (`.json`, via File ▸ Save) is separate from the
> exported `.slangp` bundle. The project file is your editable document; the bundle
> is the runnable artifact. Saving never touches a bundle, and exporting never
> touches your project file.

---

## 8. Saving, recovery, and the File menu

- **New / Open / Open Recent / Save / Save As** live in the title-bar **File**
  menu (and on keyboard shortcuts — see Help).
- An asterisk in the title bar marks **unsaved edits**. Closing the window with
  unsaved work prompts you to save, discard, or cancel.
- ShaderBuilder **autosaves** your working document to a recovery file as you
  edit; if the app is killed, the next launch offers to restore that work.

---

## 9. Keyboard shortcuts

The full, always-current list is in the in-app **Help** dialog (title-bar **Help**
button, or the link on the start screen). It covers file actions (New/Open/Save/
Save As), edit actions (undo/redo, copy/paste, duplicate, delete), and canvas
gestures (pan, zoom, add-to-selection, box-select). On macOS, use **Cmd** wherever
**Ctrl** is shown.
