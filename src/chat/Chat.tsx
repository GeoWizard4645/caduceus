/**
 * Claude-style AI workspace inside the Command Center.
 *
 * Open it with `/` + space in the palette, or after sending a `/` question.
 * Chat and Cowork (computer use) share one composer; model routing follows
 * Settings → AI and whatever localhost scan finds.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { AgentPanel } from "@/command-center/AgentPanel";
import * as api from "@/shared/api";
import { useSettings, useTauriEvent } from "@/shared/hooks";
import type { ChatMessage, Conversation, Settings } from "@/shared/types";
import type { Tab } from "@/shared/tabs";
import { EVENTS } from "@/shared/types";
import { Spinner, cx } from "@/shared/ui";

import { attachmentsToPrompt, filesToAttachments, type PickedAttachment } from "./chatAttachments";
import { Thread } from "./Thread";
import { type ChatMode, useChatModels } from "./useChatModels";

const QUICK_PROMPTS = [
  { label: "Write", icon: "✎", text: "Help me write: " },
  { label: "Learn", icon: "🎓", text: "Explain like I'm new to this: " },
  { label: "Code", icon: "{ }", text: "Write code for: " },
  { label: "Life stuff", icon: "☕", text: "Help me plan: " },
] as const;

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
  const { choices, activeBackendId, hermesModel, selectChoice } = useChatModels(mode);

  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const [agentSession, setAgentSession] = useState<{ id: string; task: string } | null>(null);
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

  useEffect(() => {
    if (activeId == null && conversations.length > 0) setActiveId(conversations[0].id);
  }, [conversations, activeId]);

  const showGreeting = messages.length === 0 && !pending && !error && !agentSession;

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

    let full = prompt;
    if (attachFiles.current.length > 0) {
      const attachmentBlock = await attachmentsToPrompt(attachFiles.current);
      full = prompt ? `${prompt}\n\n${attachmentBlock}` : attachmentBlock;
    }

    setDraft("");
    setAttachMeta([]);
    attachFiles.current = [];
    setError(null);

    if (mode === "computer") {
      setPending(full);
      try {
        const sessionId = await api.agentStartSession(full);
        setAgentSession({ id: sessionId, task: full });
      } catch (e) {
        setError(String(e));
      } finally {
        setPending(null);
      }
      return;
    }

    setPending(full);
    try {
      const reply = await api.chatAsk(full, activeId);
      setActiveId(reply.conversationId);
      await loadMessages(reply.conversationId);
      await loadConversations();
    } catch (e) {
      setError(String(e));
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
      <div className="flex h-full w-full items-center justify-center bg-[#262624] text-ink-faint">
        <Spinner />
      </div>
    );
  }

  const name = displayName(settings);

  return (
    <div className="flex h-full w-full overflow-hidden bg-[#262624] text-[#ececec]">
      <aside className="flex w-[248px] shrink-0 flex-col border-r border-white/[0.08] bg-[#1f1f1d]">
        <div className="drag h-11 shrink-0" />

        <div className="px-3 pb-2">
          <button
            type="button"
            onClick={() => void newChat()}
            className="no-drag flex w-full items-center gap-2 rounded-lg border border-white/[0.1] bg-white/[0.04] px-3 py-2 text-[13px] font-medium text-[#ececec] transition-colors hover:bg-white/[0.07]"
          >
            <span className="text-lg leading-none">+</span>
            New chat
          </button>
        </div>

        <div className="px-3 pb-2">
          <div className="inline-flex rounded-lg bg-white/[0.06] p-0.5 text-[12px]">
            <span className="rounded-md bg-white/[0.12] px-2.5 py-1 font-medium">Chat</span>
            <button
              type="button"
              className="rounded-md px-2.5 py-1 text-[#a8a8a6] transition-colors hover:text-[#ececec]"
              onClick={() => onOpenTab?.({ kind: "settings", section: "ai" })}
            >
              Models
            </button>
          </div>
        </div>

        <p className="px-4 pb-1 text-[10px] font-semibold uppercase tracking-[0.08em] text-[#737371]">
          Recents
        </p>
        <div className="flex-1 overflow-y-auto px-2 pb-4">
          {conversations.length === 0 && (
            <p className="px-2 py-3 text-[12px] leading-relaxed text-[#737371]">
              Type <code className="text-[#a8a8a6]">/</code> then space in the Command Center to open
              this view.
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
                c.id === activeId ? "bg-white/[0.1]" : "hover:bg-white/[0.06]",
              )}
            >
              <div className="flex items-start justify-between gap-2">
                <span className="truncate text-[12px] font-medium text-[#d4d4d2]">
                  {c.title || "New chat"}
                </span>
                <button
                  type="button"
                  title="Delete"
                  onClick={(e) => {
                    e.stopPropagation();
                    void remove(c.id);
                  }}
                  className="shrink-0 rounded px-1 text-[11px] text-[#737371] opacity-0 group-hover:opacity-100 hover:text-[#ececec]"
                >
                  ✕
                </button>
              </div>
              {c.preview && (
                <p className="mt-0.5 truncate text-[11px] text-[#737371]">{c.preview}</p>
              )}
            </div>
          ))}
        </div>
      </aside>

      <main className="flex min-w-0 flex-1 flex-col">
        <div className="drag h-11 shrink-0" />

        {agentSession ? (
          <div className="flex min-h-0 flex-1 flex-col px-4 pb-4">
            <AgentPanel
              sessionId={agentSession.id}
              task={agentSession.task}
              onClose={() => setAgentSession(null)}
            />
          </div>
        ) : (
          <>
            <div className="flex min-h-0 flex-1 flex-col">
              {showGreeting ? (
                <div className="flex flex-1 flex-col items-center justify-center px-6 pb-8 pt-4">
                  <div className="flex items-center gap-3">
                    <span className="text-2xl" aria-hidden="true">
                      ✦
                    </span>
                    <h1
                      className="text-[32px] font-normal leading-tight text-[#ececec]"
                      style={{ fontFamily: "'Iowan Old Style', 'Palatino Linotype', Palatino, Georgia, serif" }}
                    >
                      {timeGreeting()}, {name}
                    </h1>
                  </div>
                </div>
              ) : (
                <Thread
                  className="flex-1 text-[#ececec]"
                  messages={messages}
                  pending={pending}
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
                <div className="rounded-2xl border border-white/[0.12] bg-[#30302e] shadow-[0_8px_32px_rgba(0,0,0,0.35)]">
                  {attachMeta.length > 0 && (
                    <div className="flex flex-wrap gap-1.5 border-b border-white/[0.08] px-3 py-2">
                      {attachMeta.map((a) => (
                        <span
                          key={a.id}
                          className="inline-flex items-center gap-1 rounded-full bg-white/[0.08] px-2 py-0.5 text-[11px] text-[#c8c8c6]"
                        >
                          {a.kind === "image" ? "🖼" : "📎"} {a.preview}
                          <button
                            type="button"
                            className="text-[#737371] hover:text-[#ececec]"
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
                    placeholder={mode === "computer" ? "Tell Caduceus what to do on your Mac…" : "Type / for skills in the palette · ask anything here"}
                    className={cx(
                      "no-drag w-full resize-none bg-transparent px-4 pt-4 text-[15px] leading-relaxed",
                      "text-[#ececec] outline-none placeholder:text-[#737371]",
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
                        className="no-drag flex h-8 w-8 items-center justify-center rounded-lg border border-white/[0.1] text-[#c8c8c6] hover:bg-white/[0.06]"
                      >
                        +
                      </button>

                      <div className="inline-flex rounded-lg bg-[#262624] p-0.5 text-[12px]">
                        <button
                          type="button"
                          onClick={() => setMode("chat")}
                          className={cx(
                            "rounded-md px-2.5 py-1 font-medium transition-colors",
                            mode === "chat" ? "bg-[#404040] text-[#ececec]" : "text-[#737371] hover:text-[#c8c8c6]",
                          )}
                        >
                          Chat
                        </button>
                        <button
                          type="button"
                          onClick={() => setMode("computer")}
                          className={cx(
                            "rounded-md px-2.5 py-1 font-medium transition-colors",
                            mode === "computer"
                              ? "bg-[#404040] text-[#ececec]"
                              : "text-[#737371] hover:text-[#c8c8c6]",
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
                          className="no-drag max-w-[200px] truncate rounded-lg border border-white/[0.1] bg-[#262624] px-2 py-1.5 text-[11px] text-[#c8c8c6] outline-none"
                          title="Model / backend"
                        >
                          {choices.map((c) => (
                            <option key={c.value} value={c.value}>
                              {c.label}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <span className="text-[11px] text-[#737371]">{modelLabel}</span>
                      )}
                      <button
                        type="button"
                        onClick={() => void send()}
                        disabled={(!draft.trim() && attachFiles.current.length === 0) || !!pending}
                        className="no-drag flex h-8 w-8 items-center justify-center rounded-lg bg-[#d97757] text-[#1a1a18] disabled:opacity-40"
                        title="Send"
                      >
                        {pending ? <Spinner /> : "↑"}
                      </button>
                    </div>
                  </div>
                </div>

                <div className="mt-3 flex flex-wrap justify-center gap-2">
                  {QUICK_PROMPTS.map((p) => (
                    <button
                      key={p.label}
                      type="button"
                      onClick={() => {
                        setDraft(p.text);
                        inputRef.current?.focus();
                      }}
                      className="no-drag inline-flex items-center gap-1.5 rounded-full border border-white/[0.1] bg-white/[0.04] px-3 py-1.5 text-[12px] text-[#c8c8c6] transition-colors hover:bg-white/[0.08]"
                    >
                      <span className="text-[#737371]">{p.icon}</span>
                      {p.label}
                    </button>
                  ))}
                  <button
                    type="button"
                    onClick={() => onOpenTab?.({ kind: "settings", section: "ai" })}
                    className="no-drag inline-flex items-center gap-1.5 rounded-full border border-white/[0.1] bg-white/[0.04] px-3 py-1.5 text-[12px] text-[#737371] hover:text-[#c8c8c6]"
                  >
                    + Add model
                  </button>
                </div>

                {flash && <p className="mt-2 text-center text-[11px] text-[#737371]">{flash}</p>}
              </div>
            </div>
          </>
        )}
      </main>
    </div>
  );
}
