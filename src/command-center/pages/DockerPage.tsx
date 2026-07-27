/**
 * Manage → Docker: containers with start/stop/restart, running or not.
 *
 * Says plainly when Docker is missing or simply not running — an empty list
 * with no explanation reads as "you have no containers", which is a different
 * and wrong statement.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { Container } from "@/shared/types";
import { Button, Section, cx } from "@/shared/ui";

export function DockerPage({ active }: { active: boolean }) {
  const [containers, setContainers] = useState<Container[]>([]);
  const [unavailable, setUnavailable] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setContainers(await api.dockerContainers());
      setUnavailable(null);
    } catch (e) {
      setUnavailable(api.errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    const timer = setInterval(() => void refresh(), 4000);
    return () => clearInterval(timer);
  }, [refresh, active]);

  const act = async (container: Container, action: "start" | "stop" | "restart") => {
    setBusyId(container.id);
    try {
      const outcome = await api.dockerAction(container.id, action);
      setMessage(outcome.message);
      await refresh();
    } catch (e) {
      setMessage(api.errorMessage(e));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="mx-auto max-w-[640px] px-6 py-5">
      <Section
        title="Containers"
        description="Everything Docker knows about, running or stopped."
      >
        {message && <p className="mb-3 text-2xs text-ink-mute">{message}</p>}

        {unavailable ? (
          <p className="text-2xs text-ink-mute">{unavailable}</p>
        ) : containers.length === 0 ? (
          <p className="text-2xs text-ink-faint">No containers.</p>
        ) : (
          <ul className="space-y-1.5">
            {containers.map((container) => (
              <li
                key={container.id}
                className="flex items-center gap-3 rounded-lg border border-line bg-base/20 px-3 py-2"
              >
                <span
                  aria-hidden="true"
                  className={cx(
                    "h-2 w-2 shrink-0 rounded-full",
                    container.running ? "bg-positive" : "bg-ink-faint",
                  )}
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] text-ink">{container.name}</span>
                  <span className="block truncate text-2xs text-ink-faint">
                    {container.image} · {container.status}
                  </span>
                </span>
                <span className="row shrink-0 gap-1">
                  <Button
                    size="sm"
                    disabled={busyId === container.id}
                    onClick={() => void act(container, container.running ? "stop" : "start")}
                  >
                    {container.running ? "Stop" : "Start"}
                  </Button>
                  {container.running && (
                    <Button
                      size="sm"
                      disabled={busyId === container.id}
                      onClick={() => void act(container, "restart")}
                    >
                      Restart
                    </Button>
                  )}
                </span>
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}
