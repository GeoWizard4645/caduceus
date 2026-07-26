import { useState } from "react";

import * as api from "@/shared/api";
import type { KeywordGroup, RouteTarget, RuntimeInfo, SttBackendKind } from "@/shared/types";
import {
  Button,
  Callout,
  Field,
  HotkeyInput,
  IconButton,
  NumberInput,
  Section,
  Select,
  TextInput,
  Toggle,
} from "@/shared/ui";

import type { Draft } from "../useDraft";

const ROUTE_LABELS: Record<RouteTarget, string> = {
  web_search: "Search the web",
  primary_ai: "Ask the primary AI backend",
  computer_use: "Start a computer-use agent",
  clipboard_search: "Search clipboard history",
  insert_only: "Just put the text in the input",
};

export function VoiceTab({ draft, info }: { draft: Draft; info: RuntimeInfo | null }) {
  const [sttKey, setSttKey] = useState("");
  const [keyStatus, setKeyStatus] = useState<string | null>(null);
  const settings = draft.settings;
  if (!settings) return null;
  const voice = settings.voice;

  const backendInfo = info?.sttBackends.find((b) => b.id === voice.sttBackend);

  const mutateGroup = (id: string, change: (group: KeywordGroup) => void) =>
    draft.update((d) => {
      const group = d.voice.keywordGroups.find((g) => g.id === id);
      if (group) change(group);
    });

  return (
    <>
      <Section
        title="Dictation"
        description="Live transcription through macOS: AVAudioEngine captures your voice locally; Apple's Speech framework turns it into text in the Command Center. ScreenCaptureKit is used for system audio when you record the screen — not during ordinary dictation."
      >
        <Toggle
          label="Enable voice input"
          hint="On by default. No dictation button — press F1 or double-click the staff to start and stop. macOS Microphone permission is requested the first time you dictate."
          checked={voice.enabled}
          onChange={(checked) => draft.update((d) => (d.voice.enabled = checked))}
        />

        <div className="mt-4 grid grid-cols-2 gap-5">
          <Field
            label="Push-to-talk key (hold)"
            hint="Optional: hold this key instead of tap-to-toggle. Same AVAudioEngine + Speech stack as F1."
          >
            <HotkeyInput
              value={voice.pushToTalkHotkey}
              onChange={(value) => draft.update((d) => (d.voice.pushToTalkHotkey = value))}
            />
          </Field>

          <Field label="Maximum recording length" hint="A safety net in case the key gets stuck.">
            <NumberInput
              value={voice.maxRecordingSecs}
              min={3}
              max={600}
              suffix="sec"
              onChange={(value) => draft.update((d) => (d.voice.maxRecordingSecs = value))}
            />
          </Field>
        </div>

        <div className="mt-4 border-t border-line pt-4">
          <Toggle
            label="Run the command automatically"
            hint="Off means the transcript lands in the Command Center and waits for you to press Enter. Recommended while you are still learning how your keywords behave."
            checked={voice.autoSubmit}
            onChange={(checked) => draft.update((d) => (d.voice.autoSubmit = checked))}
          />
        </div>

        <div className="mt-4">
          <Callout tone="info" title="Starting dictation">
            <p className="text-[13px] leading-relaxed text-ink-mute">
              Press <strong className="font-medium text-ink-soft">F1</strong> or{" "}
              <strong className="font-medium text-ink-soft">double-click the staff</strong> to toggle
              recording (tap again to stop). Or hold your push-to-talk key. Grant Microphone and Speech
              Recognition in System Settings if macOS prompts you.
            </p>
          </Callout>
        </div>

        <div className="mt-4">
          <Callout tone="info" title="No wake word, deliberately">
            Caduceus never listens in the background. Always-on wake-word detection would mean a
            process with permanent microphone access that also has screen capture and input
            simulation — too much to ask of a utility you installed from GitHub. This is push-to-talk
            only.
          </Callout>
        </div>
      </Section>

      <Section title="Speech recognition" description="Where your recording gets turned into text.">
        <Field label="Backend">
          <Select
            value={voice.sttBackend}
            onChange={(v) => draft.update((d) => (d.voice.sttBackend = v as SttBackendKind))}
            options={
              info?.sttBackends.map((b) => ({
                value: b.id as SttBackendKind,
                label: b.displayName + (b.available ? "" : " — unavailable"),
              })) ?? [{ value: "system_native" as SttBackendKind, label: "System" }]
            }
          />
        </Field>

        {backendInfo && (
          <div className="mt-3">
            <Callout tone={backendInfo.available ? "info" : "warn"}>{backendInfo.detail}</Callout>
          </div>
        )}

        {voice.sttBackend === "openai_compatible" && (
          <div className="mt-4 grid grid-cols-2 gap-5">
            <Field
              label="Endpoint"
              hint="Any OpenAI-compatible /audio/transcriptions URL. A local whisper.cpp or faster-whisper server keeps audio on your machine."
              wide
            >
              <TextInput
                mono
                value={voice.sttEndpoint}
                onChange={(v) => draft.update((d) => (d.voice.sttEndpoint = v))}
                placeholder="http://127.0.0.1:8080/v1/audio/transcriptions"
              />
            </Field>

            <Field label="Model">
              <TextInput
                value={voice.sttModel}
                onChange={(v) => draft.update((d) => (d.voice.sttModel = v))}
                placeholder="whisper-1"
              />
            </Field>

            <Field label="API key" hint="Leave blank for a local server. Stored in your OS keychain.">
              <div className="row">
                <TextInput
                  type="password"
                  value={sttKey}
                  onChange={setSttKey}
                  placeholder="Leave blank for a local server"
                />
                <Button
                  size="sm"
                  onClick={async () => {
                    try {
                      const saved = await api.setSttApiKey(sttKey);
                      setSttKey("");
                      setKeyStatus(saved ? "Saved to keychain" : "Removed");
                    } catch (error) {
                      setKeyStatus(api.errorMessage(error));
                    }
                  }}
                >
                  Save
                </Button>
              </div>
            </Field>

            {keyStatus && (
              <p className="col-span-2 text-2xs text-ink-faint">{keyStatus}</p>
            )}
          </div>
        )}

        {voice.sttBackend !== "disabled" && (
          <div className="mt-4">
            <Field
              label="Language"
              hint="A BCP-47 tag such as en-US or de-DE. Leave blank to auto-detect."
            >
              <TextInput
                value={voice.sttLanguage}
                onChange={(v) => draft.update((d) => (d.voice.sttLanguage = v))}
                placeholder="Auto-detect"
              />
            </Field>
          </div>
        )}
      </Section>

      <Section
        title="Keyword routing"
        description="After transcription, Caduceus checks whether what you said starts with one of these. The longest match across every group wins, so “search my mac” beats “search”."
        actions={
          <Button
            tone="primary"
            onClick={() =>
              draft.update((d) => {
                d.voice.keywordGroups.push({
                  id: `kw-${crypto.randomUUID().slice(0, 8)}`,
                  name: "New group",
                  keywords: [],
                  route: "primary_ai",
                  matchMode: "leading_words",
                  enabled: true,
                });
              })
            }
          >
            Add group
          </Button>
        }
      >
        <div className="space-y-3">
          {voice.keywordGroups.map((group) => (
            <div key={group.id} className="rounded-lg border border-line bg-base/20 p-3">
              <div className="grid grid-cols-[1fr_1fr_auto] items-end gap-3">
                <Field label="Group name">
                  <TextInput
                    value={group.name}
                    onChange={(v) => mutateGroup(group.id, (g) => (g.name = v))}
                  />
                </Field>

                <Field label="Routes to">
                  <Select
                    value={group.route}
                    onChange={(v) => mutateGroup(group.id, (g) => (g.route = v as RouteTarget))}
                    options={(Object.keys(ROUTE_LABELS) as RouteTarget[]).map((route) => ({
                      value: route,
                      label: ROUTE_LABELS[route],
                    }))}
                  />
                </Field>

                <div className="pb-1">
                  <IconButton
                    label="Delete group"
                    tone="danger"
                    onClick={() =>
                      draft.update((d) => {
                        d.voice.keywordGroups = d.voice.keywordGroups.filter(
                          (g) => g.id !== group.id,
                        );
                      })
                    }
                  >
                    ×
                  </IconButton>
                </div>
              </div>

              <div className="mt-3 grid grid-cols-[1fr_220px] gap-3">
                <Field
                  label="Keywords"
                  hint="Comma-separated, matched case-insensitively."
                >
                  <TextInput
                    value={group.keywords.join(", ")}
                    placeholder="search, look up, browse"
                    onChange={(v) =>
                      mutateGroup(
                        group.id,
                        (g) =>
                          (g.keywords = v
                            .split(",")
                            .map((k) => k.trim())
                            .filter(Boolean)),
                      )
                    }
                  />
                </Field>

                <Field
                  label="Match"
                  hint={
                    group.matchMode === "leading_words"
                      ? "Must be at the start; the keyword is removed."
                      : "Can be anywhere; the text is kept as-is."
                  }
                >
                  <Select
                    value={group.matchMode}
                    onChange={(v) => mutateGroup(group.id, (g) => (g.matchMode = v))}
                    options={[
                      { value: "leading_words", label: "At the start" },
                      { value: "anywhere", label: "Anywhere" },
                    ]}
                  />
                </Field>
              </div>

              <div className="mt-2">
                <Toggle
                  label="Enabled"
                  checked={group.enabled}
                  onChange={(checked) => mutateGroup(group.id, (g) => (g.enabled = checked))}
                />
              </div>
            </div>
          ))}
        </div>

        <div className="mt-5 border-t border-line pt-4">
          <Field
            label="When nothing matches"
            hint="Where a transcript goes if none of the groups above claim it."
          >
            <Select
              value={voice.fallbackRoute}
              onChange={(v) => draft.update((d) => (d.voice.fallbackRoute = v as RouteTarget))}
              options={(Object.keys(ROUTE_LABELS) as RouteTarget[]).map((route) => ({
                value: route,
                label: ROUTE_LABELS[route],
              }))}
            />
          </Field>
        </div>
      </Section>
    </>
  );
}
