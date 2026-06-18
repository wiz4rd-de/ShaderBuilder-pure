// The blocking unsaved-changes / recovery modal (#63). Renders the live
// `confirmStore` prompt as a small centered dialog with up to three buttons
// (confirm / discard / cancel) whose labels come from the prompt. Renders nothing
// when no prompt is open. Esc maps to cancel (or discard if there is no cancel).
import { useEffect } from "react";

import { useConfirmStore } from "./confirmStore";

export function ConfirmDialog() {
  const prompt = useConfirmStore((s) => s.prompt);
  const answer = useConfirmStore((s) => s.answer);

  useEffect(() => {
    if (!prompt) {
      return;
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") {
        e.preventDefault();
        answer(prompt.labels.cancel ? "cancel" : "discard");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [prompt, answer]);

  if (!prompt) {
    return null;
  }

  return (
    <div className="confirm__backdrop" role="presentation">
      <div className="confirm__dialog" role="alertdialog" aria-modal="true" aria-label="Confirm">
        <p className="confirm__message">{prompt.message}</p>
        <div className="confirm__actions">
          <button type="button" className="confirm__btn confirm__btn--primary" onClick={() => answer("confirm")}>
            {prompt.labels.confirm}
          </button>
          <button type="button" className="confirm__btn" onClick={() => answer("discard")}>
            {prompt.labels.discard}
          </button>
          {prompt.labels.cancel ? (
            <button type="button" className="confirm__btn" onClick={() => answer("cancel")}>
              {prompt.labels.cancel}
            </button>
          ) : null}
        </div>
      </div>
    </div>
  );
}
