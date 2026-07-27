/**
 * Clipboard history browser.
 *
 * Reached three ways: the `clipboard_view` shortcut, the `/v` prefix, and the
 * tray menu. Keyboard-first, like the rest of the palette — arrows to move,
 * Enter to copy, ⌘P to pin, ⌘⌫ to delete. Click or drag an entry — the palette
 * stays open so you can paste or drop elsewhere.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import { relativeTime } from "@/shared/providers";
import type { ClipboardEntry } from "@/shared/types";
import { EVENTS } from "@/shared/types";
import { EmptyState, IconButton, Kbd, cx } from "@/shared/ui";

export function ClipboardView({
  query,
  selectedIndex,
  onCountChange,
  onNotify,
  registerActivate,
}: {
  query: string;
  selectedIndex: number;
  onCountChange: (count: number) => void;
  onNotify: (message: string, tone?: "info" | "error") => void;
  /** Lets the parent trigger "use the selected entry" from its Enter handler. */
  registerActivate: (fn: () => void) => void;
}) {
  const [entries, setEntries] = useState<ClipboardEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const listRef = useRef<HTMLDivElement>(null);
  const suppressClick = useRef(false);

  const load = useCallback(async () => {
    try {
      const rows = await api.clipboardList(query, 200);
      setEntries(rows);
      onCountChange(rows.length);
    } catch (error) {
      onNotify(api.errorMessage(error), "error");
    } finally {
      setLoading(false);
    }
  }, [query, onCountChange, onNotify]);

  useEffect(() => {
    void load();
  }, [load]);

  // New copies appear without the user reopening the window.
  useTauriEvent<number>(EVENTS.clipboardChanged, () => void load());

  const copy = useCallback(
    async (entry: ClipboardEntry, quiet = false) => {
      if (entry.unreadable) {
        onNotify("That entry cannot be decrypted with the current key.", "error");
        return;
      }
      try {
        await api.clipboardCopy(entry.id);
        if (!quiet) onNotify("Copied");
      } catch (error) {
        onNotify(api.errorMessage(error), "error");
      }
    },
    [onNotify],
  );

  const fillDragData = useCallback((entry: ClipboardEntry, e: React.DragEvent) => {
    if (entry.unreadable) {
      e.preventDefault();
      return;
    }

    e.dataTransfer.effectAllowed = "copy";
    e.dataTransfer.dropEffect = "copy";

    if (entry.kind === "image") {
      const url = entry.thumbnail ?? "";
      if (url) {
        e.dataTransfer.setData("text/plain", entry.preview || "Image");
        e.dataTransfer.setData("text/uri-list", url);
        const img = document.createElement("img");
        img.src = url;
        if (img.complete) e.dataTransfer.setDragImage(img, 12, 12);
        else img.onload = () => e.dataTransfer.setDragImage(img, 12, 12);
      }
      return;
    }

    const text = entry.content ?? entry.preview;
    e.dataTransfer.setData("text/plain", text);

    if (entry.kind === "files") {
      const uris = text
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean)
        .map((path) => {
          if (path.startsWith("file://")) return path;
          return `file://${path.split("/").map(encodeURIComponent).join("/")}`;
        })
        .join("\r\n");
      if (uris) e.dataTransfer.setData("text/uri-list", uris);
    }
  }, []);

  const onRowDragStart = useCallback(
    (entry: ClipboardEntry, e: React.DragEvent) => {
      suppressClick.current = true;
      fillDragData(entry, e);
      void copy(entry, true);
    },
    [copy, fillDragData],
  );

  // Give the parent a stable way to activate the current row.
  const selected = entries[selectedIndex];
  useEffect(() => {
    registerActivate(() => {
      if (selected) void copy(selected);
    });
  }, [registerActivate, selected, copy]);

  // Keep the highlighted row on screen while arrowing through a long list.
  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${selectedIndex}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  // ⌘P / ⌘⌫ act on the selected row.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!selected || !(e.metaKey || e.ctrlKey)) return;

      if (e.key.toLowerCase() === "p") {
        e.preventDefault();
        void api
          .clipboardPin(selected.id, !selected.pinned)
          .then(load)
          .catch((error) => onNotify(api.errorMessage(error), "error"));
      } else if (e.key === "Backspace" || e.key === "Delete") {
        e.preventDefault();
        void api
          .clipboardDelete(selected.id)
          .then(load)
          .catch((error) => onNotify(api.errorMessage(error), "error"));
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selected, load, onNotify]);

  if (loading) {
    return <div className="px-5 py-8 text-center text-2xs text-ink-faint">Loading history…</div>;
  }

  if (entries.length === 0) {
    return (
      <EmptyState
        icon="❐"
        title={query ? `Nothing matching “${query}”` : "No clipboard history yet"}
        hint={
          query
            ? "Try fewer words — every word has to appear in the entry."
            : "Copy something and it will show up here. History can be turned off or encrypted in Settings → Clipboard."
        }
      />
    );
  }

  return (
    <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto px-2 py-2">
      {entries.map((entry, index) => (
        <div
          key={entry.id}
          data-index={index}
          draggable={!entry.unreadable}
          onDragStart={(e) => onRowDragStart(entry, e)}
          onDragEnd={() => {
            window.setTimeout(() => {
              suppressClick.current = false;
            }, 0);
          }}
          onClick={() => {
            if (suppressClick.current) {
              suppressClick.current = false;
              return;
            }
            void copy(entry);
          }}
          className={cx(
            "group flex cursor-default items-start gap-3 rounded-lg px-3 py-2.5 transition-colors duration-100",
            index === selectedIndex ? "bg-accent/12" : "hover:bg-raised/70",
            !entry.unreadable && "cursor-grab active:cursor-grabbing",
          )}
        >
          {/* Thumbnail for images, a type glyph otherwise. */}
          {entry.thumbnail ? (
            <img
              src={entry.thumbnail}
              alt=""
              className="h-9 w-9 shrink-0 rounded-md border border-line object-cover"
            />
          ) : (
            <span
              aria-hidden="true"
              className={cx(
                "mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-md border text-[13px]",
                index === selectedIndex
                  ? "border-accent/40 bg-accent/15 text-accent"
                  : "border-line bg-raised text-ink-mute",
              )}
            >
              {entry.unreadable ? "🔒" : entry.kind === "files" ? "⌥" : "≡"}
            </span>
          )}

          <div className="min-w-0 flex-1">
            <p
              className={cx(
                "line-clamp-2 text-[13px] leading-snug",
                entry.unreadable ? "italic text-ink-faint" : "text-ink",
              )}
            >
              {entry.preview}
            </p>
            <p className="mt-0.5 truncate text-2xs text-ink-faint">
              {[
                entry.pinned ? "Pinned" : null,
                entry.sourceApp,
                entry.kind === "image" && entry.width ? `${entry.width}×${entry.height}` : null,
                relativeTime(entry.createdAt),
              ]
                .filter(Boolean)
                .join(" · ")}
            </p>
          </div>

          <div
            className="row shrink-0 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100"
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => e.stopPropagation()}
          >
            <IconButton
              label={entry.pinned ? "Unpin" : "Pin"}
              onClick={() => {
                void api.clipboardPin(entry.id, !entry.pinned).then(load);
              }}
            >
              <span className={entry.pinned ? "text-accent" : ""}>★</span>
            </IconButton>
            <IconButton
              label="Delete"
              tone="danger"
              onClick={() => {
                void api.clipboardDelete(entry.id).then(load);
              }}
            >
              ×
            </IconButton>
          </div>

          {index === selectedIndex && (
            <div className="row hidden shrink-0 pt-1 text-ink-faint md:flex">
              <Kbd>↵</Kbd>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
