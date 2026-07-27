/**
 * The Clipboard tab.
 *
 * `ClipboardView` was built to live inside the palette, driven by the palette's
 * input and arrow keys. As a tab it owns those itself: its own search field, its
 * own selection, and Enter to copy. That is the whole of this wrapper.
 */

import { useEffect, useRef, useState } from "react";

import { useDebounced, useToasts } from "@/shared/hooks";
import { cx } from "@/shared/ui";

import { ClipboardView } from "../ClipboardView";

export function ClipboardTabPage({ active }: { active: boolean }) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const [count, setCount] = useState(0);
  const { toasts, notify } = useToasts();

  const inputRef = useRef<HTMLInputElement>(null);
  const activate = useRef<() => void>(() => {});
  const debounced = useDebounced(query, 45);

  useEffect(() => {
    if (active) inputRef.current?.focus();
  }, [active]);

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.metaKey && event.key !== "p") return;

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setSelected((i) => (count === 0 ? 0 : (i + 1) % count));
        break;
      case "ArrowUp":
        event.preventDefault();
        setSelected((i) => (count === 0 ? 0 : (i - 1 + count) % count));
        break;
      case "Enter":
        event.preventDefault();
        activate.current();
        break;
      case "Escape":
        event.preventDefault();
        setQuery("");
        break;
    }
  };

  return (
    <div className="relative flex h-full w-full flex-col overflow-hidden">
      <div className="flex shrink-0 items-center gap-3 border-b border-line px-5 py-3">
        <span aria-hidden="true" className="shrink-0 text-ink-faint">
          ❐
        </span>
        <input
          ref={inputRef}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setSelected(0);
          }}
          onKeyDown={onKeyDown}
          placeholder="Search your clipboard history…"
          spellCheck={false}
          autoComplete="off"
          className="min-w-0 flex-1 bg-transparent text-[15px] text-ink placeholder:text-ink-faint focus:outline-none"
        />
        <span className="shrink-0 text-2xs tabular-nums text-ink-faint">
          {count} item{count === 1 ? "" : "s"}
        </span>
      </div>

      <ClipboardView
        query={debounced}
        selectedIndex={selected}
        onCountChange={setCount}
        onNotify={notify}
        registerActivate={(fn) => {
          activate.current = fn;
        }}
      />

      <div className="pointer-events-none absolute bottom-4 left-1/2 z-50 flex -translate-x-1/2 flex-col items-center gap-2">
        {toasts.map((toast) => (
          <div
            key={toast.id}
            className={cx(
              "animate-fade-rise glass-raised max-w-[420px] rounded-lg px-3.5 py-2 text-2xs shadow-float",
              toast.tone === "error" ? "text-danger" : "text-ink-soft",
            )}
          >
            {toast.message}
          </div>
        ))}
      </div>
    </div>
  );
}
