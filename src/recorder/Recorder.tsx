/**
 * The recording HUD.
 *
 * A pill at the bottom of the screen, with what the recogniser is hearing
 * floating just above it. On screen for exactly as long as a microphone is
 * live, and never for a moment longer.
 *
 * # The rule this window exists to enforce
 *
 * **A live microphone is always visible, and stopping it is always one click.**
 * Dictation used to report itself inside the Command Center, which meant a
 * recording could be running behind another window, on another display, or in a
 * palette that had been dismissed — with no indication and no way out. When the
 * speech helper wedged, that turned into an app you could not use and a
 * microphone you could not switch off.
 *
 * # The controls
 *
 * ```text
 *   ●  0:14   ❚❚ pause    ■ stop
 *                          └─ holds the recording and offers "End now",
 *                             so a mis-click never loses the transcript
 * ```
 *
 * Space held pauses for as long as it is down. Enter does what ■ does. Both
 * work while this window has focus — it is a non-activating panel, so clicking
 * it does not steal focus from whatever you are dictating into, which also
 * means it does not receive keys until you click it. That is the right trade:
 * globally swallowing the space bar during dictation would break typing
 * everywhere, and the buttons are always there.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import { EVENTS, type VoiceOutcome, type VoiceState } from "@/shared/types";
import { cx } from "@/shared/ui";

export function Recorder() {
  const [state, setState] = useState<VoiceState>("recording");
  const [transcript, setTranscript] = useState("");
  const [elapsed, setElapsed] = useState(0);
  /** Set once ■ has been pressed: paused, and offering to finish. */
  const [ending, setEnding] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const startedAt = useRef(Date.now());
  const transcriptRef = useRef<HTMLDivElement>(null);
  /** True while the space bar is physically down, to distinguish hold from tap. */
  const spaceHeld = useRef(false);

  useTauriEvent<VoiceState>(EVENTS.voiceState, (next) => {
    setState(next);
    if (next === "recording") {
      // A fresh session: reset everything the previous one left behind.
      startedAt.current = Date.now();
      setTranscript("");
      setElapsed(0);
      setEnding(false);
      setError(null);
    }
  });

  useTauriEvent<string>(EVENTS.voicePartial, setTranscript);

  // Start failures used to hide this window instantly, so "Live speech helper
  // did not become ready" only appeared in the log. Keep the HUD up and show
  // the error here — Discard / Esc still closes it.
  useTauriEvent<VoiceOutcome>(EVENTS.voiceResult, (outcome) => {
    if (outcome.ok) return;
    setError(outcome.error ?? "Dictation failed.");
    setState("idle");
  });

  // The clock stops while held, because a paused recording is not recording.
  useEffect(() => {
    if (state !== "recording") return;
    const timer = setInterval(
      () => setElapsed(Math.floor((Date.now() - startedAt.current) / 1000)),
      250,
    );
    return () => clearInterval(timer);
  }, [state]);

  // Follow the transcript as it grows; the newest words are the interesting ones.
  useEffect(() => {
    const el = transcriptRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [transcript]);

  const setPaused = useCallback(async (paused: boolean) => {
    try {
      await api.voicePause(paused);
      setError(null);
    } catch (e) {
      // Batch capture cannot pause. Say so rather than leaving a button that
      // looks like it did something.
      setError(api.errorMessage(e));
    }
  }, []);

  const beginEnding = useCallback(() => {
    setEnding(true);
    void setPaused(true);
  }, [setPaused]);

  const finish = useCallback(() => {
    setEnding(false);
    void api.voiceFinish().catch((e) => setError(api.errorMessage(e)));
  }, []);

  const resume = useCallback(() => {
    setEnding(false);
    void setPaused(false);
  }, [setPaused]);

  const discard = useCallback(() => {
    void api.voiceCancel().catch(() => {});
  }, []);

  // --- keyboard ------------------------------------------------------------
  useEffect(() => {
    const down = (event: KeyboardEvent) => {
      if (event.key === " " && !event.repeat && !ending) {
        event.preventDefault();
        spaceHeld.current = true;
        void setPaused(true);
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        // Enter after ■ commits; Enter during recording does what ■ does.
        if (ending) finish();
        else beginEnding();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        discard();
      }
    };

    const up = (event: KeyboardEvent) => {
      if (event.key === " " && spaceHeld.current) {
        event.preventDefault();
        spaceHeld.current = false;
        // Only un-pause a pause this handler caused. Releasing space after
        // pressing ■ must not quietly restart a recording being ended.
        if (!ending) void setPaused(false);
      }
    };

    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, [beginEnding, discard, ending, finish, setPaused]);

  const paused = state === "paused";
  const transcribing = state === "transcribing";
  const failed = Boolean(error) && state === "idle";

  return (
    // Click-through everywhere except the pill and the transcript: this window
    // is 520px wide and sitting over the middle of somebody's screen.
    <div className="pointer-events-none flex h-full w-full flex-col items-center justify-end gap-2 pb-1">
      {/* --- what it is hearing ---------------------------------------- */}
      {(transcript || transcribing) && !failed && (
        <div
          ref={transcriptRef}
          className="glass pointer-events-auto max-h-[86px] w-full overflow-y-auto rounded-cad px-4 py-2.5 text-[13px] leading-relaxed text-ink shadow-float"
        >
          {transcript || <span className="text-ink-faint">Working out what you said…</span>}
        </div>
      )}

      {error && (
        <div className="glass pointer-events-auto max-w-[520px] rounded-cad px-3 py-2 text-2xs leading-relaxed text-danger shadow-float">
          {error}
        </div>
      )}

      {/* --- the pill --------------------------------------------------- */}
      <div className="glass pointer-events-auto flex items-center gap-2.5 rounded-full py-2 pl-3.5 pr-2 shadow-float">
        <Dot state={failed ? "idle" : state} />

        <span className="min-w-[38px] font-mono text-[13px] tabular-nums text-ink">
          {formatElapsed(elapsed)}
        </span>

        <span className="text-2xs text-ink-faint">
          {failed
            ? "Could not start"
            : transcribing
              ? "Transcribing"
              : paused
                ? "Paused"
                : "Listening"}
        </span>

        <span className="mx-0.5 h-4 w-px bg-line" aria-hidden="true" />

        {failed ? (
          <PillButton onClick={discard} title="Dismiss (esc)">
            Dismiss
          </PillButton>
        ) : ending ? (
          <>
            <PillButton onClick={resume} title="Keep going (space)">
              Resume
            </PillButton>
            <PillButton tone="primary" onClick={finish} title="Transcribe and use it (↵)">
              End now
            </PillButton>
          </>
        ) : (
          <>
            <IconPill
              onClick={() => void setPaused(!paused)}
              title={paused ? "Resume (space)" : "Pause — hold space to do this briefly"}
              label={paused ? "Resume" : "Pause"}
            >
              {paused ? <PlayGlyph /> : <PauseGlyph />}
            </IconPill>
            <IconPill
              onClick={beginEnding}
              title="Stop (↵)"
              label="Stop"
              tone="danger"
              disabled={transcribing}
            >
              <StopGlyph />
            </IconPill>
          </>
        )}

        <IconPill onClick={discard} title="Throw it away (esc)" label="Discard" tone="ghost">
          <CrossGlyph />
        </IconPill>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

/** The red dot. Pulses while listening, still while held. */
function Dot({ state }: { state: VoiceState }) {
  const live = state === "recording";
  return (
    <span className="relative flex h-2.5 w-2.5 shrink-0" aria-hidden="true">
      {live && (
        <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-[#ff3b30] opacity-75" />
      )}
      <span
        className={cx(
          "relative inline-flex h-2.5 w-2.5 rounded-full",
          state === "transcribing" ? "bg-accent" : live ? "bg-[#ff3b30]" : "bg-ink-faint",
        )}
      />
    </span>
  );
}

function PillButton({
  children,
  onClick,
  title,
  tone = "default",
}: {
  children: React.ReactNode;
  onClick: () => void;
  title: string;
  tone?: "default" | "primary";
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={cx(
        "h-7 shrink-0 rounded-full px-3 text-2xs font-medium transition-colors",
        tone === "primary"
          ? "bg-accent text-accent-ink hover:brightness-110"
          : "bg-raised text-ink-soft hover:bg-overlay hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}

function IconPill({
  children,
  onClick,
  title,
  label,
  tone = "default",
  disabled,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title: string;
  label: string;
  tone?: "default" | "danger" | "ghost";
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={label}
      onClick={onClick}
      disabled={disabled}
      className={cx(
        "flex h-7 w-7 shrink-0 items-center justify-center rounded-full transition-colors disabled:opacity-40",
        tone === "danger"
          ? "bg-[#ff3b30]/15 text-[#ff5f57] hover:bg-[#ff3b30]/25"
          : tone === "ghost"
            ? "text-ink-faint hover:bg-raised hover:text-ink"
            : "bg-raised text-ink-soft hover:bg-overlay hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}

// Drawn rather than typed: the glyph characters for these render at wildly
// different sizes and baselines across fonts, and a stop button that looks
// like a typo is not reassuring.
function PauseGlyph() {
  return (
    <svg viewBox="0 0 12 12" className="h-3 w-3 fill-current" aria-hidden="true">
      <rect x="2" y="1.5" width="3" height="9" rx="1" />
      <rect x="7" y="1.5" width="3" height="9" rx="1" />
    </svg>
  );
}

function PlayGlyph() {
  return (
    <svg viewBox="0 0 12 12" className="h-3 w-3 fill-current" aria-hidden="true">
      <path d="M3 1.8v8.4a.6.6 0 0 0 .92.5l6.4-4.2a.6.6 0 0 0 0-1L3.92 1.3A.6.6 0 0 0 3 1.8Z" />
    </svg>
  );
}

function StopGlyph() {
  return (
    <svg viewBox="0 0 12 12" className="h-3 w-3 fill-current" aria-hidden="true">
      <rect x="2" y="2" width="8" height="8" rx="1.6" />
    </svg>
  );
}

function CrossGlyph() {
  return (
    <svg viewBox="0 0 12 12" className="h-3 w-3 stroke-current" aria-hidden="true" fill="none">
      <path d="M3 3l6 6M9 3l-6 6" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}

/** `0:07`, `1:42`, `12:05`. */
function formatElapsed(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}
