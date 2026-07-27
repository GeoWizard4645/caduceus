/**
 * Cite the page you are looking at.
 *
 * Reads the frontmost browser tab, offers every style at once, and lets you fix
 * what it got wrong before you copy.
 *
 * # Two things it will not do
 *
 * **It will not invent an author.** A missing one shows as the site name and a
 * missing date shows as `n.d.` — the standard way of saying "unknown". A
 * plausible-looking made-up author is worse than an obviously incomplete
 * citation: the second gets fixed, the first gets handed in.
 *
 * **It will not fetch the page unless asked.** Title and URL come from the
 * browser. Author and date live in the page's metadata, and getting them means
 * a request — so that is a button, not something that happens because you
 * opened a tab.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import { Button, Callout, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

export function CitationsPage({ active, onSetTitle }: ToolPageProps) {
  const [source, setSource] = useState<api.CitationSource | null>(null);
  const [citations, setCitations] = useState<api.Citation[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [enriching, setEnriching] = useState(false);

  useEffect(() => onSetTitle("Cite this page"), [onSetTitle]);

  const accessed = new Date().toLocaleDateString(undefined, {
    day: "numeric",
    month: "long",
    year: "numeric",
  });

  const format = useCallback(
    async (next: api.CitationSource) => {
      try {
        setCitations(await api.formatCitations(next, accessed));
      } catch (e) {
        setError(api.errorMessage(e));
      }
    },
    [accessed],
  );

  const read = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const page = await api.currentPage();
      setSource(page);
      await format(page);
    } catch (e) {
      setError(api.errorMessage(e));
    } finally {
      setBusy(false);
    }
  }, [format]);

  useEffect(() => {
    if (active && !source && !busy && !error) void read();
    // Only on first open; re-reading whenever this re-renders would fight the
    // user's edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  const enrich = async () => {
    if (!source) return;
    setEnriching(true);
    try {
      const filled = await api.enrichCitation(source);
      setSource(filled);
      await format(filled);
      setNote(
        filled.author || filled.published
          ? "Filled in what the page declares about itself."
          : "That page does not publish an author or a date. Type them in if you know them.",
      );
    } catch (e) {
      setNote(api.errorMessage(e));
    } finally {
      setEnriching(false);
    }
  };

  const edit = (patch: Partial<api.CitationSource>) => {
    if (!source) return;
    const next = { ...source, ...patch };
    setSource(next);
    void format(next);
  };

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Cite this page</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Reads whatever your browser has in front. Every style at once — check the details,
          then copy the one you were asked for.
        </p>
      </div>

      {error && (
        <Callout tone="warn" title="No page to cite">
          <p>{error}</p>
          <Button size="sm" className="mt-2" onClick={() => void read()}>
            Try again
          </Button>
        </Callout>
      )}

      {source && (
        <>
          <div className="mb-4 rounded-cad border border-line bg-surface/50 p-4">
            <div className="row mb-3 justify-between gap-2">
              <p className="eyebrow">The source</p>
              <div className="row gap-1">
                <Button size="sm" tone="ghost" onClick={() => void read()} disabled={busy}>
                  Re-read the tab
                </Button>
                <Button size="sm" onClick={() => void enrich()} disabled={enriching}>
                  {enriching ? "Fetching…" : "Find author and date"}
                </Button>
              </div>
            </div>

            <div className="space-y-2">
              <Row label="Title" value={source.title} onChange={(title) => edit({ title })} />
              <Row label="URL" value={source.url} mono onChange={(url) => edit({ url })} />
              <Row
                label="Author"
                value={source.author ?? ""}
                placeholder="unknown — the site name will be used"
                onChange={(author) => edit({ author: author || null })}
              />
              <Row
                label="Site"
                value={source.site ?? ""}
                onChange={(site) => edit({ site: site || null })}
              />
              <Row
                label="Published"
                value={source.published ?? ""}
                placeholder="YYYY-MM-DD — blank becomes n.d."
                mono
                onChange={(published) => edit({ published: published || null })}
              />
            </div>

            <p className="mt-3 text-2xs text-ink-faint">Accessed {accessed}</p>
          </div>

          {note && <p className="mb-3 text-2xs text-ink-mute">{note}</p>}

          <div className="space-y-2">
            {citations.map((citation) => (
              <div key={citation.style} className="rounded-cad border border-line bg-surface/40">
                <div className="row justify-between gap-2 border-b border-line px-3 py-1.5">
                  <span className="text-2xs font-medium text-ink">{citation.label}</span>
                  <Button
                    size="sm"
                    tone="ghost"
                    onClick={() => {
                      navigator.clipboard
                        .writeText(citation.text)
                        .then(() => setNote(`Copied the ${citation.label} citation.`))
                        .catch(() => setNote("Could not copy."));
                    }}
                  >
                    Copy
                  </Button>
                </div>
                <p
                  className={cx(
                    "px-3 py-2 text-2xs leading-relaxed text-ink-soft",
                    citation.style === "bibtex" && "whitespace-pre-wrap font-mono",
                  )}
                >
                  {citation.text}
                </p>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

function Row({
  label,
  value,
  onChange,
  placeholder,
  mono,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  mono?: boolean;
}) {
  return (
    <label className="flex items-baseline gap-3">
      <span className="w-20 shrink-0 text-2xs uppercase tracking-[0.08em] text-ink-faint">
        {label}
      </span>
      <input
        value={value}
        spellCheck={false}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        className={cx(
          "min-w-0 flex-1 rounded-lg border border-line bg-base/40 px-2.5 py-1.5 text-2xs text-ink",
          "placeholder:text-ink-faint focus:border-accent/50 focus:outline-none",
          mono && "font-mono",
        )}
      />
    </label>
  );
}
