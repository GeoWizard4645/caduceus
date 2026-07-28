/**
 * Git status plus an AI-drafted commit message — read-only end to end.
 *
 * Every git call `tools::devextra::git_commit_assist` makes is `status`,
 * `diff`, or `rev-parse`; nothing here ever stages or commits. Drafting the
 * message is the whole feature — you still read it, edit it if you want to,
 * and press commit yourself.
 */

import { useEffect, useState } from "react";

import { open } from "@tauri-apps/plugin-dialog";

import * as api from "@/shared/api";
import { Button, Callout, Field, Section, Spinner, TextInput } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

export function GitCommitPage({ onSetTitle }: ToolPageProps) {
  const [repoPath, setRepoPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<api.GitCommitAssist | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    onSetTitle("Git commit assist");
  }, [onSetTitle]);

  const pickFolder = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setRepoPath(picked);
  };

  const analyze = async () => {
    const path = repoPath.trim();
    if (!path) return;
    setBusy(true);
    setError(null);
    setResult(null);
    setCopied(false);
    try {
      setResult(await api.gitCommitAssist(path));
    } catch (err) {
      setError(api.errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const copyMessage = () => {
    if (!result?.suggestedMessage) return;
    navigator.clipboard
      .writeText(result.suggestedMessage)
      .then(() => setCopied(true))
      .catch(() => setCopied(false));
  };

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Git commit assist</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Reads a repository's status and diff and drafts a commit message through whichever AI
          backend is configured. Read-only: nothing here stages or commits — you still do that
          yourself.
        </p>
      </div>

      <Section title="Repository">
        <Field label="Repository path">
          <div className="row gap-2">
            <TextInput value={repoPath} onChange={setRepoPath} mono placeholder="/path/to/repo" />
            <Button size="sm" onClick={() => void pickFolder()}>
              Choose…
            </Button>
          </div>
        </Field>
        <div className="mt-3 row gap-2">
          <Button tone="primary" onClick={() => void analyze()} disabled={busy || !repoPath.trim()}>
            {busy ? "Reading…" : "Analyze"}
          </Button>
          {busy && <Spinner className="text-accent" />}
        </div>
        {error && (
          <p className="mt-3 whitespace-pre-line text-2xs leading-relaxed text-danger">{error}</p>
        )}
      </Section>

      {result && (
        <>
          {result.branch && (
            <p className="mb-3 text-2xs text-ink-faint">
              Branch <span className="font-mono text-ink-soft">{result.branch}</span>
            </p>
          )}

          {result.error && (
            <div className="mb-5">
              <Callout tone={result.ok ? "info" : "danger"}>{result.error}</Callout>
            </div>
          )}

          {(result.staged.length > 0 || result.unstaged.length > 0) && (
            <Section title="Changes">
              {result.staged.length > 0 && <FileList title="Staged" files={result.staged} />}
              {result.unstaged.length > 0 && <FileList title="Not staged" files={result.unstaged} />}
            </Section>
          )}

          {result.suggestedMessage && (
            <Section
              title="Suggested message"
              actions={
                <Button size="sm" onClick={copyMessage}>
                  {copied ? "Copied" : "Copy"}
                </Button>
              }
            >
              <pre className="whitespace-pre-wrap break-words font-mono text-[13px] leading-relaxed text-ink">
                {result.suggestedMessage}
              </pre>
            </Section>
          )}
        </>
      )}
    </div>
  );
}

function FileList({ title, files }: { title: string; files: api.GitFileChange[] }) {
  return (
    <div className="mb-3 last:mb-0">
      <p className="mb-1.5 text-2xs uppercase tracking-[0.08em] text-ink-faint">{title}</p>
      <ul className="flex flex-col gap-0.5">
        {files.map((file) => (
          <li
            key={file.path}
            className="row justify-between gap-3 rounded px-2 py-1 text-2xs odd:bg-raised/40"
          >
            <span className="min-w-0 truncate font-mono text-ink">{file.path}</span>
            <span className="shrink-0 text-ink-faint">{file.status}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
