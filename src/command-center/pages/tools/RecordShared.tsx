/**
 * The shared half of screen recording and meeting notes.
 *
 * Both are the same capture — ScreenCaptureKit, system audio, optionally the
 * microphone — differing only in whether the video track exists and what you do
 * with the result. So the controls, the clock, the level meter and the "where
 * did it save" line live here once.
 *
 * # Why this can record what your Mac is playing
 *
 * macOS's own recorder (⇧⌘5) cannot. It captures the screen and the microphone,
 * which makes a recording of a call into a recording of you talking to silence.
 * Until macOS 13 the only way round it was to install an audio driver, which is
 * not a thing Caduceus is going to put on your Mac. ScreenCaptureKit added a
 * system-audio tap, and this uses it.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import { Button, Callout, cx } from "@/shared/ui";

/** Emitted by Rust whenever the recording state changes. */
export const RECORDING_EVENT = "caduceus://recording";

export function useRecording() {
  const [status, setStatus] = useState<api.RecordingStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  useTauriEvent<api.RecordingStatus>(RECORDING_EVENT, setStatus);

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.recordingStatus());
    } catch {
      // A status read that fails is not worth an error message; the next tick
      // will try again.
    }
  }, []);

  // Polled as well as pushed: the clock has to tick, and the level meter is
  // sampled rather than evented.
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 500);
    return () => clearInterval(timer);
  }, [refresh]);

  const start = useCallback(
    async (mode: api.RecordMode, microphone: boolean) => {
      setError(null);
      setSaved(null);
      try {
        await api.recordingStart(mode, microphone);
        await refresh();
      } catch (e) {
        setError(api.errorMessage(e));
      }
    },
    [refresh],
  );

  const setPaused = useCallback(
    async (paused: boolean) => {
      try {
        await api.recordingPause(paused);
        await refresh();
      } catch (e) {
        setError(api.errorMessage(e));
      }
    },
    [refresh],
  );

  const stop = useCallback(async () => {
    try {
      setSaved(await api.recordingStop());
      setError(null);
    } catch (e) {
      setError(api.errorMessage(e));
    }
    await refresh();
  }, [refresh]);

  return { status, error, saved, start, setPaused, stop };
}

export function RecordControls({
  mode,
  microphone,
  onMicrophoneChange,
  recording,
  startLabel,
}: {
  mode: api.RecordMode;
  microphone: boolean;
  onMicrophoneChange: (next: boolean) => void;
  recording: ReturnType<typeof useRecording>;
  startLabel: string;
}) {
  const { status, error, saved, start, setPaused, stop } = recording;
  const active = status?.active ?? false;
  const paused = status?.paused ?? false;

  return (
    <div className="rounded-cad border border-line bg-surface/50 p-4">
      {!active ? (
        <>
          <label className="row mb-3 cursor-pointer gap-2">
            <input
              type="checkbox"
              checked={microphone}
              onChange={(e) => onMicrophoneChange(e.target.checked)}
              className="h-4 w-4 accent-current"
            />
            <span className="text-[13px] text-ink">
              Record my microphone too
              <span className="ml-2 text-2xs text-ink-faint">
                kept as a separate track, so you can tell who said what
              </span>
            </span>
          </label>

          <Button tone="primary" onClick={() => void start(mode, microphone)}>
            {startLabel}
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
              {clock(status?.seconds ?? 0)}
            </span>
            <span className="text-2xs text-ink-faint">{paused ? "Paused" : "Recording"}</span>
          </span>

          {microphone && <Level value={status?.level ?? 0} />}

          <span className="row ml-auto gap-2">
            <Button onClick={() => void setPaused(!paused)}>{paused ? "Resume" : "Pause"}</Button>
            <Button tone="primary" onClick={() => void stop()}>
              Stop and save
            </Button>
          </span>
        </div>
      )}

      {error && (
        <div className="mt-3">
          <Callout tone="warn" title="That did not record">
            <p>{error}</p>
          </Callout>
        </div>
      )}

      {saved && (
        <div className="row mt-3 gap-2">
          <p className="min-w-0 flex-1 truncate text-2xs text-ink-mute" title={saved}>
            Saved to {saved}
          </p>
          <Button size="sm" tone="ghost" onClick={() => void api.revealPath(saved)}>
            Show in Finder
          </Button>
        </div>
      )}
    </div>
  );
}

/** A level meter, so a silent recording is visibly silent while it happens. */
function Level({ value }: { value: number }) {
  // Peak decays faster than it rises, which is what makes a meter readable
  // rather than a strobe.
  const peak = useRef(0);
  peak.current = Math.max(value, peak.current * 0.88);

  return (
    <span className="row gap-0.5" aria-label={`Input level ${Math.round(value * 100)}%`}>
      {Array.from({ length: 12 }, (_, i) => (
        <span
          key={i}
          className={cx(
            "h-3 w-1 rounded-sm transition-colors",
            peak.current * 12 > i
              ? i > 9
                ? "bg-danger"
                : "bg-positive"
              : "bg-overlay",
          )}
        />
      ))}
    </span>
  );
}

export function clock(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(rest)}` : `${minutes}:${pad(rest)}`;
}
