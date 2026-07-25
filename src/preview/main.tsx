/**
 * UI preview harness — `npm run ui`.
 *
 * Renders every Orbit surface side by side against a fake IPC layer, so you can
 * work on the interface without a Rust toolchain and without launching the real
 * app. Nothing here ships: `preview.html` is not one of the bundled entry points.
 */

import React, { useState } from "react";
import ReactDOM from "react-dom/client";

import "@/styles.css";
import { installMockTauri, emitMock } from "./mockTauri";

// Must run before anything imports @tauri-apps/api, which reads
// `window.__TAURI_INTERNALS__` at module-evaluation time. Hence the dynamic
// imports below — a static import would be hoisted above this call.
installMockTauri();

const { CommandCenter } = await import("@/command-center/CommandCenter");
const { Orb } = await import("@/orb/Orb");
const { Settings } = await import("@/settings/Settings");

type Surface = "orb" | "command-center" | "settings";

function Preview() {
  const [surface, setSurface] = useState<Surface>("command-center");
  const [expanded, setExpanded] = useState(true);

  // The orb takes its hover state from a Rust event; fake it here so the
  // pop-out can be inspected. Re-emitted on a slow interval because `listen()`
  // resolves asynchronously — a single emit on mount races the subscription.
  React.useEffect(() => {
    const send = () => emitMock("orbit://orb-hover", { hovering: expanded, expanded });
    send();
    const timer = setInterval(send, 400);
    return () => clearInterval(timer);
  }, [expanded, surface]);

  return (
    <div className="flex h-screen w-screen flex-col bg-base">
      <header className="row shrink-0 border-b border-line px-4 py-2.5">
        <span className="text-[13px] font-semibold text-ink">Orbit UI preview</span>
        <span className="text-2xs text-ink-faint">mock backend — no real actions run</span>

        <div className="ml-auto row">
          {(["orb", "command-center", "settings"] as Surface[]).map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => setSurface(s)}
              className={
                "rounded-md px-2.5 py-1 text-2xs transition-colors " +
                (surface === s
                  ? "bg-accent/15 text-accent"
                  : "text-ink-mute hover:bg-raised hover:text-ink")
              }
            >
              {s}
            </button>
          ))}
          {surface === "orb" && (
            <button
              type="button"
              onClick={() => setExpanded((e) => !e)}
              className="rounded-md border border-line px-2.5 py-1 text-2xs text-ink-mute hover:text-ink"
            >
              {expanded ? "collapse" : "expand"}
            </button>
          )}
        </div>
      </header>

      <main className="min-h-0 flex-1 overflow-hidden">
        {surface === "settings" ? (
          <Settings />
        ) : (
          <div
            className="flex h-full items-center justify-center p-8"
            // A busy backdrop, so translucency and shadows can be judged the way
            // they will actually be seen — floating over other windows.
            style={{
              background:
                "radial-gradient(1200px 600px at 20% 10%, #1d2440 0%, transparent 60%)," +
                "radial-gradient(900px 500px at 80% 80%, #2a1d3a 0%, transparent 60%)," +
                "repeating-linear-gradient(45deg, #0e1016 0 18px, #12141c 18px 36px)",
            }}
          >
            {surface === "orb" ? (
              <div className="relative h-[340px] w-[340px]">
                <Orb />
              </div>
            ) : (
              <div className="h-[520px] w-[760px] overflow-hidden rounded-orbit-lg">
                <CommandCenter />
              </div>
            )}
          </div>
        )}
      </main>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Preview />
  </React.StrictMode>,
);
