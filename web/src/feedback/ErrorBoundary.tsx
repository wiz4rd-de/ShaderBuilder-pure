// A React error boundary (#62) — catches a render-time exception anywhere in the
// editor/preview subtree and shows a RECOVERABLE error screen instead of a blank
// white window. "Recoverable": a "Try again" button resets the boundary so the
// subtree re-mounts (the document store survives — it lives outside the boundary),
// so a transient render glitch does not force a full app reload.
//
// This is the LAST line of defence behind the typed error surface (toasts + the
// problems panel handle expected compile/render/IO failures non-fatally); the
// boundary only catches genuine JS exceptions that would otherwise white-screen.
import { Component, type ErrorInfo, type ReactNode } from "react";

interface ErrorBoundaryProps {
  children: ReactNode;
  /** Optional label naming the region (for the fallback heading + logging). */
  label?: string;
}

interface ErrorBoundaryState {
  error: Error | null;
  /** The React component stack of the failure (from componentDidCatch). */
  componentStack: string | null;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { error: null, componentStack: null };
  }

  static getDerivedStateFromError(error: Error): Partial<ErrorBoundaryState> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Surface to the console for diagnosis; the UI already shows the fallback.
    console.error(`ErrorBoundary (${this.props.label ?? "app"}) caught`, error, info);
    this.setState({ componentStack: info.componentStack ?? null });
  }

  private reset = (): void => {
    this.setState({ error: null, componentStack: null });
  };

  render(): ReactNode {
    const { error, componentStack } = this.state;
    if (error) {
      // The component stack pinpoints which subtree threw — invaluable for
      // diagnosing render-time loops/exceptions. Shown in a collapsed disclosure
      // so it stays out of the way until needed.
      const details = [error.stack, componentStack && `Component stack:${componentStack}`]
        .filter(Boolean)
        .join("\n\n");
      return (
        <div className="error-boundary" role="alert" data-testid="error-boundary">
          <div className="error-boundary__card">
            <h2 className="error-boundary__title">
              {this.props.label ? `${this.props.label} hit an error` : "Something went wrong"}
            </h2>
            <p className="error-boundary__message">{error.message || String(error)}</p>
            {details ? (
              <details className="error-boundary__details">
                <summary>Technical details</summary>
                <pre className="error-boundary__stack" data-testid="error-boundary-stack">
                  {details}
                </pre>
              </details>
            ) : null}
            <button type="button" className="error-boundary__retry" onClick={this.reset}>
              Try again
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
