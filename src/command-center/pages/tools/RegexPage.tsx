/**
 * A regex tester: matches, capture groups and a plain-English explanation,
 * all at once.
 *
 * The three panels update from the same two inputs (pattern and sample text)
 * rather than needing a Run button, because a regex is normally built by
 * trial and error — nudge the pattern, glance at the matches, nudge it again.
 * Making that loop wait on a click is what sends people to a browser tab
 * instead. Everything runs through the `regex` crate on the Rust side, so
 * nothing typed here — API tokens, log lines, whatever the sample happens to
 * be — leaves the machine to get an answer.
 */

import { useEffect, useState } from "react";

import * as api from "@/shared/api";
import { Field, Section, TextArea, TextInput, Toggle } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

interface FlagSpec {
  key: string;
  label: string;
  hint: string;
}

/** Only the flags the `regex` crate's builder actually exposes — see
 * `tools::regex_tool::build_regex` on the Rust side. */
const FLAGS: FlagSpec[] = [
  { key: "i", label: "Case-insensitive", hint: "\"ABC\" matches \"abc\"." },
  { key: "m", label: "Multi-line", hint: "^ and $ match at the start and end of every line, not just the whole text." },
  { key: "s", label: "Dot matches newline", hint: "Without this, . stops at a line break." },
  { key: "x", label: "Extended", hint: "Unescaped whitespace and # comments in the pattern are ignored — for writing a long pattern readably." },
];

export function RegexPage({ onSetTitle }: ToolPageProps) {
  const [pattern, setPattern] = useState("");
  const [flags, setFlags] = useState<Set<string>>(new Set(["i"]));
  const [text, setText] = useState("");

  const [matches, setMatches] = useState<api.RegexMatch[] | null>(null);
  const [matchError, setMatchError] = useState<string | null>(null);

  const [tokens, setTokens] = useState<api.ExplainToken[] | null>(null);
  const [explainError, setExplainError] = useState<string | null>(null);

  useEffect(() => {
    onSetTitle?.("Regex tester");
  }, [onSetTitle]);

  const toggleFlag = (key: string) => {
    setFlags((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  // Re-runs on every keystroke — see the header comment on why a Run button
  // would be the wrong shape for this. The debounce is just enough to not
  // recompile the pattern on every single character of a fast typist.
  useEffect(() => {
    if (!pattern.trim()) {
      setMatches(null);
      setMatchError(null);
      setTokens(null);
      setExplainError(null);
      return;
    }
    const flagString = Array.from(flags).join("");
    const timer = setTimeout(() => {
      void api
        .regexExplain(pattern)
        .then((result) => {
          setTokens(result);
          setExplainError(null);
        })
        .catch((error) => {
          setTokens(null);
          setExplainError(api.errorMessage(error));
        });

      void api
        .regexTest(pattern, flagString, text)
        .then((result) => {
          setMatches(result);
          setMatchError(null);
        })
        .catch((error) => {
          setMatches(null);
          setMatchError(api.errorMessage(error));
        });
    }, 150);
    return () => clearTimeout(timer);
  }, [pattern, flags, text]);

  const copy = (value: string) => {
    void navigator.clipboard.writeText(value);
  };

  return (
    <div className="mx-auto h-full max-w-[820px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Regex tester</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Tests a pattern against sample text and explains it token by token. Runs locally through
          the same engine Caduceus's own developer tools use — nothing here is sent anywhere.
        </p>
      </div>

      <Section title="Pattern">
        <Field label="Regular expression">
          <TextInput value={pattern} onChange={setPattern} mono placeholder={String.raw`\b\w+@\w+\.\w+\b`} />
        </Field>
        <div className="mt-3 grid grid-cols-2 gap-x-6 gap-y-1">
          {FLAGS.map((flag) => (
            <Toggle
              key={flag.key}
              checked={flags.has(flag.key)}
              onChange={() => toggleFlag(flag.key)}
              label={flag.label}
              hint={flag.hint}
            />
          ))}
        </div>
        {matchError && <p className="mt-3 whitespace-pre-line text-2xs leading-relaxed text-danger">{matchError}</p>}
      </Section>

      <Section title="Sample text">
        <TextArea value={text} onChange={setText} rows={6} mono placeholder="Paste or type text to test the pattern against." />
      </Section>

      {tokens && tokens.length > 0 && (
        <Section title="What it means" description="Read left to right — each piece explains itself, plus whatever repeats it.">
          <div className="flex flex-col gap-1.5">
            {tokens.map((token, index) => (
              <div key={index} className="flex items-baseline gap-3 rounded-lg px-2 py-1">
                <code className="w-28 shrink-0 truncate rounded bg-raised px-1.5 py-0.5 text-right font-mono text-2xs text-accent">
                  {token.token}
                </code>
                <span className="min-w-0 text-2xs leading-relaxed text-ink-soft">{token.description}</span>
              </div>
            ))}
          </div>
        </Section>
      )}
      {explainError && !matchError && (
        <p className="mb-4 whitespace-pre-line text-2xs leading-relaxed text-danger">{explainError}</p>
      )}

      {matches && (
        <Section
          title="Matches"
          description={
            matches.length === 0
              ? "No matches in the sample text."
              : `${matches.length} match${matches.length === 1 ? "" : "es"}.`
          }
        >
          {matches.length > 0 && (
            <div className="flex flex-col gap-2">
              {matches.map((match, index) => (
                <button
                  key={index}
                  type="button"
                  onClick={() => copy(match.text)}
                  title="Copy this match"
                  className="rounded-lg border border-line bg-raised/40 px-3 py-2 text-left transition-colors hover:bg-raised/70"
                >
                  <div className="row justify-between gap-2">
                    <code className="min-w-0 truncate font-mono text-2xs text-ink">
                      {match.text || <span className="italic text-ink-faint">(empty match)</span>}
                    </code>
                    <span className="shrink-0 text-2xs text-ink-faint">
                      {match.start}–{match.end}
                    </span>
                  </div>
                  {match.groups.length > 0 && (
                    <div className="mt-1.5 flex flex-col gap-0.5 border-t border-line/60 pt-1.5">
                      {match.groups.map((group) => (
                        <div key={group.index} className="row gap-2 text-2xs">
                          <span className="w-20 shrink-0 text-ink-faint">
                            {group.name ? `$${group.index} (${group.name})` : `$${group.index}`}
                          </span>
                          <code className="min-w-0 truncate font-mono text-ink-soft">
                            {group.text ?? <span className="italic text-ink-faint">did not match</span>}
                          </code>
                        </div>
                      ))}
                    </div>
                  )}
                </button>
              ))}
            </div>
          )}
        </Section>
      )}

      {!pattern.trim() && (
        <p className="text-2xs text-ink-faint">Type a pattern above to see matches and an explanation.</p>
      )}
    </div>
  );
}
