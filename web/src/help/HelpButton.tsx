// The title-bar HELP button (#66): opens the in-app help modal (user-guide
// pointer + keyboard-shortcut reference) from anywhere in the editor.
import { useState } from "react";

import { HelpDialog } from "./HelpDialog";

export function HelpButton() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        className="helpbutton"
        aria-haspopup="dialog"
        onClick={() => setOpen(true)}
      >
        Help
      </button>
      {open ? <HelpDialog onClose={() => setOpen(false)} /> : null}
    </>
  );
}
