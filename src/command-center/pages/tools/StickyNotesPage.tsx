/**
 * Sticky notes.
 *
 * The thing people reach for a launcher to do most often and the thing Caduceus
 * did not have: somewhere to put four words before you lose them.
 *
 * # Two decisions worth stating
 *
 * **It saves as you type, with no Save button.** A note you have to remember to
 * save is a note you will lose, and the whole point is to be faster than
 * opening a text editor.
 *
 * **The notes live in `localStorage`, not in a database.** They are a scratch
 * surface, not a document store: a few kilobytes of text that should survive a
 * restart and not need a schema, a migration, or a sync story. If they ever
 * want searching, tagging or pinning to the desktop, that is the point at which
 * they earn a table.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { ToolPageProps } from "../ToolPage";
import { Button, cx } from "@/shared/ui";

interface Note {
  id: string;
  text: string;
  colour: string;
  /** Epoch millis; drives the ordering and the "edited" line. */
  updatedAt: number;
}

const STORAGE_KEY = "caduceus.notes.v1";

/** Deliberately muted: a wall of saturated squares is hard to read text on. */
const COLOURS = ["#fde68a", "#bbf7d0", "#bfdbfe", "#fbcfe8", "#e9d5ff", "#fed7aa"];

export function StickyNotesPage({ onSetTitle }: ToolPageProps) {
  const [notes, setNotes] = useState<Note[]>(() => load());
  const [activeId, setActiveId] = useState<string | null>(() => load()[0]?.id ?? null);
  const [query, setQuery] = useState("");
  const editorRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => onSetTitle("Sticky Notes"), [onSetTitle]);

  // Debounced: typing a sentence should not be a dozen serialisations of every
  // note you have.
  useEffect(() => {
    const timer = setTimeout(() => save(notes), 250);
    return () => clearTimeout(timer);
  }, [notes]);

  const active = notes.find((note) => note.id === activeId) ?? null;

  const create = useCallback(() => {
    const note: Note = {
      id: `note-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
      text: "",
      colour: COLOURS[Math.floor(Math.random() * COLOURS.length)],
      updatedAt: Date.now(),
    };
    setNotes((current) => [note, ...current]);
    setActiveId(note.id);
    // The point of a new note is to type in it.
    setTimeout(() => editorRef.current?.focus(), 0);
  }, []);

  const update = (id: string, patch: Partial<Note>) =>
    setNotes((current) =>
      current.map((note) =>
        note.id === id ? { ...note, ...patch, updatedAt: Date.now() } : note,
      ),
    );

  const remove = (id: string) =>
    setNotes((current) => {
      const remaining = current.filter((note) => note.id !== id);
      if (activeId === id) setActiveId(remaining[0]?.id ?? null);
      return remaining;
    });

  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const sorted = [...notes].sort((a, b) => b.updatedAt - a.updatedAt);
    return needle ? sorted.filter((n) => n.text.toLowerCase().includes(needle)) : sorted;
  }, [notes, query]);

  return (
    <div className="flex h-full">
      {/* --- the board --------------------------------------------------- */}
      <div className="flex w-[260px] shrink-0 flex-col border-r border-line">
        <div className="row shrink-0 gap-2 border-b border-line px-3 py-2">
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search notes…"
            className="min-w-0 flex-1 rounded-lg border border-line bg-base/40 px-2.5 py-1.5 text-2xs text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
          <Button size="sm" tone="primary" onClick={create} title="New note">
            +
          </Button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {shown.length === 0 ? (
            <p className="px-2 py-8 text-center text-2xs text-ink-faint">
              {query ? "Nothing matches." : "No notes yet. Press + to start one."}
            </p>
          ) : (
            shown.map((note) => (
              <button
                key={note.id}
                type="button"
                onClick={() => setActiveId(note.id)}
                className={cx(
                  "mb-1.5 block w-full rounded-lg border px-2.5 py-2 text-left transition-colors",
                  note.id === activeId
                    ? "border-accent/40 bg-accent/10"
                    : "border-line hover:bg-raised/60",
                )}
              >
                <div className="row gap-2">
                  <span
                    aria-hidden="true"
                    className="h-2.5 w-2.5 shrink-0 rounded-full"
                    style={{ background: note.colour }}
                  />
                  <span className="min-w-0 flex-1 truncate text-2xs text-ink">
                    {firstLine(note.text) || <span className="text-ink-faint">Empty note</span>}
                  </span>
                </div>
                <p className="mt-0.5 pl-[18px] text-[10px] text-ink-faint">
                  {relative(note.updatedAt)}
                </p>
              </button>
            ))
          )}
        </div>
      </div>

      {/* --- the note ---------------------------------------------------- */}
      {active ? (
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="row shrink-0 justify-between gap-2 border-b border-line px-4 py-2">
            <div className="row gap-1.5">
              {COLOURS.map((colour) => (
                <button
                  key={colour}
                  type="button"
                  aria-label={`Colour ${colour}`}
                  onClick={() => update(active.id, { colour })}
                  className={cx(
                    "h-5 w-5 rounded-full border transition-transform hover:scale-110",
                    active.colour === colour ? "border-ink" : "border-line",
                  )}
                  style={{ background: colour }}
                />
              ))}
            </div>
            <div className="row gap-1">
              <Button
                size="sm"
                tone="ghost"
                onClick={() => {
                  void navigator.clipboard.writeText(active.text);
                }}
              >
                Copy
              </Button>
              <Button size="sm" tone="danger" onClick={() => remove(active.id)}>
                Delete
              </Button>
            </div>
          </div>

          <textarea
            ref={editorRef}
            value={active.text}
            onChange={(e) => update(active.id, { text: e.target.value })}
            placeholder="Write it down before you lose it…"
            spellCheck
            className="min-h-0 flex-1 resize-none bg-transparent px-5 py-4 text-[14px] leading-relaxed text-ink placeholder:text-ink-faint focus:outline-none"
            style={{
              // A hint of the note's colour rather than the full block: text on
              // saturated yellow is unreadable in a dark theme.
              background: `linear-gradient(to bottom, ${active.colour}14, transparent 220px)`,
            }}
          />

          <p className="shrink-0 border-t border-line px-5 py-1.5 text-2xs text-ink-faint">
            Saved as you type · edited {relative(active.updatedAt)} · {count(active.text)}
          </p>
        </div>
      ) : (
        <div className="flex flex-1 items-center justify-center">
          <div className="text-center">
            <p className="text-[13px] text-ink-mute">Nothing selected.</p>
            <Button className="mt-3" tone="primary" onClick={create}>
              New note
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

function load(): Note[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    // Validated rather than trusted: a note whose text is not a string would
    // render as `undefined` and be uneditable.
    return parsed.filter(
      (n): n is Note =>
        Boolean(n) &&
        typeof (n as Note).id === "string" &&
        typeof (n as Note).text === "string",
    );
  } catch {
    return [];
  }
}

function save(notes: Note[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(notes));
  } catch {
    // Out of quota, or storage disabled. The notes are still on screen and
    // still editable; losing them at the end of the session is bad, and
    // throwing an error box over the top of them would not save any.
  }
}

const firstLine = (text: string) => text.split("\n")[0]?.trim() ?? "";

function count(text: string): string {
  const words = text.trim() ? text.trim().split(/\s+/).length : 0;
  return `${words} word${words === 1 ? "" : "s"}`;
}

function relative(at: number): string {
  const seconds = Math.round((Date.now() - at) / 1000);
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return new Date(at).toLocaleDateString();
}
