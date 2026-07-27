/**
 * Check the tab rules.
 *
 * Caduceus is one window and everything in it is a tab, so a handful of small
 * rules decide whether the app feels right: a new tab is the palette, most
 * kinds do not duplicate, the cap holds, closing focuses left, and a lone Home
 * tab is still an overlay rather than a window. Every one of those is pure
 * arithmetic over a list — and every one is invisible to the type checker.
 *
 * Run with `npm run check:tabs`. Part of `npm run build`.
 */

import { build } from "esbuild";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "..");
const scratch = await mkdtemp(join(tmpdir(), "caduceus-tabs-"));

let failures = 0;
function check(label, ok) {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}`);
  if (!ok) failures += 1;
}

try {
  const bundle = join(scratch, "tabs.mjs");
  await build({
    entryPoints: [join(root, "src/shared/tabs.ts")],
    outfile: bundle,
    bundle: true,
    format: "esm",
    platform: "node",
    logLevel: "silent",
  });

  const { MAX_TABS, closeTab, homeTab, isFloating, openTab, tabLabel } = await import(
    pathToFileURL(bundle).href
  );

  // --- a new tab is the palette -----------------------------------------
  const start = [homeTab()];
  check("a fresh window is one Home tab", start.length === 1 && start[0].kind === "home");
  check("a lone Home tab is still an overlay", isFloating(start));

  const twoHomes = openTab(start, { kind: "home" });
  check("Home tabs duplicate — several searches at once is the point",
    twoHomes.tabs.length === 2);
  check("two tabs make it a window, not an overlay", !isFloating(twoHomes.tabs));

  // --- singletons focus rather than duplicate ---------------------------
  const withClipboard = openTab(twoHomes.tabs, { kind: "clipboard" });
  const again = openTab(withClipboard.tabs, { kind: "clipboard" });
  check("opening the clipboard twice focuses the tab you have",
    again.tabs.length === withClipboard.tabs.length &&
      again.activeId === withClipboard.activeId);

  // Re-opening with a different payload should move, not be ignored.
  const settings = openTab(again.tabs, { kind: "settings", section: "general" });
  const voice = openTab(settings.tabs, { kind: "settings", section: "voice" });
  const settingsTab = voice.tabs.find((tab) => tab.kind === "settings");
  check("asking for Settings → Voice while Settings is open moves to Voice",
    settingsTab?.section === "voice" && voice.tabs.filter((t) => t.kind === "settings").length === 1);

  // --- the cap ------------------------------------------------------------
  let filled = [homeTab()];
  for (let i = 1; i < MAX_TABS; i += 1) filled = openTab(filled, { kind: "home" }).tabs;
  check(`fills to exactly ${MAX_TABS}`, filled.length === MAX_TABS);

  const overflow = openTab(filled, { kind: "home" });
  check("the 25th is refused, with a reason",
    overflow.tabs.length === MAX_TABS && typeof overflow.refused === "string");

  // A singleton already open must still be reachable at the cap — otherwise
  // hitting 24 would lock you out of Settings entirely.
  let capped = [homeTab()];
  capped = openTab(capped, { kind: "settings" }).tabs;
  while (capped.length < MAX_TABS) capped = openTab(capped, { kind: "home" }).tabs;
  const reopened = openTab(capped, { kind: "settings" });
  check("a tab already open is still reachable at the cap", !reopened.refused);

  // --- closing ------------------------------------------------------------
  const three = openTab(openTab([homeTab()], { kind: "clipboard" }).tabs, { kind: "ports" });
  const middle = three.tabs[1];
  const afterClose = closeTab(three.tabs, middle.id, middle.id);
  check("closing the active tab focuses its left neighbour",
    afterClose.activeId === three.tabs[0].id && afterClose.tabs.length === 2);

  const closingInactive = closeTab(three.tabs, three.tabs[2].id, three.tabs[0].id);
  check("closing an inactive tab leaves focus alone",
    closingInactive.activeId === three.tabs[2].id);

  const emptied = closeTab([three.tabs[0]], three.tabs[0].id, three.tabs[0].id);
  check("closing the last tab leaves a fresh Home tab, not an empty window",
    emptied.emptied && emptied.tabs.length === 1 && emptied.tabs[0].kind === "home");

  // --- labels -------------------------------------------------------------
  check("every kind has a label",
    ["home", "clipboard", "chat", "settings", "system", "awake", "sound", "ports", "docker", "machine"]
      .every((kind) => tabLabel({ id: "x", kind }).length > 0));
  check("a Home tab shows its query once there is one",
    tabLabel({ id: "x", kind: "home", title: "sha256 hello" }) === "sha256 hello");
} finally {
  await rm(scratch, { recursive: true, force: true });
}

if (failures) {
  console.log(`\n${failures} tab rule(s) broken.`);
  process.exit(1);
}
