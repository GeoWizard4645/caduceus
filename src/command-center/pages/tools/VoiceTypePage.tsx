/**
 * Voice typing.
 *
 * A microphone that turns speech into text, and nothing else. This exists
 * because dictation-into-the-palette always *routes* what you say — keywords
 * are stripped, results may auto-submit — which is right for "search cheap
 * flights" and wrong when what you wanted was simply the words.
 *
 * So this page is the plain version: press the button, talk, watch the text
 * arrive, and then either copy it or have Caduceus type it into whatever app
 * is behind the palette. The palette is a non-activating panel, so the app
 * behind never lost keyboard focus — "Type into the app behind" lands at the
 * caret exactly where you left it.
 *
 * It listens to the same events the palette does (`voice-partial`,
 * `voice-result`); the difference is that it keeps `routed.raw` — the words as
 * recognised — rather than the keyword-stripped command the router built.
 */

import { useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import { readPersisted, usePersisted } from "@/shared/persist";
import { EVENTS, type VoiceOutcome, type VoiceState } from "@/shared/types";
import { Button, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

const TEXT_KEY = "caduceus.voice-type.v1";

export function VoiceTypePage({ active, onSetTitle }: ToolPageProps) {
  const [text, setText] = useState(() => readPersisted(TEXT_KEY));
  const [partial, setPartial] = useState("");
  const [voice, setVoice] = useState<VoiceState>("idle");
  const [note, setNote] = useState<string | null>(null);

  const textRef = useRef<HTMLTextAreaElement>(null);
  const saveError = usePersisted(TEXT_KEY, text, 300);

  useEffect(() => onSetTitle("Voice typing"), [onSetTitle]);

  // The session may already be running (started from the hotkey or the mic
  // button) when this page opens — pick up its state rather than showing a
  // Start button over a live microphone.
  useEffect(() => {
    if (!active) return;
    api
      .voiceIsRecording()
      .then((live) => setVoice((v) => (live && v === "idle" ? "recording" : v)))
      .catch(() => {});
  }, [active]);

  useTauriEvent<VoiceState>(EVENTS.voiceState, (next) => {
    if (!active) return;
    setVoice(next);
    if (next === "idle") setPartial("");
  });

  useTauriEvent<string>(EVENTS.voicePartial, (live) => {
    if (!active) return;
    setPartial(live);
  });

  useTauriEvent<VoiceOutcome>(EVENTS.voiceResult, (outcome) => {
    if (!active) return;
    setPartial("");
    if (!outcome.ok) {
      setNote(outcome.error ?? "Transcription failed.");
      return;
    }
    // `raw`, never `text`: the router strips keywords ("search …") for the
    // palette's benefit, and this page wants the words as spoken.
    const spoken = (outcome.routed?.raw ?? outcome.routed?.text ?? "").trim();
    if (!spoken) return;
    setNote(null);
    setText((prev) => (prev.trim() ? `${prev.replace(/\s+$/, "")}\n${spoken}` : spoken));
  });

  // Keep the newest words in view as they arrive.
  useEffect(() => {
    const el = textRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text, partial]);

  const listening = voice === "recording" || voice === "paused";

  const toggle = () => {
    setNote(null);
    if (voice === "idle") {
      void api.voiceStart().catch((e) => setNote(api.errorMessage(e)));
    } else if (listening) {
      void api.voiceFinish().catch((e) => setNote(api.errorMessage(e)));
    }
  };

  const copyAll = () => {
    if (!text.trim()) {
      setNote("Nothing to copy yet — dictate something first.");
      return;
    }
    navigator.clipboard
      .writeText(text)
      .then(() => setNote("Copied."))
      .catch(() => setNote("Could not copy."));
  };

  const typeIntoApp = () => {
    if (!text.trim()) {
      setNote("Nothing to type yet — dictate something first.");
      return;
    }
    setNote("Typing into the app behind…");
    api
      .typeText(text)
      .then(() => setNote("Typed into the app behind the palette."))
      .catch((e) =>
        setNote(
          `${api.errorMessage(e)} Typing into other apps needs the Accessibility permission.`,
        ),
      );
  };

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b border-line px-5 py-3">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Voice typing</h1>
        <p className="mt-0.5 max-w-prose text-[13px] text-ink-mute">
          Speak, and the words appear below — transcribed on-device, nothing uploaded. Copy them,
          or type them straight into the app behind this window.
        </p>
      </div>

      {/* --- controls --------------------------------------------------------- */}
      <div className="row shrink-0 flex-wrap gap-2 px-5 py-3">
        <Button tone={listening ? "danger" : "primary"} onClick={toggle} disabled={voice === "transcribing"}>
          {voice === "idle" && "● Start dictating"}
          {voice === "recording" && "■ Stop"}
          {voice === "paused" && "■ Stop"}
          {voice === "transcribing" && "Transcribing…"}
        </Button>
        {listening && (
          <Button
            tone="ghost"
            onClick={() =>
              void api
                .voicePause(voice !== "paused")
                .catch((e) => setNote(api.errorMessage(e)))
            }
          >
            {voice === "paused" ? "Resume" : "Pause"}
          </Button>
        )}
        <div className="flex-1" />
        <Button tone="ghost" onClick={() => { setText(""); setPartial(""); setNote(null); }}>
          Clear
        </Button>
        <Button tone="ghost" onClick={copyAll}>
          Copy
        </Button>
        <Button tone="ghost" onClick={typeIntoApp}>
          Type into the app behind
        </Button>
      </div>

      {/* --- transcript ------------------------------------------------------- */}
      <div className="mx-5 mb-3 flex min-h-0 flex-1 flex-col rounded-cad border border-line bg-surface/40">
        <div className="row shrink-0 justify-between border-b border-line px-3 py-1.5">
          <span className="text-2xs font-medium text-ink">
            Transcript
            {voice === "recording" && <span className="ml-2 text-[#ff5f57]">● listening</span>}
            {voice === "paused" && <span className="ml-2 text-ink-faint">paused</span>}
            {voice === "transcribing" && <span className="ml-2 text-ink-faint">finishing…</span>}
          </span>
          <span className="text-2xs text-ink-faint">editable</span>
        </div>
        <textarea
          ref={textRef}
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={
            listening
              ? "Listening…"
              : "Press Start dictating and talk. Each take lands here on its own line."
          }
          className="min-h-0 flex-1 resize-none bg-transparent px-3 py-2 text-[13px] leading-relaxed text-ink placeholder:text-ink-faint focus:outline-none"
        />
        {partial && (
          <div className="shrink-0 border-t border-line px-3 py-1.5 text-2xs italic text-ink-mute">
            {partial}
          </div>
        )}
      </div>

      {(saveError ?? note) && (
        <p className={cx("shrink-0 px-5 pb-3 text-2xs", saveError ? "text-danger" : "text-ink-mute")}>
          {saveError ?? note}
        </p>
      )}
    </div>
  );
}
