import * as api from "@/shared/api";
import type { RuntimeInfo } from "@/shared/types";
import { Button, Callout, Field, HotkeyInput, NumberInput, Section, Select, Toggle } from "@/shared/ui";

import type { Draft } from "../useDraft";

export function GeneralTab({ draft, info }: { draft: Draft; info: RuntimeInfo | null }) {
  const settings = draft.settings;
  if (!settings) return null;
  const { general } = settings;

  return (
    <>
      <Section
        title="Hotkeys"
        description="Global shortcuts work everywhere, including while another app is focused. Leave one blank to unbind it."
      >
        <div className="grid grid-cols-2 gap-5">
          <Field
            label="Show / hide the staff"
            hint="Also available from the menu-bar icon."
          >
            <HotkeyInput
              value={general.toggleOrbHotkey}
              onChange={(value) => draft.update((d) => (d.general.toggleOrbHotkey = value))}
            />
          </Field>

          <Field
            label="Open the Command Center"
            hint="Avoid ⌘Space on macOS — Spotlight owns it and will win."
          >
            <HotkeyInput
              value={general.commandCenterHotkey}
              onChange={(value) => draft.update((d) => (d.general.commandCenterHotkey = value))}
            />
          </Field>
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
              min={1000}
              max={10000}
              step={250}
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
