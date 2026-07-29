/**
 * Meeting notes.
 *
 * Records both sides of a call — the people on it, via system audio, and you,
 * via the microphone — while showing a live transcript you can take notes
 * beside, and popping that transcript out into its own floating window so it
 * stays visible while you are actually in the call. Everything stays on the
 * Mac.
 *
 * # Why this is not just "record the screen"
 *
 * A recording of a forty-minute call is a forty-minute file nobody opens. What
 * people want afterwards is the text, and a couple of lines they typed while it
 * was happening. So this records audio only (a fraction of the size), runs the
 * transcript live so you can see it working, and puts the notes field next to
 * it rather than in another app.
 *
 * # One meeting, one Start, one Stop
 *
 * This used to have two separate starts — the recording and the live
 * dictation were independent buttons that happened to sit next to each other,
 * and starting one without the other was exactly as easy as starting both.
 * `useMeetingSession` (see `src/meeting/useMeetingSession.ts`) is the fix:
 * `MeetingControls` renders exactly one Start/Stop pair, and it drives both
 * subsystems every time.
 *
 * # Where the live transcript comes from
 *
 * On Apple Silicon, Parakeet v3 runs locally through FluidAudio. One stream
 * previews your microphone and another receives ScreenCaptureKit's system
 * audio, so both sides appear while the meeting is still running. The two
 * rolling hypotheses are kept separate so revisions do not erase each other.
 * Once you stop, the saved call track gets a full final pass and replaces the
 * lower-latency system-audio preview. Nothing is uploaded.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { readPersisted, usePersisted } from "@/shared/persist";
import { Button, Callout, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

import { MeetingControls } from "@/meeting/MeetingControls";
import { meetingOpenPopout } from "@/meeting/meetingApi";
import { MEETING_TRANSCRIPT_KEY, useMeetingSession } from "@/meeting/useMeetingSession";

import { clock } from "./RecordShared";

const NOTES_KEY = "caduceus.meeting-notes.v1";

export function MeetingPage({ active, onSetTitle }: ToolPageProps) {
  const [notes, setNotes] = useState(() => readPersisted(NOTES_KEY));
  // There is one notes buffer, kept across restarts. Whether what is on
  // screen belongs to *this* meeting or the last one is not something to
  // leave the reader to work out — "Copy all" and "Save to Notes" would
  // bundle it either way.
  const [carriedNotes, setCarriedNotes] = useState(() => notes.trim() !== "");
  const [note, setNote] = useState<string | null>(null);
  const [popoutError, setPopoutError] = useState<string | null>(null);

  const session = useMeetingSession(active);
  const transcriptRef = useRef<HTMLDivElement>(null);

  useEffect(() => onSetTitle("Meeting notes"), [onSetTitle]);

  // Survives a restart, and survives the tab being closed mid-sentence — see
  // `usePersisted`, which flushes rather than cancelling on unmount. The
  // transcript itself is persisted inside `useMeetingTranscript`'s consumers
  // via the same mechanism (see `useMeetingSession` → `useMeetingTranscript`);
  // notes are persisted here because this page is the only place they are
  // ever typed.
  const notesError = usePersisted(NOTES_KEY, notes, 300);
  const transcriptError = usePersisted(MEETING_TRANSCRIPT_KEY, session.transcript, 300);
  const saveError = notesError ?? transcriptError;

  useEffect(() => {
    const el = transcriptRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [session.transcript]);

  const openPopout = useCallback(async () => {
    setPopoutError(null);
    try {
      await meetingOpenPopout();
    } catch (e) {
      // Most likely cause right now: the wiring `meeting.rs`'s module doc
      // asks for (`generate_handler!` + the capabilities allow-list) has not
      // landed yet. Said plainly rather than as a raw IPC error string.
      setPopoutError(
        `Could not open the pop-out window (${api.errorMessage(e)}). It may not be wired up yet.`,
      );
    }
  }, []);

  const copyEverything = () => {
    const stamp = new Date().toLocaleString();
    const body = [
      `# Meeting — ${stamp}`,
      session.recording.status?.seconds ? `Length: ${clock(session.recording.status.seconds)}` : "",
      session.recording.saved ? `Recording: ${session.recording.saved}` : "",
      "",
      "## Notes",
      notes.trim() || "(none)",
      "",
      "## Transcript",
      session.transcript.trim() || "(none)",
    ]
      .filter(Boolean)
      .join("\n");

    navigator.clipboard
      .writeText(body)
      .then(() => setNote("Copied the notes and transcript."))
      .catch(() => setNote("Could not copy."));
  };

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b border-line px-5 py-3">
        <div className="row justify-between gap-2">
          <div>
            <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">
              Meeting notes
            </h1>
            <p className="mt-0.5 max-w-prose text-[13px] text-ink-mute">
              Records both sides of the call and transcribes on-device. Nothing is uploaded and no
              bot joins the meeting.
            </p>
          </div>
          <Button size="sm" tone="ghost" onClick={() => void openPopout()}>
            Pop out
          </Button>
        </div>
        {popoutError && <p className="mt-2 text-2xs text-danger">{popoutError}</p>}
      </div>

      <div className="shrink-0 px-5 py-3">
        <MeetingControls session={session} />
      </div>

      <div className="grid min-h-0 flex-1 gap-3 px-5 pb-4 md:grid-cols-2">
        {/* --- transcript ------------------------------------------------- */}
        <div className="flex min-h-0 flex-col rounded-cad border border-line bg-surface/40">
          <div className="row shrink-0 justify-between gap-2 border-b border-line px-3 py-1.5">
            <span className="text-2xs font-medium text-ink">
              Live transcript
              {session.listening && <span className="ml-2 text-[#ff5f57]">● listening</span>}
              {session.carriedTranscript && !session.listening && (
                <span className="ml-2 text-ink-faint">from your last meeting</span>
              )}
            </span>
            <Button size="sm" tone="ghost" onClick={() => session.clearTranscript()}>
              Clear
            </Button>
          </div>
          <div
            ref={transcriptRef}
            className="min-h-0 flex-1 overflow-y-auto px-3 py-2 text-2xs leading-relaxed text-ink-soft"
          >
            {session.transcript || (
              <span className="text-ink-faint">
                Press Start meeting. Apple's on-device recogniser transcribes your microphone live
                as you speak; the other side of the call is added once you stop, once Caduceus has
                had a chance to transcribe the recording — it is not live.
              </span>
            )}
          </div>
        </div>

        {/* --- notes ------------------------------------------------------ */}
        <div className="flex min-h-0 flex-col rounded-cad border border-line bg-surface/40">
          <div className="row shrink-0 justify-between gap-2 border-b border-line px-3 py-1.5">
            <span className="text-2xs font-medium text-ink">
              Your notes
              {carriedNotes && <span className="ml-2 text-ink-faint">from your last meeting</span>}
            </span>
            <div className="row gap-1">
              <Button
                size="sm"
                tone="ghost"
                onClick={() => {
                  setNotes("");
                  setCarriedNotes(false);
                }}
              >
                Clear
              </Button>
              <Button size="sm" tone="ghost" onClick={copyEverything}>
                Copy all
              </Button>
              <Button
                size="sm"
                tone="ghost"
                onClick={() => {
                  api
                    .addToNotes(
                      `${notes}\n\n---\n\n${session.transcript}`.trim(),
                      `Meeting — ${new Date().toLocaleDateString()}`,
                    )
                    .then((r) => setNote(r.message))
                    .catch((e) => setNote(api.errorMessage(e)));
                }}
              >
                Save to Notes
              </Button>
            </div>
          </div>
          <textarea
            value={notes}
            onChange={(e) => {
              setNotes(e.target.value);
              setCarriedNotes(false);
            }}
            placeholder="Decisions, actions, who owes what…"
            className="min-h-0 flex-1 resize-none bg-transparent px-3 py-2 text-[13px] leading-relaxed text-ink placeholder:text-ink-faint focus:outline-none"
          />
        </div>
      </div>

      {/* A failed save outranks anything else there is to say: everything under
          it is written on the assumption that the text is being kept. */}
      {(saveError ?? note) && (
        <p className={cx("shrink-0 px-5 pb-2 text-2xs", saveError ? "text-danger" : "text-ink-mute")}>
          {saveError ?? note}
        </p>
      )}

      <div className={cx("shrink-0 px-5 pb-4", session.recording.status?.active && "hidden")}>
        <Callout tone="info" title="Tell people they are being recorded">
          In many places recording a conversation without saying so is illegal, and everywhere
          else it is rude. Caduceus cannot do that part for you.
        </Callout>
      </div>
    </div>
  );
}
