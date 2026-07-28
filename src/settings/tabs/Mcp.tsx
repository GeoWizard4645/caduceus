/**
 * Settings → MCP: connect Caduceus to Model Context Protocol servers.
 *
 * # What this screen is actually asking you to do
 *
 * An MCP server is not a plugin or an integration in the usual sense — it is
 * an arbitrary program on this Mac. Adding one here means Caduceus will run
 * `command arg1 arg2 ...` as a child process, hand it a JSON-RPC session over
 * stdio, and (once it answers) treat whatever "tools" it advertises as things
 * a model can be told to call. That is real code execution, so every place
 * below that could launch a process says so in those terms — "run this
 * program" — never "install" or "connect an integration".
 *
 * Three things this screen deliberately does NOT do, matching
 * `src-tauri/src/mcp.rs`'s module header:
 *
 *  1. Nothing is ever launched automatically. There is no discovery, no
 *     registry to browse, no "recommended servers" list. A server exists only
 *     because a human typed its command into the form below and pressed Add,
 *     or hand-edited the store file directly.
 *  2. A server's tool names, descriptions, and its `instructions` string are
 *     written by that server — not reviewed, not sanitised, not vouched for
 *     by Caduceus. A hostile server can call a destructive tool
 *     "safe_read_only_lookup". Every place this file renders that text is
 *     visually quoted and labelled as the server's own words, never styled
 *     like Caduceus's own copy.
 *  3. This screen only manages servers and shows what they claim to offer —
 *     it does not call a tool with real arguments. The one exception,
 *     "Test connection", only ever runs the handshake (`initialize` +
 *     `tools/list`), never `tools/call`; nothing here can be used to
 *     trigger a destructive tool by accident.
 *
 * # Why edit doesn't prefill environment variables
 *
 * `mcp::McpServerInfo` (what `mcpListServers`/`mcpServerStatus` return) never
 * carries a server's configured `env` back to the frontend — see
 * `src/shared/api.ts`. That is presumably deliberate (env values are exactly
 * where an API key would live), but it means the edit form below cannot show
 * what is already set. Editing always starts env empty and warns that saving
 * with it empty clears it, because `mcp_update_server` replaces a server's
 * config outright rather than merging into it.
 */

import { useEffect, useMemo, useState } from "react";

import * as api from "@/shared/api";
import type { McpServerInfo, McpServerStatus, McpTool } from "@/shared/api";
import {
  Button,
  Callout,
  EmptyState,
  Field,
  IconButton,
  Section,
  Spinner,
  TextInput,
  cx,
} from "@/shared/ui";

// ---------------------------------------------------------------------------
// Status presentation
// ---------------------------------------------------------------------------

function statusMeta(status: McpServerStatus): {
  label: string;
  tone: "positive" | "caution" | "danger" | "neutral";
  reason?: string;
} {
  switch (status.state) {
    case "ready":
      return { label: "Ready", tone: "positive" };
    case "connecting":
      return { label: "Connecting…", tone: "caution" };
    case "unhealthy":
      return { label: "Unhealthy", tone: "danger", reason: status.reason };
    case "disconnected":
      return { label: "Disconnected", tone: "neutral" };
  }
}

function StatusBadge({ status }: { status: McpServerStatus }) {
  const meta = statusMeta(status);
  const toneClass = {
    positive: "bg-positive/15 text-positive",
    caution: "bg-caution/15 text-caution",
    danger: "bg-danger/15 text-danger",
    neutral: "bg-overlay text-ink-faint",
  }[meta.tone];
  return (
    <span className={cx("inline-flex items-center gap-1.5 rounded px-1.5 py-0.5 text-2xs font-medium", toneClass)}>
      {status.state === "connecting" && <Spinner className="h-2.5 w-2.5" />}
      {meta.label}
    </span>
  );
}

/** The literal argv this config resolves to, as separate tokens rather than a
 *  joined string — joining with spaces would make an argument that itself
 *  contains a space indistinguishable from two arguments, which is exactly
 *  the kind of ambiguity this screen exists to not have. */
function CommandTokens({ command, args }: { command: string; args: string[] }) {
  return (
    <div className="flex flex-wrap items-center gap-1 font-mono text-2xs">
      <span className="rounded bg-accent/12 px-1.5 py-0.5 font-medium text-accent">
        {command || "(no command)"}
      </span>
      {args.filter((a) => a.length > 0).map((a, i) => (
        <span key={i} className="rounded bg-overlay px-1.5 py-0.5 text-ink-soft">
          {a}
        </span>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Draft state for the add/edit form
// ---------------------------------------------------------------------------

interface ServerDraft {
  name: string;
  command: string;
  args: string[];
  env: { key: string; value: string }[];
}

function emptyDraft(): ServerDraft {
  return { name: "", command: "", args: [""], env: [] };
}

function draftFromServer(s: McpServerInfo): ServerDraft {
  // `env` is never returned by the backend (see the module comment above),
  // so an edit always starts with none — not because the server has none.
  return { name: s.name, command: s.command, args: s.args.length ? [...s.args, ""] : [""], env: [] };
}

function draftArgsToList(args: string[]): string[] {
  return args.map((a) => a.trim()).filter((a) => a.length > 0);
}

function draftEnvToRecord(env: { key: string; value: string }[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const { key, value } of env) {
    const k = key.trim();
    if (k) out[k] = value;
  }
  return out;
}

function cloneDraft(d: ServerDraft): ServerDraft {
  return { name: d.name, command: d.command, args: [...d.args], env: d.env.map((e) => ({ ...e })) };
}

/** Turns a React state setter into the `setDraft` shape `ServerForm` wants: a
 *  function that takes a mutator and applies it to a fresh clone of the
 *  current draft, so `ServerForm`'s own field handlers can write `d.name = v`
 *  directly without either of them needing to know about the other's state
 *  shape. */
function draftSetter(setState: (updater: (prev: ServerDraft) => ServerDraft) => void) {
  return (mutator: (d: ServerDraft) => void) =>
    setState((prev) => {
      const next = cloneDraft(prev);
      mutator(next);
      return next;
    });
}

// ---------------------------------------------------------------------------
// The form shared by "add a server" and "edit a server"
// ---------------------------------------------------------------------------

function ServerForm({
  draft,
  setDraft,
  isNew,
  nameTaken,
  onSubmit,
  onCancel,
  busy,
  error,
}: {
  draft: ServerDraft;
  setDraft: (fn: (d: ServerDraft) => void) => void;
  isNew: boolean;
  nameTaken: boolean;
  onSubmit: () => void;
  onCancel: () => void;
  busy: boolean;
  error: string | null;
}) {
  // `setDraft` (built by `draftSetter` in `McpTab`) already clones the
  // current draft and applies this mutator to the clone before committing it
  // as new state — so this is a direct pass-through, not a second clone.
  // Kept as its own name only so call sites below read as "update the draft"
  // rather than "setDraft".
  const update = setDraft;

  const previewArgs = draftArgsToList(draft.args);
  const canSubmit = draft.name.trim().length > 0 && draft.command.trim().length > 0 && !nameTaken;

  return (
    <div className="rounded-lg border border-line-strong/60 bg-base/30 p-4">
      <div className="grid grid-cols-2 gap-4">
        <Field label="Name" hint="Letters, numbers, - and _ only. This becomes the prefix on every tool it exposes.">
          <TextInput
            value={draft.name}
            disabled={!isNew}
            onChange={(v) => update((d) => (d.name = v))}
            placeholder="e.g. filesystem"
            autoFocus={isNew}
          />
          {nameTaken && <p className="mt-1 text-2xs text-danger">A server with this name already exists.</p>}
        </Field>

        <Field label="Command" hint="The executable to run. No shell — this is exec'd directly, so no ; | && tricks apply or are needed.">
          <TextInput
            mono
            value={draft.command}
            onChange={(v) => update((d) => (d.command = v))}
            placeholder="e.g. npx or /usr/local/bin/my-server"
          />
        </Field>
      </div>

      <div className="mt-4">
        <span className="mb-1.5 block text-[13px] font-medium text-ink-soft">Arguments</span>
        <div className="flex flex-col gap-1.5">
          {draft.args.map((arg, i) => (
            <div key={i} className="row gap-1.5">
              <TextInput
                mono
                value={arg}
                placeholder="one argument per row"
                onChange={(v) =>
                  update((d) => {
                    d.args[i] = v;
                    // Typing into the last row grows the list, so there is
                    // always exactly one blank row to type the next arg into.
                    if (i === d.args.length - 1 && v.length > 0) d.args.push("");
                  })
                }
              />
              <IconButton
                label="Remove argument"
                tone="danger"
                disabled={draft.args.length <= 1}
                onClick={() => update((d) => void d.args.splice(i, 1))}
              >
                ×
              </IconButton>
            </div>
          ))}
        </div>
      </div>

      <div className="mt-4">
        <div className="mb-1.5 flex items-center justify-between">
          <span className="text-[13px] font-medium text-ink-soft">Environment variables</span>
          <Button size="sm" onClick={() => update((d) => void d.env.push({ key: "", value: "" }))}>
            Add variable
          </Button>
        </div>
        {!isNew && (
          <p className="mb-2 text-2xs leading-relaxed text-ink-faint">
            Existing variables are not shown here and are not preserved automatically — re-enter any
            you want this server to keep. Saving with none set here clears them.
          </p>
        )}
        {draft.env.length === 0 ? (
          <p className="text-2xs text-ink-faint">
            None. The server still gets a minimal environment (PATH, HOME, USER, SHELL, TMPDIR,
            LANG) — never Caduceus&rsquo;s full environment or its credentials.
          </p>
        ) : (
          <div className="flex flex-col gap-1.5">
            {draft.env.map((row, i) => (
              <div key={i} className="row gap-1.5">
                <TextInput mono value={row.key} placeholder="KEY" onChange={(v) => update((d) => (d.env[i].key = v))} />
                <TextInput mono value={row.value} placeholder="value" onChange={(v) => update((d) => (d.env[i].value = v))} />
                <IconButton label="Remove variable" tone="danger" onClick={() => update((d) => void d.env.splice(i, 1))}>
                  ×
                </IconButton>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="mt-4 rounded-lg border border-caution/30 bg-caution/[0.07] px-3 py-2.5">
        <p className="mb-1.5 text-2xs font-medium text-ink-soft">
          {isNew ? "Pressing Add runs this now:" : "Saving reconnects using this:"}
        </p>
        <CommandTokens command={draft.command} args={previewArgs} />
      </div>

      {error && (
        <div className="mt-3">
          <Callout tone="danger">{error}</Callout>
        </div>
      )}

      <div className="row mt-4 gap-2">
        <Button tone="primary" disabled={!canSubmit || busy} onClick={onSubmit}>
          {busy ? <Spinner /> : null} {isNew ? "Add and run" : "Save and reconnect"}
        </Button>
        <Button onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// A server's discovered tools — always rendered as the server's own,
// unverified words.
// ---------------------------------------------------------------------------

function ToolList({ tools }: { tools: McpTool[] }) {
  if (tools.length === 0) {
    return <p className="text-2xs text-ink-faint">No tools discovered.</p>;
  }
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-2xs leading-relaxed text-ink-faint">
        Names and descriptions below are written by this server, not by Caduceus. Treat them the way
        you would treat text from any other untrusted source — a server can describe a destructive
        tool as harmless.
      </p>
      {tools.map((tool) => (
        <div key={tool.id} className="rounded-md border border-line bg-base/40 px-2.5 py-2">
          <code className="text-2xs font-medium text-ink">{tool.id}</code>
          {tool.title && <span className="ml-2 text-2xs text-ink-faint">({tool.title})</span>}
          {tool.description && (
            <p className="mt-1 text-2xs leading-relaxed text-ink-mute">
              <span aria-hidden="true" className="text-ink-faint">
                &ldquo;
              </span>
              {tool.description}
              <span aria-hidden="true" className="text-ink-faint">
                &rdquo;
              </span>
            </p>
          )}
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// One configured server
// ---------------------------------------------------------------------------

function ServerRow({
  server,
  tools,
  onChanged,
  onEdit,
}: {
  server: McpServerInfo;
  tools: McpTool[];
  onChanged: () => void;
  onEdit: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState<"test" | "connect" | "remove" | null>(null);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const meta = statusMeta(server.status);
  const running = server.status.state === "ready" || server.status.state === "connecting";

  const runTest = async () => {
    setBusy("test");
    setTestResult(null);
    try {
      const info = await api.mcpConnectServer(server.name);
      const m = statusMeta(info.status);
      setTestResult(
        info.status.state === "ready"
          ? { ok: true, message: `Connected. ${info.toolCount} tool${info.toolCount === 1 ? "" : "s"} discovered.` }
          : { ok: false, message: m.reason ?? m.label },
      );
      onChanged();
    } catch (e) {
      setTestResult({ ok: false, message: api.errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  const toggleRunning = async () => {
    setBusy("connect");
    try {
      if (running) await api.mcpDisconnectServer(server.name);
      else await api.mcpConnectServer(server.name);
      onChanged();
    } catch (e) {
      setTestResult({ ok: false, message: api.errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    setBusy("remove");
    try {
      await api.mcpRemoveServer(server.name);
      onChanged();
    } catch (e) {
      setTestResult({ ok: false, message: api.errorMessage(e) });
      setBusy(null);
    }
  };

  return (
    <div className="rounded-lg border border-line bg-raised/40 px-3.5 py-3">
      <div className="row items-start justify-between gap-3">
        <button
          type="button"
          className="row min-w-0 flex-1 items-center gap-2 text-left"
          onClick={() => setExpanded((v) => !v)}
        >
          <span className={cx("text-2xs text-ink-faint transition-transform", expanded && "rotate-90")} aria-hidden="true">
            ▶
          </span>
          <div className="min-w-0">
            <div className="row flex-wrap items-center gap-2">
              <span className="truncate text-[13px] font-medium text-ink">{server.name}</span>
              <StatusBadge status={server.status} />
              {server.toolCount > 0 && (
                <span className="text-2xs text-ink-faint">
                  {server.toolCount} tool{server.toolCount === 1 ? "" : "s"}
                </span>
              )}
            </div>
            <div className="mt-1">
              <CommandTokens command={server.command} args={server.args} />
            </div>
          </div>
        </button>

        <div className="row shrink-0 gap-1.5">
          <Button size="sm" disabled={busy !== null} onClick={runTest}>
            {busy === "test" ? <Spinner /> : null} Test connection
          </Button>
          <Button size="sm" disabled={busy !== null} onClick={toggleRunning}>
            {busy === "connect" ? <Spinner /> : null} {running ? "Disconnect" : "Connect"}
          </Button>
          <Button size="sm" disabled={busy !== null} onClick={onEdit}>
            Edit
          </Button>
          <IconButton label="Remove server" tone="danger" disabled={busy !== null} onClick={() => void remove()}>
            {busy === "remove" ? <Spinner /> : "×"}
          </IconButton>
        </div>
      </div>

      {meta.reason && (
        <p className="mt-2 text-2xs leading-relaxed text-danger">{meta.reason}</p>
      )}

      {testResult && (
        <div
          className={cx(
            "mt-2 rounded-md px-2.5 py-1.5 text-2xs leading-relaxed",
            testResult.ok ? "bg-positive/10 text-positive" : "bg-danger/10 text-danger",
          )}
        >
          {testResult.message}
        </div>
      )}

      {expanded && (
        <div className="mt-3 flex flex-col gap-3 border-t border-line pt-3">
          {/* Not a Toggle: flipping "enabled" goes through `mcpUpdateServer`,
              which replaces this server's config outright — including its
              environment variables, which the backend never hands back to
              this screen to preserve (see the module comment at the top of
              this file). A one-click toggle here would silently drop a
              server's API keys with no chance to notice. Edit is the one
              path that at least shows that trade-off before it happens. */}
          <p className="text-2xs text-ink-faint">
            Auto-connect at launch:{" "}
            <span className="font-medium text-ink-soft">{server.enabled ? "on" : "off"}</span> — change
            via Edit (doing it there, not with a quick toggle, is deliberate: saving also clears this
            server&rsquo;s environment variables, since Caduceus cannot read back what was set to
            preserve them).
          </p>

          {server.identity && (
            <div className="rounded-md border border-line bg-base/30 px-2.5 py-2 text-2xs text-ink-mute">
              <p>
                <span className="text-ink-faint">Server:</span> {server.identity.serverName || "(unnamed)"}{" "}
                {server.identity.serverVersion && `v${server.identity.serverVersion}`} ·{" "}
                <span className="text-ink-faint">Protocol:</span> {server.identity.protocolVersion}
              </p>
              {server.identity.instructions && (
                <div className="mt-1.5">
                  <p className="mb-1 text-ink-faint">
                    Message from the server itself (its own words, not reviewed by Caduceus):
                  </p>
                  <p className="whitespace-pre-wrap rounded bg-overlay px-2 py-1.5 leading-relaxed text-ink-soft">
                    {server.identity.instructions}
                  </p>
                </div>
              )}
            </div>
          )}

          <ToolList tools={tools} />

          {server.recentLog.length > 0 && (
            <div>
              <p className="mb-1 text-2xs text-ink-faint">
                Recent stderr (diagnostic text only — never parsed or acted on):
              </p>
              <pre className="max-h-32 overflow-y-auto whitespace-pre-wrap break-words rounded-md bg-base/60 px-2.5 py-2 font-mono text-2xs leading-relaxed text-ink-mute">
                {server.recentLog.join("\n")}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The tab
// ---------------------------------------------------------------------------

export function McpTab() {
  const [servers, setServers] = useState<McpServerInfo[] | null>(null);
  const [tools, setTools] = useState<McpTool[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [adding, setAdding] = useState(false);
  const [addDraft, setAddDraft] = useState<ServerDraft>(emptyDraft());
  const [addBusy, setAddBusy] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);

  const [editingName, setEditingName] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState<ServerDraft>(emptyDraft());
  const [editBusy, setEditBusy] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);

  // A load that does not disturb whatever's on screen — used for the
  // background poll below, so a "Connecting…" badge updates in place rather
  // than flashing the whole list back to a loading state every couple of
  // seconds.
  const refresh = async (silent: boolean) => {
    if (!silent) setLoadError(null);
    try {
      const [nextServers, nextTools] = await Promise.all([api.mcpListServers(), api.mcpListTools()]);
      setServers(nextServers);
      setTools(nextTools);
    } catch (e) {
      if (!silent) setLoadError(api.errorMessage(e));
    }
  };

  useEffect(() => {
    void refresh(false);
  }, []);

  // Poll while anything is mid-handshake, so a server that takes a couple of
  // seconds to answer visibly settles into Ready or Unhealthy without the
  // user having to leave and come back to this tab.
  const hasConnecting = (servers ?? []).some((s) => s.status.state === "connecting");
  useEffect(() => {
    if (!hasConnecting) return;
    const id = setInterval(() => void refresh(true), 1500);
    return () => clearInterval(id);
  }, [hasConnecting]);

  const toolsByServer = useMemo(() => {
    const map = new Map<string, McpTool[]>();
    for (const t of tools) {
      const list = map.get(t.server) ?? [];
      list.push(t);
      map.set(t.server, list);
    }
    return map;
  }, [tools]);

  const nameTaken = (name: string) => (servers ?? []).some((s) => s.name === name.trim());

  const submitAdd = async () => {
    setAddBusy(true);
    setAddError(null);
    try {
      await api.mcpAddServer(addDraft.name.trim(), addDraft.command.trim(), draftArgsToList(addDraft.args), draftEnvToRecord(addDraft.env));
      setAdding(false);
      setAddDraft(emptyDraft());
      await refresh(false);
    } catch (e) {
      setAddError(api.errorMessage(e));
    } finally {
      setAddBusy(false);
    }
  };

  const submitEdit = async () => {
    if (!editingName) return;
    setEditBusy(true);
    setEditError(null);
    try {
      await api.mcpUpdateServer(
        editingName,
        editDraft.command.trim(),
        draftArgsToList(editDraft.args),
        draftEnvToRecord(editDraft.env),
        (servers ?? []).find((s) => s.name === editingName)?.enabled ?? true,
      );
      setEditingName(null);
      await refresh(false);
    } catch (e) {
      setEditError(api.errorMessage(e));
    } finally {
      setEditBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <Callout tone="warn" title="Every server here is a program Caduceus runs on this Mac">
        Adding a server means Caduceus executes its command directly (no shell) and lets it read and
        write over a private pipe. Once running, its tools can do anything a normal program on this
        Mac can do. Nothing is added, enabled, or discovered automatically — only servers you type in
        below, and only when you press Add.
      </Callout>

      <Section
        title="Configured servers"
        description="What Caduceus currently knows about, whether or not it is running right now."
        actions={
          !adding && (
            <Button
              tone="primary"
              onClick={() => {
                setAddDraft(emptyDraft());
                setAddError(null);
                setAdding(true);
              }}
            >
              Add a server
            </Button>
          )
        }
      >
        {adding && (
          <div className="mb-3">
            <ServerForm
              draft={addDraft}
              setDraft={draftSetter(setAddDraft)}
              isNew
              nameTaken={nameTaken(addDraft.name)}
              onSubmit={() => void submitAdd()}
              onCancel={() => setAdding(false)}
              busy={addBusy}
              error={addError}
            />
          </div>
        )}

        {loadError && (
          <div className="mb-3">
            <Callout tone="danger">{loadError}</Callout>
          </div>
        )}

        {servers === null ? (
          <div className="flex justify-center py-8">
            <Spinner />
          </div>
        ) : servers.length === 0 && !adding ? (
          <EmptyState
            title="No servers configured"
            hint="Add one above. Nothing runs until you do."
            icon="⌁"
          />
        ) : (
          <div className="flex flex-col gap-2">
            {servers.map((s) =>
              editingName === s.name ? (
                <ServerForm
                  key={s.name}
                  draft={editDraft}
                  setDraft={draftSetter(setEditDraft)}
                  isNew={false}
                  nameTaken={false}
                  onSubmit={() => void submitEdit()}
                  onCancel={() => setEditingName(null)}
                  busy={editBusy}
                  error={editError}
                />
              ) : (
                <ServerRow
                  key={s.name}
                  server={s}
                  tools={toolsByServer.get(s.name) ?? []}
                  onChanged={() => void refresh(false)}
                  onEdit={() => {
                    setEditDraft(draftFromServer(s));
                    setEditError(null);
                    setEditingName(s.name);
                    setAdding(false);
                  }}
                />
              ),
            )}
          </div>
        )}
      </Section>

      <Callout>
        <strong>How a model gets to use these.</strong> The aggregated tool list (namespaced as{" "}
        <code>server__tool</code>) is only ever built from servers you enabled above — it is what the
        agent layer offers a model, not something the model can add to itself. Calling a tool is a
        separate, explicit step gated by the same show-then-approve confirmation the agent loop uses
        before any computer-use action; nothing on this screen calls a tool with real arguments.
      </Callout>
    </div>
  );
}
