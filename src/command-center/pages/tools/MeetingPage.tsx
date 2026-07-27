/**
 * Meeting notes.
 *
 * Records both sides of a call — the people on it, via system audio, and you,
 * via the microphone — while showing a live transcript you can take notes
 * beside. Everything stays on the Mac.
 *
 * # Why this is not just "record the screen"
 *
 * A recording of a forty-minute call is a forty-minute file nobody opens. What
 * people want afterwards is the text, and a couple of lines they typed while it
 * was happening. So this records audio only (a fraction of the size), runs the
 * transcript live so you can see it working, and puts the notes field next to
 * it rather than in another app.
 *
 * # Where the transcription happens
 *
 * On the Mac, through Apple's Speech framework — the same on-device recogniser
 * dictation uses. Nothing is uploaded, no account is involved, and it works
 * with the network off. The trade is that it transcribes the microphone, so on
 * a call it captures your side live; the other side is in the recording and can
 * be transcribed from the file afterwards.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import { EVENTS, type VoiceState } from "@/shared/types";
import { Button, Callout, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";
import { RecordControls, clock, useRecording } from "./RecordShared";

const NOTES_KEY = "caduceus.meeting-notes.v1";

export function MeetingPage({ onSetTitle }: ToolPageProps) {
  const [microphone, setMicrophone] = useState(true);
  const [transcript, setTranscript] = useState("");
  const [notes, setNotes] = useState(() => localStorage.getItem(NOTES_KEY) ?? "");
  const [listening, setListening] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const recording = useRecording();

  const transcriptRef = useRef<HTMLDivElement>(null);

  useEffect(() => onSetTitle("Meeting notes"), [onSetTitle]);

  // Notes survive a restart. Losing what you typed during a call because the
  // app was updated is not acceptable for something people lean on.
  useEffect(() => {
    const timer = setTimeout(() => localStorage.setItem(NOTES_KEY, notes), 300);
    return () => clearTimeout(timer);
  }, [notes]);

  useTauriEvent<string>(EVENTS.voicePartial, (text) => setTranscript(text));
  useTauriEvent<VoiceState>(EVENTS.voiceState, (state) => setListening(state === "recording"));

  useEffect(() => {
    const el = transcriptRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [transcript]);

  const toggleTranscription = useCallback(async () => {
    try {
      if (listening) await api.voiceFinish();
      else await api.voiceStart();
    } catch (e) {
      setNote(api.errorMessage(e));
    }
  }, [listening]);

  const copyEverything = () => {
    const stamp = new Date().toLocaleString();
    const body = [
      `# Meeting — ${stamp}`,
      recording.status?.seconds ? `Length: ${clock(recording.status.seconds)}` : "",
      recording.saved ? `Recording: ${recording.saved}` : "",
      "",
      "## Notes",
      notes.trim() || "(none)",
      "",
      "## Transcript",
      transcript.trim() || "(none)",
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
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Meeting notes</h1>
        <p className="mt-0.5 max-w-prose text-[13px] text-ink-mute">
          Records both sides of the call and transcribes on-device. Nothing is uploaded and no
          bot joins the meeting.
        </p>
      </div>

      <div className="shrink-0 px-5 py-3">
        <RecordControls
          mode="audio"
          microphone={microphone}
          onMicrophoneChange={setMicrophone}
          recording={recording}
          startLabel="Start recording the call"
        />
      </div>

      <div className="grid min-h-0 flex-1 gap-3 px-5 pb-4 md:grid-cols-2">
        {/* --- transcript ------------------------------------------------- */}
        <div className="flex min-h-0 flex-col rounded-cad border border-line bg-surface/40">
          <div className="row shrink-0 justify-between gap-2 border-b border-line px-3 py-1.5">
            <span className="text-2xs font-medium text-ink">
              Live transcript
              {listening && <span className="ml-2 text-[#ff5f57]">● listening</span>}
            </span>
            <Button size="sm" tone="ghost" onClick={() => void toggleTranscription()}>
              {listening ? "Stop" : "Start"}
            </Button>
          </div>
          <div
            ref={transcriptRef}
            className="min-h-0 flex-1 overflow-y-auto px-3 py-2 text-2xs leading-relaxed text-ink-soft"
          >
            {transcript || (
              <span className="text-ink-faint">
                Press Start. Apple's on-device recogniser transcribes your microphone as you
                speak; the other side is captured in the recording.
              </span>
            )}
          </div>
        </div>

        {/* --- notes ------------------------------------------------------ */}
        <div className="flex min-h-0 flex-col rounded-cad border border-line bg-surface/40">
          <div className="row shrink-0 justify-between gap-2 border-b border-line px-3 py-1.5">
            <span className="text-2xs font-medium text-ink">Your notes</span>
            <div className="row gap-1">
              <Button size="sm" tone="ghost" onClick={copyEverything}>
                Copy all
              </Button>
              <Button
                size="sm"
                tone="ghost"
                onClick={() => {
                  api
                    .addToNotes(
                      `${notes}\n\n---\n\n${transcript}`.trim(),
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
            onChange={(e) => setNotes(e.target.value)}
            placeholder="Decisions, actions, who owes what…"
            className="min-h-0 flex-1 resize-none bg-transparent px-3 py-2 text-[13px] leading-relaxed text-ink placeholder:text-ink-faint focus:outline-none"
          />
        </div>
      </div>

      {note && <p className="shrink-0 px-5 pb-2 text-2xs text-ink-mute">{note}</p>}

      <div className={cx("shrink-0 px-5 pb-4", recording.status?.active && "hidden")}>
        <Callout tone="info" title="Tell people they are being recorded">
          In many places recording a conversation without saying so is illegal, and everywhere
          else it is rude. Caduceus cannot do that part for you.
        </Callout>
      </div>
    </div>
  );
}
