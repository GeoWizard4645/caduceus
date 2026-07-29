/**
 * The floating meeting window — always-on-top, visible over a full-screen
 * call, the "pop out, just like macparakeet" half of the product complaint
 * this feature was written against.
 *
 * # Why there is no notes field here
 *
 * See `meeting.rs::meeting_open_popout`'s module doc for the full reasoning.
 * In short: this window is built with `configure_staff_floating`
 * (`Kind::Staff` in `window/panel.rs`), which can never become the key
 * window — so it can never receive typed keystrokes, only clicks. Notes stay
 * in the Command Center tab (`MeetingPage.tsx`), which already has the full
 * editor, "Copy all", and "Save to Notes". The "Open notes" button below is
 * how you get there.
 *
 * # Why this window works at all without any Rust wiring for itself
 *
 * It does not — not yet. See `meeting.rs`'s module doc for the two pieces of
 * wiring (`generate_handler!`, `capabilities/default.json`) this window
 * needs before it can invoke a single command or receive a single event.
 * Written to work the moment that wiring lands, not written to work now.
 *
 * # Staying in sync with the Command Center tab
 *
 * This window and `MeetingPage.tsx` are separate webviews with separate React
 * trees, but they listen to the same global Tauri events
 * (`voice-partial`/`voice-state`/`voice-result`/the recording status event)
 * and run the identical accumulation logic in `useMeetingTranscript.ts`, so
 * their transcripts converge in real time without either one talking to the
 * other directly — except for the one case that genuinely needs it, the
 * post-call system-audio segment, which is broadcast explicitly (see
 * `meetingApi.ts`). A transcript segment finalised *before* this window was
 * opened is picked up from `localStorage` at mount instead — a few hundred
 * milliseconds behind the Command Center tab's own debounced write at worst,
 * per `usePersisted`'s save delay.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef } from "react";

import * as api from "@/shared/api";
import { cx } from "@/shared/ui";

import { MeetingControls } from "./MeetingControls";
import { useMeetingSession } from "./useMeetingSession";

export function MeetingPopout() {
  // The pop-out is its own window, always "the active one" for the purposes
  // of the `active` gate every event listener in `useMeetingTranscript`
  // takes — there is no tab-switching concept inside a single-purpose
  // floating panel to gate against.
  const session = useMeetingSession(true);
  const transcriptRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = transcriptRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [session.transcript]);

  return (
    <div className="relative h-full w-full overflow-hidden p-2">
      <div className="glass shadow-panel flex h-full w-full flex-col overflow-hidden rounded-cad border border-line">
        {/* --- chrome: drag strip + close ----------------------------- */}
        <div
          data-tauri-drag-region
          title="Drag to move"
          className="row shrink-0 cursor-grab justify-between gap-2 px-3 py-1.5 active:cursor-grabbing"
        >
          <span className="text-2xs font-medium text-ink">
            Meeting notes
            {session.listening && <span className="ml-2 text-[#ff5f57]">● live</span>}
          </span>
          <button
            type="button"
            aria-label="Close pop-out"
            title="Close (the meeting keeps running)"
            onClick={() => void getCurrentWindow().hide()}
            className="no-drag flex h-[18px] w-[18px] items-center justify-center rounded-full text-[10px] leading-none text-ink-faint transition-colors hover:bg-raised hover:text-ink"
          >
            ✕
          </button>
        </div>

        {/* --- transcript ----------------------------------------------- */}
        <div
          ref={transcriptRef}
          className="no-drag min-h-0 flex-1 overflow-y-auto px-3 py-2 text-2xs leading-relaxed text-ink-soft"
        >
          {session.transcript || (
            <span className="text-ink-faint">
              {session.meetingActive
                ? "Listening…"
                : "Start the meeting to transcribe your microphone and call audio live."}
            </span>
          )}
        </div>

        {/* --- controls --------------------------------------------------- */}
        <div className="no-drag shrink-0 px-2 pb-2">
          <MeetingControls session={session} compact />
          <button
            type="button"
            onClick={() => void api.openCommandCenter().catch(() => {})}
            className={cx(
              "mt-2 w-full rounded-cad border border-line bg-transparent py-1.5",
              "text-2xs text-ink-faint transition-colors hover:bg-raised hover:text-ink",
            )}
          >
            Open notes in Command Center
          </button>
        </div>
      </div>
    </div>
  );
}
