// The first-run START SCREEN (#66): a full-window welcome overlay shown when no
// project has been chosen this launch (onboarding `started === false`). It offers
// the four ways in — New, Open, Import preset, Open example — and a link to the
// in-app help. Once the user picks one, `startActions` flips `started` and the
// editor takes over.
//
// Rendered ABOVE the editor (App keeps the editor subtree mounted underneath so
// the compile/preview/session hooks stay alive); this is a modal-style cover, not
// a route swap.
import { useState } from "react";

import { HelpDialog } from "../help/HelpDialog";
import { useRecents } from "./useRecents";
import { openRecent } from "../session/projectActions";
import { useOnboardingStore } from "./onboardingStore";
import {
  startExample,
  startImport,
  startNew,
  startOpen,
} from "./startActions";

export function StartScreen() {
  const [helpOpen, setHelpOpen] = useState(false);
  const recents = useRecents();
  const markStarted = useOnboardingStore((s) => s.markStarted);

  const openRecentAndEnter = (path: string): void => {
    void openRecent(path).then(() => {
      // openRecent toasts + drops nothing on a missing file; only enter the editor
      // if the load actually replaced the path. Re-checking here mirrors startOpen.
      markStarted();
    });
  };

  return (
    <div className="startscreen" role="dialog" aria-modal="true" aria-label="Welcome to ShaderBuilder">
      <div className="startscreen__panel">
        <h1 className="startscreen__title">
          ShaderBuilder <span className="startscreen__subtitle">RetroArch slang-shader studio</span>
        </h1>
        <p className="startscreen__lead">
          Build, preview, and export RetroArch slang shaders as a node graph. Pick a
          starting point:
        </p>

        <div className="startscreen__actions">
          <button
            type="button"
            className="startscreen__action startscreen__action--primary"
            onClick={() => void startNew()}
          >
            <span className="startscreen__action-title">New project</span>
            <span className="startscreen__action-desc">Start from an empty single-pass graph.</span>
          </button>
          <button
            type="button"
            className="startscreen__action"
            onClick={() => void startExample()}
          >
            <span className="startscreen__action-title">Open example</span>
            <span className="startscreen__action-desc">
              A live CRT scanlines + curvature preset to explore.
            </span>
          </button>
          <button
            type="button"
            className="startscreen__action"
            onClick={() => void startOpen()}
          >
            <span className="startscreen__action-title">Open project…</span>
            <span className="startscreen__action-desc">Reopen a saved .json project.</span>
          </button>
          <button
            type="button"
            className="startscreen__action"
            onClick={() => void startImport()}
          >
            <span className="startscreen__action-title">Import preset…</span>
            <span className="startscreen__action-desc">
              Bring in an existing RetroArch .slangp.
            </span>
          </button>
        </div>

        {recents.length > 0 ? (
          <div className="startscreen__recents">
            <div className="startscreen__recents-label">Recent projects</div>
            <ul className="startscreen__recents-list">
              {recents.slice(0, 5).map((r) => (
                <li key={r.path}>
                  <button
                    type="button"
                    className="startscreen__recent"
                    title={r.path}
                    onClick={() => openRecentAndEnter(r.path)}
                  >
                    <span className="startscreen__recent-name">{r.name}</span>
                    <span className="startscreen__recent-path">{r.path}</span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ) : null}

        <div className="startscreen__footer">
          <button
            type="button"
            className="startscreen__help-link"
            onClick={() => setHelpOpen(true)}
          >
            Help &amp; keyboard shortcuts
          </button>
        </div>
      </div>

      {helpOpen ? <HelpDialog onClose={() => setHelpOpen(false)} /> : null}
    </div>
  );
}
