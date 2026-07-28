/**
 * The two Rust commands this feature needs that are not yet reachable.
 *
 * `src-tauri/src/meeting.rs` defines `meeting_open_popout` and
 * `meeting_transcribe_system_audio` as ordinary `#[tauri::command]`s, but per
 * this task's file boundaries `lib.rs` only gained a `pub mod meeting;` line
 * — neither command is in `generate_handler!` yet, and the pop-out window's
 * label is not in `capabilities/default.json`'s `windows` allow-list. Both
 * calls below will fail (a clean, catchable rejection, not a crash) until
 * someone adds that wiring — see `meeting.rs`'s module doc for the exact two
 * steps.
 *
 * Not in `@/shared/api.ts`: that file is explicitly off-limits for this
 * change, so these live next to the feature that needs them instead of
 * pretending to be part of the shared surface.
 */

import { emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

/** Open the floating pop-out, or bring it back if it is already open. */
export const meetingOpenPopout = () => invoke<void>("meeting_open_popout");

/**
 * Transcribe the system-audio track of a finished meeting recording — the
 * "other side" of the call, available only once it has ended. See
 * `meeting.rs::meeting_transcribe_system_audio` for exactly what this does
 * and does not cover.
 */
export const meetingTranscribeSystemAudio = (path: string) =>
  invoke<string>("meeting_transcribe_system_audio", { path });

/**
 * A purely frontend event: no Rust code emits or expects this one. It exists
 * because the meeting can be stopped from either the Command Center tab or
 * the pop-out window, and whichever one the user stopped it from is the only
 * one that awaited `recording_stop` and therefore the only one holding the
 * saved path to transcribe. Broadcasting the result here, rather than
 * letting each window discover it independently, is what keeps both windows'
 * transcripts in agreement regardless of which one did the work — see
 * `useMeetingTranscript.ts` for the listener.
 */
export const MEETING_CALL_AUDIO_EVENT = "caduceus://meeting-call-audio";

export interface MeetingCallAudioPayload {
  text: string;
}

export const emitMeetingCallAudio = (text: string) =>
  emit(MEETING_CALL_AUDIO_EVENT, { text } satisfies MeetingCallAudioPayload);
