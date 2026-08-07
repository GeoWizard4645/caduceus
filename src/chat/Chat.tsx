/**
 * Caduceus AI workspace inside the Command Center.
 *
 * Open it with the primary AI prefix + space in Search, from palette rows
 * (ai, chat, local), or after sending a prefixed question from the palette.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { AgentPanel } from "@/command-center/AgentPanel";
import * as api from "@/shared/api";
import { useSettings, useTauriEvent } from "@/shared/hooks";
import type { AgentStep, ChatChunk, ChatMessage, Conversation, Settings, TtsState } from "@/shared/types";
import type { Tab } from "@/shared/tabs";
import { EVENTS } from "@/shared/types";
import { Spinner, cx } from "@/shared/ui";

import { attachmentsToPrompt, filesToAttachments, type PickedAttachment } from "./chatAttachments";
import { Thread, type StreamStatus } from "./Thread";
import { type ChatMode, useChatModels } from "./useChatModels";
import { CaduceusMark } from "@/shared/CaduceusMark";

const QUICK_PROMPTS = [
  { label: "Summarize", icon: "≡", text: "Summarize this clearly: " },
  { label: "Draft", icon: "✎", text: "Help me write: " },
  { label: "Explain", icon: "◈", text: "Explain like I'm new to this: " },
  { label: "Code", icon: "{ }", text: "Write code for: " },
  { label: "Plan", icon: "☰", text: "Help me plan: " },
] as const;

function primaryAiPrefix(settings: Settings): string {
  return settings.commandCenter.prefixes.find((p) => p.action === "primary_ai")?.prefix ?? "/";
}

function displayName(_settings: Settings): string {
  return "there";
}

function timeGreeting(): string {
  const h = new Date().getHours();
  if (h < 12) return "Good morning";
  if (h < 17) return "Good afternoon";
  return "Good evening";
}

export function Chat({
  initialConversationId,
  initialPrefill,
  initialMode = "chat",
  onOpenTab,
}: {
  initialConversationId?: number;
  initialPrefill?: string;
  initialMode?: ChatMode;
  onOpenTab?: (request: Omit<Tab, "id">) => void;
} = {}) {
  const { settings } = useSettings();
  const [mode, setMode] = useState<ChatMode>(initialMode);
  const { choices, groups, activeBackendId, hermesModel, selectChoice } = useChatModels(mode);

  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState<string | null>(null);
  const [stream, setStream] = useState<StreamStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const [agentSession, setAgentSession] = useState<
    { id: string; task: string; kind: "computer" | "tool" } | null
  >(null);
  // Whether a spoken reply is currently playing — drives the "Speaking… Stop"
  // control. Kept separate from `pending`/`stream`: a reply can finish
  // generating and rendering while it is still partway through being read
  // aloud, and the reverse (agent sessions have no `stream` at all but can
  // still speak a final message — see the `agentStep` listener below).
  const [ttsState, setTtsState] = useState<TtsState>("idle");
  const [attachMeta, setAttachMeta] = useState<PickedAttachment[]>([]);
  const attachFiles = useRef<File[]>([]);

  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const loadConversations = useCallback(async () => {
    try {
      setConversations(await api.chatConversations());
    } catch {
      // ignore
    }
  }, []);

  const loadMessages = useCallback(async (id: number | null) => {
    if (id == null) {
      setMessages([]);
      return;
    }
    try {
      setMessages(await api.chatMessages(id));
    } catch {
      setMessages([]);
    }
  }, []);

  useEffect(() => {
    void loadConversations();
  }, [loadConversations]);

  useEffect(() => {
    void loadMessages(activeId);
  }, [activeId, loadMessages]);

  useEffect(() => {
    if (initialConversationId != null) {
      void loadConversations();
      setActiveId(initialConversationId);
    }
  }, [initialConversationId, loadConversations]);

  useEffect(() => {
    if (initialPrefill != null && initialPrefill !== "") {
      setDraft(initialPrefill);
      inputRef.current?.focus();
    }
  }, [initialPrefill]);

  useEffect(() => {
    setMode(initialMode);
  }, [initialMode]);

  useTauriEvent<number>(EVENTS.chatChanged, (id) => {
    void loadConversations();
    if (activeId == null || id === activeId) {
      if (activeId == null && id) setActiveId(id);
      else void loadMessages(activeId);
    }
  });

  // Live tokens from Rust while chat_ask is in flight. Conversation id may
  // only become known on the Started event (new thread), so we accept chunks
  // for whichever turn we currently have pending.
  useTauriEvent<ChatChunk>(EVENTS.chatChunk, (chunk) => {
    if (!pending) return;
    switch (chunk.type) {
      case "started":
        setActiveId(chunk.conversationId);
        setStream({
          text: "",
          startedAt: Date.now(),
          active: true,
        });
        break;
      case "delta":
        setStream((current) => ({
          text: (current?.text ?? "") + chunk.text,
          startedAt: current?.startedAt ?? Date.now(),
          usage: current?.usage,
          active: true,
        }));
        break;
      case "done":
        setStream({
          text: chunk.text,
          startedAt: Date.now() - chunk.elapsedMs,
          usage: chunk.usage,
          active: false,
        });
        break;
      case "error":
        setStream(null);
        break;
    }
  });

  useTauriEvent<TtsState>(EVENTS.ttsState, setTtsState);

  /**
   * Speak a finished reply aloud, if both voice switches say to.
   *
   * `ttsEnabled` gates `speak` itself (Rust rejects the call outright when it
   * is off — see `voice::TtsRuntime::speak`); `ttsSpeakReplies` is the
   * separate "narrate automatically" switch this consumes. `stopSpeaking`
   * runs first and is not awaited: it only interrupts whatever the *previous*
   * `speak` call installed as the active backend, which is a distinct
   * instance from the one about to start (see `TtsRuntime::speak`'s
   * ptr-equality guard) — so this is what stops a fast-arriving reply from
   * talking over one still being read, without delaying the new one.
   */
  const speakReply = useCallback(
    (text: string) => {
      if (!settings?.voice.ttsEnabled || !settings.voice.ttsSpeakReplies) return;
      const clean = text.trim();
      if (!clean) return;
      void api.stopSpeaking();
      void api.speak(clean).catch(() => {
        // The reply itself already landed and is on screen either way; a
        // backend that turned out to be misconfigured is not a chat error.
      });
    },
    [settings],
  );

  // Speak a tool/computer-use session's closing message the same way a plain
  // chat reply is spoken — see `speakReply`. Matched by session id because
  // `AgentStep::Finished` (unlike `AwaitingApproval`) carries one on its
  // `outcome`, not on the step itself; every other step type carries neither,
  // which is also why this listener only ever looks at `finished`.
  useTauriEvent<AgentStep>(EVENTS.agentStep, (step) => {
    if (step.type === "finished" && agentSession && step.outcome.sessionId === agentSession.id) {
      speakReply(step.outcome.finalMessage);
    }
  });

  useEffect(() => {
    if (activeId == null && conversations.length > 0) setActiveId(conversations[0].id);
  }, [conversations, activeId]);

  const showGreeting = messages.length === 0 && !pending && !error && !agentSession && !stream;

  const modelLabel = useMemo(() => {
    const row = choices.find((c) => c.backendId === activeBackendId && !c.connect);
    if (row) return row.label;
    if (activeBackendId === "hermes" && hermesModel) return `Hermes · ${hermesModel}`;
    return activeBackendId;
  }, [activeBackendId, choices, hermesModel]);

  const selectValue =
    choices.find((c) => c.backendId === activeBackendId && !c.connect)?.value ??
    (choices[0]?.value ?? "");

  const send = async () => {
    const prompt = draft.trim();
    if (!prompt && attachFiles.current.length === 0) return;
    if (pending || agentSession) return;

    // A new question moving on is as much "the user wants quiet" as
    // push-to-talk barging in is — see `speakReply`'s doc. Unconditional and
    // unawaited: `stopSpeaking` is a documented no-op when nothing is playing.
    void api.stopSpeaking();

    let full = prompt;
    if (attachFiles.current.length > 0) {
      const attachmentBlock = await attachmentsToPrompt(attachFiles.current);
      full = prompt ? `${prompt}\n\n${attachmentBlock}` : attachmentBlock;
    }

    setDraft("");
    setAttachMeta([]);
    attachFiles.current = [];
    setError(null);

    if (mode === "computer" || mode === "agent") {
      setPending(full);
      try {
        const sessionId =
          mode === "computer" ? await api.agentStartSession(full) : await api.agentStartToolSession(full);
        setAgentSession({ id: sessionId, task: full, kind: mode === "computer" ? "computer" : "tool" });
      } catch (e) {
        setError(String(e));
      } finally {
        setPending(null);
      }
      return;
    }

    setPending(full);
    setStream({ text: "", startedAt: Date.now(), active: true });
    try {
      const reply = await api.chatAsk(full, activeId);
      setActiveId(reply.conversationId);
      setStream({
        text: reply.text,
        startedAt: Date.now() - reply.elapsedMs,
        usage: reply.usage,
        active: false,
      });
      speakReply(reply.text);
      await loadMessages(reply.conversationId);
      await loadConversations();
      setStream(null);
    } catch (e) {
      setError(String(e));
      setStream(null);
    } finally {
      setPending(null);
      inputRef.current?.focus();
    }
  };

  const newChat = async () => {
    setAgentSession(null);
    setError(null);
    try {
      const id = await api.chatNewConversation();
      setActiveId(id);
      setMessages([]);
      await loadConversations();
      inputRef.current?.focus();
    } catch (e) {
      setError(String(e));
    }
  };

  const remove = async (id: number) => {
    try {
      await api.chatDeleteConversation(id);
      if (id === activeId) setActiveId(null);
      await loadConversations();
    } catch (e) {
      setError(String(e));
    }
  };

  const say = (msg: string) => {
    setFlash(msg);
    setTimeout(() => setFlash(null), 2600);
  };

  const onPickFiles = async (list: FileList | null) => {
    if (!list?.length) return;
    const files = [...list];
    attachFiles.current = [...attachFiles.current, ...files];
    setAttachMeta(await filesToAttachments(attachFiles.current));
  };

  const removeAttachment = (id: string) => {
    const idx = attachMeta.findIndex((a) => a.id === id);
    if (idx === -1) return;
    attachFiles.current = attachFiles.current.filter((_, i) => i !== idx);
    setAttachMeta((m) => m.filter((a) => a.id !== id));
  };

  if (!settings) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-base text-ink-faint">
        <Spinner />
      </div>
    );
  }

  const name = displayName(settings);
  const paletteAiHint = `${primaryAiPrefix(settings)} then space in Search`;

  return (
    <div className="flex h-full w-full overflow-hidden bg-base text-ink">
      <aside className="flex w-[252px] shrink-0 flex-col border-r border-line bg-surface/80">
        <div className="drag h-11 shrink-0" />

        <div className="flex items-center gap-2.5 px-4 pb-3">
          <CaduceusMark height={22} title="Caduceus" className="shrink-0" />
          <div className="min-w-0">
            <p className="truncate text-[13px] font-semibold tracking-[-0.01em] text-ink">Caduceus AI</p>
            <p className="truncate text-[11px] text-ink-faint">Chat, Agent & Cowork</p>
          </div>
        </div>

        <div className="px-3 pb-2">
          <button
            type="button"
            onClick={() => void newChat()}
            className="no-drag flex w-full items-center justify-center gap-2 rounded-lg border border-line bg-raised/80 px-3 py-2 text-[13px] font-medium text-ink transition-colors hover:border-accent/35 hover:bg-accent/8"
          >
            <span className="text-lg leading-none text-accent">+</span>
            New chat
          </button>
        </div>

        <div className="px-3 pb-2">
          <div className="inline-flex w-full rounded-lg border border-line bg-base/40 p-0.5 text-[12px]">
            <span className="flex-1 rounded-md bg-accent/15 px-2.5 py-1 text-center font-medium text-accent">
              Chat
            </span>
            <button
              type="button"
              className="flex-1 rounded-md px-2.5 py-1 text-ink-mute transition-colors hover:text-ink"
              onClick={() => onOpenTab?.({ kind: "settings", section: "ai" })}
            >
              Models
            </button>
          </div>
        </div>

        <p className="px-4 pb-1 text-[10px] font-semibold uppercase tracking-[0.08em] text-ink-faint">
          Recents
        </p>
        <div className="flex-1 overflow-y-auto px-2 pb-4">
          {conversations.length === 0 && (
            <p className="rounded-lg border border-line/80 bg-raised/30 px-2.5 py-3 text-[11px] leading-relaxed text-ink-mute">
              Open Search and type{" "}
              <code className="rounded bg-base/60 px-1 py-0.5 font-mono text-[10px] text-ink-soft">
                {paletteAiHint}
              </code>{" "}
              for a quick question without leaving the palette.
            </p>
          )}
          {conversations.map((c) => (
            <div
              key={c.id}
              role="button"
              tabIndex={0}
              onClick={() => {
                setAgentSession(null);
                setActiveId(c.id);
              }}
              onKeyDown={(e) => e.key === "Enter" && setActiveId(c.id)}
              className={cx(
                "group mb-0.5 cursor-pointer rounded-lg px-2.5 py-2 text-left transition-colors",
                c.id === activeId ? "bg-accent/12 ring-1 ring-accent/25" : "hover:bg-raised/80",
              )}
            >
              <div className="flex items-start justify-between gap-2">
                <span className="truncate text-[12px] font-medium text-ink-soft">
                  {c.title || "New chat"}
                </span>
                <button
                  type="button"
                  title="Delete"
                  onClick={(e) => {
                    e.stopPropagation();
                    void remove(c.id);
                  }}
                  className="shrink-0 rounded px-1 text-[11px] text-ink-faint opacity-0 group-hover:opacity-100 hover:text-ink"
                >
                  ✕
                </button>
              </div>
              {c.preview && (
                <p className="mt-0.5 truncate text-[11px] text-ink-faint">{c.preview}</p>
              )}
            </div>
          ))}
        </div>
      </aside>

      <main className="flex min-w-0 flex-1 flex-col">
        {/* A dedicated row rather than folding this into the composer toolbar:
            that toolbar does not exist while an agent session is showing
            below, and a reply's closing message can still be mid-sentence
            aloud in that state — see the `agentStep` listener above. Living
            on the shared drag bar means the control stays reachable
            regardless of which of the two is rendered underneath it. */}
        <div className="drag flex h-11 shrink-0 items-center justify-end px-4">
          {ttsState === "speaking" && (
            <button
              type="button"
              onClick={() => void api.stopSpeaking()}
              aria-label="Stop the spoken reply"
              title="Stop the spoken reply"
              className="no-drag inline-flex items-center gap-1.5 rounded-full border border-accent/30 bg-accent/10 px-2.5 py-1 text-[12px] font-medium text-accent transition-colors hover:bg-accent/16"
            >
              <span aria-hidden="true" className="relative flex h-1.5 w-1.5 shrink-0">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-60" />
                <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-accent" />
              </span>
              Speaking… Stop
            </button>
          )}
        </div>

        {agentSession ? (
          <div className="flex min-h-0 flex-1 flex-col px-4 pb-4">
            <AgentPanel
              sessionId={agentSession.id}
              task={agentSession.task}
              sessionKind={agentSession.kind}
              onClose={() => setAgentSession(null)}
            />
          </div>
        ) : (
          <>
            <div className="flex min-h-0 flex-1 flex-col">
              {showGreeting ? (
                <div className="flex flex-1 flex-col items-center justify-center px-6 pb-6 pt-2">
                  <div className="mb-5 rounded-2xl border border-line bg-surface/60 p-4 shadow-panel">
                    <CaduceusMark height={40} title="Caduceus" />
                  </div>
                  <h1 className="text-center text-[28px] font-semibold tracking-[-0.02em] text-ink sm:text-[32px]">
                    {timeGreeting()}, {name}
                  </h1>
                  <p className="mt-2 max-w-md text-center text-[14px] leading-relaxed text-ink-mute">
                    Ask anything, attach files, switch to Agent to use your connected tools, or
                    Cowork to act on your Mac. Routing via{" "}
                    <span className="text-ink-soft">{modelLabel}</span>.
                  </p>
                  {choices.length === 0 && (
                    <button
                      type="button"
                      onClick={() => onOpenTab?.({ kind: "settings", section: "ai" })}
                      className="no-drag mt-4 rounded-lg border border-accent/35 bg-accent/10 px-4 py-2 text-[13px] font-medium text-accent transition-colors hover:bg-accent/16"
                    >
                      Set up a model in Settings → AI
                    </button>
                  )}
                </div>
              ) : (
                <Thread
                  className="flex-1 text-ink"
                  messages={messages}
                  pending={pending}
                  stream={stream}
                  error={error}
                  onCopy={(t) => void navigator.clipboard.writeText(t).then(() => say("Copied."))}
                  onSaveToNotes={(t) =>
                    void api.addToNotes(t).then((o) => say(o.message)).catch((e) => say(String(e)))
                  }
                />
              )}
            </div>

            <div className="shrink-0 px-6 pb-5 pt-2">
              <div className="mx-auto max-w-[720px]">
                <div className="rounded-2xl border border-line bg-surface shadow-float ring-1 ring-white/[0.04]">
                  {attachMeta.length > 0 && (
                    <div className="flex flex-wrap gap-1.5 border-b border-line px-3 py-2">
                      {attachMeta.map((a) => (
                        <span
                          key={a.id}
                          className="inline-flex items-center gap-1 rounded-full bg-raised px-2 py-0.5 text-[11px] text-ink-soft"
                        >
                          {a.kind === "image" ? "🖼" : "📎"} {a.preview}
                          <button
                            type="button"
                            className="text-ink-faint hover:text-ink"
                            onClick={() => removeAttachment(a.id)}
                          >
                            ×
                          </button>
                        </span>
                      ))}
                    </div>
                  )}

                  <textarea
                    ref={inputRef}
                    value={draft}
                    onChange={(e) => setDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && !e.shiftKey) {
                        e.preventDefault();
                        void send();
                      }
                    }}
                    rows={2}
                    placeholder={
                      mode === "computer"
                        ? "Tell Caduceus what to do on your Mac…"
                        : mode === "agent"
                          ? "Tell Caduceus what to do with your connected tools…"
                          : `Ask Caduceus anything · Search: ${paletteAiHint}`
                    }
                    className={cx(
                      "no-drag w-full resize-none bg-transparent px-4 pt-4 text-[15px] leading-relaxed",
                      "text-ink outline-none placeholder:text-ink-faint",
                    )}
                  />

                  <div className="flex flex-wrap items-center justify-between gap-2 px-3 pb-3 pt-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <input
                        ref={fileRef}
                        type="file"
                        multiple
                        className="hidden"
                        onChange={(e) => void onPickFiles(e.target.files)}
                      />
                      <button
                        type="button"
                        title="Attach files or images"
                        onClick={() => fileRef.current?.click()}
                        className="no-drag flex h-8 w-8 items-center justify-center rounded-lg border border-line text-ink-mute hover:bg-raised/80 hover:text-ink"
                      >
                        +
                      </button>

                      <div
                        role="group"
                        aria-label="Response mode"
                        className="inline-flex rounded-lg border border-line bg-base/50 p-0.5 text-[12px]"
                      >
                        <button
                          type="button"
                          aria-pressed={mode === "chat"}
                          title="Plain conversation, no tools"
                          onClick={() => setMode("chat")}
                          className={cx(
                            "rounded-md px-2.5 py-1 font-medium transition-colors",
                            mode === "chat" ? "bg-accent/15 text-accent" : "text-ink-mute hover:text-ink",
                          )}
                        >
                          Chat
                        </button>
                        <button
                          type="button"
                          aria-pressed={mode === "agent"}
                          title="Calls tools connected in Settings → MCP"
                          onClick={() => setMode("agent")}
                          className={cx(
                            "rounded-md px-2.5 py-1 font-medium transition-colors",
                            mode === "agent" ? "bg-accent/15 text-accent" : "text-ink-mute hover:text-ink",
                          )}
                        >
                          Agent
                        </button>
                        <button
                          type="button"
                          aria-pressed={mode === "computer"}
                          title="Drives your screen directly"
                          onClick={() => setMode("computer")}
                          className={cx(
                            "rounded-md px-2.5 py-1 font-medium transition-colors",
                            mode === "computer"
                              ? "bg-accent/15 text-accent"
                              : "text-ink-mute hover:text-ink",
                          )}
                        >
                          Cowork
                        </button>
                      </div>
                    </div>

                    <div className="flex items-center gap-2">
                      {choices.length > 0 ? (
                        <select
                          value={selectValue}
                          onChange={(e) => void selectChoice(e.target.value)}
                          className="no-drag max-w-[200px] truncate rounded-lg border border-line bg-base px-2 py-1.5 text-[11px] text-ink-soft outline-none"
                          title="Model / backend"
                        >
                          {groups.map((g) => (
                            <optgroup key={g.label} label={g.label}>
                              {g.choices.map((c) => (
                                <option key={c.value} value={c.value}>
                                  {c.label}
                                </option>
                              ))}
                            </optgroup>
                          ))}
                        </select>
                      ) : (
                        <span className="text-[11px] text-ink-faint">{modelLabel}</span>
                      )}
                      <button
                        type="button"
                        onClick={() => void send()}
                        disabled={(!draft.trim() && attachFiles.current.length === 0) || !!pending}
                        className="no-drag flex h-8 w-8 items-center justify-center rounded-lg bg-accent text-accent-ink shadow-glow disabled:opacity-40"
                        title="Send"
                      >
                        {pending ? <Spinner /> : "↑"}
                      </button>
                    </div>
                  </div>
                </div>

                {showGreeting && (
                  <div className="mt-3 flex flex-wrap justify-center gap-2">
                    {QUICK_PROMPTS.map((p) => (
                      <button
                        key={p.label}
                        type="button"
                        onClick={() => {
                          setDraft(p.text);
                          inputRef.current?.focus();
                        }}
                        className="no-drag inline-flex items-center gap-1.5 rounded-full border border-line bg-raised/40 px-3 py-1.5 text-[12px] text-ink-soft transition-colors hover:border-accent/30 hover:bg-accent/8 hover:text-ink"
                      >
                        <span className="text-ink-faint">{p.icon}</span>
                        {p.label}
                      </button>
                    ))}
                    <button
                      type="button"
                      onClick={() => onOpenTab?.({ kind: "settings", section: "ai" })}
                      className="no-drag inline-flex items-center gap-1.5 rounded-full border border-dashed border-line px-3 py-1.5 text-[12px] text-ink-mute hover:border-accent/35 hover:text-ink"
                    >
                      + Add model
                    </button>
                  </div>
                )}

                {flash && <p className="mt-2 text-center text-[11px] text-ink-faint">{flash}</p>}
              </div>
            </div>
          </>
        )}
      </main>
    </div>
  );
}
