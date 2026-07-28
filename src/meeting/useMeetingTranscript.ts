/**
 * How a meeting's transcript accumulates across everything that can happen
 * to it — and why a plain `setTranscript(text)` on every partial, which is
 * what this replaces, only ever held the current utterance.
 *
 * # The pipeline's own accumulation, and exactly where it stops
 *
 * `voice/live_macos.rs` runs one `SFSpeechRecognizer` task per `LiveSession`
 * — one Start-to-Stop dictation session — and Apple's `partial` results for
 * that task are already cumulative: each one is the *whole session*
 * transcribed so far, not just the newest words (see `CaduceusSTTLive.swift`
 * — a single `SFSpeechAudioBufferRecognitionRequest` fed continuously until
 * `stop`, never replaced mid-session). That is why `setTranscript(text)`
 * looked correct in a quick manual test: for one continuous session, it is.
 *
 * It breaks the moment there is a *second* session, because the helper
 * process has no memory of the first one — a fresh `LiveSession` starts
 * Apple's recogniser from nothing, and its first partial is short and looks
 * nothing like a continuation of what came before. Before this fix, meeting
 * notes had exactly that shape: the recording and the dictation were
 * separate starts (see `MeetingPage.tsx`'s former `toggleTranscription`), so
 * anyone who paused and resumed dictation independently of the recording, or
 * whose live session ended and was restarted for any reason, watched their
 * transcript silently replaced rather than continued.
 *
 * # The fix: segments, not one growing string
 *
 * This hook keeps a list of *finalised* segments — the full text of every
 * dictation session that has ended — plus one *live* segment for whichever
 * session is currently running, if any:
 *
 * * `voice-partial` replaces the live segment. That is correct, not a
 *   regression to the old bug, because of the cumulative-partial behaviour
 *   above — within one session, "replace" and "accumulate" are the same
 *   operation.
 * * `voice-result` (the session ending, successfully or not) moves whatever
 *   the live segment currently holds into the finalised list and starts the
 *   next live segment empty. Two sessions become two paragraphs instead of
 *   one erasing the other.
 * * A defensive third rule: if a *new* session starts (`voice-state` →
 *   `"recording"`) while the live segment still has unfinalised text in it —
 *   `voice-result` never arrived for the previous session, which happens if
 *   the speech helper is killed rather than stopped cleanly — that leftover
 *   text is finalised right then, before the new session's first partial can
 *   overwrite it. Without this, a session that ends abnormally loses its
 *   tail the instant the next one begins.
 */

import { useCallback, useRef, useState } from "react";

import { useTauriEvent } from "@/shared/hooks";
import { readPersisted } from "@/shared/persist";
import { EVENTS, type VoiceOutcome, type VoiceState } from "@/shared/types";

import { MEETING_CALL_AUDIO_EVENT, type MeetingCallAudioPayload } from "./meetingApi";

/** Precedes the text pulled from the finished recording after the call ends
 *  — see `MeetingPage.tsx`'s module doc for why that half cannot be live. */
const CALL_AUDIO_LABEL = "— the call, transcribed after it ended —";

export interface MeetingTranscript {
  /** Every finalised segment plus the live one, joined for display and
   *  persistence — a plain string, so it round-trips through `usePersisted`
   *  exactly the way the single-string version did. */
  transcript: string;
  /** A dictation session is live right now. */
  listening: boolean;
  /** Whether `transcript` is entirely left over from a previous meeting and
   *  nothing has happened in this one yet — see `MeetingPage.tsx` for why
   *  the reader needs to be told rather than left to guess. */
  carried: boolean;
  /** Wipe the transcript for a fresh meeting. */
  clear: () => void;
}

/**
 * `active` gates every listener here exactly the way the rest of
 * `MeetingPage.tsx` already gated the old single-partial handler: without
 * it, dictation started from an unrelated tab or window would overwrite a
 * meeting transcript that merely was not focused. `persistKey` is the
 * localStorage key a *previous* meeting's transcript was saved under, so it
 * can be picked up as the first segment on mount — same behaviour the old
 * `carriedTranscript` had, generalised to a list.
 */
export function useMeetingTranscript(active: boolean, persistKey: string): MeetingTranscript {
  const [segments, setSegments] = useState<string[]>(() => {
    const prior = readPersisted(persistKey);
    return prior.trim() ? [prior] : [];
  });

  // Held in a ref as well as state: the finaliser needs the *current* live
  // text synchronously (from an event handler that fires between renders),
  // and reading it back out of `live` there would be one render stale.
  const liveRef = useRef("");
  const [live, setLiveState] = useState("");
  const setLive = useCallback((text: string) => {
    liveRef.current = text;
    setLiveState(text);
  }, []);

  const [listening, setListening] = useState(false);
  const [carried, setCarried] = useState(() => readPersisted(persistKey).trim() !== "");

  /** Move whatever the live segment holds into the finalised list. Safe to
   *  call with nothing to finalise — the empty-live case is the common one,
   *  since it fires defensively on every session start. */
  const finalizeLive = useCallback(
    (overrideText?: string) => {
      const text = (overrideText ?? liveRef.current).trim();
      if (text) setSegments((prev) => [...prev, text]);
      setLive("");
    },
    [setLive],
  );

  useTauriEvent<VoiceState>(EVENTS.voiceState, (state) => {
    if (!active) return;
    if (state === "recording") finalizeLive();
    setListening(state === "recording");
  });

  useTauriEvent<string>(EVENTS.voicePartial, (text) => {
    if (!active) return;
    setLive(text);
    setCarried(false);
  });

  useTauriEvent<VoiceOutcome>(EVENTS.voiceResult, (outcome) => {
    if (!active) return;
    // `routed.text` has already been through keyword routing (see
    // `voice/router.rs`), which this hook cannot avoid without touching
    // `hotkeys.rs` — out of scope for this change. In practice it only
    // matters if the very *first* words of an entire dictation session
    // happen to match a configured routing keyword (e.g. a meeting that
    // opens "search cheap flights..."), in which case that leading phrase is
    // stripped from this one segment; nothing else in the transcript is
    // affected, and the fallback below (the last partial) carries no such
    // risk at all.
    finalizeLive(outcome.ok && outcome.routed?.text ? outcome.routed.text : undefined);
  });

  // A segment produced by the *other* window — whichever one actually
  // called `recording_stop` and awaited the system-audio transcription. See
  // `meetingApi.ts`'s `MEETING_CALL_AUDIO_EVENT` doc for why this has to be
  // an event rather than each window independently noticing the recording
  // stopped.
  useTauriEvent<MeetingCallAudioPayload>(MEETING_CALL_AUDIO_EVENT, ({ text }) => {
    if (!active) return;
    const trimmed = text.trim();
    if (!trimmed) return;
    setSegments((prev) => [...prev, `${CALL_AUDIO_LABEL}\n${trimmed}`]);
    setCarried(false);
  });

  const clear = useCallback(() => {
    setSegments([]);
    setLive("");
    setCarried(false);
  }, [setLive]);

  const transcript = [...segments, live].filter((s) => s.trim() !== "").join("\n\n");

  return { transcript, listening, carried, clear };
}
