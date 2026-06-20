// The File menu (#63): New / Open / Open Recent / Save / Save As. A click-to-open
// dropdown in the title bar that drives the project-session flows. Recents are
// fetched (pruned of missing files) when the menu opens, so the list is always
// current. Each entry routes through `projectActions`, which guards unsaved edits.
import { useEffect, useRef, useState } from "react";

import { basename } from "./paths";
import type { RecentProject } from "../bindings/RecentProject";
import { newProject, openProject, openRecent, save, saveAs } from "./projectActions";
import { loadRecents as fetchRecents } from "./api";
import { useExportStore } from "../export/exportStore";

export function FileMenu() {
  const [open, setOpen] = useState(false);
  const [recents, setRecents] = useState<RecentProject[]>([]);
  const rootRef = useRef<HTMLDivElement>(null);
  const openExport = useExportStore((s) => s.openDialog);

  // Refresh the recents list whenever the menu opens.
  useEffect(() => {
    if (!open) {
      return;
    }
    let cancelled = false;
    void fetchRecents()
      .then((list) => {
        if (!cancelled) {
          setRecents(list);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [open]);

  // Close on an outside click.
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const run = (action: () => Promise<unknown>): void => {
    setOpen(false);
    void action();
  };

  return (
    <div className="filemenu" ref={rootRef}>
      <button
        type="button"
        className="filemenu__trigger"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        File
      </button>
      {open ? (
        <div className="filemenu__dropdown" role="menu" aria-label="File">
          <button type="button" role="menuitem" onClick={() => run(() => newProject())}>
            New <span className="filemenu__shortcut">Ctrl+N</span>
          </button>
          <button type="button" role="menuitem" onClick={() => run(() => openProject())}>
            Open… <span className="filemenu__shortcut">Ctrl+O</span>
          </button>
          <div className="filemenu__submenu" role="group" aria-label="Open Recent">
            <div className="filemenu__submenu-label">Open Recent</div>
            {recents.length === 0 ? (
              <div className="filemenu__empty">No recent projects</div>
            ) : (
              recents.map((r) => (
                <button
                  key={r.path}
                  type="button"
                  role="menuitem"
                  className="filemenu__recent"
                  title={r.path}
                  onClick={() => run(() => openRecent(r.path))}
                >
                  <span className="filemenu__recent-name">{r.name}</span>
                  <span className="filemenu__recent-path">{basename(r.path)}</span>
                </button>
              ))
            )}
          </div>
          <hr className="filemenu__sep" />
          <button type="button" role="menuitem" onClick={() => run(() => save())}>
            Save <span className="filemenu__shortcut">Ctrl+S</span>
          </button>
          <button type="button" role="menuitem" onClick={() => run(() => saveAs())}>
            Save As… <span className="filemenu__shortcut">Ctrl+Shift+S</span>
          </button>
          <hr className="filemenu__sep" />
          <button type="button" role="menuitem" onClick={() => run(() => openExport())}>
            Export Bundle…
          </button>
        </div>
      ) : null}
    </div>
  );
}
