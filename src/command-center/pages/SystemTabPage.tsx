/**
 * The System Monitor tab: `SystemView` with its own filter field.
 */

import { useEffect, useRef, useState } from "react";

import { useDebounced, useToasts } from "@/shared/hooks";
import { cx } from "@/shared/ui";

import { SystemView } from "../SystemView";

export function SystemTabPage({ active }: { active: boolean }) {
  const [query, setQuery] = useState("");
  const { toasts, notify } = useToasts();
  const inputRef = useRef<HTMLInputElement>(null);
  const debounced = useDebounced(query, 45);

  useEffect(() => {
    if (active) inputRef.current?.focus();
  }, [active]);

  return (
    <div className="relative flex h-full w-full flex-col overflow-hidden">
      <div className="flex shrink-0 items-center gap-3 border-b border-line px-5 py-3">
        <span aria-hidden="true" className="shrink-0 text-ink-faint">
          ◔
        </span>
        <input
          ref={inputRef}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Escape" || !query) return;
            // Claimed, or the shell's handler closes the whole tab in the same
            // keypress — clearing the filter and losing the page with it.
            event.preventDefault();
            setQuery("");
          }}
          placeholder="Filter processes…"
          spellCheck={false}
          autoComplete="off"
          className="min-w-0 flex-1 bg-transparent text-[15px] text-ink placeholder:text-ink-faint focus:outline-none"
        />
      </div>

      <SystemView query={debounced} onNotify={notify} />

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
