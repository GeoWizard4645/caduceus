/**
 * A conversation, rendered.
 *
 * Shared deliberately: the Command Center shows this inline and the chat window
 * shows the same component beside a thread list. Two implementations would drift
 * — the palette would gain a fix the window never got, or the reverse.
 *
 * The component owns *rendering* only. Sending is the caller's job, because the
 * two surfaces disagree about what should happen afterwards: the palette keeps
 * its own input and its own focus rules, the window has a composer at the bottom.
 */

import { useEffect, useRef, useState } from "react";

import type { ChatMessage, Usage } from "@/shared/types";
import { Spinner, cx } from "@/shared/ui";

export interface StreamStatus {
  /** Partial assistant text so far. */
  text: string;
  /** Epoch ms when the request started. */
  startedAt: number;
  /** Final usage once the server reports it (often only on the last chunk). */
  usage?: Usage | null;
  /** True until the Done/Error event. */
  active: boolean;
}

export function Thread({
  messages,
  pending,
  stream,
  error,
  className,
  onCopy,
  onSaveToNotes,
}: {
  messages: ChatMessage[];
  /** A question that has been sent but not yet answered. */
  pending?: string | null;
  /** Live assistant draft while tokens stream in. */
  stream?: StreamStatus | null;
  error?: string | null;
  className?: string;
  onCopy?: (text: string) => void;
  onSaveToNotes?: (text: string) => void;
}) {
  const endRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [elapsedMs, setElapsedMs] = useState(0);

  // Tick the timer while a reply is in flight so "20 silent seconds" never
  // looks like a freeze.
  useEffect(() => {
    if (!stream?.active || !stream.startedAt) {
      setElapsedMs(0);
      return;
    }
    const tick = () => setElapsedMs(Date.now() - stream.startedAt);
    tick();
    const id = window.setInterval(tick, 200);
    return () => window.clearInterval(id);
  }, [stream?.active, stream?.startedAt]);

  // Follow the conversation, but only when already at the bottom — yanking the
  // view down while someone is reading back through the thread is worse than
  // making them scroll.
  useEffect(() => {
    const box = scrollRef.current;
    if (!box) return;
    const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 120;
    if (nearBottom) endRef.current?.scrollIntoView({ block: "end" });
  }, [messages, pending, stream?.text]);

  const empty = messages.length === 0 && !pending && !error && !stream;

  return (
    <div ref={scrollRef} className={cx("overflow-y-auto", className)}>
      {empty && (
        <div className="flex h-full items-center justify-center px-6 text-center">
          <p className="max-w-[38ch] text-2xs leading-relaxed text-ink-faint">
            Ask anything. This thread is saved, so you can pick it back up later.
          </p>
        </div>
      )}

      <div className="flex flex-col gap-3 px-4 py-4">
        {messages.map((m) => (
          <Bubble
            key={m.id}
            role={m.role}
            text={m.text}
            onCopy={onCopy}
            onSaveToNotes={onSaveToNotes}
          />
        ))}

        {pending && (
          <>
            <Bubble role="user" text={pending} />
            {stream && (stream.text || stream.active) ? (
              <div className="flex flex-col gap-1 self-stretch items-start">
                <div className="max-w-[85%] whitespace-pre-wrap break-words rounded-cad border border-line/60 bg-raised px-3 py-2 text-[13px] leading-relaxed text-ink-soft">
                  {stream.text || (
                    <span className="inline-flex items-center gap-2 text-ink-faint">
                      <Spinner />
                      Waiting for first token…
                    </span>
                  )}
                  {stream.active && stream.text ? (
                    <span className="ml-0.5 inline-block h-[1em] w-[2px] animate-pulse bg-accent align-text-bottom" />
                  ) : null}
                </div>
                <StatusLine
                  elapsedMs={elapsedMs}
                  usage={stream.usage}
                  chars={stream.text.length}
                  active={stream.active}
                />
              </div>
            ) : (
              <div className="row gap-2 self-start rounded-cad bg-raised px-3 py-2 text-2xs text-ink-faint">
                <Spinner />
                Thinking…
              </div>
            )}
          </>
        )}

        {error && (
          <div className="self-start rounded-cad border border-danger/30 bg-danger/10 px-3 py-2 text-2xs leading-relaxed text-danger">
            {error}
          </div>
        )}
      </div>
      <div ref={endRef} />
    </div>
  );
}

function StatusLine({
  elapsedMs,
  usage,
  chars,
  active,
}: {
  elapsedMs: number;
  usage?: Usage | null;
  chars: number;
  active: boolean;
}) {
  const seconds = (Math.max(0, elapsedMs) / 1000).toFixed(1);
  const out =
    usage?.outputTokens ??
    (chars > 0 ? Math.max(1, Math.round(chars / 4)) : null);
  const inn = usage?.inputTokens ?? null;
  const parts: string[] = [`${seconds}s`];
  if (inn != null) parts.push(`${inn} in`);
  if (out != null) {
    parts.push(usage?.outputTokens != null ? `${out} out` : `~${out} out`);
  }
  if (active && chars === 0) parts.push("loading model…");

  return (
    <p className="px-1 text-2xs tabular-nums text-ink-faint">
      {parts.join(" · ")}
    </p>
  );
}

function Bubble({
  role,
  text,
  onCopy,
  onSaveToNotes,
}: {
  role: "user" | "assistant";
  text: string;
  onCopy?: (text: string) => void;
  onSaveToNotes?: (text: string) => void;
}) {
  const mine = role === "user";
  return (
    <div className={cx("group flex flex-col gap-1", mine ? "items-end" : "items-start")}>
      <div
        className={cx(
          "max-w-[85%] whitespace-pre-wrap break-words rounded-cad px-3 py-2 text-[13px] leading-relaxed",
          mine
            ? "bg-accent/15 text-ink"
            : "border border-line/60 bg-raised text-ink-soft",
        )}
      >
        {text}
      </div>

      {/* Actions stay hidden until hover: a row of buttons under every turn
          turns a conversation into a toolbar. */}
      {!mine && (onCopy || onSaveToNotes) && (
        <div className="row gap-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
          {onCopy && (
            <MiniButton onClick={() => onCopy(text)} label="Copy" />
          )}
          {onSaveToNotes && (
            <MiniButton onClick={() => onSaveToNotes(text)} label="Save to Notes" />
          )}
        </div>
      )}
    </div>
  );
}

function MiniButton({ onClick, label }: { onClick: () => void; label: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="rounded px-1.5 py-0.5 text-2xs text-ink-faint transition-colors hover:bg-raised hover:text-ink"
    >
      {label}
    </button>
  );
}
