import { useState } from "react";

import * as api from "@/shared/api";
import { formatBytes } from "@/shared/providers";
import type { RuntimeInfo } from "@/shared/types";
import {
  Button,
  Callout,
  Field,
  NumberInput,
  Section,
  Select,
  TextArea,
  Toggle,
} from "@/shared/ui";

import type { Draft } from "../useDraft";

export function ClipboardTab({
  draft,
  info,
  onReloadInfo,
}: {
  draft: Draft;
  info: RuntimeInfo | null;
  onReloadInfo: () => void;
}) {
  const [status, setStatus] = useState<string | null>(null);
  const settings = draft.settings;
  if (!settings) return null;
  const clipboard = settings.clipboard;

  return (
    <>
      <Section
        title="History"
        description="Caduceus watches the clipboard in the background and keeps what you copy, so you can get it back later."
      >
        <Toggle
          label="Keep clipboard history"
          hint="Turning this off stops the watcher immediately. Existing history stays until you clear it."
          checked={clipboard.enabled}
          onChange={(checked) => draft.update((d) => (d.clipboard.enabled = checked))}
        />

        <div className="mt-4 grid grid-cols-2 gap-5">
          <Field label="Keep at most" hint="Pinned entries are never removed by this limit.">
            <NumberInput
              value={clipboard.maxItems}
              min={10}
              max={100000}
              step={50}
              suffix="items"
              onChange={(value) => draft.update((d) => (d.clipboard.maxItems = value))}
            />
          </Field>

          <Field label="Discard after" hint="Pinned entries are exempt from this too.">
            <Select
              value={clipboard.maxAgeDays === null ? "never" : String(clipboard.maxAgeDays)}
              onChange={(v) =>
                draft.update((d) => (d.clipboard.maxAgeDays = v === "never" ? null : Number(v)))
              }
              options={[
                { value: "1", label: "1 day" },
                { value: "7", label: "1 week" },
                { value: "30", label: "30 days" },
                { value: "90", label: "90 days" },
                { value: "365", label: "1 year" },
                { value: "never", label: "Never" },
              ]}
            />
          </Field>

          <Field
            label="Check every"
            hint="How often Caduceus looks at the clipboard. Lower feels snappier; the cost is negligible either way."
          >
            <NumberInput
              value={clipboard.pollIntervalMs}
              min={100}
              max={10000}
              step={100}
              suffix="ms"
              onChange={(value) => draft.update((d) => (d.clipboard.pollIntervalMs = value))}
            />
          </Field>

          <Field label="Skip anything larger than" hint="Stops a huge image from bloating the database.">
            <NumberInput
              value={Math.round(clipboard.maxEntryBytes / (1024 * 1024))}
              min={1}
              max={256}
              suffix="MB"
              onChange={(value) =>
                draft.update((d) => (d.clipboard.maxEntryBytes = value * 1024 * 1024))
              }
            />
          </Field>
        </div>

        <div className="mt-4 space-y-1 border-t border-line pt-4">
          <Toggle
            label="Capture text"
            checked={clipboard.captureText}
            onChange={(checked) => draft.update((d) => (d.clipboard.captureText = checked))}
          />
          <Toggle
            label="Capture images"
            checked={clipboard.captureImages}
            onChange={(checked) => draft.update((d) => (d.clipboard.captureImages = checked))}
          />
          <Toggle
            label="Capture copied files"
            hint="Stores the paths, not the file contents."
            checked={clipboard.captureFiles}
            onChange={(checked) => draft.update((d) => (d.clipboard.captureFiles = checked))}
          />
        </div>
      </Section>

      <Section
        title="Privacy"
        description="A clipboard history is only trustworthy if it knows what not to record."
      >
        <Toggle
          label="Honour the “do not record” marker"
          hint="Password managers tag their copies as concealed. Caduceus skips anything carrying that marker. macOS only — no equivalent convention exists on Windows or Linux."
          checked={clipboard.respectConcealedMarker}
          onChange={(checked) => draft.update((d) => (d.clipboard.respectConcealedMarker = checked))}
        />

        <div className="mt-4">
          <Field
            label="Never capture from these apps"
            hint="One per line, matched case-insensitively against the frontmost app name. Detection is best-effort and unavailable on Windows."
          >
            <TextArea
              rows={6}
              value={clipboard.excludedApps.join("\n")}
              onChange={(v) =>
                draft.update(
                  (d) =>
                    (d.clipboard.excludedApps = v
                      .split("\n")
                      .map((line) => line.trim())
                      .filter(Boolean)),
                )
              }
            />
          </Field>
        </div>
      </Section>

      <Section
        title="Encryption at rest"
        description="Encrypts stored entries with ChaCha20-Poly1305, keyed from your OS keychain."
      >
        <Toggle
          label="Encrypt clipboard history"
          hint="Toggling this rewrites every existing entry, which can take a moment on a large history."
          checked={clipboard.encryptAtRest}
          disabled={info ? !info.keychainAvailable : false}
          onChange={(checked) => draft.update((d) => (d.clipboard.encryptAtRest = checked))}
        />

        <div className="mt-4 space-y-3">
          <Callout tone="info" title="What this does and does not protect">
            It protects your history from anything that can read the database file — a synced
            backup, another user account, a disk pulled out of the machine. It does <em>not</em>
            {" "}protect against software running as you while Caduceus is unlocked, because that can ask
            the keychain for the key exactly as Caduceus does.
          </Callout>

          <Callout tone="warn" title="Losing the key loses the history">
            The key lives only in your OS keychain and Caduceus provides no way to export it. If the
            keychain is reset or the entry is deleted, existing encrypted history becomes
            permanently unreadable and is removed the next time you toggle this. That is the
            intended behaviour, not a bug.
          </Callout>
        </div>
      </Section>

      <Section title="Stored data">
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-[13px]">
          <dt className="text-ink-faint">Entries</dt>
          <dd className="text-ink-soft">{info?.clipboardEntries ?? 0}</dd>
          <dt className="text-ink-faint">On disk</dt>
          <dd className="text-ink-soft">{formatBytes(info?.clipboardBytes ?? 0)}</dd>
        </dl>

        <div className="row mt-5 border-t border-line pt-4">
          <Button
            onClick={async () => {
              const removed = await api.clipboardClear(true);
              setStatus(`Removed ${removed} unpinned ${removed === 1 ? "entry" : "entries"}.`);
              onReloadInfo();
            }}
          >
            Clear unpinned
          </Button>

          <Button
            tone="danger"
            onClick={async () => {
              if (!window.confirm("Delete all clipboard history, including pinned entries?")) return;
              const removed = await api.clipboardClear(false);
              setStatus(`Removed ${removed} ${removed === 1 ? "entry" : "entries"}.`);
              onReloadInfo();
            }}
          >
            Clear everything
          </Button>

          {status && <span className="text-2xs text-ink-faint">{status}</span>}
        </div>
      </Section>
    </>
  );
}
