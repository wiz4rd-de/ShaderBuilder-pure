# Release checklist — v1 (Linux)

This is the operator runbook for cutting a ShaderBuilder release. v1 is
**Linux-only** (Decision Log #10) and ships as a self-contained **AppImage** +
**.deb**, built and published by CI. The app is **MIT** licensed and the shipped
artifact is **license-clean**: no GPL/LGPL ffmpeg dependency (Decision Log #13).

The version bump, bundler config, release workflow, README, and license guard all
land on a branch and merge to `main` through normal review. The **git tag** and
the **GitHub Release publish** are the only manual release actions.

---

## 1. Version consistency (already in the tree for v0.1.1)

The version must match across all three manifests, or the bundle metadata drifts:

| File | Field |
| --- | --- |
| `Cargo.toml` | `[workspace.package] version` (every crate inherits it) |
| `crates/app/tauri.conf.json` | `version` (the bundle/app version) |
| `web/package.json` | `version` |

To bump for a future release, edit all three to the same value, run
`cargo check -p app` (refreshes `Cargo.lock`), and commit `Cargo.lock`.

Quick consistency check:

```bash
grep -m1 '^version' Cargo.toml
grep '"version"' crates/app/tauri.conf.json
grep '"version"' web/package.json
```

## 2. License cleanliness (enforced)

`crates/testing/tests/license_cleanliness.rs` fails CI if any ffmpeg / gstreamer /
GPL codec crate enters `Cargo.lock`. ffmpeg stays optional and out-of-bundle; PNG
sequences decode in-core via the `image` crate. Manual spot check:

```bash
cargo tree -p app -e no-dev | grep -iE 'ffmpeg|gstreamer|x264|x265' || echo clean
```

## 3. What ships in the bundle

Configured in `crates/app/tauri.conf.json` → `bundle.resources`:

- `resources/example-project.json` — the #66 CRT example (also `include_str!`'d
  into the binary for `load_example_project`).
- `docs/user-guide.md`, `docs/example-project.md`, `docs/import-walkthrough.md`,
  `docs/README.md`, `docs/LICENSE` — user docs + license, under the install's
  resource dir.

Bundle targets: `appimage` + `deb`. The `.deb` declares `libwebkit2gtk-4.1-0` +
`libgtk-3-0` runtime depends; the AppImage carries them.

## 4. Cut the tag (manual — auto-mode may block this)

After the version bump is merged to `main`:

```bash
git checkout main && git pull
git tag -a v0.1.1 -m "ShaderBuilder v0.1.1"
git push origin v0.1.1
```

> If you are running under an automation mode that blocks tag creation, hand the
> tag push to the operator. The tag is the trigger; nothing else is needed.

Pushing the `v*` tag fires `.github/workflows/release.yml`, which:

1. installs the Tauri system deps (same step as CI),
2. builds the frontend (`npm ci && npm run build`),
3. runs `tauri build` → `target/release/bundle/{appimage,deb}/`,
4. creates a **draft** GitHub Release for the tag and uploads the AppImage + .deb,
5. also uploads the bundle as a workflow artifact.

## 5. Publish the Release (manual)

The workflow creates a **draft** Release so a human verifies before it goes live:

1. Open the draft Release on GitHub for the tag.
2. Confirm the AppImage and `.deb` are attached.
3. Run the smoke test (§6) against the attached artifact.
4. Edit the notes if needed, then **Publish**.

---

## 6. Release smoke test

The automated half runs in CI (the workflow proves the bundle *builds* and is
uploadable). The end-to-end "does it run on a clean box" half is **manual** — it
needs a webview + GPU, which the headless CI runner does not have. Do not skip it.

### 6a. Automated (CI)

- `release.yml` on `workflow_dispatch` builds the bundle with no Release publish
  and uploads `shaderbuilder-linux-bundle` as a workflow artifact. Trigger it from
  the Actions tab to get a downloadable AppImage/.deb for the manual run below.
- A green `release.yml` run proves: frontend builds, the Tauri bundler accepts
  `tauri.conf.json`, and the AppImage + .deb are produced.

### 6b. Manual (clean Linux box, NO dev toolchain)

Use a fresh VM/container or a machine with **no** Rust/Node/cargo installed, to
prove the artifact is self-contained.

1. **Launch** — download the artifact from the (draft) Release:
   - AppImage: `chmod +x ShaderBuilder_0.1.1_amd64.AppImage && ./ShaderBuilder_0.1.1_amd64.AppImage`
   - .deb: `sudo apt install ./ShaderBuilder_0.1.1_amd64.deb && ShaderBuilder`
   - On a headless/odd-GPU box, fall back to software GL:
     `LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 ./ShaderBuilder_*.AppImage`
   - PASS: the window opens to the start screen.
2. **Open the example** — start screen → **Open example**.
   - PASS: the "CRT Scanlines + Curvature" project loads into the editor.
3. **Preview streams** — the preview pane shows the multi-pass render (scanlines +
   curvature over the test source), updating live.
   - PASS: a non-placeholder frame is visible (not the flat "waiting" gray).
4. **Edit reflects live** — tweak a parameter (e.g. a scanline knob).
   - PASS: the preview updates.
5. **Export a bundle** — File ▸ Export (or the export action) → choose a temp dir.
   - PASS: a `.slangp` + `.slang` set is written and the validation gate passes;
     "reveal" opens the output folder.
6. **Docs present** — confirm the bundled docs shipped with the install (AppImage:
   inside the mounted AppDir under `usr/lib/.../resources/docs`; .deb: under
   `/usr/lib/ShaderBuilder/resources/docs` or the app's resource dir).
   - PASS: `user-guide.md`, `README.md`, `LICENSE` are present.

Record PASS/FAIL for each step in the Release notes or the PR that cut the tag.
Any FAIL blocks publishing the Release.

### 6c. Headless functional proxy (already in CI)

The full editor→engine path is exercised headlessly by the workspace tests, so a
green build already proves the slice works on the GPU before any bundle is cut:

```bash
WGPU_BACKEND=vulkan cargo test -p testing --test example_project -- --test-threads=1
WGPU_BACKEND=vulkan cargo test -p preview-engine --test e2e_curvature
```

These render the bundled example / curvature slice through the real engine and
assert non-trivial output. They are the automated stand-in for "preview streams";
the manual §6b run confirms it also holds inside the packaged webview.
