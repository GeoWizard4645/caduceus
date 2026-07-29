/**
 * Updates: the mode picker, the honest caveat about what updating costs you,
 * and — when one is actually available — the button that installs it.
 *
 * Three modes, persisted on `settings.update.mode`:
 *
 * * `off` — the background watcher (`update::spawn_update_watcher` in Rust)
 *   never checks. The manual "Check now" button below still works.
 * * `notify` (the default) — checked automatically, announced once per
 *   version with a macOS notification, and installed on a click here.
 * * `auto` — checked and installed without asking, except when this copy is
 *   Homebrew-managed or Caduceus is busy with a recording, in which case it
 *   behaves like `notify` for that cycle.
 *
 * This component owns its own read/write of `settings.update` via
 * `useSettings()` rather than the Settings window's shared draft — the same
 * pattern already used elsewhere for a narrow, self-contained write (see
 * `General.tsx`'s restart button, `OnboardingQuiz.tsx`) — because it is not
 * passed the draft as a prop.
 *
 * Updating still runs the same one-liner the website hands out, in Terminal,
 * for a Homebrew-free install — see the doc comment on `update::run_installer`
 * for why Terminal owns that process rather than Caduceus. A Homebrew-managed
 * copy gets `brew upgrade --cask caduceus` instead: silently replacing a cask
 * with the curl installer would leave Homebrew's own bookkeeping pointing at
 * a version that is no longer actually on disk.
 */

import { useState } from "react";

import * as api from "@/shared/api";
import { INSTALL_SCRIPT_URL } from "@/shared/docsUrls";
import { useSettings, useUpdateCheck } from "@/shared/hooks";
import { STALE_GRANT_EXPLANATION } from "@/shared/permissions";
import type { UpdateMode } from "@/shared/types";
import { Button, Callout, Section, Select } from "@/shared/ui";

/** Shown, never executed from here. Mirrors `update::INSTALL_COMMAND`. */
const INSTALL_COMMAND = `curl -fsSL ${INSTALL_SCRIPT_URL} | bash`;

const BREW_UPGRADE_COMMAND = "brew upgrade --cask caduceus";

const MODE_OPTIONS: { value: UpdateMode; label: string }[] = [
  { value: "off", label: "Off — never check automatically" },
  { value: "notify", label: "Notify — check, then ask before installing" },
  { value: "auto", label: "Automatic — check and install without asking" },
];

function timeAgo(unixSecs: number): string {
  const diffSecs = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (diffSecs < 90) return "just now";
  const minutes = Math.round(diffSecs / 60);
  if (minutes < 90) return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 36) return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  const days = Math.round(hours / 24);
  return `${days} day${days === 1 ? "" : "s"} ago`;
}

export function UpdateSection() {
  const { settings } = useSettings();
  const update = useUpdateCheck(settings?.update.mode !== "off");
  const [starting, setStarting] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  if (!settings) return null;

  const mode = settings.update.mode;

  const setMode = async (next: UpdateMode) => {
    setNote(null);
    try {
      await api.updateSettings({ ...settings, update: { ...settings.update, mode: next } });
    } catch (error) {
      setNote(api.errorMessage(error));
    }
  };

  const runUpdate = async () => {
    setStarting(true);
    setNote(null);
    try {
      await api.runInstallerUpdate();
      // Nothing to await beyond the hand-off: the installer quits Caduceus a
      // moment from now and reopens it once the new build is in place.
      setNote("Terminal is running the installer. Caduceus will close and reopen itself.");
    } catch (error) {
      setNote(api.errorMessage(error));
      setStarting(false);
    }
  };

  const copyBrewCommand = () => {
    navigator.clipboard
      .writeText(BREW_UPGRADE_COMMAND)
      .then(() => setNote("Command copied."))
      .catch(() => setNote("Could not copy that."));
  };

  return (
    <Section
      title="Updates"
      description="Check for new releases automatically, and choose whether Caduceus installs them by itself."
    >
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-[minmax(0,260px)_1fr] sm:items-start">
        <Select
          value={mode}
          onChange={(v) => void setMode(v)}
          options={MODE_OPTIONS}
        />
        <p className="text-2xs leading-relaxed text-ink-faint sm:pt-2">
          {mode === "off" &&
            "Nothing is checked in the background. Use “Check now” below whenever you want to look."}
          {mode === "notify" &&
            "Checked roughly every 12 hours. You get a notification once per new version, and nothing installs until you click."}
          {mode === "auto" &&
            "Checked roughly every 12 hours and installed automatically — unless this copy is Homebrew-managed or Caduceus is mid-recording, in which case it asks instead, once."}
        </p>
      </div>

      {settings.update.lastCheckedAt && (
        <p className="mt-2.5 text-2xs text-ink-faint">
          Last checked {timeAgo(settings.update.lastCheckedAt)}.
        </p>
      )}

      <Callout tone="warn" title="Every update can reset your permissions">
        <p className="text-[13px] leading-relaxed text-ink-soft">
          Caduceus is not notarised, so macOS ties Accessibility, Screen Recording and Microphone
          grants to this exact build. Updating more often means running into that more often —
          each new build can look like a different app to macOS, even with nothing else changed.
        </p>
        <p className="mt-1.5 text-2xs leading-relaxed text-ink-mute">{STALE_GRANT_EXPLANATION}</p>
        <Button
          size="sm"
          className="mt-2.5"
          onClick={() => void api.openCommandCenter(undefined, "permissions")}
        >
          Open the Permissions tool
        </Button>
      </Callout>

      {update?.updateAvailable && (
        <Callout tone="info" title={`Caduceus ${update.latestVersion ?? ""} is available`}>
          <p className="text-[13px] leading-relaxed text-ink-soft">
            You are on {update.currentVersion}. Settings, shortcuts, clipboard history and AI setup
            are kept — only the app itself is replaced.
          </p>

          {update.homebrewManaged ? (
            <>
              <p className="mt-2 text-[13px] leading-relaxed text-ink-soft">
                This copy is managed by Homebrew, so Caduceus will not replace it itself — that
                would leave <code className="text-ink-mute">brew</code>&apos;s own records pointing
                at a version that is no longer on disk. Run this instead:
              </p>
              <pre className="mt-2.5 overflow-x-auto rounded-cad border border-line bg-raised/60 px-3 py-2 font-mono text-2xs leading-relaxed text-ink">
                {BREW_UPGRADE_COMMAND}
              </pre>
              <div className="row mt-3 flex-wrap gap-2">
                <Button tone="primary" size="sm" onClick={copyBrewCommand}>
                  Copy command
                </Button>
                {update.releaseUrl && (
                  <Button size="sm" onClick={() => void api.openExternalUrl(update.releaseUrl!)}>
                    Release notes
                  </Button>
                )}
              </div>
              {mode === "auto" && (
                <p className="mt-2.5 text-2xs leading-relaxed text-ink-faint">
                  Automatic mode will keep noticing this release and telling you about it, but will
                  never run <code className="text-ink-mute">brew</code> for you — that can prompt
                  for a password and take a while, which is not something to do without asking.
                </p>
              )}
            </>
          ) : (
            <>
              <pre className="mt-2.5 overflow-x-auto rounded-cad border border-line bg-raised/60 px-3 py-2 font-mono text-2xs leading-relaxed text-ink">
                {INSTALL_COMMAND}
              </pre>
              <div className="row mt-3 flex-wrap gap-2">
                <Button tone="primary" size="sm" disabled={starting} onClick={() => void runUpdate()}>
                  {starting ? "Terminal is running it…" : "Update now"}
                </Button>
                <Button
                  size="sm"
                  onClick={() => {
                    navigator.clipboard
                      .writeText(INSTALL_COMMAND)
                      .then(() => setNote("Command copied."))
                      .catch(() => setNote("Could not copy that."));
                  }}
                >
                  Copy command
                </Button>
                {update.releaseUrl && (
                  <Button size="sm" onClick={() => void api.openExternalUrl(update.releaseUrl!)}>
                    Release notes
                  </Button>
                )}
              </div>
              {update.downloadUrl && (
                <p className="mt-3 text-2xs leading-relaxed text-ink-faint">
                  Would rather do it by hand?{" "}
                  <button
                    type="button"
                    className="underline underline-offset-2 hover:text-ink-mute"
                    onClick={() => void api.openExternalUrl(update.downloadUrl!)}
                  >
                    Download the .dmg
                  </button>
                  .
                </p>
              )}
            </>
          )}
        </Callout>
      )}

      {note && <p className="mt-2.5 text-2xs leading-relaxed text-ink-mute">{note}</p>}
    </Section>
  );
}
