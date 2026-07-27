/**
 * Manage → Sound: every audio device, and one click to make it the default.
 *
 * The palette's `output` / `input` keywords answer the quick case; this page is
 * for the desk with a dock, two interfaces and AirPods, where you want to *see*
 * the routing rather than remember it.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { AudioDevice } from "@/shared/types";
import { Button, Section, cx } from "@/shared/ui";

export function SoundPage() {
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setDevices(await api.audioDevices());
      setError(null);
    } catch (e) {
      setError(api.errorMessage(e));
    }
  }, []);

  // Devices come and go (AirPods, docks); poll gently rather than only on open.
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 4000);
    return () => clearInterval(timer);
  }, [refresh]);

  const pick = async (device: AudioDevice, input: boolean) => {
    try {
      const outcome = await api.setAudioDevice(device.uid, input);
      setMessage(outcome.message);
      await refresh();
    } catch (e) {
      setMessage(api.errorMessage(e));
    }
  };

  const outputs = devices.filter((device) => device.isOutput);
  const inputs = devices.filter((device) => device.isInput);

  return (
    <div className="mx-auto max-w-[640px] px-6 py-5">
      {error && <p className="mb-4 text-2xs text-danger">{error}</p>}
      {message && <p className="mb-4 text-2xs text-ink-mute">{message}</p>}

      <DeviceList
        title="Output"
        description="Where sound plays. Switching is immediate and system-wide."
        devices={outputs}
        isCurrent={(device) => device.isDefaultOutput}
        onPick={(device) => void pick(device, false)}
      />
      <DeviceList
        title="Microphone"
        description="Where recording and dictation listen."
        devices={inputs}
        isCurrent={(device) => device.isDefaultInput}
        onPick={(device) => void pick(device, true)}
      />
    </div>
  );
}

function DeviceList({
  title,
  description,
  devices,
  isCurrent,
  onPick,
}: {
  title: string;
  description: string;
  devices: AudioDevice[];
  isCurrent: (device: AudioDevice) => boolean;
  onPick: (device: AudioDevice) => void;
}) {
  return (
    <Section title={title} description={description}>
      {devices.length === 0 ? (
        <p className="text-2xs text-ink-faint">No devices found.</p>
      ) : (
        <ul className="space-y-1.5">
          {devices.map((device) => {
            const current = isCurrent(device);
            return (
              <li
                key={device.uid}
                className={cx(
                  "flex items-center gap-3 rounded-lg border px-3 py-2",
                  current ? "border-accent/40 bg-accent/8" : "border-line bg-base/20",
                )}
              >
                <span className="min-w-0 flex-1 truncate text-[13px] text-ink">
                  {device.name}
                </span>
                {current ? (
                  <span className="text-2xs font-medium text-accent">Current</span>
                ) : (
                  <Button size="sm" onClick={() => onPick(device)}>
                    Use
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </Section>
  );
}
