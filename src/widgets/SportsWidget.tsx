/**
 * A floating widget that follows one league's scoreboard — or, for F1, the
 * current race weekend's session results. See `marketApi.ts` for the
 * `sports:<league>[:<team>]` kind encoding this reads its config from, and
 * `MarketWidget.tsx`'s module docs for the lazy-fetch contract this widget
 * follows identically: every request happens inside the polling `useEffect`
 * below, started on mount and stopped on unmount, nothing at module scope.
 */

import { useEffect, useMemo, useState } from "react";

import { cx } from "@/shared/ui";

import { PixelText } from "./PixelDigits";
import {
  DEFAULT_SPORTS_KIND,
  SPORTS_POLL_MS,
  fetchF1,
  fetchScoreboard,
  formatAge,
  parseSportsKind,
  useOnlineStatus,
  type GameEvent,
  type RaceSession,
  type RaceWeekend,
  type SportsWidgetConfig,
} from "./marketApi";

type SportsSelection =
  | { kind: "team"; game: GameEvent }
  | { kind: "f1"; weekend: RaceWeekend; session: RaceSession | null }
  | { kind: "none" };

async function loadSelection(config: SportsWidgetConfig): Promise<SportsSelection> {
  if (config.league === "f1") {
    const board = await fetchF1();
    const weekend = board.weekends[0];
    if (!weekend) return { kind: "none" };
    // Prefer the race itself; fall back to whichever session is furthest
    // along (the last one ESPN lists) so a Friday practice day still shows
    // something instead of nothing.
    const session = weekend.sessions.find((s) => s.session === "Race") ?? weekend.sessions.at(-1) ?? null;
    return { kind: "f1", weekend, session };
  }

  const board = await fetchScoreboard(config.league);
  let events = board.events;
  if (config.team) {
    events = events.filter((e) => e.competitors.some((c) => c.abbreviation.toUpperCase() === config.team));
  }
  if (events.length === 0) return { kind: "none" };

  // Prefer a live game; otherwise the soonest upcoming one; otherwise the
  // most recently finished one — the order someone glancing at a floating
  // widget would actually want, not just "first in the response".
  const live = events.find((e) => e.status.state === "in");
  const upcoming = [...events]
    .filter((e) => e.status.state === "pre")
    .sort((a, b) => a.date.localeCompare(b.date))[0];
  const finished = [...events]
    .filter((e) => e.status.state === "post")
    .sort((a, b) => b.date.localeCompare(a.date))[0];
  return { kind: "team", game: live ?? upcoming ?? finished ?? events[0] };
}

/** Which team is ahead, if that concept even applies yet. `null` before
 * kickoff (0–0 tells you nothing) and whenever scores are tied. */
function leaderAbbreviation(game: GameEvent): string | null {
  if (game.competitors.length < 2) return null;
  const explicitWinner = game.competitors.find((c) => c.winner === true);
  if (explicitWinner) return explicitWinner.abbreviation;
  if (game.status.state !== "in") return null;
  const [a, b] = game.competitors;
  const scoreA = Number(a.score);
  const scoreB = Number(b.score);
  if (!Number.isFinite(scoreA) || !Number.isFinite(scoreB) || scoreA === scoreB) return null;
  return scoreA > scoreB ? a.abbreviation : b.abbreviation;
}

function statusLabel(status: GameEvent["status"]): string {
  // ESPN's own `detail` is already human-written ("Final", "Q3 4:12", "Fri
  // 8:00 PM EDT") — reusing it beats a hand-rolled formatter that has to
  // guess at timezones and OT/shootout conventions per sport.
  return status.detail || (status.state === "in" ? "LIVE" : status.state === "post" ? "Final" : "Scheduled");
}

function TeamGameView({ game }: { game: GameEvent }) {
  const leader = leaderAbbreviation(game);
  const away = game.competitors.find((c) => c.homeAway === "away") ?? game.competitors[0];
  const home = game.competitors.find((c) => c.homeAway === "home") ?? game.competitors[1];
  const teams = [away, home].filter(Boolean) as GameEvent["competitors"];

  return (
    <div className="flex flex-1 flex-col justify-center gap-1 px-1">
      <div className="flex items-center justify-center gap-1 text-center text-[9px] font-semibold uppercase tracking-wide text-ink-faint">
        {game.status.state === "in" && (
          <span aria-hidden="true" className="h-1.5 w-1.5 animate-pulse rounded-full bg-danger" />
        )}
        <span className="truncate">{statusLabel(game.status)}</span>
      </div>
      {teams.map((team) => {
        const isLeader = leader != null && team.abbreviation === leader;
        return (
          <div key={team.abbreviation || team.team} className="flex items-center justify-between">
            <span
              className={cx(
                "truncate text-2xs font-semibold",
                isLeader ? "text-ink" : "text-ink-mute",
              )}
              title={team.team}
            >
              {team.abbreviation || team.team}
            </span>
            <span className="flex items-center gap-1">
              {/* The arrow, not just colour, is what says "ahead" — see
                  MarketWidget's DeltaArrow for the same rule applied to
                  price direction. */}
              {isLeader && (
                <span aria-hidden="true" className="text-[9px] text-positive">
                  ▲
                </span>
              )}
              <PixelText
                text={/^\d+$/.test(team.score) ? team.score : "0"}
                cell={3}
                color={isLeader ? "rgb(var(--c-positive))" : "rgb(var(--c-ink))"}
              />
            </span>
          </div>
        );
      })}
    </div>
  );
}

function F1View({ weekend, session }: { weekend: RaceWeekend; session: RaceSession | null }) {
  const top = session?.top.slice(0, 3) ?? [];
  return (
    <div className="flex flex-1 flex-col justify-center gap-0.5 px-1">
      <div className="flex items-center justify-between gap-1">
        <span className="truncate text-2xs font-semibold text-ink">{weekend.name}</span>
        <span className="shrink-0 text-[9px] font-semibold uppercase tracking-wide text-ink-faint">
          {session ? (session.completed ? session.session : `${session.session} · LIVE`) : "Upcoming"}
        </span>
      </div>
      {top.length === 0 && <div className="py-2 text-center text-2xs text-ink-faint">No results yet</div>}
      {top.map((d) => (
        <div key={d.position} className="flex items-center justify-between gap-1">
          <span className="flex min-w-0 items-center gap-1">
            <span className="shrink-0 text-[9px] tabular-nums text-ink-faint">P{d.position}</span>
            <span className="truncate text-2xs text-ink-mute">{d.driver}</span>
          </span>
          {d.winner && (
            <span aria-hidden="true" className="shrink-0 text-[9px] text-positive">
              ▲
            </span>
          )}
        </div>
      ))}
    </div>
  );
}

function EmptyView({ config }: { config: SportsWidgetConfig }) {
  const message =
    config.league === "f1"
      ? "No race weekend scheduled."
      : config.team
        ? `No ${config.team} game right now.`
        : "No games right now.";
  return <div className="flex flex-1 items-center justify-center px-1 text-center text-2xs text-ink-faint">{message}</div>;
}

function StatusPill({ tone, label }: { tone: "danger" | "caution" | "muted"; label: string }) {
  const dot = tone === "danger" ? "bg-danger" : tone === "caution" ? "bg-caution" : "bg-ink-faint";
  const text = tone === "danger" ? "text-danger" : tone === "caution" ? "text-caution" : "text-ink-faint";
  return (
    <span className={cx("flex items-center gap-1 text-[9px] font-semibold uppercase tracking-wide", text)}>
      <span aria-hidden="true" className={cx("h-1.5 w-1.5 rounded-full", dot)} />
      {label}
    </span>
  );
}

function FreshnessBadge({
  hasSelection,
  lastError,
  online,
  fetchedAt,
  now,
}: {
  hasSelection: boolean;
  lastError: string | null;
  online: boolean;
  fetchedAt: number | null;
  now: number;
}) {
  if (!online) return <StatusPill tone="danger" label="OFFLINE" />;
  if (lastError) {
    const rateLimited = /rate.?limit/i.test(lastError);
    return <StatusPill tone={rateLimited ? "caution" : "danger"} label={rateLimited ? "RATE LIMITED" : "OFFLINE"} />;
  }
  if (!hasSelection || fetchedAt == null) return <StatusPill tone="muted" label="LOADING" />;
  return <span className="text-[9px] text-ink-faint">{formatAge(now - fetchedAt)} ago</span>;
}

function leagueLabel(config: SportsWidgetConfig): string {
  switch (config.league) {
    case "nfl":
      return config.team ? `NFL · ${config.team}` : "NFL";
    case "nba":
      return config.team ? `NBA · ${config.team}` : "NBA";
    case "mlb":
      return config.team ? `MLB · ${config.team}` : "MLB";
    case "worldcup":
      return config.team ? `World Cup · ${config.team}` : "World Cup";
    case "f1":
      return "Formula 1";
  }
}

export function SportsWidget({ kind }: { kind: string }) {
  const config = useMemo(() => parseSportsKind(kind) ?? parseSportsKind(DEFAULT_SPORTS_KIND)!, [kind]);
  const online = useOnlineStatus();

  const [selection, setSelection] = useState<SportsSelection>({ kind: "none" });
  const [hasLoadedOnce, setHasLoadedOnce] = useState(false);
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    async function tick() {
      if (cancelled) return;
      if (!navigator.onLine) {
        setLastError("Offline.");
        schedule();
        return;
      }
      try {
        const next = await loadSelection(config);
        if (cancelled) return;
        setSelection(next);
        setFetchedAt(Date.now());
        setLastError(null);
        setHasLoadedOnce(true);
      } catch (e) {
        if (cancelled) return;
        // Same rule as MarketWidget: the previous selection stays on screen,
        // dimmed and timestamped by FreshnessBadge, rather than being wiped
        // for a transient failure — but never re-labelled as fresh.
        setLastError(e instanceof Error ? e.message : String(e));
      }
      schedule();
    }
    function schedule() {
      if (cancelled) return;
      timer = setTimeout(() => void tick(), SPORTS_POLL_MS);
    }

    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [config]);

  const degraded = !online || lastError != null;

  return (
    <div className="flex h-full w-full flex-col gap-1">
      <div className="flex items-center justify-between gap-1">
        <span className="truncate text-[9px] font-semibold uppercase tracking-[0.08em] text-ink-faint">
          {leagueLabel(config)}
        </span>
        <FreshnessBadge
          hasSelection={selection.kind !== "none"}
          lastError={lastError}
          online={online}
          fetchedAt={fetchedAt}
          now={now}
        />
      </div>

      {!hasLoadedOnce && !degraded && (
        // See MarketWidget.tsx: PixelText has no "-" glyph, so a dash
        // placeholder would silently render as blank cells. Plain text says
        // "nothing yet" without looking broken.
        <div className="flex flex-1 items-center justify-center text-2xs text-ink-faint">Loading…</div>
      )}

      {!hasLoadedOnce && degraded && (
        <div className="flex flex-1 items-center justify-center px-1 text-center text-2xs text-ink-faint">
          {online ? (lastError ?? "Could not load.") : "You're offline."}
        </div>
      )}

      {hasLoadedOnce && (
        <div className={cx("flex flex-1 flex-col", degraded && "opacity-50 grayscale")}>
          {selection.kind === "none" && <EmptyView config={config} />}
          {selection.kind === "team" && <TeamGameView game={selection.game} />}
          {selection.kind === "f1" && <F1View weekend={selection.weekend} session={selection.session} />}
        </div>
      )}
    </div>
  );
}
