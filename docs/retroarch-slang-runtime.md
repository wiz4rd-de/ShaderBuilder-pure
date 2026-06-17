# RetroArch slang Runtime — Fidelity Reference (Phase 2)

Internal fidelity reference for the ShaderBuilder Phase-2 engine: a faithful Rust/wgpu
reimplementation of RetroArch's *slang* multi-pass shader-preset runtime. Distilled and
de-duplicated from the libretro slang docs + `SHADER_SPEC.md`, the **librashader** pure-Rust
reimplementation (`librashader-runtime`, `-runtime-vk`, `-runtime-wgpu`, `librashader-presets`,
`librashader-common`), and RetroArch C source (`gfx/video_shader_parse.c`, `gfx/video_driver.c`).
Where RetroArch C and librashader defaults conflict, **we follow RetroArch C** unless noted.
Conventions throughout: `N`/`K`/`i` = zero-based pass index; passes run `0 .. N-1`; "FBO" = a
pass's offscreen render target; every `*Size` uniform is `vec4(w, h, 1.0/w, 1.0/h)`. All identifiers
are **case-sensitive and exact**; the entire binding contract is **name-based**, never positional.

---

## 1. Preset format & keys (with defaults)

`.slangp` is a flat INI-style `key = value` list (no sections). Values may be quoted/unquoted;
booleans accept `true`/`false`. All paths are relative to the preset file. `N` = pass index.

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `shaders` | uint | (required) | Number of passes; passes are `0 .. shaders-1`. |
| `shaderN` | path | (required/pass) | `.slang` file for pass `N`. |
| `aliasN` | string | `""` | Semantic name for pass `N`. Enables binding `<alias>`, `<alias>Size`, `<alias>Feedback`, `<alias>FeedbackSize` from later passes. |
| `feedback_pass` | int | `-1` | Global pass index double-buffered for feedback; `-1` = none. |
| `scale_typeN` / `scale_type_xN` / `scale_type_yN` | enum | (see §2) | Scale mode; `_x`/`_y` override the combined key per axis. |
| `scaleN` / `scale_xN` / `scale_yN` | float or int | (see §2) | Scale factor; `_x`/`_y` override per axis. Parsed **int** for `absolute`, else **float**. |
| `float_framebufferN` | bool | `false` | `true` → RGBA16F target (`FBO_SCALE_FLAG_FP_FBO`). |
| `srgb_framebufferN` | bool | `false` | `true` → RGBA8 sRGB target (`FBO_SCALE_FLAG_SRGB_FBO`). |
| `filter_linearN` | bool | unspecified (→ §3) | `true`=linear, `false`=nearest filtering of this pass's input. |
| `wrap_modeN` | enum | `clamp_to_border` | Sampler wrap (see §3). |
| `mipmap_inputN` | bool | `false` | `true` → mip chain generated for this pass's input texture. |
| `frame_count_modN` | uint | `0` | If `>0`, `FrameCount` fed to pass `N` is `global_frame_count % mod`. Parsed `strtoul(.,.,0)`. |
| `textures` | string | `""` | `;`-separated LUT names, e.g. `"BORDER;OVERLAY"`. |
| `<NAME>` | path | (required if listed) | LUT image path. |
| `<NAME>_linear` | bool | `false` | LUT filter (`false`=nearest). |
| `<NAME>_wrap_mode` | enum | `clamp_to_border` | LUT wrap (string set as §3). |
| `<NAME>_mipmap` | bool | `false` | Generate LUT mips. |
| `parameters` | string | `""` | Informational `;`-list of declared parameter ids (not required for overrides). |
| `<param_id>` | float | (pragma INITIAL) | Bare top-level line overriding a `#pragma parameter` initial value (→ §8). |

Causal chain: `Original → Shader0 → FBO0 → Shader1 → FBO1 → … → ShaderN-1 → back buffer`.

---

## 2. Scale types & FBO sizing math

Scale-type strings (RetroArch `video_shader` parser → librashader `ScaleType`):

| String | RetroArch enum | librashader |
|--------|----------------|-------------|
| `source` | `RARCH_SCALE_INPUT` | `Input` |
| `viewport` | `RARCH_SCALE_VIEWPORT` | `Viewport` |
| `absolute` | `RARCH_SCALE_ABSOLUTE` | `Absolute` |
| (none/extension) | — | `Original` (no upstream string maps to it; ⚠ §11) |

Sizing inputs: `source` = this pass's input size (= `OriginalSize` for pass 0, else FBO `N-1` size);
`viewport` = the simulated final viewport size (`FinalViewportSize`, §9); `original` = `OriginalSize`.

Per-axis raw size (width shown; height identical with `.height`):

```
raw_w = match scale_type_x:
    source/Input -> source.width   * factor_x
    original     -> original.width * factor_x      # librashader extension
    viewport     -> viewport.width * factor_x
    absolute     -> factor_x                        # literal integer pixel count; input ignored

width = clamp( round(raw_w), 1, 16384 )            # MAX_TEXEL_SIZE = 16384; round-to-nearest
```

Default / chaining rules:
- `scaleN` sets **both** axes; if present, `scale_xN`/`scale_yN` are ignored.
- If only one of `scale_xN`/`scale_yN` is given, the other axis defaults to `source` × `1.0`.
- All scale keys present-but-factor-omitted: `type_x = type_y = source`, `scale = 1.0`.
- **All scale keys for a pass omitted** (the common case): block is not `FBO_SCALE_FLAG_VALID`; the
  implicit default differs by position:
  - **Intermediate pass:** `source × 1.0` (FBO == its input's size). With `source` scale, pass `n`'s
    FBO is relative to **FBO `n-1`**, not to `Original`.
  - **Last pass (incl. sole pass of a 1-pass preset):** renders at **full viewport**, bypasses its
    own FBO, draws straight to the back buffer. Effectively `viewport × 1.0`.
- If the **last pass DOES set any scale**, it renders to an FBO first, then that FBO is stretched to
  the back buffer.

**Final-pass rule (runtime):** the last pass targets the viewport with `OutputSize == FinalViewportSize`.
Exception: the `draw_last_pass_feedback` path (§4) allocates a final-pass FBO so the feedback ring can
capture the last pass output. Any pass using `viewport`/final-relative scale must **reallocate its FBO
when the window/viewport size changes**.

---

## 3. FBO formats & samplers (filter / wrap / mipmap)

**Render-target format** is selected (librashader `PassMeta::get_format_override()`):

```
if   srgb_framebufferN:  R8G8B8A8_SRGB        # 8-bit, hardware sRGB encode/decode
elif float_framebufferN: R16G16B16A16_SFLOAT  # 16-bit float (NOT 32-bit by default)
else:                    R8G8B8A8_UNORM        # default: 8-bit linear UNORM
```

- The shader's `#pragma format <FMT>` (§8) is **authoritative** and overrides these preset hints
  (which exist mainly for `.cgp`/`.glslp` back-compat). It can select e.g. `R32G32B32A32_SFLOAT`,
  `A2B10G10R10_UNORM_PACK32`.
- If both `srgb` and `float` are set, we treat **float as winner** (⚠ §11).
- **Final pass** rendering to the back buffer: format keys ignored; format = swapchain format.

**sRGB behavior:** sRGB targets do linear→sRGB encode on store and sRGB→linear decode on load (in HW);
the shader always works in linear space. Match by using the `*Srgb` wgpu format for **both** the
render-target view and the sampled view. Float targets are linear, preserve out-of-`[0,1]`/HDR; UNORM
is linear, clamped to `[0,1]`.

**Sampler state** (per source texture; LUTs use `<name>_linear`/`_wrap_mode`/`_mipmap`):

| Setting | RetroArch default | librashader default | v1 choice |
|---------|-------------------|---------------------|-----------|
| filter | unspecified → global `video_smooth` | `Linear` | **linear** (⚠ §11) |
| wrap | `clamp_to_border` (`RARCH_WRAP_DEFAULT=RARCH_WRAP_BORDER=0`) | `ClampToEdge` | **clamp_to_border** |
| mipmap | `false` | `false` | `false` |

Wrap-mode strings (`video_shader_wrap_str_to_mode`); unrecognized → `clamp_to_border`:

| String | RetroArch | librashader |
|--------|-----------|-------------|
| `clamp_to_border` | `RARCH_WRAP_BORDER` (DEFAULT) | `ClampToBorder` |
| `clamp_to_edge` | `RARCH_WRAP_EDGE` | `ClampToEdge` |
| `repeat` | `RARCH_WRAP_REPEAT` | `Repeat` |
| `mirrored_repeat` | `RARCH_WRAP_MIRRORED_REPEAT` | `MirroredRepeat` |

LUTs default to **nearest** filter. In practice essentially no preset sets `mipmap_input0 = true`.

---

## 4. Feedback (double-buffering)

`PassFeedbackK` / `<alias>Feedback` = pass `K`'s output from the **previous** frame. Reading feedback
is always causal in time, so **any pass may read any pass's feedback** (unlike `PassOutputK`, which
must come from an earlier pass *this* frame).

librashader keeps two parallel arrays, `output_framebuffers[i]` and `feedback_framebuffers[i]`.
Once per **frame**, before recording draws:

```
swap(output_framebuffers, feedback_framebuffers)        # feedback_* now holds LAST frame's outputs
feedback_textures[i] := view(feedback_framebuffers[i])  # what PassFeedbackK samples this frame
... run all passes writing into output_framebuffers[i] ...
# next frame's swap flips them: this frame's outputs become next frame's feedback
```

- The swap is **per-frame, not per-pass**. Both buffer sets need identical per-pass geometry/format.
- Only **one** frame of feedback per pass is supported.
- Opt-in: global `feedback_pass = <index>`, or alias-based (`<alias>Feedback` reference; a pass
  commonly reads its **own** previous output). A size-optimized impl allocates a feedback twin only
  for passes actually referenced (⚠ §11).
- **Cold start:** never-written feedback FBOs read as their clear color, assume transparent black
  `(0,0,0,0)`. `options.clear_history` force-clears.

---

## 5. Frame history ring (`Original`)

`OriginalHistoryK` exposes the **core input frame** `K` frames ago. Ring semantics (CONFIRMED):
`OriginalHistory0 ≡ Original` (the live current frame); `OriginalHistory1` = previous frame;
`OriginalHistory2` = two frames ago; … (so "History1 == previous", not History0).

- **Depth:** no fixed limit; required depth = max `K` referenced across the preset (+1 for slot 0).
  The legacy "≤7" is a Cg artifact (`PREV` + `PREV1..PREV6`); slang removed the cap.
- Advancement (librashader `push_history`), once per **source frame, after all passes render**
  (not per pass):

```
ring = history_framebuffers          # VecDeque: front = newest, back = oldest
slot = ring.pop_back()               # recycle oldest
if slot.size != input.size: slot.reallocate
slot.copy_from(input)                # copy THIS frame's Original into recycled slot
ring.push_front(slot)                # now the freshest history entry
```

- Resolution on frame `F`: `OriginalHistory0` = live `Original(F)`; `OriginalHistoryK (K≥1)` = ring
  slot holding `Original(F-K)`.
- **Cold start:** unwritten ring slots are cleared (black/zero). Feedback (§4) is one frame back only.

---

## 6. Builtin semantic uniforms

Matched by **exact member name** inside the single UBO or single Push block. Every `*Size` packs
`vec4(w, h, 1/w, 1/h)` (CONFIRMED). Unique scalar semantics:

| Member name | GLSL type | Meaning |
|-------------|-----------|---------|
| `MVP` | `mat4` | Model-View-Projection (vertex). Final vertex: `gl_Position = MVP * Position`. |
| `OutputSize` | `vec4` | Current pass's render-target size. |
| `FinalViewportSize` | `vec4` | Final output viewport size (any pass). |
| `FrameCount` | `uint` | +1/frame; pre-wrapped by `frame_count_modN` if set (default mod 0 = no wrap). |
| `FrameDirection` | `int` | `+1` normal, `-1` rewinding. |
| `Rotation` | `uint` | Content rotation 0..3 → 0/90/180/270°. |
| `OriginalAspect` | `float` | Core-reported content aspect. |
| `OriginalAspectRotated` | `float` | Aspect adjusted for `Rotation`. |
| `OriginalFPS` | `float` | Core frame rate. |
| `FrameTimeDelta` | `uint` | Microseconds since previous frame. |
| `CurrentSubFrame` | `uint` | Current subframe index (BFI). |
| `TotalSubFrames` | `uint` | Total subframes per frame. |

Per-texture size uniforms — name = `<TextureName>Size`, `vec4(w,h,1/w,1/h)`:
`OriginalSize`, `SourceSize`, `OriginalHistoryNSize`, `PassOutputNSize` (`PassNSize` alias, ⚠ §11),
`PassFeedbackNSize`, `<ALIAS>Size`, `<ALIAS>FeedbackSize`, `<LUTNAME>Size`/`UserNSize`.

HDR/sensor extensions (in librashader's enum; not fed by all frontends — guard with `#ifdef`):
`HDRMode` (uint), `BrightnessNits`/`Scanlines`/`InverseTonemap`/`HDR10` (float),
`SubpixelLayout`/`ExpandGamut` (uint), `Gyroscope`/`Accelerometer`/`AccelerometerRest` (vec3).
Probe macros: `_HAS_ORIGINALASPECT_UNIFORMS`, `_HAS_FRAMETIME_UNIFORMS`. User params bind as `float`
members (librashader `UniqueSemantics::FloatParameter`).

---

## 7. Texture semantics & name → resource mapping

`sampler2D` may appear in the **fragment** stage only; each matched by name to a runtime resource.

| Sampler name | Indexed | Resolves to (pass `i`, frame `F`) |
|--------------|---------|-----------------------------------|
| `Original` | no | Core source frame for `F` (whole-chain input). Any pass. `≡ OriginalHistory0`. |
| `Source` | no | Output FBO of pass `i-1`; for pass 0, `Source == Original`. |
| `OriginalHistoryK` | yes | `Original` frame `F-K` (§5). |
| `PassOutputK` | yes | Pass `K`'s output **this frame**. Causal: error if `K ≥ i`. |
| `PassK` | yes | Accepted spelling for `PassOutputK` (canonicalized; ⚠ §11). |
| `PassFeedbackK` | yes | Pass `K`'s output from frame `F-1` (§4). |
| `UserK` | yes | LUT/lookup texture (loaded once, static); prefer the alias name. |
| `<ALIAS>` | — | A pass's `#pragma name FOO` (its output) or a preset LUT named `FOO`. `FOOFeedback` = its prev-frame output. |

Mapping rules (reflection): (1) strip trailing index digits → base semantic, digits = index `K`;
(2) `Original`/`Source` match directly; (3) aliases (`#pragma name` + `textures=` entries) are added to
the name table before reflection and take precedence, so `FOO`/`FOOSize`/`FOOFeedback`/`FOOFeedbackSize`
resolve; `UserN` is the un-aliased LUT fallback; (4) size uniform = base name + `Size` (+ same index).
Cold frames (history/feedback before populated) read transparent black.

---

## 8. `#pragma` directives & parameters

- `#pragma stage vertex|fragment` — marks following code's stage. **Both stages required.** Code before
  the first `#pragma stage` is shared. Vertex↔fragment linkage is by `location`, not name. Required IO:
  `layout(location=0) in vec4 Position;`, `layout(location=1) in vec2 TexCoord;` (`(0,0)` = top-left),
  `layout(location=0) out vec4 FragColor;`.
- `#pragma name IDENTIFIER` (a.k.a. `#pragma alias`) — names the pass; bindable as `IDENTIFIER` and
  `IDENTIFIERFeedback`. `#pragma name` is preferred.
- `#pragma format <FMT>` — output format; **default `R8G8B8A8_UNORM`**. Allowed list (8/10/16/32-bit):
  `R8_UNORM/UINT/SINT`, `R8G8_*`, `R8G8B8A8_UNORM/UINT/SINT/SRGB`, `A2B10G10R10_UNORM_PACK32/UINT_PACK32`,
  `R16_UINT/SINT/SFLOAT`, `R16G16_*`, `R16G16B16A16_*`, `R32_*`, `R32G32_*`, `R32G32B32A32_*`. GLES2
  guarantees only `R8G8B8A8`-class; float/10-bit need GLES3/GL3/Vulkan-class. wgpu maps e.g.
  `R8G8B8A8_UNORM`→`Rgba8Unorm`, `R8G8B8A8_SRGB`→`Rgba8UnormSrgb`, `R16G16B16A16_SFLOAT`→`Rgba16Float`.
- `#pragma parameter IDENTIFIER "DESCRIPTION" INITIAL MINIMUM MAXIMUM [STEP]` — `IDENTIFIER` becomes a
  `float` UBO/Push member matched by name; `INITIAL/MIN/MAX` are floats, `STEP` optional. **Merge rule:**
  the same id declared across multiple files must match exactly (default/min/max/step) or error.
- `#pragma include_optional "path"` (optional) / `#include "path"` (hard).

**Parameter overrides:** a bare top-level `IDENTIFIER = <float>` in the preset overrides a parameter's
runtime value. RetroArch (`video_shader_resolve_parameters` then
`video_shader_load_current_parameter_values`) writes the float into the parameter's **`current`** field
via `config_get_float`; the pragma `initial` is unchanged. The `parameters = "..."` list is informational;
overrides work via bare `id = value` regardless.

---

## 9. Simulated viewport (integer-scale / aspect / letterbox)

The final pass renders into a viewport rect `(x, y, w, h)` in the output window;
`FinalViewportSize = (w, h, 1/w, 1/h)`. Math from RetroArch `gfx/video_driver.c`
(`video_viewport_get_scaled_integer` / `..._get_scaled_aspect2`). Given window `(W,H)`, content base
`(cw,ch)`, `desired_aspect`, `device_aspect = W/H`, bias `vp_bias_x/y` (`0.5` = centered):

**Integer scale ON** (round down to integer multiple, then center):
```
max_scale = min( floor(W/cw), floor(H/ch) )
vp_w = cw*max_scale;  vp_h = ch*max_scale
x = floor((W - vp_w) * vp_bias_x);  y = floor((H - vp_h) * vp_bias_y)
```

**Integer OFF, keep aspect** (letterbox/pillarbox, centered):
```
delta = (desired_aspect / device_aspect - 1.0)/2.0 + 0.5
if device_aspect > desired_aspect:                    # pillarbox (bars L/R)
    x += round( W * ((0.5 - delta) * (vp_bias_x*2.0)) )
    vp_w = round( 2.0*W*delta );  vp_h = H
else:                                                 # letterbox (bars T/B)
    delta = (device_aspect / desired_aspect - 1.0)/2.0 + 0.5
    y += round( H * ((0.5 - delta) * (vp_bias_y*2.0)) )
    vp_h = round( 2.0*H*delta );  vp_w = W
```

**Aspect OFF** (stretch): `vp = (0, 0, W, H)`. "Smart integer" falls back to aspect scaling if integer
letterbox margins exceed ~12% of the display. The final draw uses an MVP mapping the fullscreen quad to
the viewport rect.

---

## 10. Per-frame render-loop pseudocode

```text
fn render_frame(chain, original_input, output_window, frame_count):
    # 0. resize check
    viewport = compute_viewport(output_window, desired_aspect, integer_scale, vp_bias)   # §9
    if viewport.size changed or first_frame:
        for each pass i: realloc output_fbo[i] & feedback_fbo[i] per scale (§2); full mips if needed

    # 1. feedback ping-pong (per FRAME)                                                   # §4
    swap(output_framebuffers, feedback_framebuffers)
    feedback_textures[i] := view(feedback_framebuffers[i])
    if opts.clear_history: clear all history_framebuffers                                 # §5

    # 2. frame-wide bindings
    Original          := original_input
    OriginalHistoryK  := history ring slot for F-K (cleared if not populated)             # §5
    FrameCount        := frame_count (% frame_count_modN if set)
    FrameDirection    := +1 normal / -1 rewind
    FinalViewportSize := viewport.size

    # 3. intermediate passes 0..N-2
    source = Original
    for i in 0 .. N-2:
        tgt = output_framebuffers[i]                          # size per §2
        OutputSize = tgt.size
        bind Source=source, Original, OriginalHistoryK,
             PassOutputK (K<i)=output_textures[K],
             PassFeedbackK=feedback_textures[K], UserK=luts
        draw fullscreen quad with pass[i].pipeline -> tgt     # sRGB/float per §3
        if tgt.max_miplevels > 1: generate_mipmaps(tgt)       # eager, immediately after draw
        output_textures[i] = view(tgt);  source = output_textures[i]

    # 4. final pass N-1 -> viewport
    OutputSize = FinalViewportSize
    bind Source=source, Original, history, PassOutput(<N-1), PassFeedback, luts
    if final pass is itself fed back (draw_last_pass_feedback):                           # §4 edge
        draw -> output_framebuffers[N-1]    # capture for feedback ring
    draw final pass with MVP mapping quad -> viewport rect    # direct to viewport, no own FBO

    # 5. advance Original history (per FRAME, after all passes)                           # §5
    push_history(original_input)
    # next frame's swap turns this frame's outputs into feedback
```

**Mipmap timing (§6/render):** generate a pass's mips **immediately after that pass draws**, before any
consumer samples it; only when some consumer requested `mipmap_input` on that texture (FBO allocated
with full mip count). Generation via blit-down chain or a dedicated mip pass.

---

## Decisions for our wgpu implementation

Codifying the Phase-1 convention (`crates/preview-engine/src/renderer.rs` bind-group layout) and
extending it for Phase 2. We use **separate `texture2d` + `sampler`**, never combined `sampler2D`
(slang's combined-image-samplers are split during SPIR-V→WGSL via naga; we must allocate one texture
binding and one sampler binding per slang `sampler2D`).

Fixed Phase-1 bindings (descriptor set 0, honored by reflection-by-name):

| binding | resource | visibility |
|---------|----------|------------|
| 0 | **builtin UBO** (`MVP` + `*Size` + scalar semantics) | VERTEX_FRAGMENT |
| 1 | `Source` texture | FRAGMENT |
| 2 | sampler for `Source` | FRAGMENT |
| 3 | **parameter UBO** (`#pragma parameter` floats) | VERTEX_FRAGMENT |

Phase-2 binding assignment (reflection-by-name): after reflecting each pass, we map every discovered
texture semantic (`PassOutputK`, `PassFeedbackK`, `OriginalHistoryK`, `Original`, `<alias>`, `<LUT>`/
`UserK`) to a `(texture, sampler)` binding pair allocated above the fixed slots. Each pair takes two
adjacent/derived binding numbers; the bind-group layout is built from the reflected name set per pass,
so a pass binds only what it actually references (unused semantics are not bound). `Source`/sampler keep
slots 1/2; the builtin UBO stays at 0 and the parameter UBO stays at 3 for continuity with Phase 1.
Push-constant blocks are emulated as a second uniform buffer (baseline WebGPU lacks native push
constants); its binding is an implementation choice — we reserve a dedicated slot rather than overload
binding 3. librashader caps to honor: `MAX_BINDINGS_COUNT = 16`, push/emulated-push ≤ `128` bytes
(std140 UBO layout, offsets from reflection).

---

## Open questions / fidelity risks

Each ⚠ flag carried from the source notes, with our v1 default (prefer RetroArch C over librashader):

1. **`original` scale-type string** — librashader has `ScaleType::Original` but no upstream preset
   string maps to it; the C parser recognizes only `source`/`viewport`/`absolute`. **v1:** accept only
   the three C strings; treat `original` as unsupported (warn if seen).
2. **Last-pass scale default** — unspecified last pass = `viewport × 1.0` rendered straight to back
   buffer; unspecified intermediate = `source × 1.0`. **v1:** apply this final-pass-to-viewport rule at
   draw time.
3. **srgb + float both set** — tie-break unstated. **v1:** **float wins** (RGBA16F).
4. **`filter_linearN` default** — RetroArch leaves it unspecified (resolves to global `video_smooth`);
   librashader defaults `Linear`. **v1:** **linear** (matches RetroArch-in-practice).
5. **`wrap_modeN` / LUT wrap default** — RetroArch = `clamp_to_border`; librashader = `clamp_to_edge`.
   **v1:** **`clamp_to_border`** (RetroArch fidelity).
6. **LUT `<NAME>_repeat` legacy key** — Cg-only; not in the slang spec; unclear if RetroArch's slang
   path honors it. **v1:** ignore for slang; read only `<NAME>_wrap_mode`.
7. **Parameter override clamping** — RetroArch stores the raw float into `current`; UI clamps. **v1:**
   store raw, **clamp to `[MIN,MAX]` at use time**.
8. **`parameters = "..."` strictness** — uncertain how strictly upstream uses it; overrides work via
   bare `id = value`. **v1:** treat as informational; rely on bare `id = value`.
9. **`PassN` vs `PassOutputN` spelling** — librashader canonicalizes both to `PassOutput`. **v1:**
   accept both, normalize to `PassOutput`.
10. **Emulated push-constant binding number** — not fixed by spec (an implementation choice). **v1:**
    reserve a dedicated bind slot for the emulated push UBO (see Decisions); do not overload binding 3.
11. **Per-pass texture+sampler binding-numbering scheme** — paired numbering is impl-defined. **v1:**
    reflection-by-name, allocate adjacent `(texture, sampler)` pairs above the fixed 0–3 slots.
12. **Multiple alias-based feedbacks vs single global `feedback_pass`** — upstream stores one global
    index, but multi-pass alias feedback is used in practice (CRT presets). **v1:** scan each `.slang`
    for `<alias>Feedback`/`PassFeedbackK`, allocate a feedback twin for **every** referenced pass.
13. **First-frame feedback clear color** — driver-defined in some cases. **v1:** clear to `(0,0,0,0)` on
    allocation (librashader-consistent).
14. **`OriginalHistory0` as ring slot vs live input** — functionally identical. **v1:** treat slot 0 as
    the **live `Original`** (librashader approach).
15. **History/feedback texture mipmapping** — uncertain whether RetroArch mips history textures. **v1:**
    generate mips only per actual consumer `mipmap_input` requirement.
16. **sRGB conversion on `Original`/LUT inputs** — slang treats core frame as already-linear-ish 8-bit
    unless a LUT declares `_srgb`/`_linear`. **v1:** honor per-LUT srgb/linear flags; leave core
    `Original` in the frontend-provided format. Final/viewport sRGB-ness = swapchain format (last-pass
    `srgb_framebuffer` ignored).
17. **Viewport aspect-corrected base-size derivation** — differs between integer/non-integer paths.
    **v1:** drive from explicit `(desired_aspect, integer_scale, vp_bias)` config rather than RetroArch's
    aspect-ratio-index table.
18. **wgpu push-constant emulation (binding)** — exact wgpu emulation binding not fixed by spec.
    **v1:** as #10 (dedicated reserved slot).
