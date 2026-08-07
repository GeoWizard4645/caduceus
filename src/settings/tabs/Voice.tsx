import { useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { KeywordGroup, RouteTarget, RuntimeInfo, SttBackendKind, TtsBackendKind } from "@/shared/types";
import {
  Button,
  Callout,
  Field,
  HotkeyInput,
  IconButton,
  NumberInput,
  Section,
  Select,
  Spinner,
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
  const [ttsKey, setTtsKey] = useState("");
  const [ttsKeyStatus, setTtsKeyStatus] = useState<string | null>(null);
  // Populated from `say -v ?` for the system-voice picker. Empty off macOS —
  // `listSpeechVoices` never errors for that, it just has nothing to offer.
  const [sayVoices, setSayVoices] = useState<string[]>([]);
  const [testingVoice, setTestingVoice] = useState(false);
  const [testError, setTestError] = useState<string | null>(null);
  const settings = draft.settings;

  useEffect(() => {
    let cancelled = false;
    void api.listSpeechVoices().then((voices) => {
      if (!cancelled) setSayVoices(voices);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!settings) return null;
  const voice = settings.voice;
  const agents = settings.agents;

  const backendInfo = info?.sttBackends.find((b) => b.id === voice.sttBackend);
  const ttsBackendInfo = info?.ttsBackends.find((b) => b.id === voice.ttsBackend);

  // `say -r` wants whole words-per-minute; the HTTP backend's `speed` is a
  // 0.25–4.0 multiplier. Same field, different unit depending on which
  // backend will actually read it — see `VoiceSettings.ttsRate`'s doc.
  const rateConfig =
    voice.ttsBackend === "openai_compatible"
      ? { min: 0, max: 4, step: 0.05, suffix: "×", hint: "0.25–4.0. 0 leaves it at the server's own default." }
      : { min: 0, max: 400, step: 5, suffix: "wpm", hint: "Words per minute. 0 leaves it at `say`'s own default." };

  const testVoice = async () => {
    setTestingVoice(true);
    setTestError(null);
    try {
      await api.speak("This is how Caduceus will sound.");
    } catch (error) {
      setTestError(api.errorMessage(error));
    } finally {
      setTestingVoice(false);
    }
  };

  const mutateGroup = (id: string, change: (group: KeywordGroup) => void) =>
    draft.update((d) => {
      const group = d.voice.keywordGroups.find((g) => g.id === id);
      if (group) change(group);
    });

  return (
    <>
      <Section
        title="Dictation"
        description="Live local transcription on macOS: AVAudioEngine captures your voice and Parakeet turns it into text continuously on Apple Silicon. Apple Speech remains the fallback on older Macs."
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
            hint="Optional: hold this key instead of tap-to-toggle. Uses the same live local transcription as F1."
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
            hint="Where a transcript goes if none of the groups above claim it. Automatic searches the web when no AI is set up, and otherwise just fills the Command Center, exactly as typing would."
          >
            {/*
              The empty string stands in for `null` because a `<select>` has no
              way to carry one — every option value is a string. It is mapped
              back on the way out, so "Automatic" round-trips as the absence of
              a choice rather than as a route that happens to be spelled "".
            */}
            <Select
              value={voice.fallbackRoute ?? ""}
              onChange={(v) =>
                draft.update((d) => (d.voice.fallbackRoute = v === "" ? null : (v as RouteTarget)))
              }
              options={[
                { value: "", label: "Automatic" },
                ...(Object.keys(ROUTE_LABELS) as RouteTarget[]).map((route) => ({
                  value: route,
                  label: ROUTE_LABELS[route],
                })),
              ]}
            />
          </Field>

          <div className="mt-4">
            <Toggle
              label="Open an obvious match on its own"
              hint="Say a single name — “Terminal” — and pause, and Caduceus opens it. Only fires on an exact, unambiguous match, and always counts down first so you can stop it."
              checked={voice.autoOpenShortUtterance}
              onChange={(checked) =>
                draft.update((d) => (d.voice.autoOpenShortUtterance = checked))
              }
            />
          </div>
        </div>
      </Section>

      <Section
        title="Spoken replies"
        description="Have Caduceus read its answers aloud. Off by default — nothing speaks until this is turned on, the same rule push-to-talk input already follows for the microphone."
      >
        <Toggle
          label="Enable text-to-speech"
          hint="Master switch. On its own this only allows Test voice below to work — it does not narrate replies by itself; that is the separate toggle at the bottom of this section."
          checked={voice.ttsEnabled}
          onChange={(checked) => draft.update((d) => (d.voice.ttsEnabled = checked))}
        />

        <div className="mt-4">
          <Field label="Backend">
            <Select
              value={voice.ttsBackend}
              onChange={(v) => draft.update((d) => (d.voice.ttsBackend = v as TtsBackendKind))}
              options={
                info?.ttsBackends.map((b) => ({
                  value: b.id as TtsBackendKind,
                  label: b.displayName + (b.available ? "" : " — unavailable"),
                })) ?? [{ value: "system_native" as TtsBackendKind, label: "System (say)" }]
              }
            />
          </Field>

          {ttsBackendInfo && (
            <div className="mt-3">
              <Callout tone={ttsBackendInfo.available ? "info" : "warn"}>{ttsBackendInfo.detail}</Callout>
            </div>
          )}
        </div>

        {voice.ttsBackend === "openai_compatible" && (
          <div className="mt-4 grid grid-cols-2 gap-5">
            <Field
              label="Endpoint"
              hint="Any OpenAI-compatible /audio/speech URL. Always called with response_format=wav, since that is the one format Caduceus can play back without a compressed-audio decoder."
              wide
            >
              <TextInput
                mono
                value={voice.ttsEndpoint}
                onChange={(v) => draft.update((d) => (d.voice.ttsEndpoint = v))}
                placeholder="https://api.openai.com/v1/audio/speech"
              />
            </Field>

            <Field label="Model">
              <TextInput
                value={voice.ttsModel}
                onChange={(v) => draft.update((d) => (d.voice.ttsModel = v))}
                placeholder="tts-1"
              />
            </Field>

            <Field
              label="Voice"
              hint="Backend-specific voice name, e.g. alloy. Leave blank for the server's own default."
            >
              <TextInput
                value={voice.ttsVoice}
                onChange={(v) => draft.update((d) => (d.voice.ttsVoice = v))}
                placeholder="alloy"
              />
            </Field>

            <Field label="API key" hint="Leave blank for a local server. Stored in your OS keychain.">
              <div className="row">
                <TextInput
                  type="password"
                  value={ttsKey}
                  onChange={setTtsKey}
                  placeholder="Leave blank for a local server"
                />
                <Button
                  size="sm"
                  onClick={async () => {
                    try {
                      const saved = await api.setTtsApiKey(ttsKey);
                      setTtsKey("");
                      setTtsKeyStatus(saved ? "Saved to keychain" : "Removed");
                    } catch (error) {
                      setTtsKeyStatus(api.errorMessage(error));
                    }
                  }}
                >
                  Save
                </Button>
              </div>
            </Field>

            {ttsKeyStatus && (
              <p className="col-span-2 text-2xs text-ink-faint">{ttsKeyStatus}</p>
            )}
          </div>
        )}

        {voice.ttsBackend === "system_native" && (
          <div className="mt-4">
            <Field
              label="Voice"
              hint={
                sayVoices.length > 0
                  ? "Installed system voices (say -v ?). Leave as Default for whatever `say` uses with no -v flag."
                  : "No installed voices were found on this Mac — leave as Default, or add some in System Settings → Accessibility → Spoken Content."
              }
            >
              <Select
                value={voice.ttsVoice}
                onChange={(v) => draft.update((d) => (d.voice.ttsVoice = v))}
                options={[
                  { value: "", label: "System default" },
                  ...sayVoices.map((name) => ({ value: name, label: name })),
                ]}
              />
            </Field>
          </div>
        )}

        {voice.ttsBackend !== "disabled" && (
          <div className="mt-4 grid grid-cols-2 gap-5">
            <Field label="Speaking rate" hint={rateConfig.hint}>
              <NumberInput
                value={voice.ttsRate}
                min={rateConfig.min}
                max={rateConfig.max}
                step={rateConfig.step}
                suffix={rateConfig.suffix}
                onChange={(value) => draft.update((d) => (d.voice.ttsRate = value))}
              />
            </Field>
          </div>
        )}

        <div className="mt-4 border-t border-line pt-4">
          <div className="row">
            <Button
              onClick={() => void testVoice()}
              disabled={testingVoice || !voice.ttsEnabled}
              title={voice.ttsEnabled ? undefined : "Turn on text-to-speech above to preview a voice."}
            >
              {testingVoice ? <Spinner /> : null} Test voice
            </Button>
            {testingVoice && (
              <Button tone="danger" size="sm" onClick={() => void api.stopSpeaking()}>
                Stop
              </Button>
            )}
            {!voice.ttsEnabled && (
              <span className="text-2xs text-ink-faint">
                Turn on text-to-speech above to preview a voice.
              </span>
            )}
          </div>
          {testError && <p className="mt-2 text-2xs text-danger">{testError}</p>}
        </div>

        <div className="mt-4">
          <Toggle
            label="Speak assistant replies automatically"
            hint="Reads each finished chat and agent reply aloud as it lands — the JARVIS-style behaviour. Needs text-to-speech enabled above; this is the separate switch for narrating automatically rather than only on Test voice."
            checked={voice.ttsSpeakReplies}
            onChange={(checked) => draft.update((d) => (d.voice.ttsSpeakReplies = checked))}
          />
        </div>
      </Section>

      <Section
        title="Assistant persona"
        description="How Caduceus refers to itself when it replies, in text and out loud alike — independent of whether replies are spoken at all."
      >
        <Toggle
          label="JARVIS-style persona"
          hint="Adopts a poised, economical butler register in every reply. Composes with spoken replies rather than requiring them: a text-only reply picks up the persona too, and speech with the persona off just reads replies aloud plainly."
          checked={agents.jarvisPersonaEnabled}
          onChange={(checked) => draft.update((d) => (d.agents.jarvisPersonaEnabled = checked))}
        />

        {agents.jarvisPersonaEnabled && (
          <div className="mt-3">
            <Field
              label="How it addresses you"
              hint={`Used the way the persona would say it — e.g. “Quite well, ${
                agents.jarvisHonorific.trim() || "…"
              }. Ever ready to assist you.” Leave blank to omit the address entirely.`}
            >
              <TextInput
                value={agents.jarvisHonorific}
                onChange={(v) => draft.update((d) => (d.agents.jarvisHonorific = v))}
                placeholder="sir"
              />
            </Field>
          </div>
        )}
      </Section>
    </>
  );
}
