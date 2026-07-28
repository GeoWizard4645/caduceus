/**
 * One meeting, one Start, one Stop.
 *
 * # The bug this replaces
 *
 * The old `MeetingPage.tsx` had two independent starts: `toggleTranscription`
 * called `voiceStart()`, and the recording was started separately by
 * `RecordControls`' own button, both bound to the same `microphone` checkbox
 * but with no other connection to each other. Starting one without the other
 * was not a misuse — it was just as available as starting both, and nothing
 * in the UI said that "the call" and "the transcript" were meant to be the
 * same action.
 *
 * This hook is the fix: [`start`] and [`stop`] are the only entry points, and
 * both halves — `recording_start`/`recording_stop` (system audio to a file,
 * see `capture/recorder.rs`) and `voice_start`/`voice_finish` (live mic
 * dictation, see `voice/live_macos.rs`) — go together every time.
 *
 * They are still two backend calls, not one atomic operation, because they
 * are two different subsystems Caduceus already has and this task was told
 * not to touch (`capture/`, `voice/`, `commands.rs`). [`start`] does not
 * treat them as all-or-nothing: if the recording half fails — a Screen
 * Recording permission that was never granted, say — the dictation half
 * still starts, because a live transcript with no saved recording is still
 * useful and the module's own reason for existing ("what people want
 * afterwards is the text") says the transcript is the half that matters
 * more. Both failures are surfaced, separately, rather than one hiding the
 * other.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useRecording } from "@/command-center/pages/tools/RecordShared";

import { emitMeetingCallAudio, meetingTranscribeSystemAudio } from "./meetingApi";
import { useMeetingTranscript } from "./useMeetingTranscript";

/** Exported so `MeetingPage.tsx` can persist the same derived string this
 *  hook computes, rather than re-deriving or duplicating the key by hand. */
export const MEETING_TRANSCRIPT_KEY = "caduceus.meeting-transcript.v1";

/** Where the system-audio half of the transcript is, once the call ends. */
export type CallAudioStatus = "idle" | "working" | "error";

export function useMeetingSession(active: boolean) {
  const recording = useRecording();
  const transcript = useMeetingTranscript(active, MEETING_TRANSCRIPT_KEY);

  const [microphone, setMicrophone] = useState(true);
  const [voiceError, setVoiceError] = useState<string | null>(null);
  const [callAudioStatus, setCallAudioStatus] = useState<CallAudioStatus>("idle");
  const [callAudioError, setCallAudioError] = useState<string | null>(null);

  // Set the instant Stop is pressed, cleared once the resulting `saved` path
  // has been picked up — see the effect below for why this, rather than
  // reading `recording.stop()`'s return value directly, is what drives the
  // post-call transcription.
  const [awaitingSavedPath, setAwaitingSavedPath] = useState(false);
  const lastTranscribedPath = useRef<string | null>(null);

  const meetingActive = (recording.status?.active ?? false) || transcript.listening;

  const start = useCallback(async () => {
    setVoiceError(null);
    setCallAudioStatus("idle");
    setCallAudioError(null);
    // `recording.start` never throws — see `RecordShared.tsx` — it reports
    // failure through `recording.error` (including the permission-wall UX),
    // so awaiting it here is only about sequencing, not error handling.
    await recording.start("audio", microphone);
    try {
      await api.voiceStart();
    } catch (e) {
      setVoiceError(api.errorMessage(e));
    }
  }, [recording, microphone]);

  const stop = useCallback(async () => {
    // Fire-and-forget, same as the recording HUD: the transcript arrives on
    // `voice-result`, so Stop is never the button waiting on a slow speech
    // helper to finalise (`voice_finish`'s own doc comment says this
    // explicitly).
    if (transcript.listening) {
      void api.voiceFinish().catch((e) => setVoiceError(api.errorMessage(e)));
    }
    if (recording.status?.active) {
      setAwaitingSavedPath(true);
      await recording.stop();
    }
  }, [recording, transcript.listening]);

  // Picks up the saved recording path once `recording.stop()` has actually
  // updated `recording.saved`, rather than trusting a return value the hook
  // does not have — `useRecording().stop` resolves `void`; the path only
  // ever lands in its own state, one render after the promise settles. A
  // ref-guarded effect reacting to that state change sidesteps the
  // stale-closure problem a callback capturing `recording.saved` directly
  // would have.
  useEffect(() => {
    if (!awaitingSavedPath) return;
    if (!recording.saved) return;
    if (lastTranscribedPath.current === recording.saved) return;
    lastTranscribedPath.current = recording.saved;
    setAwaitingSavedPath(false);

    const path = recording.saved;
    setCallAudioStatus("working");
    setCallAudioError(null);
    meetingTranscribeSystemAudio(path)
      .then((text) => {
        setCallAudioStatus("idle");
        // Broadcast rather than append locally — see `meetingApi.ts`'s
        // `MEETING_CALL_AUDIO_EVENT` doc. This window is a listener for its
        // own emission too, so nothing else has to happen here.
        return emitMeetingCallAudio(text);
      })
      .catch((e) => {
        setCallAudioStatus("error");
        setCallAudioError(api.errorMessage(e));
      });
  }, [awaitingSavedPath, recording.saved]);

  return {
    recording,
    microphone,
    setMicrophone,
    meetingActive,
    start,
    stop,
    voiceError,
    callAudioStatus,
    callAudioError,
    transcript: transcript.transcript,
    listening: transcript.listening,
    carriedTranscript: transcript.carried,
    clearTranscript: transcript.clear,
  };
}

export type MeetingSession = ReturnType<typeof useMeetingSession>;
