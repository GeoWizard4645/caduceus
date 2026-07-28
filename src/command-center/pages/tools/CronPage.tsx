/**
 * A cron expression parser and visualiser: what it means in plain English,
 * and the next several times it would actually fire.
 *
 * Cron's five fields read backwards from how anyone thinks about a schedule —
 * "the 15th minute of every hour" is easier to say than to parse out of
 * `15 * * * *` at a glance, and the classic day-of-month/day-of-week quirk
 * (see the Rust side) trips people up in the opposite direction, making a
 * schedule look stricter than it runs. Both are answered here without having
 * to wait for the job to actually fire to find out.
 */

import { useEffect, useState } from "react";

import * as api from "@/shared/api";
import { Callout, Field, Section, TextInput } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

interface Preset {
  label: string;
  expression: string;
}

const PRESETS: Preset[] = [
  { label: "Every minute", expression: "* * * * *" },
  { label: "Every 15 minutes", expression: "*/15 * * * *" },
  { label: "Every hour", expression: "0 * * * *" },
  { label: "Every day at 9am", expression: "0 9 * * *" },
  { label: "Weekdays at 9am", expression: "0 9 * * 1-5" },
  { label: "Midnight on the 1st", expression: "0 0 1 * *" },
];

/** Local-time, no year — the schedule repeats, so the year is noise until it
 * flips over, and the machine's own clock already agrees on the zone. */
const RUN_FORMAT = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});

export function CronPage({ onSetTitle }: ToolPageProps) {
  const [expression, setExpression] = useState("*/15 * * * 1-5");
  const [analysis, setAnalysis] = useState<api.CronAnalysis | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    onSetTitle?.("Cron parser");
  }, [onSetTitle]);

  // Live, like the regex tester — a cron expression is also normally built by
  // trial and error, and the whole point of the next-runs list is to catch a
  // mistake ("that's every day, not every weekday") before it ships.
  useEffect(() => {
    if (!expression.trim()) {
      setAnalysis(null);
      setError(null);
      return;
    }
    const timer = setTimeout(() => {
      void api
        .parseCron(expression, 10)
        .then((result) => {
          setAnalysis(result);
          setError(null);
        })
        .catch((err) => {
          setAnalysis(null);
          setError(api.errorMessage(err));
        });
    }, 150);
    return () => clearTimeout(timer);
  }, [expression]);

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Cron parser</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Reads the standard 5-field crontab syntax — minute, hour, day of month, month, day of
          week — and says what it means and when it next runs, in this Mac's own time zone.
        </p>
      </div>

      <Section title="Expression">
        <Field label="Cron expression" hint="minute · hour · day of month · month · day of week">
          <TextInput value={expression} onChange={setExpression} mono placeholder="*/15 * * * 1-5" />
        </Field>
        <div className="mt-3 flex flex-wrap gap-1.5">
          {PRESETS.map((preset) => (
            <button
              key={preset.expression}
              type="button"
              onClick={() => setExpression(preset.expression)}
              className="rounded-full border border-line-strong/60 bg-raised px-2.5 py-1 text-2xs text-ink-soft transition-colors hover:bg-overlay"
            >
              {preset.label}
            </button>
          ))}
        </div>
        {error && <p className="mt-3 whitespace-pre-line text-2xs leading-relaxed text-danger">{error}</p>}
      </Section>

      {analysis && (
        <>
          <Section title="What it means">
            <Callout tone="info">{analysis.description}</Callout>
          </Section>

          <Section
            title="Next runs"
            description={
              analysis.nextRuns.length === 0
                ? "This expression does not fire within the next several years — it likely names a calendar day that never occurs, such as day 31 of a month that never has one."
                : undefined
            }
          >
            {analysis.nextRuns.length > 0 && (
              <ol className="flex flex-col gap-1">
                {analysis.nextRuns.map((run, index) => (
                  <li
                    key={run}
                    className="row justify-between gap-3 rounded-lg px-2.5 py-1.5 text-[13px] text-ink-soft odd:bg-raised/40"
                  >
                    <span className="w-6 shrink-0 text-2xs text-ink-faint">{index + 1}</span>
                    <span className="min-w-0 flex-1 font-mono text-2xs text-ink">
                      {RUN_FORMAT.format(new Date(run))}
                    </span>
                  </li>
                ))}
              </ol>
            )}
          </Section>
        </>
      )}
    </div>
  );
}
