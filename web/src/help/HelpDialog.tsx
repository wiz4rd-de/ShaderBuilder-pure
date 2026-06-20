// In-app HELP (#66): a lightweight modal with a keyboard-shortcut reference and a
// pointer to the full user guide. Reachable from the start screen and the editor
// toolbar's Help button. The shortcut table mirrors `useEditorShortcuts` (the one
// authority for the bindings) — keep them in sync.
import { SHORTCUT_GROUPS } from "./shortcuts";

export interface HelpDialogProps {
  onClose: () => void;
}

export function HelpDialog({ onClose }: HelpDialogProps) {
  return (
    <div
      className="helpdialog__backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Help"
      onClick={onClose}
    >
      <div className="helpdialog" onClick={(e) => e.stopPropagation()}>
        <header className="helpdialog__header">
          <h2 className="helpdialog__title">Help</h2>
          <button
            type="button"
            className="helpdialog__close"
            aria-label="Close help"
            onClick={onClose}
          >
            ×
          </button>
        </header>

        <section className="helpdialog__section">
          <h3 className="helpdialog__heading">User guide</h3>
          <p className="helpdialog__text">
            The full user guide covers the editing model (pipeline view + per-pass
            graph), the node taxonomy, preview controls (source, viewport,
            parameters, A/B compare, the pixel inspector), custom-GLSL nodes,
            importing a RetroArch preset, and exporting a bundle. It ships in the
            repository at <code>docs/user-guide.md</code>, with a focused walkthrough
            in <code>docs/import-walkthrough.md</code>.
          </p>
        </section>

        <section className="helpdialog__section">
          <h3 className="helpdialog__heading">Keyboard shortcuts</h3>
          {SHORTCUT_GROUPS.map((group) => (
            <div key={group.title} className="helpdialog__shortcut-group">
              <div className="helpdialog__shortcut-group-title">{group.title}</div>
              <table className="helpdialog__shortcuts">
                <tbody>
                  {group.shortcuts.map((s) => (
                    <tr key={s.label}>
                      <th scope="row" className="helpdialog__shortcut-label">
                        {s.label}
                      </th>
                      <td className="helpdialog__shortcut-keys">
                        {s.keys.map((k, i) => (
                          <span key={k}>
                            {i > 0 ? <span className="helpdialog__or"> or </span> : null}
                            <kbd className="helpdialog__kbd">{k}</kbd>
                          </span>
                        ))}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ))}
          <p className="helpdialog__note">
            On macOS use <kbd className="helpdialog__kbd">Cmd</kbd> wherever{" "}
            <kbd className="helpdialog__kbd">Ctrl</kbd> is shown.
          </p>
        </section>
      </div>
    </div>
  );
}
