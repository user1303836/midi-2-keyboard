import { Component, StrictMode, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";

/**
 * A bug in the interface must never look like the app died: the engine keeps running in the
 * background regardless, so say so and offer a way back.
 */
class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
        <div className="text-xl font-semibold">The interface hit a problem</div>
        <div className="max-w-[520px] text-fg-muted">
          Your mappings keep working in the background. Reloading the window usually fixes this.
        </div>
        <pre className="selectable max-h-[160px] max-w-[640px] overflow-auto rounded-lg border bg-surface p-3 text-left font-mono text-sm text-fg-muted">
          {String(this.state.error?.stack ?? this.state.error)}
        </pre>
        <button
          type="button"
          className="h-[32px] rounded-md bg-accent px-4 font-semibold text-accent-fg hover:bg-accent-hover"
          onClick={() => window.location.reload()}
        >
          Reload
        </button>
      </div>
    );
  }
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
