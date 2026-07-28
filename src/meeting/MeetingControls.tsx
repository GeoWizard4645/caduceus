/**
 * The one-button meeting control, shared between the Command Center tab and
 * the pop-out — see `useMeetingSession.ts` for why there is exactly one
 * Start and one Stop rather than the old page's two independent ones.
 *
 * Deliberately not `RecordShared.tsx`'s `RecordControls`: that component's
 * own Start button calls `recording.start()` directly, which is precisely
 * the "separate start" this feature exists to remove. Its `clock()` helper
 * is still reused below — no reason to reimplement time formatting.
 */

import { clock } from "@/command-center/pages/tools/RecordShared";
import { Button, Callout, cx } from "@/shared/ui";

import type { MeetingSession } from "./useMeetingSession";

export function MeetingControls({
  session,
  compact = false,
}: {
  session: MeetingSession;
  compact?: boolean;
}) {
  const { recording, meetingActive } = session;
  const paused = recording.status?.paused ?? false;

  return (
    <div
      className={cx(
        "rounded-cad border border-line bg-surface/50",
        compact ? "p-2.5" : "p-4",
      )}
    >
      {!meetingActive ? (
        <>
          {!compact && (
            <label className="row mb-3 cursor-pointer gap-2">
              <input
                type="checkbox"
                checked={session.microphone}
                onChange={(e) => session.setMicrophone(e.target.checked)}
                className="h-4 w-4 accent-current"
              />
              <span className="text-[13px] text-ink">
                Record my microphone too
                <span className="ml-2 text-2xs text-ink-faint">
                  kept as a separate track in the saved file
                </span>
              </span>
            </label>
          )}
          <Button tone="primary" onClick={() => void session.start()}>
            Start meeting
          </Button>
        </>
      ) : (
        <div className="row flex-wrap gap-3">
          <span className="row gap-2">
            <span className="relative flex h-2.5 w-2.5" aria-hidden="true">
              {!paused && (
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-[#ff3b30] opacity-75" />
              )}
              <span
                className={cx(
                  "relative inline-flex h-2.5 w-2.5 rounded-full",
                  paused ? "bg-ink-faint" : "bg-[#ff3b30]",
                )}
              />
            </span>
            <span className="font-mono text-[15px] tabular-nums text-ink">
              {clock(recording.status?.seconds ?? 0)}
            </span>
            {!compact && (
              <span className="text-2xs text-ink-faint">{paused ? "Paused" : "Recording"}</span>
            )}
          </span>

          <span className="row ml-auto gap-2">
            {recording.status?.active && (
              <Button
                size="sm"
                onClick={() => void recording.setPaused(!paused)}
              >
                {paused ? "Resume" : "Pause"}
              </Button>
            )}
            <Button size="sm" tone="primary" onClick={() => void session.stop()}>
              End meeting
            </Button>
          </span>
        </div>
      )}

      {(session.voiceError || recording.error) && (
        <div className="mt-3">
          <Callout tone="warn" title="Something did not start">
            <p>{[session.voiceError, recording.error].filter(Boolean).join(" — ")}</p>
          </Callout>
        </div>
      )}

      {session.callAudioStatus === "working" && (
        <p className="mt-3 text-2xs text-ink-faint">Transcribing the call audio…</p>
      )}
      {session.callAudioStatus === "error" && session.callAudioError && (
        <div className="mt-3">
          <Callout tone="warn" title="Could not transcribe the call audio">
            <p>{session.callAudioError}</p>
          </Callout>
        </div>
      )}
    </div>
  );
}
