import * as api from "@/shared/api";
import { SUPPORT_EMAIL, SUPPORT_MAILTO } from "@/shared/docsUrls";
import type { FunctionKeyBinding, RuntimeInfo } from "@/shared/types";
import { Button, Callout, Field, HotkeyInput, NumberInput, Section, Select, Toggle } from "@/shared/ui";

import type { TutorialId } from "./Learn";
import type { Draft } from "../useDraft";

export function GeneralTab({
  draft,
  info,
  onOpenTutorial,
}: {
  draft: Draft;
  info: RuntimeInfo | null;
  onOpenTutorial: (topic: TutorialId) => void;
}) {
  const settings = draft.settings;
  if (!settings) return null;
  const { general } = settings;

  return (
    <>
      <Section
        title="If Caduceus disappears"
        description="There is no Dock icon on purpose — Caduceus is a menu-bar app. You can always get back in."
      >
        <ul className="list-disc space-y-2 pl-5 text-[13px] text-ink-mute">
          <li>
            <strong className="font-medium text-ink-soft">Spotlight</strong> — press ⌘Space (or your
            Spotlight shortcut), type <strong className="text-ink-soft">Caduceus</strong>, and press
            Return. Opens from <code className="text-ink-soft">/Applications/Caduceus.app</code> like
            any other app.
          </li>
          <li>
            <strong className="font-medium text-ink-soft">Applications</strong> — open{" "}
            <em>Applications → Caduceus</em> in Finder, or search “Caduceus” in Launchpad.
          </li>
          <li>
            <strong className="font-medium text-ink-soft">Command Center palette</strong> — with
            Caduceus running, open the palette and type <strong className="text-ink-soft">Caduceus</strong>{" "}
            to focus or relaunch it.
          </li>
          <li>
            <strong className="font-medium text-ink-soft">Menu bar</strong> — look for the Caduceus
            icon near the clock; click for the Command Center, right-click for Settings, Restart,
            and Quit.
          </li>
        </ul>
        <p className="mt-3 text-2xs text-ink-faint">
          Enable <strong className="font-normal text-ink-mute">Launch Caduceus at login</strong> below
          so a crash or force-quit is one Spotlight launch away from recovery.
        </p>
      </Section>

      <Section
        title="Hotkeys"
        description="Global shortcuts work everywhere, including while another app is focused. Leave one blank to unbind it."
      >
        <div className="grid grid-cols-2 gap-5">
          <Field
            label="Show / hide the staff"
            hint="Also on the menu-bar icon. Leave blank to use F12 — function keys are set in the table below."
          >
            <HotkeyInput
              value={general.toggleOrbHotkey}
              onChange={(value) => draft.update((d) => (d.general.toggleOrbHotkey = value))}
            />
          </Field>

          <Field
            label="Open the Command Center"
            hint={
              <>
                Want ⌘Space to open this instead of Spotlight?{" "}
                {/* Field wraps its hint in the <label>, so a bare click here
                    would also toggle the hotkey capture below. */}
                <button
                  type="button"
                  className="text-accent underline underline-offset-2"
                  onClick={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    onOpenTutorial("spotlight");
                  }}
                >
                  Read this tutorial
                </button>
                .
              </>
            }
          >
            <HotkeyInput
              value={general.commandCenterHotkey}
              onChange={(value) => draft.update((d) => (d.general.commandCenterHotkey = value))}
            />
          </Field>
        </div>
      </Section>

      <Section
        title="Function keys"
        description={
          <>
            Assign actions to <kbd className="rounded border border-line px-1 font-mono text-2xs">F1</kbd>
            –<kbd className="rounded border border-line px-1 font-mono text-2xs">F20</kbd> while
            Caduceus is running. On Mac, set{" "}
            <span className="text-ink-soft">Keyboard → Keyboard Shortcuts → Function Keys</span> to
            use F-keys as standard function keys (not brightness/volume) if you want them globally.
          </>
        }
      >
        <div className="space-y-2">
          {general.functionKeys.map((row, index) => {
            const actionOptions: { value: FunctionKeyBinding["action"]; label: string }[] = [
              { value: "none", label: "Off" },
              { value: "voice_memo", label: "Voice Memos — new recording (macOS)" },
              { value: "start_dictation", label: "Dictation — tap to toggle (F1 default)" },
              { value: "push_to_talk", label: "Dictation — hold to talk" },
              { value: "command_center", label: "Open Command Center" },
              { value: "toggle_staff", label: "Show / hide staff" },
              { value: "screenshot", label: "Screenshot to clipboard" },
              { value: "run_shortcut", label: "Run shortcut…" },
            ];

            return (
              <div
                key={row.key}
                className="grid grid-cols-[3.5rem_1fr_minmax(0,14rem)] items-center gap-3"
              >
                <span className="font-mono text-[13px] text-ink-soft">{row.key}</span>
                <Select
                  value={row.action}
                  onChange={(value) => {
                    draft.update((d) => {
                      const binding = d.general.functionKeys[index];
                      if (!binding) return;
                      binding.action = value as FunctionKeyBinding["action"];
                      if (value !== "run_shortcut") binding.shortcutId = "";
                    });
                  }}
                  options={actionOptions}
                />
                {row.action === "run_shortcut" ? (
                  <Select
                    value={row.shortcutId || ""}
                    onChange={(value) =>
                      draft.update((d) => {
                        const binding = d.general.functionKeys[index];
                        if (binding) binding.shortcutId = value;
                      })
                    }
                    options={[
                      { value: "", label: "Choose shortcut…" },
                      ...settings.shortcuts.map((s) => ({
                        value: s.id,
                        label: s.label,
                      })),
                    ]}
                  />
                ) : (
                  <span className="text-2xs text-ink-faint" />
                )}
              </div>
            );
          })}
        </div>
      </Section>

      <Section
        title="The staff"
        description="The floating circle that sits on top of everything. Drag it anywhere; Caduceus remembers where you left it."
      >
        <div className="grid grid-cols-2 gap-5">
          <Field label="Docked edge" hint="Used until you drag the staff somewhere else.">
            <Select
              value={general.staffEdge}
              onChange={(value) => {
                draft.update((d) => {
                  d.general.staffEdge = value;
                  // Clearing the saved position is what makes the new edge take
                  // effect; without this the staff stays where it was dragged.
                  d.general.staffPosition = null;
                });
              }}
              options={[
                { value: "right", label: "Right" },
                { value: "left", label: "Left" },
              ]}
            />
          </Field>

          <Field label="Position">
            <div className="row h-[38px]">
              <span className="text-2xs text-ink-faint">
                {general.staffPosition
                  ? `Custom · ${Math.round(general.staffPosition.x)}, ${Math.round(general.staffPosition.y)}`
                  : "Snapped to the edge"}
              </span>
              {general.staffPosition && (
                <Button
                  size="sm"
                  onClick={() => draft.update((d) => (d.general.staffPosition = null))}
                >
                  Reset
                </Button>
              )}
            </div>
          </Field>

          <Field
            label="Expand delay"
            hint="How long the pointer must rest on the staff before the icons appear. 0 means instantly."
          >
            <NumberInput
              value={general.hoverExpandDelayMs}
              min={0}
              max={2000}
              step={50}
              suffix="ms"
              onChange={(value) => draft.update((d) => (d.general.hoverExpandDelayMs = value))}
            />
          </Field>

          <Field
            label="Auto-collapse after"
            hint="Time with the pointer elsewhere before the icons fold back in."
          >
            <NumberInput
              value={general.collapseIdleMs}
              min={0}
              max={10000}
              step={50}
              suffix="ms"
              onChange={(value) => draft.update((d) => (d.general.collapseIdleMs = value))}
            />
          </Field>
        </div>

        <div className="mt-4 space-y-1 border-t border-line pt-4">
          <Toggle
            label="Show the staff"
            hint="Caduceus keeps working without it — the hotkey and menu-bar icon still open everything."
            checked={general.staffVisible}
            onChange={(checked) => draft.update((d) => (d.general.staffVisible = checked))}
          />
        </div>
      </Section>

      <Section title="Startup">
        <Toggle
          label="Launch Caduceus at login"
          hint="Starts in the background with no window — just the menu-bar icon and the staff."
          checked={general.launchAtLogin}
          onChange={(checked) => draft.update((d) => (d.general.launchAtLogin = checked))}
        />
        <div className="row mt-4 border-t border-line pt-4">
          <Button
            onClick={async () => {
              try {
                // Persist any edit still inside the settings debounce before
                // the backend exits; beforeunload cannot await an IPC save.
                await api.updateSettings(settings);
                await api.restartApp();
              } catch (error) {
                window.alert(api.errorMessage(error));
              }
            }}
          >
            Restart Caduceus
          </Button>
          <span className="text-2xs text-ink-faint">
            Quit and reopen the app, preserving your settings and open tabs.
          </span>
        </div>
      </Section>

      <Section
        title="Advanced"
        description="Sensible defaults; change these only if something feels wrong."
      >
        <Field
          label="Cursor tracking interval"
          hint="How often Caduceus checks where your pointer is, to drive staff hover and auto-collapse. Lower is snappier and uses marginally more CPU."
        >
          <NumberInput
            value={general.cursorPollMs}
            min={8}
            max={200}
            step={1}
            suffix="ms"
            onChange={(value) => draft.update((d) => (d.general.cursorPollMs = value))}
          />
        </Field>
      </Section>

      <Section
        title="About"
        description="Caduceus is MIT-licensed and works entirely on your machine unless you configure an AI backend."
      >
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-[13px]">
          <dt className="text-ink-faint">Version</dt>
          <dd className="text-ink-soft">{info?.version ?? "—"}</dd>
          <dt className="text-ink-faint">Platform</dt>
          <dd className="text-ink-soft">
            {info ? `${info.platform} · ${info.arch}` : "—"}
          </dd>
          <dt className="text-ink-faint">Secret storage</dt>
          <dd className="text-ink-soft">
            {info?.keychainAvailable ? "OS keychain available" : "Unavailable"}
          </dd>
        </dl>

        {info && !info.keychainAvailable && (
          <div className="mt-4">
            <Callout tone="warn" title="No keychain on this system">
              Caduceus stores API keys in your OS keychain and refuses to write them anywhere else.
              Without one, AI backends that need a key cannot be configured. On Linux, install and
              run a Secret Service provider such as <code>gnome-keyring</code> or KWallet.
            </Callout>
          </div>
        )}

        <p className="mt-4 text-[13px] leading-relaxed text-ink-mute">
          Questions or feedback? Open the <strong className="font-medium text-ink-soft">Help</strong>{" "}
          tab in the sidebar, or email{" "}
          <button
            type="button"
            className="text-accent underline decoration-accent/40 underline-offset-2"
            onClick={() => void api.openExternalUrl(SUPPORT_MAILTO)}
          >
            {SUPPORT_EMAIL}
          </button>
          .
        </p>

        <div className="row mt-5 border-t border-line pt-4">
          <Button
            tone="danger"
            onClick={async () => {
              const ok = window.confirm(
                "Reset every setting to its default? Your clipboard history is kept, but shortcuts, prefixes, keyword groups and AI backends (including their stored API keys) are removed.",
              );
              if (!ok) return;
              draft.replace(await api.resetSettings());
            }}
          >
            Reset all settings
          </Button>
          <span className="text-2xs text-ink-faint">
            Removes stored API keys from the keychain as well.
          </span>
        </div>
      </Section>
    </>
  );
}
