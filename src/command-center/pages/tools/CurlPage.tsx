/**
 * A cURL / HTTP request playground: paste a `curl ...` command — including
 * whatever a browser's "Copy as cURL" produces — and see it parsed, shown
 * back, and replayed for real.
 *
 * Parsing and sending are the same round trip on the Rust side
 * (`tools::devextra::execute`), but the response panel makes the two halves
 * visible separately: what was understood from the pasted text, then what
 * actually came back over the network. `-k`/`--insecure` is recorded on the
 * parsed request but never honoured — TLS is always verified regardless of
 * what the pasted command asked for, so a snippet copied from a support
 * ticket cannot quietly turn verification off.
 */

import { useEffect, useState } from "react";

import * as api from "@/shared/api";
import { Button, Callout, Field, Section, Spinner, TextArea } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

const PLACEHOLDER = `curl 'https://api.example.com/v1/things' \\\n  -H 'accept: application/json' \\\n  -d '{"a":1}'`;

export function CurlPage({ onSetTitle }: ToolPageProps) {
  const [command, setCommand] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<api.HttpPlaygroundResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    onSetTitle("cURL playground");
  }, [onSetTitle]);

  const send = async () => {
    if (!command.trim()) return;
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      setResult(await api.runCurl(command));
    } catch (err) {
      setError(api.errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const copy = (text: string) => {
    void navigator.clipboard.writeText(text);
  };

  return (
    <div className="mx-auto h-full max-w-[820px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">cURL playground</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Paste a curl command and it is parsed, shown back, and replayed for real over the
          network. Nothing here goes anywhere except the address named in the command.
        </p>
      </div>

      <Section title="Command">
        <Field label="curl command">
          <TextArea value={command} onChange={setCommand} rows={6} mono placeholder={PLACEHOLDER} />
        </Field>
        <div className="mt-3 row gap-2">
          <Button tone="primary" onClick={() => void send()} disabled={busy || !command.trim()}>
            {busy ? "Sending…" : "Send"}
          </Button>
          {busy && <Spinner className="text-accent" />}
        </div>
        {error && (
          <p className="mt-3 whitespace-pre-line text-2xs leading-relaxed text-danger">{error}</p>
        )}
      </Section>

      {result && (
        <>
          <Section title="Parsed request">
            <dl className="flex flex-col gap-1.5 text-2xs">
              <Row label="Method" value={result.request.method} />
              <Row label="URL" value={result.request.url} mono />
              {result.request.headers.map(([k, v]) => (
                <Row key={`h:${k}:${v}`} label={k} value={v} mono />
              ))}
              {result.request.basicAuth && (
                <Row label="Basic auth" value={`${result.request.basicAuth[0]} : ••••••`} mono />
              )}
              {result.request.body && <Row label="Body" value={result.request.body} mono />}
            </dl>
            <div className="mt-2 flex flex-wrap gap-3 text-2xs text-ink-faint">
              {result.request.followRedirects && <span>Follows redirects</span>}
              {result.request.compressed && <span>Requests compression</span>}
              {result.request.insecure && (
                <span className="text-caution">
                  --insecure was in the command but is not honoured — TLS is still verified.
                </span>
              )}
            </div>
            {result.request.ignoredFlags.length > 0 && (
              <p className="mt-2 text-2xs text-ink-faint">
                Ignored: {result.request.ignoredFlags.join(", ")}
              </p>
            )}
          </Section>

          <Section
            title="Response"
            description={
              result.error ? undefined : `${result.status ?? "?"} ${result.statusText ?? ""}`.trim()
            }
            actions={
              !result.error ? (
                <Button size="sm" tone="ghost" onClick={() => copy(result.body)}>
                  Copy body
                </Button>
              ) : undefined
            }
          >
            {result.error ? (
              <Callout tone="danger">{result.error}</Callout>
            ) : (
              <>
                {result.headers.length > 0 && (
                  <div className="mb-3 flex flex-col gap-1 text-2xs text-ink-faint">
                    {result.headers.map(([k, v]) => (
                      <div key={`rh:${k}:${v}`} className="row gap-2">
                        <span className="w-40 shrink-0 truncate text-ink-soft">{k}</span>
                        <span className="min-w-0 truncate font-mono">{v}</span>
                      </div>
                    ))}
                  </div>
                )}
                {result.bodyTruncated && (
                  <p className="mb-2 text-2xs text-caution">
                    Body truncated — the response was larger than what is shown.
                  </p>
                )}
                <pre className="max-h-[420px] overflow-auto whitespace-pre-wrap break-words rounded-lg border border-line bg-raised/40 p-3 font-mono text-2xs leading-relaxed text-ink-soft">
                  {result.body || <span className="italic text-ink-faint">(empty body)</span>}
                </pre>
              </>
            )}
          </Section>
        </>
      )}
    </div>
  );
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="row gap-2">
      <dt className="w-28 shrink-0 text-ink-faint">{label}</dt>
      <dd className={mono ? "min-w-0 flex-1 truncate font-mono text-ink" : "min-w-0 flex-1 truncate text-ink"}>
        {value}
      </dd>
    </div>
  );
}
