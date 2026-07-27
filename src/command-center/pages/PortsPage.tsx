/**
 * Manage → Ports: what is listening, and a button to stop it.
 *
 * The page equivalent of typing `port` into the palette, for when you want the
 * list to stay open while you restart the thing that was squatting on 3000.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { PortUser } from "@/shared/types";
import { Button, Section } from "@/shared/ui";

export function PortsPage({ active }: { active: boolean }) {
  const [ports, setPorts] = useState<PortUser[]>([]);
  const [filter, setFilter] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [pendingPid, setPendingPid] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      setPorts(await api.listeningPorts());
    } catch (e) {
      setMessage(api.errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    const timer = setInterval(() => void refresh(), 3000);
    return () => clearInterval(timer);
  }, [refresh, active]);

  const stop = async (entry: PortUser) => {
    // First click arms, second click fires — the same two-step the palette
    // uses, because SIGTERM to the wrong dev server loses unsaved state.
    if (pendingPid !== entry.pid) {
      setPendingPid(entry.pid);
      return;
    }
    setPendingPid(null);
    try {
      const outcome = await api.freePort(entry.port);
      setMessage(outcome.message);
      await refresh();
    } catch (e) {
      setMessage(api.errorMessage(e));
    }
  };

  const query = filter.trim();
  const shown = query
    ? ports.filter(
        (entry) =>
          String(entry.port).includes(query) ||
          entry.process.toLowerCase().includes(query.toLowerCase()),
      )
    : ports;

  return (
    <div className="mx-auto max-w-[640px] px-6 py-5">
      <Section
        title="Listening ports"
        description="Every process holding a TCP port open. Stopping sends SIGTERM, so a dev server flushes its logs and removes its socket rather than being killed outright."
      >
        <input
          type="search"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          placeholder="Filter by port or process…"
          className="mb-3 w-full rounded-lg border border-line bg-base/40 px-3 py-2 text-[13px] text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
        />

        {message && <p className="mb-3 text-2xs text-ink-mute">{message}</p>}

        {shown.length === 0 ? (
          <p className="text-2xs text-ink-faint">
            {query ? "Nothing matches that." : "Nothing is listening."}
          </p>
        ) : (
          <ul className="space-y-1.5">
            {shown.map((entry) => (
              <li
                key={`${entry.port}:${entry.pid}`}
                className="flex items-center gap-3 rounded-lg border border-line bg-base/20 px-3 py-2"
              >
                <span className="w-16 shrink-0 text-[13px] font-semibold tabular-nums text-ink">
                  {entry.port}
                </span>
                <span className="min-w-0 flex-1 truncate text-[13px] text-ink-soft">
                  {entry.process}
                  <span className="ml-2 text-2xs text-ink-faint">pid {entry.pid}</span>
                </span>
                <Button size="sm" onClick={() => void stop(entry)}>
                  {pendingPid === entry.pid ? "Really stop?" : "Stop"}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}
