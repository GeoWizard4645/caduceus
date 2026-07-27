/**
 * An extension's page: where somebody else's JavaScript actually runs.
 *
 * The palette hands off to here rather than showing a result inline, for two
 * reasons that both come down to the extension being live rather than a value.
 * A list of rows can carry an `action` closure, and a closure is only good while
 * the worker that made it exists — so something has to own that worker, and a
 * palette row that disappears on the next keystroke cannot. And an extension is
 * the one thing in Caduceus that can fail in ways the author has to debug: a
 * page has room for the error, the permissions it was refused, and the input box
 * to try again without retyping the whole query.
 *
 * The run is disposed when the tab closes. A Web Worker outlives every reference
 * React holds to it, so that cleanup is the difference between closing a tab and
 * leaking a running program.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { ExtensionRun, type ExtensionResult } from "@/shared/extensionRuntime";
import type { Extension } from "@/shared/types";
import { Button, Callout, EmptyState, Section, Spinner, TextInput, cx } from "@/shared/ui";

export function ExtensionPage({
  active,
  extensionId,
  prefill,
}: {
  active: boolean;
  extensionId: string;
  /** What the user typed after the extension's name in the palette. */
  prefill?: string;
  onSetTitle?: (title: string | undefined) => void;
}) {
  const [ext, setExt] = useState<Extension | null>(null);
  const [missing, setMissing] = useState(false);
  const [input, setInput] = useState(prefill ?? "");
  const [result, setResult] = useState<ExtensionResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  // What choosing a row said. Kept beside the list rather than replacing it:
  // a row action is usually "copy this one", and the answer to that is not a
  // reason to throw away the other nine.
  const [rowMessage, setRowMessage] = useState<string | null>(null);

  const run = useRef<ExtensionRun | null>(null);
  // Only the first visit runs on its own. Coming back to a tab you left open
  // should not silently re-run something that sent an email.
  const ranOnce = useRef(false);

  // --- find the extension -------------------------------------------------
  useEffect(() => {
    let live = true;
    void api
      .listExtensions()
      .then((all) => {
        if (!live) return;
        const found = all.find((candidate) => candidate.id === extensionId) ?? null;
        setExt(found);
        setMissing(!found);
      })
      .catch(() => live && setMissing(true));
    return () => {
      live = false;
    };
  }, [extensionId]);

  // --- one worker per extension, ended with the tab -----------------------
  useEffect(() => {
    if (!ext) return;
    const session = new ExtensionRun(ext);
    run.current = session;
    return () => {
      session.dispose();
      run.current = null;
    };
  }, [ext]);

  const start = useCallback(async (value: string) => {
    const session = run.current;
    if (!session) return;
    setBusy(true);
    setError(null);
    setRowMessage(null);
    try {
      setResult(await session.run(value));
    } catch (failure) {
      setError(api.errorMessage(failure));
      setResult(null);
    } finally {
      setBusy(false);
    }
  }, []);

  /**
   * Choose a row.
   *
   * An action that returns a new list replaces the old one; anything else is a
   * message about the row you picked, and the list stays. The worker keeps the
   * closures alive either way, which is why the rest of the rows still work.
   */
  const choose = useCallback(async (index: number) => {
    const session = run.current;
    if (!session) return;
    setBusy(true);
    setError(null);
    try {
      const outcome = await session.invoke(index);
      if (outcome.kind === "rows") {
        setResult(outcome);
        setRowMessage(null);
      } else {
        setRowMessage(outcome.kind === "text" ? outcome.text : "Done.");
      }
    } catch (failure) {
      setError(api.errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    if (!ext || ranOnce.current) return;
    ranOnce.current = true;
    void start(input);
    // `input` is deliberately not a dependency: this is the first run, with
    // whatever the palette passed in, and every later one is explicit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ext, start]);

  const text = result?.kind === "text" ? result.text : null;

  const copy = () => {
    if (text === null) return;
    navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      })
      .catch(() => setError("Could not copy that."));
  };

  const permissions = useMemo(() => ext?.permissions ?? [], [ext]);

  if (missing) {
    return (
      <div className="p-6">
        <EmptyState
          title="That extension is not installed"
          icon="⊞"
          hint="It may have been removed, or its file edited into something without a @caduceus header."
        />
      </div>
    );
  }

  if (!ext) {
    return (
      <div className="row h-full items-center justify-center">
        <Spinner />
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-5 overflow-y-auto p-5">
      <Section
        title={ext.name}
        description={ext.description || "No description in this extension's header."}
      >
        <div className="row flex-wrap gap-1.5">
          {permissions.length === 0 ? (
            <span className="rounded bg-overlay px-1.5 py-0.5 text-2xs text-ink-faint">
              no permissions
            </span>
          ) : (
            permissions.map((permission) => (
              <span
                key={permission}
                className="rounded bg-accent/15 px-1.5 py-0.5 text-2xs text-accent"
              >
                {permission}
              </span>
            ))
          )}
          {ext.author && <span className="text-2xs text-ink-faint">by {ext.author}</span>}
        </div>

        <form
          className="row mt-3 gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void start(input);
          }}
        >
          <div className="min-w-0 flex-1">
            <TextInput
              value={input}
              onChange={setInput}
              placeholder="Input — whatever this extension takes, or nothing"
              autoFocus={active}
            />
          </div>
          <Button type="submit" tone="primary" disabled={busy}>
            {busy ? "Running" : "Run"}
          </Button>
        </form>
      </Section>

      {error && (
        <Callout tone="danger">
          <p className="whitespace-pre-wrap break-words">{error}</p>
        </Callout>
      )}

      {busy && !result && (
        <div className="row justify-center py-6">
          <Spinner />
        </div>
      )}

      {result?.kind === "text" && (
        <Section title="Result">
          <pre className="max-h-[46vh] overflow-auto whitespace-pre-wrap break-words rounded-cad border border-line bg-raised/60 p-3 text-2xs leading-relaxed text-ink">
            {text}
          </pre>
          <div className="row mt-2 gap-2">
            <Button size="sm" onClick={copy} disabled={!text}>
              {copied ? "Copied" : "Copy"}
            </Button>
          </div>
        </Section>
      )}

      {result?.kind === "rows" && (
        <Section
          title="Result"
          description={`${result.rows.length} row${result.rows.length === 1 ? "" : "s"}.`}
        >
          {result.rows.length === 0 ? (
            <p className="text-2xs text-ink-faint">Nothing to show.</p>
          ) : (
            <div className="flex flex-col gap-1">
              {result.rows.map((row, index) => (
                <button
                  key={`${index}-${row.title}`}
                  type="button"
                  disabled={!row.hasAction || busy}
                  onClick={() => void choose(index)}
                  className={cx(
                    "rounded-cad border border-line bg-raised/50 px-3 py-2 text-left transition-colors",
                    row.hasAction
                      ? "cursor-pointer hover:border-accent/60 hover:bg-accent/5"
                      : "cursor-default",
                  )}
                >
                  <p className="truncate text-[13px] text-ink">{row.title}</p>
                  {row.subtitle && (
                    <p className="mt-0.5 truncate text-2xs text-ink-mute">{row.subtitle}</p>
                  )}
                </button>
              ))}
            </div>
          )}
          {rowMessage !== null && (
            <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-cad border border-line bg-raised/60 p-2.5 text-2xs leading-relaxed text-ink-mute">
              {rowMessage}
            </pre>
          )}
        </Section>
      )}

      {result?.kind === "none" && !busy && (
        <p className="text-2xs text-ink-faint">
          That returned nothing. Extensions show a result by returning a string, or a list by
          returning an array of rows.
        </p>
      )}
    </div>
  );
}
