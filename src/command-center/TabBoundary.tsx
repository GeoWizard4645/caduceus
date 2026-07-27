/**
 * One broken tab must not take the window with it.
 *
 * React unmounts the entire tree when a render throws and nothing catches it.
 * In a single-window app whose every surface is a tab, that means one page with
 * a bad assumption — a restored tab holding a value from an older version, a
 * command whose shape changed — blanks the Command Center completely. No
 * message, no tabs, nothing to click. The only way out is to quit from the menu
 * bar, and on the next launch the same tab is restored and it happens again.
 *
 * So every tab renders inside one of these. A page that throws becomes a page
 * that says so, beside all the other tabs, with a way to close it.
 *
 * # Why this is a class
 *
 * `componentDidCatch` has no hook equivalent. React has never shipped one, and
 * every "useErrorBoundary" library is this class with a nicer face on it.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";

import { Button } from "@/shared/ui";

interface Props {
  children: ReactNode;
  /** Shown in the message, so it is obvious which tab broke. */
  label: string;
  onClose: () => void;
}

interface State {
  error: Error | null;
}

export class TabBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // The console is the only place this can go — an app that reported crashes
    // anywhere else would be doing the one thing Caduceus promises it does not.
    console.error(`The ${this.props.label} tab crashed:`, error, info.componentStack);
  }

  /** Try the same page again. Some failures are transient; nothing is lost by asking. */
  private retry = () => this.setState({ error: null });

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="mx-auto flex h-full max-w-[560px] flex-col items-start justify-center px-6">
        <p className="eyebrow">Something broke</p>
        <h1 className="mt-1 text-[17px] font-semibold tracking-[-0.015em] text-ink">
          The {this.props.label} tab stopped working
        </h1>
        <p className="mt-2 text-[13px] leading-relaxed text-ink-mute">
          Everything else is fine — your other tabs, the palette and the staff are all still
          running. This is a bug in Caduceus; the details are in the webview console.
        </p>
        <pre className="mt-3 max-h-[160px] w-full overflow-auto rounded-lg border border-line bg-base/40 px-3 py-2 font-mono text-2xs text-ink-faint">
          {error.message || String(error)}
        </pre>
        <div className="row mt-4 gap-2">
          <Button tone="primary" onClick={this.retry}>
            Try again
          </Button>
          <Button onClick={this.props.onClose}>Close this tab</Button>
        </div>
      </div>
    );
  }
}
