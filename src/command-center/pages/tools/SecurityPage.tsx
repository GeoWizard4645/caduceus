/**
 * Privacy & security utilities: a diceware passphrase generator, a clipboard
 * auto-clear timer, a microphone mute switch, a camera/mic activity log, a
 * read-only firewall status, and a file vault.
 *
 * # Why this is one page and not six commands
 *
 * Every tool here is small enough to be its own one-field command, but they
 * share a theme a user thinks of together ("the security stuff") and several
 * of them cooperate — the passphrase generator's Copy button is more useful
 * next to the auto-clear timer than three navigations away from it. See
 * `QrPage.tsx` for the sibling case of "this is one page because the
 * interaction is the feature", which this follows structurally: sections,
 * plain `useState`, no form-schema machinery.
 *
 * # Where the backend calls come from
 *
 * `tools::security` and its `#[tauri::command]` wrappers in
 * `tools/security_cmds.rs` were built and reviewed as a self-contained pair
 * before this page existed — this file is the last piece that makes them
 * reachable. Every call below goes straight through `invoke()` rather than
 * `@/shared/api`, because adding exports to `shared/api.ts` is outside what
 * this page is allowed to touch; see that file's own header for why every
 * other IPC call in this app normally goes through it.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import { errorMessage } from "@/shared/api";
import type { ToolOutcome } from "@/shared/types";
import { Button, Callout, Field, NumberInput, Section, Spinner, TextInput, Toggle, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

// ---------------------------------------------------------------------------
// Types mirroring tools::security's Serialize structs
// ---------------------------------------------------------------------------
//
// Not added to `shared/types.ts` — this page does not own that file, and
// nothing else in the frontend needs these shapes yet. If a second page ever
// needs them, that is the moment to promote them, not before.

interface ActivityEvent {
  timestamp: string;
  app: string;
  service: string;
}

type FirewallState = "on" | "off";

// ---------------------------------------------------------------------------
// Small local helpers
// ---------------------------------------------------------------------------

/** Run a `ToolOutcome`-returning command, copying `copied` when present. */
async function runOutcome(
  command: string,
  args: Record<string, unknown>,
): Promise<{ ok: boolean; text: string; copiedOk: boolean }> {
  const result = await invoke<ToolOutcome>(command, args);
  if (!result.ok) return { ok: false, text: result.message, copiedOk: false };
  if (result.copied) {
    try {
      await navigator.clipboard.writeText(result.copied);
      return { ok: true, text: result.copied, copiedOk: true };
    } catch {
      return { ok: true, text: result.copied, copiedOk: false };
    }
  }
  return { ok: true, text: result.message, copiedOk: false };
}

export function SecurityPage({ onSetTitle }: ToolPageProps) {
  useEffect(() => {
    onSetTitle?.("Security");
  }, [onSetTitle]);

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Security</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          A passphrase generator, a clipboard timer, a microphone switch, a look at what has used
          your camera and mic, the firewall's current state, and a file vault. Every one of these
          runs on this Mac — nothing here is uploaded anywhere.
        </p>
      </div>

      <PassphraseSection />
      <ClipboardSection />
      <MicrophoneSection />
      <ActivitySection />
      <FirewallSection />
      <VaultSection />
      <TouchIdSection />
    </div>
  );
}

// ---------------------------------------------------------------------------
// 1. Passphrase generator
// ---------------------------------------------------------------------------

function PassphraseSection() {
  const [words, setWords] = useState(6);
  const [phrase, setPhrase] = useState<string | null>(null);
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);
  const [busy, setBusy] = useState(false);

  const generate = useCallback(async () => {
    setBusy(true);
    try {
      const { ok, text } = await runOutcome("security_generate_passphrase", { words });
      if (ok) {
        // `text` here is the passphrase itself (the command's `copied` field) —
        // `runOutcome` already put it on the clipboard.
        setPhrase(text);
        setNote({ text: "Copied.", ok: true });
      } else {
        setPhrase(null);
        setNote({ text, ok: false });
      }
    } catch (error) {
      setNote({ text: errorMessage(error), ok: false });
    } finally {
      setBusy(false);
    }
  }, [words]);

  return (
    <Section
      title="Passphrase generator"
      description="Real words, picked independently at random and joined with dashes — easy to read off a screen and retype on a phone, at comparable strength to a shorter random string."
    >
      <div className="grid grid-cols-2 gap-3">
        <Field label="Word count" hint="More words means more entropy. Six is a reasonable default.">
          <NumberInput value={words} onChange={setWords} min={4} max={20} suffix="words" />
        </Field>
        <div className="flex items-end">
          <Button tone="primary" onClick={() => void generate()} disabled={busy}>
            {busy ? "Generating…" : "Generate & copy"}
          </Button>
        </div>
      </div>

      {phrase && (
        <div className="mt-3 flex items-center justify-between gap-3 rounded-lg border border-line-strong/60 bg-base/60 px-3 py-2">
          <span className="truncate font-mono text-[13px] text-ink">{phrase}</span>
          <Button
            size="sm"
            onClick={() => {
              navigator.clipboard
                .writeText(phrase)
                .then(() => setNote({ text: "Copied.", ok: true }))
                .catch(() => setNote({ text: "Could not copy.", ok: false }));
            }}
          >
            Copy
          </Button>
        </div>
      )}

      {note && (
        <p className={cx("mt-2 text-2xs", note.ok ? "text-ink-faint" : "text-danger")}>{note.text}</p>
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 2. Clipboard auto-clear
// ---------------------------------------------------------------------------

function ClipboardSection() {
  const [seconds, setSeconds] = useState(30);
  const [armed, setArmed] = useState(false);
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);
  const [busy, setBusy] = useState(false);

  const toggle = useCallback(
    async (next: boolean) => {
      setBusy(true);
      try {
        if (next) {
          const { ok, text } = await runOutcome("security_clipboard_auto_clear", { seconds });
          setNote({ text, ok });
          // Only counts as armed if the backend actually found something to
          // arm against — an empty clipboard refuses rather than lying.
          setArmed(ok);
        } else {
          await invoke<ToolOutcome>("security_cancel_auto_clear");
          setArmed(false);
          setNote({ text: "Auto-clear cancelled.", ok: true });
        }
      } catch (error) {
        setNote({ text: errorMessage(error), ok: false });
      } finally {
        setBusy(false);
      }
    },
    [seconds],
  );

  return (
    <Section
      title="Clipboard auto-clear"
      description="Arms a one-shot timer against whatever is on the clipboard right now — the passphrase above, a generated password, anything. It only ever clears if the clipboard still holds exactly that when the timer fires, so copying something else first cancels it for you."
    >
      <Field label="Delay" hint="How long the current clipboard contents stay before they are wiped.">
        <NumberInput value={seconds} onChange={setSeconds} min={5} max={600} suffix="seconds" />
      </Field>
      <div className="mt-3">
        <Toggle
          checked={armed}
          onChange={(next) => void toggle(next)}
          disabled={busy}
          label={armed ? "Auto-clear armed" : "Arm auto-clear for the current clipboard"}
          hint="Turning this off only cancels a future clear — it never touches what is on the clipboard right now."
        />
      </div>
      {note && (
        <p className={cx("mt-2 text-2xs", note.ok ? "text-ink-faint" : "text-danger")}>{note.text}</p>
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 3. Microphone mute
// ---------------------------------------------------------------------------

function MicrophoneSection() {
  const [muted, setMuted] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    invoke<boolean>("security_mic_muted")
      .then(setMuted)
      .catch((error) => setNote(errorMessage(error)));
  }, []);

  const toggle = useCallback(async (next: boolean) => {
    setBusy(true);
    try {
      const outcome = await invoke<ToolOutcome>("security_set_mic_muted", { mute: next });
      setNote(outcome.message);
      if (outcome.ok) setMuted(next);
    } catch (error) {
      setNote(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }, []);

  return (
    <Section
      title="Microphone"
      description="macOS has no real “input muted” flag — mute here means the input volume is remembered and set to zero, then restored on unmute. If Caduceus restarts while muted, unmuting falls back to 50% rather than claiming to restore a value it no longer has."
    >
      <Toggle
        checked={muted ?? false}
        onChange={(next) => void toggle(next)}
        disabled={busy || muted === null}
        label={muted === null ? "Checking…" : muted ? "Microphone muted" : "Microphone live"}
      />
      {note && <p className="mt-2 text-2xs text-ink-faint">{note}</p>}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 4. Camera & microphone activity log
// ---------------------------------------------------------------------------

function ActivitySection() {
  const [minutes, setMinutes] = useState(60);
  const [events, setEvents] = useState<ActivityEvent[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setEvents(await invoke<ActivityEvent[]>("security_activity_log", { minutes }));
    } catch (err) {
      setError(errorMessage(err));
      setEvents(null);
    } finally {
      setBusy(false);
    }
  }, [minutes]);

  return (
    <Section
      title="Camera & microphone activity"
      description="Reads the system's own privacy log (the same record System Settings → Privacy & Security draws its indicator dots from) for which apps have checked the camera or microphone recently."
    >
      <div className="grid grid-cols-2 gap-3">
        <Field label="Look back" hint="Up to 24 hours — a longer window makes the system log slow to search.">
          <NumberInput value={minutes} onChange={setMinutes} min={1} max={1440} suffix="minutes" />
        </Field>
        <div className="flex items-end gap-2">
          <Button onClick={() => void refresh()} disabled={busy}>
            {busy ? "Reading…" : "Refresh"}
          </Button>
          {busy && <Spinner className="text-accent" />}
        </div>
      </div>

      {error && <p className="mt-3 text-2xs text-danger">{error}</p>}

      {events && (
        <div className="mt-3">
          {events.length === 0 ? (
            <p className="text-2xs text-ink-faint">No microphone or camera activity in that window.</p>
          ) : (
            <ul className="divide-y divide-line/60 rounded-lg border border-line-strong/60">
              {events.map((event, i) => (
                <li key={`${event.timestamp}-${event.app}-${i}`} className="flex items-center justify-between gap-3 px-3 py-2">
                  <span className="min-w-0 flex-1 truncate font-mono text-2xs text-ink">{event.app}</span>
                  <span className="shrink-0 text-2xs text-ink-mute">{event.service}</span>
                  <span className="shrink-0 text-2xs text-ink-faint">{event.timestamp}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 5. Firewall
// ---------------------------------------------------------------------------

function FirewallSection() {
  const [state, setState] = useState<FirewallState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<FirewallState>("security_firewall_state")
      .then(setState)
      .catch((error) => setError(errorMessage(error)));
  }, []);

  const openSettings = useCallback(async () => {
    setBusy(true);
    try {
      const outcome = await invoke<ToolOutcome>("security_open_firewall_settings");
      if (!outcome.ok) setError(outcome.message);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }, []);

  return (
    <Section
      title="Firewall"
      description="Read-only here on purpose. Turning the firewall on or off needs an admin password, and that password should only ever be typed into Apple's own dialog — not into Caduceus."
    >
      <div className="flex items-center justify-between gap-3">
        <span className="row gap-2 text-[13px] text-ink">
          <span
            aria-hidden="true"
            className={cx(
              "h-2 w-2 rounded-full",
              state === "on" ? "bg-positive" : state === "off" ? "bg-danger" : "bg-ink-faint",
            )}
          />
          {state === "on" ? "Firewall is on" : state === "off" ? "Firewall is off" : "Checking…"}
        </span>
        <Button size="sm" onClick={() => void openSettings()} disabled={busy}>
          Open Firewall settings
        </Button>
      </div>
      {error && <p className="mt-2 text-2xs text-danger">{error}</p>}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 6. File vault
// ---------------------------------------------------------------------------

function VaultSection() {
  const [lockPath, setLockPath] = useState("");
  const [lockPassphrase, setLockPassphrase] = useState("");
  const [deleteOriginal, setDeleteOriginal] = useState(false);
  const [lockBusy, setLockBusy] = useState(false);
  const [lockNote, setLockNote] = useState<{ text: string; ok: boolean } | null>(null);

  const [unlockPath, setUnlockPath] = useState("");
  const [unlockPassphrase, setUnlockPassphrase] = useState("");
  const [unlockBusy, setUnlockBusy] = useState(false);
  const [unlockNote, setUnlockNote] = useState<{ text: string; ok: boolean } | null>(null);

  const pickLockFile = async () => {
    const path = await open({ multiple: false, directory: false });
    if (typeof path === "string") setLockPath(path);
  };
  const pickUnlockFile = async () => {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Caduceus vault", extensions: ["vault"] }],
    });
    if (typeof path === "string") setUnlockPath(path);
  };

  const lock = async () => {
    setLockBusy(true);
    setLockNote(null);
    try {
      const outcome = await invoke<ToolOutcome>("security_lock_file", {
        path: lockPath,
        passphrase: lockPassphrase,
        deleteOriginal,
      });
      setLockNote({ text: outcome.message, ok: outcome.ok });
      if (outcome.ok) {
        setLockPath("");
        setLockPassphrase("");
      }
    } catch (error) {
      setLockNote({ text: errorMessage(error), ok: false });
    } finally {
      setLockBusy(false);
    }
  };

  const unlock = async () => {
    setUnlockBusy(true);
    setUnlockNote(null);
    try {
      const outcome = await invoke<ToolOutcome>("security_unlock_file", {
        path: unlockPath,
        passphrase: unlockPassphrase,
      });
      setUnlockNote({ text: outcome.message, ok: outcome.ok });
      if (outcome.ok) {
        setUnlockPath("");
        setUnlockPassphrase("");
      }
    } catch (error) {
      setUnlockNote({ text: errorMessage(error), ok: false });
    } finally {
      setUnlockBusy(false);
    }
  };

  return (
    <Section
      title="File vault"
      description="Encrypts a single file into a sibling .vault next to it (ChaCha20-Poly1305, a fresh random nonce and salt per file). The key comes from your passphrase through Argon2id, which takes a few hundred milliseconds on purpose — that delay is what makes guessing expensive for anyone who gets hold of the file."
    >
      <Callout tone="warn" title="There is no reset">
        A vault's key is derived only from the passphrase you type. If you forget it, the file is
        unrecoverable — not by Caduceus, not by anyone. Write it down somewhere durable before you
        rely on this, or use the passphrase generator above and store the result.
      </Callout>

      {/* --- lock ------------------------------------------------------- */}
      <div className="mt-5">
        <h3 className="mb-2 text-[13px] font-semibold text-ink">Lock a file</h3>
        <Field label="File">
          <div className="row gap-2">
            <TextInput value={lockPath} onChange={setLockPath} placeholder="No file chosen" mono />
            <Button size="sm" onClick={() => void pickLockFile()}>
              Choose…
            </Button>
          </div>
        </Field>
        <div className="mt-3">
          <Field label="Passphrase" hint="At least 12 characters. The passphrase generator above is a good source.">
            <TextInput value={lockPassphrase} onChange={setLockPassphrase} type="password" />
          </Field>
        </div>
        <div className="mt-3">
          <Toggle
            checked={deleteOriginal}
            onChange={setDeleteOriginal}
            label="Delete the original after locking"
            hint="Off by default: locking a copy is fully reversible by just deleting the .vault file. Deleting the source is a second, separate decision."
          />
        </div>
        <div className="mt-3 row gap-2">
          <Button
            tone="primary"
            onClick={() => void lock()}
            disabled={lockBusy || !lockPath || !lockPassphrase}
          >
            {lockBusy ? "Deriving key & encrypting…" : "Lock"}
          </Button>
          {lockBusy && <Spinner className="text-accent" />}
        </div>
        {lockNote && (
          <p className={cx("mt-2 text-2xs", lockNote.ok ? "text-ink-faint" : "text-danger")}>
            {lockNote.text}
          </p>
        )}
      </div>

      {/* --- unlock ------------------------------------------------------- */}
      <div className="mt-6 border-t border-line pt-5">
        <h3 className="mb-2 text-[13px] font-semibold text-ink">Unlock a .vault file</h3>
        <Field label="Vault file">
          <div className="row gap-2">
            <TextInput value={unlockPath} onChange={setUnlockPath} placeholder="No file chosen" mono />
            <Button size="sm" onClick={() => void pickUnlockFile()}>
              Choose…
            </Button>
          </div>
        </Field>
        <div className="mt-3">
          <Field label="Passphrase">
            <TextInput value={unlockPassphrase} onChange={setUnlockPassphrase} type="password" />
          </Field>
        </div>
        <div className="mt-3 row gap-2">
          <Button
            tone="primary"
            onClick={() => void unlock()}
            disabled={unlockBusy || !unlockPath || !unlockPassphrase}
          >
            {unlockBusy ? "Deriving key & decrypting…" : "Unlock"}
          </Button>
          {unlockBusy && <Spinner className="text-accent" />}
        </div>
        {unlockNote && (
          <p className={cx("mt-2 text-2xs", unlockNote.ok ? "text-ink-faint" : "text-danger")}>
            {unlockNote.text}
          </p>
        )}
        <p className="mt-2 text-2xs text-ink-faint">
          A wrong passphrase and a corrupted file report the same error on purpose — telling the two
          apart would tell an attacker something they should not learn from failed guesses.
        </p>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 7. TouchID app lock — documented gap
// ---------------------------------------------------------------------------

function TouchIdSection() {
  const [available, setAvailable] = useState<boolean | null>(null);
  const seenPath = useRef(false);

  useEffect(() => {
    if (seenPath.current) return;
    seenPath.current = true;
    invoke<boolean>("security_touch_id_available")
      .then(setAvailable)
      .catch(() => setAvailable(false));
  }, []);

  // Nothing to offer while it is unavailable — no toggle, no button. A control
  // that looked like it authenticated with TouchID but did not would be worse
  // than no control at all.
  if (available !== false) return null;

  return (
    <Section title="App lock with Touch ID">
      <p className="text-[13px] leading-relaxed text-ink-mute">
        Not available in this build. Touch ID authentication needs Apple's LocalAuthentication
        framework, which nothing in Caduceus currently binds to — adding it means either a new
        dependency or hand-written bindings, neither of which this pass includes. Rather than ship a
        toggle that only remembers a flag and calls that "locked," this is left off until it can be
        built for real.
      </p>
    </Section>
  );
}
