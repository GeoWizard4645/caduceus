/**
 * Screen recording that captures what your Mac is playing.
 *
 * ⇧⌘5 does not. It records the screen and your microphone, so a recording of
 * anything with sound in it — a call, a video, a demo of your own app — comes
 * out silent apart from you. That is the gap this fills, and it is the only
 * reason it exists rather than deferring to macOS.
 */

import { useEffect, useState } from "react";

import { Callout } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";
import { RecordControls, useRecording } from "./RecordShared";

export function ScreenRecordPage({ onSetTitle }: ToolPageProps) {
  const [microphone, setMicrophone] = useState(false);
  const recording = useRecording();

  useEffect(() => onSetTitle("Record the screen"), [onSetTitle]);

  return (
    <div className="mx-auto h-full max-w-[720px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">
          Record the screen
        </h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Video plus <strong className="text-ink">the audio your Mac is playing</strong> — which
          is the thing macOS's own recorder cannot do. Saved to your Movies folder as an MP4.
        </p>
      </div>

      <RecordControls
        mode="screen"
        microphone={microphone}
        onMicrophoneChange={setMicrophone}
        recording={recording}
        startLabel="Start recording"
      />

      <div className="mt-4 space-y-3">
        <Callout tone="info" title="What gets captured">
          The whole display, the cursor, and the system audio mix with Caduceus's own sounds
          excluded — so the click that starts the recording is not in it. With the microphone
          on, your voice is written as a second audio track rather than mixed in, because
          mixing is something you can always do afterwards and unmixing is not.
        </Callout>

        <Callout tone="info" title="System audio needs macOS 13">
          Before Ventura there was no way to capture system audio without installing an audio
          driver, and Caduceus will not put a driver on your Mac to work around a missing API.
          On macOS 11 and 12 this reports that rather than recording silence.
        </Callout>
      </div>
    </div>
  );
}
