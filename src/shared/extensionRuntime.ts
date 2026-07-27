/**
 * Running an extension.
 *
 * An extension is somebody else's JavaScript, and this is the file that decides
 * what happens when it executes. Three things bound it, and it is worth being
 * precise about which one is doing the work:
 *
 * 1. **A Web Worker.** No `document`, no `window`, no access to the Command
 *    Center's DOM, and — because Tauri injects its IPC bridge into a page and
 *    not into a worker — no `__TAURI_INTERNALS__`. An extension cannot invoke a
 *    Tauri command, which matters more than everything else here put together:
 *    `run_apple_script` sits on the same IPC surface as `clipboard_list`.
 * 2. **The CSP.** `connect-src 'self'` applies to the worker as much as to the
 *    page, so even a network primitive this file failed to remove could not
 *    reach a host. `ctx.fetch` goes through Rust for exactly this reason.
 * 3. **The permission check in Rust.** {@link ctxBridge} refuses an ungranted
 *    operation before it sends it, which is what produces a readable error — but
 *    it is JavaScript sitting beside the extension's JavaScript, so it is not
 *    the boundary. `extensions::require` re-reads the header off disk on every
 *    call and is.
 *
 * The worker is created from a Blob rather than a bundled entry point because
 * the script is *made of* the extension: bootstrap, then the extension's own
 * source, then an epilogue. That ordering is what makes a syntax error in the
 * extension a worker load error with a line number, rather than a silent
 * nothing. It is also the only reason `worker-src 'self' blob:` is in the CSP
 * in `tauri.conf.json` — `connect-src` is deliberately untouched, so widening
 * that line did not widen where anything can connect to.
 */

import * as api from "./api";
import type { Extension, ExtensionFetchRequest } from "./types";

// ---------------------------------------------------------------------------
// What a run produces
// ---------------------------------------------------------------------------

export interface ExtensionRow {
  title: string;
  subtitle?: string;
  /** Whether choosing this row does anything — an `action` on the row. */
  hasAction: boolean;
}

export type ExtensionResult =
  | { kind: "text"; text: string }
  | { kind: "rows"; rows: ExtensionRow[] }
  /** Returned nothing. A row action that only had a side effect ends here. */
  | { kind: "none" };

/** How long one call gets before the worker is assumed to be stuck. */
const CALL_TIMEOUT = 120_000;

/** How long the worker gets to load before its source is declared broken. */
const LOAD_TIMEOUT = 5_000;

/** More rows than anyone reads; a runaway loop should not freeze the list. */
const MAX_ROWS = 500;

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/**
 * Every operation the sandbox can name, and the command it becomes.
 *
 * This map *is* the extension API's reach. An op that is not a key here has no
 * implementation, so widening what extensions can do means adding a line to
 * this table on purpose — not finding a command that happens to be invokable.
 */
function ctxBridge(ext: Extension) {
  const granted = new Set(ext.permissions);

  /** Refuse early, in words that say how to fix it. */
  const need = (permission: string, what: string) => {
    if (granted.has(permission)) return;
    throw new Error(
      `This extension did not ask for the “${permission}” permission, so ${what} is not ` +
        `available. Add it to the permissions line in the file's header and install it again.`,
    );
  };

  const id = ext.id;

  return async (op: string, payload: Record<string, unknown>): Promise<unknown> => {
    switch (op) {
      case "clipboard.read":
        need("clipboard", "ctx.clipboard.read()");
        return api.extensionClipboardRead(id);
      case "clipboard.write":
        need("clipboard", "ctx.clipboard.write()");
        return api.extensionClipboardWrite(id, String(payload.text ?? ""));
      case "fetch":
        need("network", "ctx.fetch()");
        return api.extensionFetch(id, payload as unknown as ExtensionFetchRequest);
      case "selection":
        need("selection", "ctx.selection()");
        return api.extensionSelection(id);
      case "notify":
        need("notifications", "ctx.notify()");
        return api.extensionNotify(id, String(payload.text ?? ""));
      case "open":
        return api.extensionOpen(id, String(payload.url ?? ""));
      case "storage.get":
        return api.extensionStorageGet(id, String(payload.key ?? ""));
      case "storage.set":
        return api.extensionStorageSet(id, String(payload.key ?? ""), payload.value ?? null);
      case "shell.run":
        need("shell", "ctx.shell.run()");
        return api.extensionShellRun(
          id,
          String(payload.command ?? ""),
          payload.input != null ? String(payload.input) : undefined,
          payload.timeoutSecs != null ? Number(payload.timeoutSecs) : undefined,
        );
      case "automation.script":
        need("automation", "ctx.automation.runAppleScript()");
        return api.extensionAutomationScript(id, String(payload.script ?? ""));
      case "automation.shortcut":
        need("automation", "ctx.automation.runShortcut()");
        return api.extensionAutomationShortcut(
          id,
          String(payload.name ?? ""),
          payload.input != null ? String(payload.input) : undefined,
        );
      case "files.read":
        need("files", "ctx.files.read()");
        return api.extensionFilesRead(id, String(payload.path ?? ""));
      case "files.write":
        need("files", "ctx.files.write()");
        return api.extensionFilesWrite(
          id,
          String(payload.path ?? ""),
          String(payload.content ?? ""),
        );
      case "settings.get":
        need("settings", "ctx.settings.get()");
        return api.extensionSettingsGet(id);
      case "settings.set":
        need("settings", "ctx.settings.set()");
        return api.extensionSettingsSet(id, payload.settings as import("./types").Settings);
      case "commands.dispatch":
        need("commands", "ctx.commands.dispatch()");
        return api.extensionCommandsDispatch(id, String(payload.input ?? ""));
      case "commands.runTool":
        need("commands", "ctx.commands.runTool()");
        return api.extensionCommandsRunTool(
          id,
          String(payload.toolId ?? ""),
          String(payload.input ?? ""),
        );
      case "ai.ask":
        need("ai", "ctx.ai.ask()");
        return api.extensionAiAsk(id, String(payload.prompt ?? ""));
      case "shortcuts.run":
        need("shortcuts", "ctx.shortcuts.run()");
        return api.extensionShortcutsRun(
          id,
          String(payload.shortcutId ?? ""),
          payload.query != null ? String(payload.query) : undefined,
        );
      default:
        throw new Error(`There is no ctx operation called “${op}”.`);
    }
  };
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/**
 * The code that wraps an extension.
 *
 * Written with string concatenation rather than interpolation inside: this is a
 * template literal that becomes source, and a stray `${` in it would be read by
 * the wrong language. The one interpolation is a numeric constant.
 *
 * `__CADUCEUS_PERMISSIONS__` is substituted before the Blob is made — it is the
 * granted list, not a check, and the sandbox uses it only to say why a call was
 * refused. What decides is `extensions::require` in Rust.
 */
const BOOTSTRAP = `"use strict";
var __caduceusEntry;
(function () {
  var GRANTED = __CADUCEUS_PERMISSIONS__;

  // Captured before they are sealed away, so the bridge keeps the only
  // references to them.
  var post = self.postMessage.bind(self);
  var listen = self.addEventListener.bind(self);

  // Everything that could reach the network, spawn another context, or talk to
  // the host directly. The CSP already blocks the network ones; removing them
  // means an extension gets a sentence about ctx.fetch instead of an opaque
  // security error, which is the difference between a bug and a mystery.
  var SEALED = [
    "fetch", "XMLHttpRequest", "WebSocket", "EventSource", "importScripts",
    "indexedDB", "caches", "Worker", "SharedWorker", "BroadcastChannel",
    "Notification", "postMessage", "addEventListener", "removeEventListener",
    "dispatchEvent", "reportError"
  ];
  for (var i = 0; i < SEALED.length; i++) {
    try {
      Object.defineProperty(self, SEALED[i], {
        value: undefined, writable: false, configurable: false, enumerable: false
      });
    } catch (e) {
      // A global that refuses to be shadowed is one the CSP is already holding.
    }
  }
  try { delete self.navigator.sendBeacon; } catch (e) {}

  // --- talking to the host ------------------------------------------------

  var seq = 0;
  var pending = new Map();

  function call(op, payload) {
    return new Promise(function (resolve, reject) {
      seq += 1;
      pending.set(seq, { resolve: resolve, reject: reject });
      post({ t: "call", id: seq, op: op, payload: payload || {} });
    });
  }

  // --- ctx ----------------------------------------------------------------

  function headersOf(list) {
    var map = Object.create(null);
    for (var i = 0; i < (list || []).length; i++) {
      map[String(list[i][0]).toLowerCase()] = String(list[i][1]);
    }
    var headers = { get: function (name) {
      var found = map[String(name).toLowerCase()];
      return found === undefined ? null : found;
    } };
    for (var key in map) headers[key] = map[key];
    return headers;
  }

  // Response-like rather than a real Response: the request happened in Rust,
  // so there is no stream left to wrap. ok/status/headers/text()/json() are
  // what extensions actually use.
  function responseOf(raw) {
    return {
      ok: raw.ok,
      status: raw.status,
      statusText: raw.statusText,
      url: raw.url,
      headers: headersOf(raw.headers),
      text: function () { return Promise.resolve(raw.body); },
      json: function () {
        return Promise.resolve().then(function () { return JSON.parse(raw.body); });
      }
    };
  }

  var ctx = {
    permissions: GRANTED.slice(),
    clipboard: {
      read: function () { return call("clipboard.read", {}); },
      write: function (text) { return call("clipboard.write", { text: String(text) }); }
    },
    fetch: function (url, init) {
      init = init || {};
      var headers = [];
      var given = init.headers || {};
      if (typeof given.forEach === "function" && !Array.isArray(given)) {
        given.forEach(function (value, name) { headers.push([String(name), String(value)]); });
      } else if (Array.isArray(given)) {
        for (var i = 0; i < given.length; i++) {
          headers.push([String(given[i][0]), String(given[i][1])]);
        }
      } else {
        for (var name in given) headers.push([String(name), String(given[name])]);
      }
      var body = init.body;
      return call("fetch", {
        url: String(url),
        method: init.method ? String(init.method) : "GET",
        headers: headers,
        body: body === undefined || body === null ? null : String(body)
      }).then(responseOf);
    },
    selection: function () { return call("selection", {}); },
    notify: function (text) { call("notify", { text: String(text) }).catch(function () {}); },
    storage: {
      get: function (key) { return call("storage.get", { key: String(key) }); },
      set: function (key, value) {
        return call("storage.set", { key: String(key), value: value === undefined ? null : value });
      }
    },
    open: function (url) { return call("open", { url: String(url) }); },
    shell: {
      run: function (command, input, timeoutSecs) {
        return call("shell.run", {
          command: String(command),
          input: input === undefined ? null : String(input),
          timeoutSecs: timeoutSecs === undefined ? null : Number(timeoutSecs)
        });
      }
    },
    automation: {
      runAppleScript: function (script) { return call("automation.script", { script: String(script) }); },
      runShortcut: function (name, input) {
        return call("automation.shortcut", { name: String(name), input: input === undefined ? null : String(input) });
      }
    },
    files: {
      read: function (path) { return call("files.read", { path: String(path) }); },
      write: function (path, content) { return call("files.write", { path: String(path), content: String(content) }); }
    },
    settings: {
      get: function () { return call("settings.get", {}); },
      set: function (settings) { return call("settings.set", { settings: settings }); }
    },
    commands: {
      dispatch: function (input) { return call("commands.dispatch", { input: String(input) }); },
      runTool: function (toolId, input) {
        return call("commands.runTool", { toolId: String(toolId), input: input === undefined ? "" : String(input) });
      }
    },
    ai: {
      ask: function (prompt) { return call("ai.ask", { prompt: String(prompt) }); }
    },
    shortcuts: {
      run: function (shortcutId, query) {
        return call("shortcuts.run", {
          shortcutId: String(shortcutId),
          query: query === undefined ? null : String(query)
        });
      }
    }
  };

  // --- results ------------------------------------------------------------

  // The closures from the last list. Replaced when a result *is* a list, and
  // otherwise left alone: choosing one row and getting a line of text back must
  // not quietly disarm the other rows, which are still on screen.
  var actions = [];

  function normalise(value) {
    if (value === undefined || value === null) return { kind: "none" };
    if (typeof value === "string") return { kind: "text", text: value };
    if (typeof value === "number" || typeof value === "boolean") {
      return { kind: "text", text: String(value) };
    }
    if (Array.isArray(value)) {
      var rows = [];
      actions = [];
      for (var i = 0; i < value.length && i < ${MAX_ROWS}; i++) {
        var row = value[i];
        if (typeof row === "string") {
          actions.push(null);
          rows.push({ title: row, hasAction: false });
          continue;
        }
        row = row || {};
        // A row's action may be a function to run, or a string to put on the
        // clipboard — the second is what most list extensions actually want,
        // and asking for a closure to do it is ceremony.
        var action = typeof row.action === "function" ? row.action
          : typeof row.action === "string" ? row.action : null;
        actions.push(action);
        rows.push({
          title: String(row.title === undefined || row.title === null ? "" : row.title),
          subtitle: row.subtitle === undefined || row.subtitle === null
            ? undefined : String(row.subtitle),
          hasAction: action !== null
        });
      }
      return { kind: "rows", rows: rows };
    }
    // An object is somebody returning data rather than a result. Showing it is
    // more useful than "[object Object]".
    try {
      return { kind: "text", text: JSON.stringify(value, null, 2) };
    } catch (e) {
      return { kind: "text", text: String(value) };
    }
  }

  function describe(error) {
    if (error && error.message) return String(error.message);
    return String(error);
  }

  // --- the protocol -------------------------------------------------------

  listen("message", function (event) {
    var message = event.data || {};

    if (message.t === "call-reply") {
      var waiting = pending.get(message.id);
      if (!waiting) return;
      pending.delete(message.id);
      if (message.ok) waiting.resolve(message.value);
      else waiting.reject(new Error(message.error));
      return;
    }

    if (message.t === "run") {
      // A fresh run replaces whatever was on screen, so the old list's
      // closures go with it even if this run returns a string.
      actions = [];
      Promise.resolve()
        .then(function () {
          if (typeof __caduceusEntry !== "function") {
            throw new Error(
              "This file does not export a function. A Caduceus extension ends with " +
              "\`export default async function (input, ctx) { … }\`."
            );
          }
          return __caduceusEntry(String(message.input === undefined ? "" : message.input), ctx);
        })
        .then(function (value) {
          post({ t: "result", id: message.id, ok: true, value: normalise(value) });
        })
        .catch(function (error) {
          post({ t: "result", id: message.id, ok: false, error: describe(error) });
        });
      return;
    }

    if (message.t === "invoke") {
      var action = actions[message.index];
      if (action === null || action === undefined) {
        post({ t: "result", id: message.id, ok: true, value: { kind: "none" } });
        return;
      }
      if (typeof action === "string") {
        post({
          t: "result", id: message.id, ok: true,
          value: { kind: "text", text: action }
        });
        return;
      }
      Promise.resolve()
        .then(function () { return action(ctx); })
        .then(function (value) {
          post({ t: "result", id: message.id, ok: true, value: normalise(value) });
        })
        .catch(function (error) {
          post({ t: "result", id: message.id, ok: false, error: describe(error) });
        });
    }
  });

  self.__caduceusReady = function () { post({ t: "ready" }); };
})();
`;

/** Posted after the extension's own source, so it only runs if that parsed. */
const EPILOGUE = `\n;self.__caduceusReady();\n`;

/**
 * Turn an extension file into something a classic worker can execute.
 *
 * The documented shape is a module (`export default …`), and the documented
 * shape is also the only one, so this is a rewrite rather than a parser: the
 * default export becomes an assignment, and any other `export` keyword is
 * dropped so a stray `export const` is a working extension rather than a
 * syntax error nobody can read.
 *
 * A module worker would avoid the rewrite. It would also require a WebKit new
 * enough for module workers, which macOS 11 — the floor this app ships to — does
 * not have.
 */
export function toWorkerSource(source: string): string {
  const withEntry = source.replace(
    /^[ \t]*export[ \t]+default[ \t]+/m,
    "__caduceusEntry = ",
  );
  if (withEntry === source && !/^[ \t]*export[ \t]+default\b/m.test(source)) {
    // Left as-is; the worker reports the missing entry point at run time, where
    // the message can name the file the user is looking at.
    return source;
  }
  return withEntry.replace(/^[ \t]*export[ \t]+(?!default\b)/gm, "");
}

// ---------------------------------------------------------------------------
// One live extension
// ---------------------------------------------------------------------------

interface Waiting {
  resolve: (result: ExtensionResult) => void;
  reject: (error: Error) => void;
  timer: number;
}

/**
 * A running extension, and the only handle on it.
 *
 * It stays alive between calls on purpose: a list of rows whose `action` is a
 * closure is only meaningful while the closure exists, so choosing a row talks
 * to the same worker that produced it. {@link dispose} is not optional — a
 * worker outlives the page's reference to it.
 */
export class ExtensionRun {
  private worker: Worker | null = null;
  private url: string | null = null;
  private ready: Promise<void> | null = null;
  private readonly waiting = new Map<number, Waiting>();
  private readonly bridge: (op: string, payload: Record<string, unknown>) => Promise<unknown>;
  private seq = 0;
  private dead: string | null = null;

  constructor(private readonly ext: Extension) {
    this.bridge = ctxBridge(ext);
  }

  /** Run the extension's default export with `input`. */
  run(input: string): Promise<ExtensionResult> {
    return this.send({ t: "run", input });
  }

  /** Choose row `index` from the last result. */
  invoke(index: number): Promise<ExtensionResult> {
    return this.send({ t: "invoke", index });
  }

  dispose(): void {
    for (const [, waiting] of this.waiting) {
      clearTimeout(waiting.timer);
      waiting.reject(new Error("That extension was stopped."));
    }
    this.waiting.clear();
    this.worker?.terminate();
    this.worker = null;
    if (this.url) URL.revokeObjectURL(this.url);
    this.url = null;
    this.ready = null;
  }

  // --- internals ----------------------------------------------------------

  private async send(message: { t: string } & Record<string, unknown>): Promise<ExtensionResult> {
    await this.start();
    const worker = this.worker;
    if (!worker) throw new Error(this.dead ?? "That extension is not running.");

    this.seq += 1;
    const id = this.seq;

    return new Promise<ExtensionResult>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.waiting.delete(id);
        // Terminate rather than wait: a worker in a `while (true)` never
        // reads another message, so the only way back is to end it.
        this.fail(
          `“${this.ext.name}” did not finish within ${CALL_TIMEOUT / 1000} seconds and was stopped.`,
        );
        reject(new Error(this.dead!));
      }, CALL_TIMEOUT);

      this.waiting.set(id, { resolve, reject, timer });
      worker.postMessage({ ...message, id });
    });
  }

  private start(): Promise<void> {
    if (this.dead) return Promise.reject(new Error(this.dead));
    if (this.ready) return this.ready;

    this.ready = (async () => {
      const source = await api.extensionSource(this.ext.id);
      const code =
        BOOTSTRAP.replace(
          "__CADUCEUS_PERMISSIONS__",
          JSON.stringify(this.ext.permissions),
        ) +
        "\n" +
        toWorkerSource(source) +
        EPILOGUE;

      let worker: Worker;
      try {
        this.url = URL.createObjectURL(new Blob([code], { type: "text/javascript" }));
        worker = new Worker(this.url);
      } catch (error) {
        throw new Error(`Could not start the extension sandbox: ${api.errorMessage(error)}`);
      }
      this.worker = worker;

      await new Promise<void>((resolve, reject) => {
        const timer = window.setTimeout(
          () => reject(new Error(`“${this.ext.name}” did not finish loading.`)),
          LOAD_TIMEOUT,
        );

        worker.addEventListener("message", (event: MessageEvent) => {
          const message = event.data ?? {};
          if (message.t === "ready") {
            clearTimeout(timer);
            resolve();
            return;
          }
          this.receive(message);
        });

        // A worker error is almost always the extension failing to parse. The
        // message carries the line, which is the one thing worth passing on.
        worker.addEventListener("error", (event: ErrorEvent) => {
          event.preventDefault();
          clearTimeout(timer);
          const detail = event.message || "it could not be loaded";
          this.fail(`“${this.ext.name}” failed: ${detail}`);
          reject(new Error(this.dead!));
        });
      });
    })();

    // A failed start must not be remembered as a start in progress.
    this.ready.catch(() => {
      this.ready = null;
    });

    return this.ready;
  }

  private receive(message: Record<string, unknown>): void {
    if (message.t === "call") {
      const id = message.id as number;
      void Promise.resolve()
        .then(() => this.bridge(String(message.op), (message.payload ?? {}) as Record<string, unknown>))
        .then(
          (value) => this.worker?.postMessage({ t: "call-reply", id, ok: true, value }),
          (error) =>
            this.worker?.postMessage({
              t: "call-reply",
              id,
              ok: false,
              error: api.errorMessage(error),
            }),
        );
      return;
    }

    if (message.t === "result") {
      const waiting = this.waiting.get(message.id as number);
      if (!waiting) return;
      this.waiting.delete(message.id as number);
      clearTimeout(waiting.timer);
      if (message.ok) waiting.resolve(clampResult(message.value as ExtensionResult));
      else waiting.reject(new Error(String(message.error)));
    }
  }

  /** End the worker and make every later call fail with the same reason. */
  private fail(reason: string): void {
    this.dead = reason;
    this.dispose();
  }
}

/** Trust the sandbox's shape as far as the type says and no further. */
function clampResult(value: ExtensionResult | undefined): ExtensionResult {
  if (!value || typeof value !== "object") return { kind: "none" };
  if (value.kind === "text") return { kind: "text", text: String(value.text ?? "") };
  if (value.kind === "rows") {
    return {
      kind: "rows",
      rows: (Array.isArray(value.rows) ? value.rows : []).slice(0, MAX_ROWS).map((row) => ({
        title: String(row?.title ?? ""),
        subtitle: row?.subtitle == null ? undefined : String(row.subtitle),
        hasAction: Boolean(row?.hasAction),
      })),
    };
  }
  return { kind: "none" };
}
