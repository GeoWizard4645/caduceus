/**
 * "There is a newer version" — and the button that installs it.
 *
 * Updating runs the same one-liner the website hands out, in Terminal, rather
 * than downloading a disk image for you to drag across. Two reasons, and the
 * second is the one that decided it:
 *
 * * It is the same path a fresh install takes, so there is one update mechanism
 *   to keep working rather than two that can drift.
 * * Caduceus is not notarised. An app that asks you to trust a script piped into
 *   a shell should not then replace itself invisibly — the command is printed
 *   here before you press anything, and you watch it run.
 *
 * The `.dmg` is still linked for anyone who would rather do it by hand.
 */

import { useState } from "react";

import * as api from "@/shared/api";
import { INSTALL_SCRIPT_URL } from "@/shared/docsUrls";
import { useUpdateCheck } from "@/shared/hooks";
import { Button, Callout, Section } from "@/shared/ui";

/** Shown, never executed from here. Mirrors `update::INSTALL_COMMAND`. */
const INSTALL_COMMAND = `curl -fsSL ${INSTALL_SCRIPT_URL} | bash`;

export function UpdateSection() {
  const update = useUpdateCheck(true);
  const [starting, setStarting] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  if (!update?.updateAvailable) {
    return null;
  }

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

  return (
    <Section
      title="Update available"
      description={`Caduceus ${update.latestVersion ?? ""} is out — you are on ${update.currentVersion}.`}
    >
      <Callout tone="info">
        <p className="text-[13px] leading-relaxed text-ink-soft">
          Updating runs the same command the website gives you, in Terminal. Your settings,
          shortcuts, clipboard history and AI setup are kept — only the app itself is replaced.
        </p>
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

        {note && <p className="mt-2.5 text-2xs leading-relaxed text-ink-mute">{note}</p>}

        <p className="mt-3 text-2xs leading-relaxed text-ink-faint">
          Installed with Homebrew? Use{" "}
          <code className="text-ink-mute">brew upgrade --cask caduceus</code> instead.
          {update.downloadUrl && (
            <>
              {" "}
              Would rather do it by hand?{" "}
              <button
                type="button"
                className="underline underline-offset-2 hover:text-ink-mute"
                onClick={() => void api.openExternalUrl(update.downloadUrl!)}
              >
                Download the .dmg
              </button>
              .
            </>
          )}
        </p>
      </Callout>
    </Section>
  );
}
