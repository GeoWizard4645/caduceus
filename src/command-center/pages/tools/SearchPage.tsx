/**
 * Semantic file search: ask in plain language, get back the note or PDF that
 * meant it — not just the one whose filename happened to match.
 *
 * # This page is built against commands that do not exist yet
 *
 * `tools::semantic::SemanticIndex` (BM25 lexical search always, local-Ollama
 * embeddings layered on top when one is available) is written and tested on
 * the Rust side, but nothing in `commands.rs`/`lib.rs` exposes it over IPC —
 * see the wrappers this page calls in `shared/api.ts` (`semanticIndexStats`,
 * `semanticIndexSync`, `semanticIndexCancel`, `semanticSearch`) for the exact
 * shape those commands need. Until they are registered, the very first call
 * below rejects with Tauri's own "command … not found", which is caught once
 * and turned into the explanation in {@link NotWired} rather than a page that
 * looks broken or, worse, a Search box that quietly does nothing when pressed.
 *
 * # Why syncing is a loop, not one call
 *
 * `SemanticIndex::sync` is deliberately incremental and bounded per call (see
 * `IndexConfig` in the Rust module) so a first index of a large folder is many
 * cheap, interruptible calls rather than one long one. This page calls it
 * repeatedly while a chunk comes back `truncated`, adding each chunk's counts
 * to a running total — the closest thing to a progress bar available without
 * a Tauri event channel streaming per-file updates, which is out of scope for
 * a frontend-only pass (see the report for that as a real follow-up).
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { Button, Callout, Spinner, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

interface Accumulated {
  scanned: number;
  indexed: number;
  updated: number;
  removed: number;
  errors: number;
  embedded: number;
  truncated: boolean;
}

const ZERO: Accumulated = { scanned: 0, indexed: 0, updated: 0, removed: 0, errors: 0, embedded: 0, truncated: false };

const MATCH_LABEL: Record<api.SemanticMatchKind, string> = {
  lexical: "Lexical",
  semantic: "Semantic",
  hybrid: "Hybrid",
};

export function SearchPage({ onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Search"), [onSetTitle]);

  const [wired, setWired] = useState<boolean | null>(null);
  const [wireError, setWireError] = useState<string | null>(null);
  const [stats, setStats] = useState<api.SemanticIndexSnapshot | null>(null);

  const [syncing, setSyncing] = useState(false);
  const [progress, setProgress] = useState<Accumulated | null>(null);
  const [syncError, setSyncError] = useState<string | null>(null);
  const cancelledRef = useRef(false);

  const [query, setQuery] = useState("");
  const [results, setResults] = useState<api.SemanticSearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  const refreshStats = useCallback(async () => {
    try {
      const snapshot = await api.semanticIndexStats();
      setStats(snapshot);
      setWired(true);
      setWireError(null);
    } catch (error) {
      setWired(false);
      setWireError(api.errorMessage(error));
    }
  }, []);

  useEffect(() => {
    void refreshStats();
  }, [refreshStats]);

  const sync = async () => {
    cancelledRef.current = false;
    setSyncing(true);
    setSyncError(null);
    let acc = ZERO;
    setProgress(acc);
    try {
      for (;;) {
        const chunk = await api.semanticIndexSync();
        acc = {
          scanned: acc.scanned + chunk.scanned,
          indexed: acc.indexed + chunk.indexed,
          updated: acc.updated + chunk.updated,
          removed: acc.removed + chunk.removed,
          errors: acc.errors + chunk.errors,
          embedded: acc.embedded + chunk.embedded,
          truncated: chunk.truncated,
        };
        setProgress(acc);
        if (!chunk.truncated || cancelledRef.current) break;
      }
    } catch (error) {
      setSyncError(api.errorMessage(error));
    } finally {
      setSyncing(false);
      void refreshStats();
    }
  };

  const cancelSync = () => {
    cancelledRef.current = true;
    void api.semanticIndexCancel().catch(() => {});
  };

  // Re-searches as you type, the same debounce QrPage uses for its own
  // instant-feedback field — cheap enough per keystroke that a Search button
  // would only add a step between typing and the answer.
  useEffect(() => {
    if (!wired) return;
    const q = query.trim();
    if (!q) {
      setResults([]);
      setSearchError(null);
      setSearching(false);
      return;
    }
    setSearching(true);
    const timer = setTimeout(() => {
      void api
        .semanticSearch(q, 40)
        .then((hits) => {
          setResults(hits);
          setSearchError(null);
        })
        .catch((error) => {
          setResults([]);
          setSearchError(api.errorMessage(error));
        })
        .finally(() => setSearching(false));
    }, 220);
    return () => clearTimeout(timer);
  }, [query, wired]);

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Search</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Ask for what a file is about, not what it is named. Runs entirely on this Mac — a local
          Ollama, if one is running, sharpens the results but nothing is required to get started.
        </p>
      </div>

      {wired === false && <NotWired detail={wireError} />}

      {wired === true && (
        <>
          <Section title="Index">
            <div className="row flex-wrap items-center justify-between gap-3">
              <div>
                <p className="text-[13px] text-ink">
                  {stats?.documentCount ?? 0} document{stats?.documentCount === 1 ? "" : "s"} indexed
                </p>
                <p className="mt-0.5 text-2xs text-ink-faint">
                  {stats && stats.roots.length > 0
                    ? `Watching ${stats.roots.join(", ")}`
                    : "No folders configured to index yet."}
                </p>
              </div>
              <div className="row gap-2">
                {syncing ? (
                  <Button size="sm" tone="danger" onClick={cancelSync}>
                    Cancel
                  </Button>
                ) : (
                  <Button size="sm" tone="primary" onClick={() => void sync()}>
                    {stats?.documentCount ? "Refresh index" : "Build index"}
                  </Button>
                )}
                {syncing && <Spinner className="text-accent" />}
              </div>
            </div>

            {progress && (
              <p className="mt-3 text-2xs text-ink-faint">
                Scanned {progress.scanned} · indexed {progress.indexed} · updated {progress.updated} · removed{" "}
                {progress.removed}
                {progress.errors > 0 && ` · ${progress.errors} errors`}
                {syncing && progress.truncated && !cancelledRef.current && " · continuing…"}
                {!syncing && cancelledRef.current && " · stopped early"}
              </p>
            )}
            {syncError && <p className="mt-2 text-2xs text-danger">{syncError}</p>}
          </Section>

          <Section title="Ask">
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              spellCheck={false}
              placeholder="what I decided about the pricing page redesign"
              className="w-full rounded-lg border border-line bg-base/40 px-3 py-2 text-[13px] text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
              autoFocus
            />
            {searching && <p className="mt-2 text-2xs text-ink-faint">Searching…</p>}
            {searchError && <p className="mt-2 text-2xs text-danger">{searchError}</p>}
          </Section>

          {results.length > 0 && (
            <Section title={`Results (${results.length})`}>
              <div className="space-y-2">
                {results.map((hit) => (
                  <button
                    key={hit.path}
                    type="button"
                    onClick={() => void api.revealPath(hit.path).catch(() => {})}
                    className="block w-full rounded-lg border border-line bg-raised/40 p-3 text-left transition-colors hover:bg-raised/70"
                  >
                    <div className="row items-baseline justify-between gap-2">
                      <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-ink">{hit.title}</span>
                      <span
                        className={cx(
                          "shrink-0 rounded-full border px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-[0.06em]",
                          hit.matchedVia === "hybrid" && "border-positive/30 bg-positive/10 text-positive",
                          hit.matchedVia === "semantic" && "border-accent/30 bg-accent/10 text-accent",
                          hit.matchedVia === "lexical" && "border-line text-ink-faint",
                        )}
                      >
                        {MATCH_LABEL[hit.matchedVia]}
                      </span>
                    </div>
                    <p className="mt-1 line-clamp-2 text-2xs leading-relaxed text-ink-mute">{hit.snippet}</p>
                    <p className="mt-1 truncate text-2xs text-ink-faint">{hit.path}</p>
                  </button>
                ))}
              </div>
            </Section>
          )}

          {query.trim() && !searching && results.length === 0 && !searchError && (
            <p className="py-6 text-center text-2xs text-ink-faint">Nothing found for that yet.</p>
          )}
        </>
      )}

      {wired === null && <p className="text-2xs text-ink-faint">Checking the index…</p>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mb-5">
      <p className="eyebrow mb-2">{title}</p>
      <div className="rounded-cad border border-line bg-surface/50 p-3">{children}</div>
    </section>
  );
}

/**
 * What shows instead of a search box when the backend has no `semantic_*`
 * commands registered yet. Named, not inlined, so it is impossible to forget
 * this is a real, expected state rather than an error to swallow quietly.
 */
function NotWired({ detail }: { detail: string | null }) {
  return (
    <Callout tone="warn" title="Not wired up yet">
      <p>
        The search engine itself (BM25 lexical search, with local-Ollama embeddings layered on top
        when available) is built and tested in <code>tools::semantic</code> — this page just has
        nothing on the other end of the wire to call. Four commands need to be registered in{" "}
        <code>commands.rs</code>/<code>lib.rs</code>: <code>semantic_index_stats</code>,{" "}
        <code>semantic_index_sync</code>, <code>semantic_index_cancel</code>, and{" "}
        <code>semantic_search</code>. This page is already built against them — see{" "}
        <code>shared/api.ts</code> for the exact request/response shapes expected.
      </p>
      {detail && <p className="mt-2 font-mono text-2xs text-ink-faint">{detail}</p>}
    </Callout>
  );
}
