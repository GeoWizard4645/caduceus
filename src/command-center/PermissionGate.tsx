/**
 * Blocks a tool until macOS privacy grants it needs are in place.
 *
 * Opens the system consent dialog and Settings automatically, polls detectable
 * grants, and uses a walkthrough overlay so the user is not left reading a
 * passive warning on the page underneath.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import * as api from "@/shared/api";
import { PERMISSIONS } from "@/shared/permissions";
import { rememberResume, type PermissionId, type Tab } from "@/shared/tabs";
import { Button, Spinner, cx } from "@/shared/ui";

const POLL_MS = 1200;

const PermissionGateContext = createContext<(id: PermissionId) => void>(() => {});

/** Report a permission wall from an action that failed at runtime. */
export function usePermissionGate() {
  return useContext(PermissionGateContext);
}

async function readGranted(id: PermissionId): Promise<boolean | null> {
  if (id === "accessibility") {
    try {
      return await api.windowPermission();
    } catch {
      return null;
    }
  }
  if (id === "screen-recording") {
    try {
      const report = await api.systemPermissions();
      return report.screenRecording;
    } catch {
      return null;
    }
  }
  return null;
}

function sessionAckKey(id: PermissionId, scope: string) {
  return `caduceus:perm-ack:${scope}:${id}`;
}

export function PermissionGate({
  active,
  permissions,
  scope,
  retryCommandId,
  onOpenTab,
  children,
}: {
  active: boolean;
  permissions: PermissionId[];
  /** Distinguishes session acks per command or page. */
  scope: string;
  retryCommandId?: string;
  onOpenTab: (request: Omit<Tab, "id">) => void;
  children: ReactNode;
}) {
  const [blocking, setBlocking] = useState<PermissionId | null>(null);
  const [granted, setGranted] = useState<boolean | null>(null);
  const [opening, setOpening] = useState(false);
  const [repairing, setRepairing] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const prompted = useRef<string | null>(null);
  const wasScreenGranted = useRef<boolean | null>(null);

  const unique = useMemo(
    () => [...new Set(permissions.filter((p) => p in PERMISSIONS))],
    [permissions],
  );

  const evaluate = useCallback(async () => {
    for (const id of unique) {
      const state = await readGranted(id);
      if (state === true) continue;
      if (state === false) return id;
      if (typeof sessionStorage !== "undefined") {
        if (sessionStorage.getItem(sessionAckKey(id, scope))) continue;
      }
      return id;
    }
    return null;
  }, [scope, unique]);

  const refresh = useCallback(async () => {
    const missing = await evaluate();
    setBlocking(missing);
    if (!missing) {
      setGranted(null);
      return;
    }
    const info = PERMISSIONS[missing];
    if (info.detectable) {
      const state = await readGranted(missing);
      setGranted(state);
    } else {
      setGranted(null);
    }
  }, [evaluate]);

  const reportMissing = useCallback((id: PermissionId) => {
    prompted.current = null;
    setBlocking(id);
    setGranted(PERMISSIONS[id].detectable ? false : null);
  }, []);

  const beginGrantFlow = useCallback(
    async (id: PermissionId) => {
      const info = PERMISSIONS[id];
      setOpening(true);
      setNote(null);
      try {
        if (prompted.current !== id) {
          prompted.current = id;
          await api.requestPermission(id);
          await api.openSystemSettings(info.pane);
          // Deliberately *not* `onOpenTab({ kind: "permission" })`.
          //
          // This component already renders the page underneath with a
          // walkthrough overlay on top of it, which is the behaviour the
          // permission flow is supposed to have: the tool loads, discovers it
          // is blocked, and says so in place. Opening a second tab on top of
          // that moved you off the page you had just asked for and left you to
          // find your way back — the tool was still open, but you had been
          // taken somewhere else to read about it.
        }
      } catch (error) {
        setNote(api.errorMessage(error));
      } finally {
        setOpening(false);
      }
    },
    [onOpenTab, retryCommandId],
  );

  useEffect(() => {
    if (!active || unique.length === 0) {
      setBlocking(null);
      return;
    }
    void refresh();
  }, [active, refresh, unique.length]);

  useEffect(() => {
    if (!active || !blocking) return;
    void beginGrantFlow(blocking);
  }, [active, beginGrantFlow, blocking]);

  useEffect(() => {
    if (!active || !blocking) return;
    const info = PERMISSIONS[blocking];
    if (!info.detectable) return;

    const tick = () => void refresh();
    tick();
    const timer = setInterval(tick, POLL_MS);
    return () => clearInterval(timer);
  }, [active, blocking, refresh]);

  useEffect(() => {
    if (granted === null) return;
    if (
      blocking === "screen-recording" &&
      wasScreenGranted.current === false &&
      granted === true
    ) {
      // Write down what to come back to *before* asking the process to end.
      // The tab autosave is debounced, and `relaunchApp` does not wait for it —
      // so the thing the user was in the middle of is exactly the thing most
      // likely to be lost.
      if (retryCommandId) {
        rememberResume({ kind: "tool", commandId: retryCommandId });
      }
      void api.relaunchApp();
    }
    wasScreenGranted.current = granted;
  }, [blocking, granted, retryCommandId]);

  const info = blocking ? PERMISSIONS[blocking] : null;
  const highlightStep = info ? Math.min(1, info.steps.length - 1) : 0;

  const acknowledge = () => {
    if (!blocking) return;
    sessionStorage.setItem(sessionAckKey(blocking, scope), "1");
    void refresh();
  };

  const repair = async () => {
    if (!blocking) return;
    setRepairing(true);
    setNote(null);
    try {
      const outcome = await api.repairPermission(blocking);
      setNote(
        outcome.willRelaunch
          ? `${outcome.message} If Caduceus closes, it should reopen in a second.`
          : outcome.message,
      );
      if (outcome.granted) await refresh();
      else if (!outcome.willRelaunch) await beginGrantFlow(blocking);
    } catch (error) {
      setNote(api.errorMessage(error));
    } finally {
      setRepairing(false);
    }
  };

  const showOverlay = active && blocking !== null && granted !== true;

  return (
    <PermissionGateContext.Provider value={reportMissing}>
      <div className="relative flex h-full min-h-0 flex-1 flex-col">
        <div
          className={cx(
            "flex h-full min-h-0 flex-1 flex-col",
            showOverlay && "pointer-events-none select-none opacity-40",
          )}
          aria-hidden={showOverlay}
        >
          {children}
        </div>

        {showOverlay && info && (
          <div
            className="absolute inset-0 z-20 flex items-start justify-center overflow-y-auto bg-base/75 p-6 backdrop-blur-[2px]"
            role="dialog"
            aria-modal="true"
            aria-labelledby="permission-gate-title"
          >
            <div className="w-full max-w-[480px] rounded-cad border border-line bg-surface shadow-2xl">
              <div className="border-b border-line px-5 py-4">
                <p className="eyebrow">Before you continue</p>
                <h2
                  id="permission-gate-title"
                  className="mt-1 text-[17px] font-semibold tracking-[-0.015em] text-ink"
                >
                  Turn on {info.title}
                </h2>
                <p className="mt-2 text-[13px] leading-relaxed text-ink-mute">{info.why}</p>
              </div>

              <div className="space-y-3 px-5 py-4">
                <ol className="space-y-2.5">
                  {info.steps.map((step, index) => (
                    <li
                      key={index}
                      className={cx(
                        "flex gap-3 rounded-lg border px-3 py-2.5 transition-colors",
                        index === highlightStep
                          ? "border-accent/45 bg-accent/[0.08] shadow-[0_0_0_1px_rgb(124_124_255_/_0.12)]"
                          : "border-transparent bg-raised/40",
                      )}
                    >
                      <span
                        aria-hidden="true"
                        className={cx(
                          "mt-px flex h-[20px] w-[20px] shrink-0 items-center justify-center rounded-full text-2xs font-semibold",
                          index === highlightStep
                            ? "bg-accent text-white"
                            : "bg-accent/15 text-accent",
                        )}
                      >
                        {index + 1}
                      </span>
                      <span className="text-[13px] leading-relaxed text-ink-soft">{step}</span>
                    </li>
                  ))}
                </ol>

                {blocking === "screen-recording" && (
                  <p className="rounded-lg border border-caution/30 bg-caution/[0.06] px-3 py-2.5 text-[12px] leading-relaxed text-ink-mute">
                    After you turn this on, <strong className="text-ink-soft">quit Caduceus from the menu-bar icon and reopen it</strong>{" "}
                    — Screen Recording is the one grant macOS only applies after a restart.
                  </p>
                )}

                <div className="flex flex-wrap items-center gap-2 pt-1">
                  <Button
                    tone="primary"
                    disabled={opening}
                    onClick={() => void beginGrantFlow(blocking)}
                  >
                    {opening ? "Opening…" : "Open System Settings again"}
                  </Button>
                  <Button disabled={repairing} onClick={() => void repair()}>
                    {repairing ? "Repairing…" : "Repair stale grant"}
                  </Button>
                  {!info.detectable && (
                    <Button tone="ghost" onClick={acknowledge}>
                      I turned it on — continue
                    </Button>
                  )}
                </div>

                {info.detectable && granted === false && (
                  <div className="row gap-2 text-[13px] text-ink-mute">
                    <Spinner className="text-accent" />
                    <span>Waiting for the switch — this overlay closes when macOS reports it on.</span>
                  </div>
                )}

                {note && <p className="text-2xs text-ink-faint">{note}</p>}
              </div>
            </div>
          </div>
        )}
      </div>
    </PermissionGateContext.Provider>
  );
}
