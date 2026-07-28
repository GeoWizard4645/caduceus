/**
 * A dependency pin-tightness inspector: reads a `package.json`, `Cargo.toml`,
 * or `requirements.txt` and says how loosely each dependency is pinned.
 *
 * No network lookups happen on the Rust side (`tools::devextra::inspect_dependencies`)
 * — no freshness or vulnerability data, only what the manifest itself says.
 */

import { useEffect, useState } from "react";

import { open } from "@tauri-apps/plugin-dialog";

import * as api from "@/shared/api";
import { Button, Field, Section, Spinner, TextInput, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

const PIN_LABEL: Record<api.PinKind, string> = {
  exact: "Exact",
  range: "Range",
  unpinned: "Unpinned",
  other: "Other",
};

const PIN_TONE: Record<api.PinKind, string> = {
  exact: "text-positive",
  range: "text-caution",
  unpinned: "text-danger",
  other: "text-ink-faint",
};

export function DependenciesPage({ onSetTitle }: ToolPageProps) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<api.DependencyReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    onSetTitle("Dependency inspector");
  }, [onSetTitle]);

  const pickFile = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Manifest", extensions: ["json", "toml", "txt"] }],
    });
    if (typeof picked === "string") setPath(picked);
  };

  const inspect = async () => {
    const manifestPath = path.trim();
    if (!manifestPath) return;
    setBusy(true);
    setError(null);
    setReport(null);
    try {
      setReport(await api.inspectDependencies(manifestPath));
    } catch (err) {
      setError(api.errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const groups = report ? Array.from(new Set(report.entries.map((e) => e.group))) : [];

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">
          Dependency inspector
        </h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Reads a package.json, Cargo.toml, or requirements.txt and shows how tightly each
          dependency is pinned. Nothing is looked up over the network — only what the manifest
          itself says.
        </p>
      </div>

      <Section title="Manifest">
        <Field label="Manifest path" hint="package.json, Cargo.toml, or requirements.txt">
          <div className="row gap-2">
            <TextInput value={path} onChange={setPath} mono placeholder="/path/to/package.json" />
            <Button size="sm" onClick={() => void pickFile()}>
              Choose…
            </Button>
          </div>
        </Field>
        <div className="mt-3 row gap-2">
          <Button tone="primary" onClick={() => void inspect()} disabled={busy || !path.trim()}>
            {busy ? "Reading…" : "Inspect"}
          </Button>
          {busy && <Spinner className="text-accent" />}
        </div>
        {error && (
          <p className="mt-3 whitespace-pre-line text-2xs leading-relaxed text-danger">{error}</p>
        )}
      </Section>

      {report && (
        <Section
          title={report.manifest}
          description={`${report.entries.length} ${report.entries.length === 1 ? "dependency" : "dependencies"} · ${report.exactCount} exact · ${report.looseCount} loose`}
        >
          {groups.map((group) => (
            <div key={group} className="mb-3 last:mb-0">
              <p className="mb-1.5 text-2xs uppercase tracking-[0.08em] text-ink-faint">{group}</p>
              <ul className="flex flex-col gap-0.5">
                {report.entries
                  .filter((entry) => entry.group === group)
                  .map((entry) => (
                    <li
                      key={`${entry.group}:${entry.name}`}
                      className="row justify-between gap-3 rounded px-2 py-1 text-2xs odd:bg-raised/40"
                    >
                      <span className="min-w-0 truncate font-mono text-ink">{entry.name}</span>
                      <span className="min-w-0 flex-1 truncate text-ink-faint">
                        {entry.version || "—"}
                      </span>
                      <span className={cx("shrink-0 font-medium", PIN_TONE[entry.pin])}>
                        {PIN_LABEL[entry.pin]}
                      </span>
                    </li>
                  ))}
              </ul>
            </div>
          ))}
          {report.entries.length === 0 && (
            <p className="text-2xs text-ink-faint">No dependencies found in this manifest.</p>
          )}
        </Section>
      )}
    </div>
  );
}
