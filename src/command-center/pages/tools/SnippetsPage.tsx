/**
 * Text expander, emoji search, Markdown paste, and a proofreader — four
 * small writing utilities that share nothing but a shape ("take text the
 * user has, hand back different text they actually want") and a backend
 * module (`src-tauri/src/tools/expander.rs`, 55 tests). See that file's
 * module docs for why they live together and why translation is
 * deliberately not one of them.
 *
 * All four Rust commands were written and tested but never wired into
 * `generate_handler!` before this page existed — see the module doc there.
 * They are ordinary commands now that this page calls them; nothing about
 * that wiring needed touching Rust.
 */

import { useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { EmojiHit, ExpansionOutcome, ProofreadResult, Snippet } from "@/shared/api";
import { useDebounced, useEscape } from "@/shared/hooks";
import { Button, Callout, Field, IconButton, Section, Spinner, TextArea, TextInput, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

/**
 * The placeholder syntax [`expand_body`](../../../src-tauri/src/tools/expander.rs)
 * understands, documented here rather than only in a Rust doc comment nobody
 * writing a snippet will ever open. A placeholder syntax nobody can discover
 * is a placeholder syntax nobody uses.
 */
const PLACEHOLDERS: { token: string; description: string }[] = [
  { token: "{date}", description: "Today, as YYYY-MM-DD." },
  {
    token: "{date+7d}",
    description:
      "Today offset by a signed amount and a unit — d (days), w (weeks), m (months), y (years). {date-3d} and a bare {date+10} (days) work too.",
  },
  { token: "{time}", description: "The current time, as HH:MM." },
  { token: "{clipboard}", description: "Whatever is on the clipboard right now, or nothing if it's empty." },
  {
    token: "{cursor}",
    description: "Not shown in the output — marks where the caret lands after this expands.",
  },
];

export function SnippetsPage({ active, onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Text Expander"), [onSetTitle]);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-[880px] px-6 py-5">
        <div className="mb-4">
          <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Text expander</h1>
          <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
            Snippets, an emoji finder that searches by meaning, a Markdown-to-rich-text pane, and a
            copy-editor pass — the writing utilities Caduceus's roadmap called out as missing.
            Everything below runs on this Mac except the proofreader, which uses whichever backend
            Settings → AI has configured.
          </p>
        </div>

        <SnippetsSection active={active} />
        <EmojiSection />
        <MarkdownSection />
        <ProofreadSection />
      </div>
    </div>
  );
}

// ===========================================================================
// Snippets: list + editor + live placeholder preview
// ===========================================================================

function SnippetsSection({ active }: { active: boolean }) {
  const [snippets, setSnippets] = useState<Snippet[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [listError, setListError] = useState<string | null>(null);

  // `null` selectedId means "editing a brand-new, unsaved snippet" —
  // distinct from there being no snippets at all, which is why this isn't
  // just derived from `snippets.length === 0`.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [isNew, setIsNew] = useState(true);
  const [shortcut, setShortcut] = useState("");
  const [body, setBody] = useState("");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [deleteArmedId, setDeleteArmedId] = useState<string | null>(null);

  const [preview, setPreview] = useState<ExpansionOutcome | null>(null);
  const debouncedBody = useDebounced(body, 200);

  const reload = () => {
    api
      .expanderListSnippets()
      .then((list) => {
        setSnippets(list);
        setLoaded(true);
      })
      .catch((e) => setListError(api.errorMessage(e)));
  };

  useEffect(reload, []);

  // The live preview: what the body would expand to right now, against the
  // real clock and the real clipboard. Calls the backend (not a local
  // reimplementation) so the preview can never drift from what expansion
  // actually does.
  useEffect(() => {
    if (!debouncedBody.trim()) {
      setPreview(null);
      return;
    }
    let cancelled = false;
    api
      .expanderPreview(debouncedBody)
      .then((outcome) => {
        if (!cancelled) setPreview(outcome);
      })
      .catch(() => {
        if (!cancelled) setPreview(null);
      });
    return () => {
      cancelled = true;
    };
  }, [debouncedBody]);

  const selectNew = () => {
    setSelectedId(null);
    setIsNew(true);
    setShortcut("");
    setBody("");
    setSaveError(null);
  };

  const select = (snippet: Snippet) => {
    setSelectedId(snippet.id);
    setIsNew(false);
    setShortcut(snippet.shortcut);
    setBody(snippet.body);
    setSaveError(null);
    setDeleteArmedId(null);
  };

  // Escape means "not that one after all" before it means "close the tab" —
  // same convention as Sticky Notes' delete confirmation.
  useEscape(active, () => {
    if (!deleteArmedId) return false;
    setDeleteArmedId(null);
    return true;
  });

  const save = async () => {
    if (!shortcut.trim()) {
      setSaveError('Give the snippet a shortcut, like ":addr".');
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      const saved = await api.expanderSaveSnippet(isNew ? null : selectedId, shortcut, body);
      setSnippets((current) => {
        const exists = current.some((s) => s.id === saved.id);
        return exists ? current.map((s) => (s.id === saved.id ? saved : s)) : [saved, ...current];
      });
      select(saved);
      setNote(isNew ? "Snippet created." : "Snippet saved.");
    } catch (e) {
      setSaveError(api.errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (snippet: Snippet) => {
    if (deleteArmedId !== snippet.id) {
      setDeleteArmedId(snippet.id);
      return;
    }
    setDeleteArmedId(null);
    try {
      await api.expanderDeleteSnippet(snippet.id);
      setSnippets((current) => current.filter((s) => s.id !== snippet.id));
      if (selectedId === snippet.id) selectNew();
      setNote("Snippet deleted.");
    } catch (e) {
      setListError(api.errorMessage(e));
    }
  };

  return (
    <Section
      title="Snippets"
      description='A shortcut like ":addr" expands into a saved body wherever you type it. Placeholders inside the body are filled in at expansion time — editing a snippet never requires re-saving it just because "today" changed.'
      actions={
        <Button size="sm" tone="primary" onClick={selectNew}>
          + New
        </Button>
      }
    >
      {listError && <p className="mb-3 text-2xs text-danger">{listError}</p>}

      <div className="grid grid-cols-[220px_1fr] gap-4">
        {/* --- the list ------------------------------------------------- */}
        <div className="max-h-[360px] overflow-y-auto rounded-lg border border-line">
          {!loaded ? (
            <div className="flex items-center justify-center py-8 text-ink-faint">
              <Spinner />
            </div>
          ) : snippets.length === 0 ? (
            <p className="px-3 py-6 text-center text-2xs leading-relaxed text-ink-faint">
              No snippets yet. Fill in the shortcut and body on the right, then Save.
            </p>
          ) : (
            snippets.map((snippet) => (
              <div
                key={snippet.id}
                className={cx(
                  "flex items-center gap-1 border-b border-line px-2 py-1.5 last:border-b-0",
                  snippet.id === selectedId ? "bg-accent/10" : "hover:bg-raised/60",
                )}
              >
                <button
                  type="button"
                  onClick={() => select(snippet)}
                  className="min-w-0 flex-1 truncate text-left text-2xs"
                >
                  <span className="font-mono text-ink">{snippet.shortcut}</span>
                  <span className="ml-1.5 text-ink-faint">
                    {snippet.body.trim().split("\n")[0]?.slice(0, 40) || "—"}
                  </span>
                </button>
                <IconButton
                  label={deleteArmedId === snippet.id ? "Confirm delete" : "Delete snippet"}
                  tone="danger"
                  onClick={() => void remove(snippet)}
                >
                  {deleteArmedId === snippet.id ? "!" : "×"}
                </IconButton>
              </div>
            ))
          )}
        </div>

        {/* --- the editor ------------------------------------------------ */}
        <div className="min-w-0">
          <div className="grid grid-cols-[160px_1fr] gap-3">
            <Field label="Shortcut" error={saveError} hint='Whatever prefix you like — ":addr", ";sig", "//todo".'>
              <TextInput mono value={shortcut} onChange={setShortcut} placeholder=":addr" />
            </Field>
            <Field label="Body">
              <TextArea rows={5} mono value={body} onChange={setBody} placeholder="123 Cedar St, Apt 4&#10;Portland, OR" />
            </Field>
          </div>

          <div className="mt-2 flex flex-wrap gap-1.5">
            {PLACEHOLDERS.map((p) => (
              <button
                key={p.token}
                type="button"
                title={p.description}
                onClick={() => setBody((current) => current + p.token)}
                className="rounded-md border border-line-strong/50 bg-raised/60 px-2 py-1 font-mono text-2xs text-ink-soft transition-colors hover:border-accent/50 hover:text-ink"
              >
                {p.token}
              </button>
            ))}
          </div>
          <p className="mt-1.5 text-2xs leading-relaxed text-ink-faint">
            Click one to insert it at the end of the body. Hover any of them for what it does.
          </p>

          {preview && (
            <div className="mt-3 rounded-lg border border-line bg-base/40 p-3">
              <p className="mb-1 text-2xs font-medium text-ink-soft">Resolves right now to</p>
              <p className="whitespace-pre-wrap text-[13px] leading-relaxed text-ink">
                {preview.text || <span className="text-ink-faint">(empty)</span>}
              </p>
              {preview.cursorOffset !== null && (
                <p className="mt-1 text-2xs text-ink-faint">
                  Caret would land after character {preview.cursorOffset}.
                </p>
              )}
            </div>
          )}

          <div className="row mt-3 gap-2">
            <Button tone="primary" onClick={() => void save()} disabled={saving}>
              {saving ? <Spinner /> : null} {isNew ? "Create" : "Save"}
            </Button>
            {note && <span className="text-2xs text-ink-faint">{note}</span>}
          </div>
        </div>
      </div>
    </Section>
  );
}

// ===========================================================================
// Emoji: concept search
// ===========================================================================

function EmojiSection() {
  const [query, setQuery] = useState("");
  const debounced = useDebounced(query, 150);
  const [hits, setHits] = useState<EmojiHit[]>([]);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    if (!debounced.trim()) {
      setHits([]);
      return;
    }
    let cancelled = false;
    api
      .expanderSearchEmoji(debounced, 30)
      .then((results) => {
        if (!cancelled) setHits(results);
      })
      .catch(() => {
        if (!cancelled) setHits([]);
      });
    return () => {
      cancelled = true;
    };
  }, [debounced]);

  return (
    <Section
      title="Emoji search"
      description='Searched by meaning, not by Unicode name — "celebrate" finds 🎉🥳🥂 rather than nothing, because none of them are officially named "celebrate".'
    >
      <TextInput value={query} onChange={setQuery} placeholder="celebrate, thinking, coffee…" />

      {query.trim() && hits.length === 0 && (
        <p className="mt-3 text-2xs text-ink-faint">Nothing matched "{query.trim()}".</p>
      )}

      {hits.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-2">
          {hits.map((hit, i) => (
            <button
              key={`${hit.emoji}-${i}`}
              type="button"
              title={`Matched “${hit.keyword}” — click to copy`}
              onClick={() => {
                void navigator.clipboard.writeText(hit.emoji);
                setNote(`Copied ${hit.emoji}`);
              }}
              className="flex flex-col items-center gap-1 rounded-lg border border-line bg-raised/50 px-2.5 py-2 transition-colors hover:border-accent/50 hover:bg-raised"
            >
              <span className="text-xl leading-none">{hit.emoji}</span>
              <span className="max-w-[64px] truncate text-[10px] text-ink-faint">{hit.keyword}</span>
            </button>
          ))}
        </div>
      )}

      {note && <p className="mt-3 text-2xs text-ink-faint">{note}</p>}
    </Section>
  );
}

// ===========================================================================
// Markdown -> styled rich text
// ===========================================================================

function MarkdownSection() {
  const [markdown, setMarkdown] = useState("");
  const debounced = useDebounced(markdown, 150);
  const [previewHtml, setPreviewHtml] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!debounced.trim()) {
      setPreviewHtml("");
      return;
    }
    let cancelled = false;
    api
      .expanderMarkdownPreview(debounced)
      .then((html) => {
        if (!cancelled) setPreviewHtml(html);
      })
      .catch(() => {
        if (!cancelled) setPreviewHtml("");
      });
    return () => {
      cancelled = true;
    };
  }, [debounced]);

  const copy = async () => {
    if (!markdown.trim()) return;
    try {
      const outcome = await api.expanderCopyMarkdownAsRichText(markdown);
      setNote(outcome.message);
      setError(outcome.ok ? null : outcome.message);
    } catch (e) {
      setError(api.errorMessage(e));
    }
  };

  return (
    <Section
      title="Markdown → rich text"
      description="Headings, bold, italic, inline code, fenced code blocks, links, lists, blockquotes, horizontal rules. Copies as real styled text — paste into Mail, Notes or Word and it renders bold as bold, not as literal asterisks."
      actions={
        <Button tone="primary" onClick={() => void copy()}>
          Copy as rich text
        </Button>
      }
    >
      <div className="grid grid-cols-2 gap-4">
        <Field label="Markdown">
          <TextArea
            rows={8}
            mono
            value={markdown}
            onChange={setMarkdown}
            placeholder={"## Heading\n\nSome **bold** and _italic_ text with a [link](https://example.com)."}
          />
        </Field>
        <Field label="Preview">
          <div className="h-[168px] overflow-y-auto rounded-lg border border-line-strong/60 bg-base/60 px-3 py-2 text-[13px] leading-relaxed text-ink [&_a]:text-accent [&_a]:underline [&_code]:rounded [&_code]:bg-raised [&_code]:px-1 [&_code]:font-mono [&_code]:text-2xs [&_h1]:text-[16px] [&_h1]:font-semibold [&_h2]:text-[14px] [&_h2]:font-semibold [&_li]:ml-4 [&_ol]:list-decimal [&_pre]:overflow-x-auto [&_pre]:rounded [&_pre]:bg-raised [&_pre]:p-2 [&_ul]:list-disc">
            {/* Generated by this app's own Markdown renderer
                (`expander::markdown_to_html`), which HTML-escapes every
                character of user content before it ever builds a tag —
                including inside a link's `href` — so this is exactly as
                safe as QrPage's SVG injection just above it in this same
                folder. See the Rust module's doc comment for how that was
                verified. */}
            {previewHtml ? (
              <div dangerouslySetInnerHTML={{ __html: previewHtml }} />
            ) : (
              <span className="text-ink-faint">Preview appears here as you type.</span>
            )}
          </div>
        </Field>
      </div>

      {error && <p className="mt-3 text-2xs text-danger">{error}</p>}
      {note && !error && <p className="mt-3 text-2xs text-ink-faint">{note}</p>}
    </Section>
  );
}

// ===========================================================================
// Proofreader
// ===========================================================================

function ProofreadSection() {
  const [text, setText] = useState("");
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<ProofreadResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    if (!text.trim()) return;
    setLoading(true);
    setError(null);
    setResult(null);
    try {
      setResult(await api.expanderProofread(text));
    } catch (e) {
      setError(api.errorMessage(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <Section
      title="Proofread"
      description='Catches the class of mistake a spellchecker cannot see — every word spelled correctly, the sentence wrong: "their/there", subject-verb agreement, a stale date, a dropped "not". Sends the text through whichever backend Settings → AI has configured, so this is the one section here that leaves this Mac.'
      actions={
        <Button tone="primary" onClick={() => void run()} disabled={loading || !text.trim()}>
          {loading ? <Spinner /> : null} Proofread
        </Button>
      }
    >
      <TextArea rows={6} value={text} onChange={setText} placeholder="Paste or write the text to check…" />

      {error && (
        <div className="mt-3">
          <Callout tone="danger">{error}</Callout>
        </div>
      )}

      {result && (
        <div className="mt-4 space-y-3">
          {result.issues.length === 0 ? (
            <Callout tone="positive">Nothing wrong found.</Callout>
          ) : (
            <div className="space-y-2">
              {result.issues.map((issue, i) => (
                <div key={i} className="rounded-lg border border-line bg-base/40 p-3 text-[13px]">
                  <p>
                    <span className="text-danger line-through">{issue.original}</span>
                    {" → "}
                    <span className="font-medium text-positive">{issue.suggestion}</span>
                  </p>
                  <p className="mt-1 text-2xs text-ink-faint">{issue.reason}</p>
                </div>
              ))}
            </div>
          )}

          <Field label="Corrected text">
            <TextArea rows={6} value={result.corrected} onChange={() => {}} />
          </Field>
          <Button
            size="sm"
            onClick={() => {
              void navigator.clipboard.writeText(result.corrected);
            }}
          >
            Copy corrected text
          </Button>
        </div>
      )}
    </Section>
  );
}
