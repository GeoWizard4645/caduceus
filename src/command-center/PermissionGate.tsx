/**
 * Lets a tool that hits a macOS privacy wall say so without standing in its
 * way.
 *
 * # Why this used to be different
 *
 * The old version of this file was an interstitial: the instant a tab needed
 * a grant it did not have, it dimmed the tool to 40% opacity, switched off
 * its pointer events, and dropped a modal card in front of it — then, on its
 * own, fired `CGRequestScreenCaptureAccess`/`AXIsProcessTrusted` and opened
 * System Settings, before the user had asked for anything. Pressing Enter on
 * a tool did not open the tool; it opened a wall about the tool. That is a
 * large part of what "the permissions keep asking for you" meant in
 * practice — every tab that merely *might* need a grant interrupted itself
 * with System Settings on first render, whether or not the thing you came to
 * do that moment needed it.
 *
 * Now the tool always renders. If a grant this component knows how to check
 * is missing, a banner appears above the tool — not over it — and everything
 * underneath stays exactly as usable as it would have been anyway. Most
 * commands never show the banner at all, because most of the grants Caduceus
 * needs are handled during onboarding, once, before there is a tab to
 * interrupt.
 *
 * # Why this is the rare path, not the front door
 *
 * Microphone, Speech Recognition and Accessibility are asked for during
 * first run. What lands here afterwards is narrower: a grant declined at
 * onboarding and needed later; Screen Recording, which onboarding does not
 * collect because asking for it before there is anything to capture is its
 * own kind of nagging; Automation, which is inherently per-target-app and
 * has no "ask once" moment — Caduceus does not know which apps you use until
 * you reach for one; and the ordinary case of a grant reading as stale after
 * a rebuild, which `STALE_GRANT_EXPLANATION` covers. All of that is edge
 * case relative to a normal session, and the banner is sized accordingly:
 * quiet, dismissible, and never blocking the thing it is a footnote to.
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
import { PermissionCoach } from "@/shared/PermissionCoach";
import { rememberResume, type PermissionId } from "@/shared/tabs";

const PermissionGateContext = createContext<(id: PermissionId) => void>(() => {});

/**
 * Report a permission wall hit at runtime.
 *
 * For the two grants macOS lets an app read back — Accessibility and Screen
 * Recording — this component can notice a missing one on its own. Everything
 * else (Automation, and Microphone/Speech Recognition should onboarding have
 * been skipped) has no such API, so the only way Caduceus finds out is that
 * the command actually failed and named what it needed. Callers do that
 * through this hook rather than rendering their own banner, so there is one
 * place that decides what a permission wall looks like.
 */
export function usePermissionGate() {
  return useContext(PermissionGateContext);
}

/** The two grants Caduceus can probe without asking the user anything. */
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

export function PermissionGate({
  active,
  permissions,
  // No longer used for anything here — a session-scoped "acknowledged, stop
  // asking" flag lived on this key in the old interstitial, and there is
  // nothing left to acknowledge once the wall is a dismissible banner rather
  // than something standing between the user and the tool. Kept in the
  // props so call sites naming a command or page for their own bookkeeping
  // do not need to change.
  scope: _scope,
  retryCommandId,
  children,
}: {
  active: boolean;
  permissions: PermissionId[];
  scope: string;
  retryCommandId?: string;
  children: ReactNode;
}) {
  const [missing, setMissing] = useState<PermissionId | null>(null);
  // Guards against remembering the same resume point twice — `handleGranted`
  // can in principle fire more than once before the relaunch actually tears
  // the window down.
  const resumeRemembered = useRef(false);

  const unique = useMemo(
    () => [...new Set(permissions.filter((p) => p in PERMISSIONS))],
    [permissions],
  );

  // Silent and read-only: this decides whether to show the banner, full
  // stop. It never calls `requestPermission` and never opens System
  // Settings — both are things a person does by pressing a button in
  // `PermissionCoach`, not things a tab does to itself on arrival.
  useEffect(() => {
    if (!active || unique.length === 0) {
      setMissing(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      for (const id of unique) {
        const state = await readGranted(id);
        if (state === false) {
          if (!cancelled) setMissing(id);
          return;
        }
      }
      if (!cancelled) setMissing(null);
    })();
    return () => {
      cancelled = true;
    };
  }, [active, unique]);

  const reportMissing = useCallback((id: PermissionId) => {
    if (id in PERMISSIONS) setMissing(id);
  }, []);

  const handleGranted = useCallback(() => {
    // Screen Recording is the one grant that can read as on before capture
    // actually works — macOS applies it fully only after Caduceus restarts.
    // Write down what was open before asking the process to end, the same
    // way the rest of the app resumes across a restart, then relaunch.
    if (missing === "screen-recording") {
      if (retryCommandId && !resumeRemembered.current) {
        resumeRemembered.current = true;
        rememberResume({ kind: "tool", commandId: retryCommandId });
      }
      void api.relaunchApp();
      return;
    }
    setMissing(null);
  }, [missing, retryCommandId]);

  return (
    <PermissionGateContext.Provider value={reportMissing}>
      <div className="flex h-full min-h-0 flex-1 flex-col">
        {missing && (
          <div className="shrink-0 border-b border-line px-5 py-3">
            <PermissionCoach
              ids={[missing]}
              variant="inline"
              onAllGranted={handleGranted}
              onSkip={() => setMissing(null)}
            />
          </div>
        )}
        <div className="flex h-full min-h-0 flex-1 flex-col">{children}</div>
      </div>
    </PermissionGateContext.Provider>
  );
}
