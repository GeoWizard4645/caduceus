/**
 * The full chat window: saved threads on the left, the conversation on the right.
 *
 * The palette shows the same [`Thread`] inline. This window exists for the
 * things a 760px palette cannot do — reading back through old conversations,
 * and starting a new one without losing the last.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import type { ChatMessage, Conversation } from "@/shared/types";
import { EVENTS } from "@/shared/types";
import { Button, EmptyState, Spinner, cx } from "@/shared/ui";

import { Thread } from "./Thread";

export function Chat() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const loadConversations = useCallback(async () => {
    try {
      setConversations(await api.chatConversations());
    } catch {
      // An unreadable history should not blank the window you are typing in.
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

  // Opening the window from the palette lands on the thread being discussed.
  useTauriEvent<number | null>(EVENTS.chatOpen, (id) => {
    void loadConversations();
    if (id != null) setActiveId(id);
    inputRef.current?.focus();
  });

  // The palette and this window write to the same threads.
  useTauriEvent<number>(EVENTS.chatChanged, (id) => {
    void loadConversations();
    if (activeId == null || id === activeId) {
      if (activeId == null && id) setActiveId(id);
      else void loadMessages(activeId);
    }
  });

  // Land on the newest thread rather than an empty pane.
  useEffect(() => {
    if (activeId == null && conversations.length > 0) setActiveId(conversations[0].id);
  }, [conversations, activeId]);

  const send = async () => {
    const prompt = draft.trim();
    if (!prompt || pending) return;
    setDraft("");
    setPending(prompt);
    setError(null);
    try {
      const reply = await api.chatAsk(prompt, activeId);
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
    try {
      const id = await api.chatNewConversation();
      setActiveId(id);
      setMessages([]);
      setError(null);
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

  const saveToNotes = async (text: string) => {
    try {
      const out = await api.addToNotes(text);
      say(out.message);
    } catch (e) {
      say(String(e));
    }
  };

  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      say("Copied.");
    } catch {
      say("Could not copy.");
    }
  };

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-base text-ink">
      {/* --- thread list ------------------------------------------------- */}
      <aside className="flex w-[248px] shrink-0 flex-col border-r border-line bg-surface/60">
        {/* Room for the traffic lights: the title bar is an overlay. */}
        <div className="drag h-11 shrink-0" />

        <div className="row justify-between px-3 pb-2">
          <span className="text-2xs font-medium uppercase tracking-[0.1em] text-ink-faint">
            Chats
          </span>
          <Button size="sm" tone="ghost" onClick={() => void newChat()} title="Start a new chat">
            New
          </Button>
        </div>

        <div className="flex-1 overflow-y-auto px-2 pb-3">
          {conversations.length === 0 && (
            <p className="px-2 py-4 text-2xs leading-relaxed text-ink-faint">
              Nothing saved yet. Ask something with <code>/</code> in the Command Center, or
              start one here.
            </p>
          )}

          {conversations.map((c) => (
            <div
              key={c.id}
              className={cx(
                "group mb-1 cursor-pointer rounded-lg px-2.5 py-2 transition-colors",
                c.id === activeId ? "bg-raised" : "hover:bg-raised/60",
              )}
              onClick={() => setActiveId(c.id)}
            >
              <div className="row justify-between gap-2">
                <span className="truncate text-2xs font-medium text-ink">
                  {c.title || "New chat"}
                </span>
                <button
                  type="button"
                  title="Delete this chat"
                  onClick={(e) => {
                    e.stopPropagation();
                    void remove(c.id);
                  }}
                  className="shrink-0 rounded px-1 text-2xs text-ink-faint opacity-0 transition-opacity hover:text-danger group-hover:opacity-100"
                >
                  ✕
                </button>
              </div>
              {c.preview && (
                <p className="mt-0.5 truncate text-2xs text-ink-faint">{c.preview}</p>
              )}
            </div>
          ))}
        </div>
      </aside>

      {/* --- conversation ------------------------------------------------ */}
      <main className="flex min-w-0 flex-1 flex-col">
        <div className="drag h-11 shrink-0" />

        {activeId == null && conversations.length === 0 ? (
          <div className="flex flex-1 items-center justify-center">
            <EmptyState
              title="No chats yet"
              hint="Ask something below, or use / in the Command Center."
            />
          </div>
        ) : (
          <Thread
            className="flex-1"
            messages={messages}
            pending={pending}
            error={error}
            onCopy={(t) => void copy(t)}
            onSaveToNotes={(t) => void saveToNotes(t)}
          />
        )}

        <div className="shrink-0 border-t border-line px-4 py-3">
          <div className="row items-end gap-2">
            <textarea
              ref={inputRef}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                // Enter sends; Shift+Enter is a newline. Matches every chat app,
                // and a multi-line question is rare enough to cost a modifier.
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  void send();
                }
              }}
              rows={1}
              placeholder="Ask anything…"
              className={cx(
                "no-drag max-h-40 min-h-[38px] flex-1 resize-none rounded-cad border border-line-strong/60",
                "bg-raised px-3 py-2 text-[13px] text-ink outline-none",
                "placeholder:text-ink-faint focus:border-accent/60",
              )}
            />
            <Button tone="primary" onClick={() => void send()} disabled={!draft.trim() || !!pending}>
              {pending ? <Spinner /> : "Send"}
            </Button>
          </div>
          {flash && <p className="mt-2 text-2xs text-ink-faint">{flash}</p>}
        </div>
      </main>
    </div>
  );
}
