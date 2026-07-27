/**
 * Check that a permission wall is still recognised as one.
 *
 * When macOS refuses something, Caduceus does not print the refusal — it opens
 * the page that walks you through granting it. Which page depends on reading
 * the permission back out of the message, and the messages are written in Rust:
 * `window::accessibility::describe_error`, `tools::system::osa`,
 * `capture`, `voice::recorder`. Nothing in either compiler connects the two
 * sides, so rewording a sentence on the Rust side silently turns a guided page
 * back into a dead end.
 *
 * This asserts the round trip, against the real strings taken from the Rust
 * source rather than copies of them.
 *
 * Run with `npm run check:permissions`. Part of `npm run build`.
 */

import { build } from "esbuild";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "..");
const scratch = await mkdtemp(join(tmpdir(), "caduceus-permissions-"));

let failures = 0;
function check(label, ok) {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}`);
  if (!ok) failures += 1;
}

/** Collapse the line breaks and indentation Rust string literals wrap with. */
function normalise(text) {
  return text.replace(/\s+/g, " ").trim();
}

try {
  const stub = join(scratch, "tauri-stub.js");
  await writeFile(stub, "export const invoke = () => Promise.resolve({});\n");

  const bundle = join(scratch, "permissions.mjs");
  await build({
    entryPoints: [join(root, "src/shared/permissions.ts")],
    outfile: bundle,
    bundle: true,
    format: "esm",
    platform: "node",
    logLevel: "silent",
    alias: { "@tauri-apps/api/core": stub },
  });

  const { PERMISSIONS, PERMISSION_WALL, permissionFromMessage } = await import(
    pathToFileURL(bundle).href
  );

  // --- every permission is described well enough to act on --------------
  for (const [id, info] of Object.entries(PERMISSIONS)) {
    check(`${id} says what it is for`, info.why.length > 20);
    check(`${id} has clicks, not just a pane name`, info.steps.length >= 2);
    check(`${id} names a pane the Rust side can open`, typeof info.pane === "string");
  }

  // --- the canonical sentences route to themselves -----------------------
  for (const [id, sentence] of Object.entries(PERMISSION_WALL)) {
    check(`PERMISSION_WALL.${id} routes back to ${id}`, permissionFromMessage(sentence) === id);
  }

  // --- the sentences Rust actually produces ------------------------------
  const sources = {
    "src-tauri/src/window/accessibility.rs": "accessibility",
    // `tools::system::osa` used to translate these itself; it now defers to
    // `tools::apple::run_script`, so that is where the sentences live.
    "src-tauri/src/tools/apple.rs": "accessibility",
    "src-tauri/src/capture/mod.rs": "screen-recording",
    "src-tauri/src/voice/recorder.rs": "microphone",
    "src-tauri/src/notes.rs": "automation",
  };

  for (const [path, expected] of Object.entries(sources)) {
    const whole = await readFile(join(root, path), "utf8");
    // Tests are cut first. A test that asserts a message merely *mentions*
    // System Settings quotes that fragment, and a fragment routes nowhere —
    // which would fail this check for a file whose real messages are fine.
    const source = whole.split("#[cfg(test)]")[0];
    // Every user-facing wall message mentions System Settings by name; that is
    // what makes it findable here without hard-coding the sentence twice.
    const quoted = [...source.matchAll(/"((?:[^"\\]|\\.)*System Settings(?:[^"\\]|\\.)*)"/gs)]
      .map((match) => normalise(match[1].replace(/\\\s+/g, "")))
      // `\u{2192}` is how the arrow is written in a couple of these.
      .map((text) => text.replace(/\\u\{2192\}/g, "→"));

    check(`${path} still has a permission message`, quoted.length > 0);
    const routed = quoted.map(permissionFromMessage);
    check(
      `${path} routes to ${expected}`,
      routed.includes(expected),
      );
    check(
      `${path} has no message that routes nowhere`,
      routed.every((value) => value !== null),
    );
  }

  // --- and ordinary failures are left alone ------------------------------
  check(
    "a plain error is not mistaken for a permission wall",
    permissionFromMessage("That window did not respond.") === null &&
      permissionFromMessage("Could not reach the host.") === null,
  );
} finally {
  await rm(scratch, { recursive: true, force: true });
}

if (failures) {
  console.log(`\n${failures} permission rule(s) broken.`);
  process.exit(1);
}
