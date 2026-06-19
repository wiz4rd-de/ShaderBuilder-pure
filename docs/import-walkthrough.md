# Import a slang-shaders preset and preview it

A short walkthrough: take an existing RetroArch `.slangp` preset, bring it into
ShaderBuilder, and see it preview live. This complements the full
[user guide](./user-guide.md).

## What you need

- A RetroArch slang preset: a `.slangp` file plus the `.slang` shader files (and
  any LUT `.png` textures) it references, with the relative paths intact. The
  community [`slang-shaders`](https://github.com/libretro/slang-shaders)
  repository is a large source of these; a single self-contained preset folder is
  the easiest to start with.

## Steps

1. **Launch ShaderBuilder.** On the start screen, click **Import preset…**. (If you
   already have a project open, the same action is available — importing replaces
   the current document, so save first if you have unsaved work.)

2. **Pick the `.slangp`.** A native file dialog opens, filtered to `.slangp`
   files. Choose the preset's `.slangp` (not one of the individual `.slang`
   files). ShaderBuilder reads the preset and every `.slang` it references.

3. **The preset loads as passes.** Each `shaderN` becomes one **whole-pass code**
   pass: its `.slang` source is read in verbatim (so a later re-export is
   byte-for-byte lossless), its `#pragma parameter` knobs become live sliders, and
   the textures it references — `Source`, `PassOutputN`, history, feedback, LUTs —
   are reconstructed for the pipeline wiring. LUT PNGs are mapped in.

4. **Watch the live preview.** The compile → preview loop runs automatically. Pick
   a **source** (a test pattern or an image) in the preview controls if the preview
   is empty, set the **viewport** size, and the rendered result of the imported
   chain appears. If a pass fails to compile, the preview holds the last good frame
   and the **Problems** panel explains why.

5. **Tune it.** Drag the **parameter sliders** to see the effect update in real
   time. Use the **A/B compare** (set a reference, then toggle or split) to judge a
   change against the original look, and the **pixel inspector** (hover/click the
   preview) to read exact pixel values.

6. **Re-export (optional).** **File ▸ Export Bundle…** writes the (possibly
   tweaked) preset back out as a clean `.slangp` bundle with relative paths, ready
   to drop back into RetroArch.

## Notes

- Importing brings passes in as **opaque whole-pass code**, not decomposed node
  graphs — ShaderBuilder does not reverse-engineer hand-written GLSL into visual
  nodes. You can still edit the source directly, and you can author *new* passes as
  node graphs alongside the imported ones.
- Unrecognized preset keys are **preserved** so a round trip doesn't lose them.
- If a referenced `.slang` or texture can't be read, the import still succeeds with
  that pass's body left empty and a note in the Problems/diagnostics — fix the path
  and re-import.
