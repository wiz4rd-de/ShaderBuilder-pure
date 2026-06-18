// The toast/banner host (#62) — renders the live, non-blocking toasts from the
// toast store in a fixed corner stack. Each toast is dismissable; engine/render/IO
// errors flow here so a transient failure is visible without a modal or a crash.
import { useToastStore } from "./toastStore";

export function Toasts(): React.JSX.Element {
  const toasts = useToastStore((s) => s.toasts);
  const dismiss = useToastStore((s) => s.dismiss);

  return (
    <div className="toasts" role="region" aria-label="Notifications" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={`toast toast--${t.severity}`}
          role={t.severity === "error" ? "alert" : "status"}
          data-testid="toast"
        >
          <span className="toast__message">{t.message}</span>
          <button
            type="button"
            className="toast__dismiss"
            aria-label="Dismiss notification"
            onClick={() => dismiss(t.id)}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
