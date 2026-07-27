/**
 * The Settings window.
 *
 * A sidebar of seven tabs over a single settings draft. Changes save
 * automatically 500ms after you stop typing — see `useDraft.ts` for why there is
 * no Save button.
 */

import { useEffect, useState } from "react";

import * as api from "@/shared/api";
import { useAsync, useTauriEvent } from "@/shared/hooks";
import { EVENTS } from "@/shared/types";
import { Callout, Spinner, cx } from "@/shared/ui";
import { ThemeToggle } from "@/shared/ThemeToggle";

import { AppearanceTab } from "./tabs/Appearance";
import { AiTab } from "./tabs/Ai";
import { ClipboardTab } from "./tabs/Clipboard";
import { CommandCenterTab } from "./tabs/CommandCenterTab";
import { ExtensionsTab } from "./tabs/Extensions";
import { FeaturesCatalogTab } from "./tabs/FeaturesCatalog";
import { GeneralTab } from "./tabs/General";
import { HelpTab } from "./tabs/Help";
import { LearnTab, type TutorialId } from "./tabs/Learn";
import { ShortcutsTab } from "./tabs/Shortcuts";
import { VoiceTab } from "./tabs/Voice";
import { useDraft } from "./useDraft";

const TABS = [
  { id: "general", label: "General", icon: "◐" },
  { id: "shortcuts", label: "Shortcuts", icon: "✦" },
  { id: "command-center", label: "Command Center", icon: "⌕" },
  { id: "voice", label: "Voice", icon: "◍" },
  { id: "ai", label: "AI", icon: "✳" },
  { id: "clipboard", label: "Clipboard", icon: "❐" },
  { id: "appearance", label: "Appearance", icon: "◑" },
  { id: "extensions", label: "Extensions", icon: "⊞" },
  { id: "features", label: "Features", icon: "☰" },
  { id: "learn", label: "Learn", icon: "◆" },
  { id: "help", label: "Help", icon: "?" },
] as const;

type TabId = (typeof TABS)[number]["id"];

export function Settings() {
  const draft = useDraft();
  const [tab, setTab] = useState<TabId>("general");
  const [hotkeyProblems, setHotkeyProblems] = useState<string[]>([]);
  // Which tutorial to open when another tab links into Learn. Cleared on the
  // way out so returning to Learn by hand does not re-open it.
  const [learnFocus, setLearnFocus] = useState<TutorialId | null>(null);

  const openTutorial = (topic: TutorialId) => {
    setLearnFocus(topic);
    setTab("learn");
  };

  const goToTab = (next: string) => {
    if (!TABS.some((t) => t.id === next)) return;
    setLearnFocus(null);
    setTab(next as TabId);
  };

  const info = useAsync(() => api.getRuntimeInfo(), []);

  // The tray and the Command Center can ask for a specific tab.
  useTauriEvent<string>(EVENTS.settingsTab, (requested) => {
    if (TABS.some((t) => t.id === requested)) setTab(requested as TabId);
  });

  useTauriEvent<string[]>(EVENTS.hotkeyProblems, setHotkeyProblems);

  // Clashing-hotkey warnings come back with every save.
  useEffect(() => {
    if (draft.warnings.length) setHotkeyProblems(draft.warnings);
  }, [draft.warnings]);

  if (!draft.settings) {
    return (
      <div className="flex h-full items-center justify-center bg-base text-ink-faint">
        <Spinner />
      </div>
    );
  }

  const shared = { draft, info: info.data };

  return (
    <div className="flex h-full w-full overflow-hidden bg-base text-ink">
      {/* --- sidebar --------------------------------------------------- */}
      <nav className="drag-region flex w-[210px] shrink-0 flex-col border-r border-line bg-surface/60">
        {/* Space for the macOS traffic lights, which overlay the content. */}
        <div className="h-11 shrink-0" />

        <div className="px-3 pb-3">
          <div className="row justify-between gap-2 px-2">
            <p className="text-[15px] font-semibold tracking-[-0.01em] text-ink">Caduceus</p>
            <ThemeToggle />
          </div>
          <p className="px-2 text-2xs text-ink-faint">
            {info.data ? `Version ${info.data.version}` : " "}
          </p>
        </div>

        <div className="no-drag flex-1 space-y-0.5 overflow-y-auto px-2 pb-3">
          {TABS.map((item) => (
            <button
              key={item.id}
              type="button"
              onClick={() => goToTab(item.id)}
              className={cx(
                "flex w-full items-center gap-2.5 rounded-lg px-2.5 py-[7px] text-left text-[13px] transition-colors duration-100",
                tab === item.id
                  ? "bg-accent/14 font-medium text-ink"
                  : "text-ink-mute hover:bg-raised/70 hover:text-ink",
              )}
            >
              <span
                aria-hidden="true"
                className={cx("w-4 text-center", tab === item.id ? "text-accent" : "text-ink-faint")}
              >
                {item.icon}
              </span>
              {item.label}
            </button>
          ))}
        </div>

        <SaveIndicator draft={draft} />
      </nav>

      {/* --- content --------------------------------------------------- */}
      <main className="min-w-0 flex-1 overflow-y-auto">
        <div className="drag-region h-11 w-full" />

        <div className="mx-auto max-w-3xl px-8 pb-16">
          {hotkeyProblems.length > 0 && (
            <div className="mb-6">
              <Callout tone="warn" title="Some hotkeys could not be registered">
                <ul className="list-disc space-y-1 pl-4">
                  {hotkeyProblems.map((problem, i) => (
                    <li key={i}>{problem}</li>
                  ))}
                </ul>
              </Callout>
            </div>
          )}

          {tab === "general" && <GeneralTab {...shared} onOpenTutorial={openTutorial} />}
          {tab === "shortcuts" && <ShortcutsTab {...shared} />}
          {tab === "command-center" && <CommandCenterTab {...shared} />}
          {tab === "voice" && <VoiceTab {...shared} />}
          {tab === "ai" && <AiTab {...shared} onReloadInfo={info.reload} />}
          {tab === "clipboard" && <ClipboardTab {...shared} onReloadInfo={info.reload} />}
          {tab === "appearance" && <AppearanceTab draft={draft} />}
          {tab === "extensions" && <ExtensionsTab />}
          {tab === "features" && <FeaturesCatalogTab />}
          {tab === "learn" && (
            <LearnTab draft={draft} focus={learnFocus} onNavigate={goToTab} />
          )}
          {tab === "help" && (
            <HelpTab draft={draft} info={info.data} onNavigate={goToTab} />
          )}
        </div>
      </main>
    </div>
  );
}

function SaveIndicator({ draft }: { draft: ReturnType<typeof useDraft> }) {
  const { save } = draft;
  const [showSaved, setShowSaved] = useState(false);

  // "Saved" fades out; "saving" and errors persist while they are true.
  useEffect(() => {
    if (save.status !== "saved") return;
    setShowSaved(true);
    const timer = setTimeout(() => setShowSaved(false), 2000);
    return () => clearTimeout(timer);
  }, [save]);

  return (
    <div className="border-t border-line px-4 py-2.5 text-2xs">
      {save.status === "saving" ? (
        <span className="row text-ink-faint">
          <Spinner /> Saving…
        </span>
      ) : save.status === "error" ? (
        <span className="text-danger" title={save.message}>
          Could not save — {save.message}
        </span>
      ) : showSaved ? (
        <span className="text-positive">✓ Saved</span>
      ) : (
        <span className="text-ink-faint">Changes save automatically</span>
      )}
    </div>
  );
}
