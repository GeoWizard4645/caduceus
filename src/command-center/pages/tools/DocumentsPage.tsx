/**
 * Comprehension over things too long to read right now: a PDF, a web
 * article, or (honestly caveated below) a YouTube video.
 *
 * All three funnel into the same shape on the Rust side — extract text, then
 * summarise or answer a question about it through whichever AI backend is
 * configured — so this page mirrors that: three small sections instead of
 * three unrelated tools, each ending in the same kind of result panel.
 *
 * These calls can run long. A summary that chunks a real PDF map-reduces
 * across up to two dozen model calls (see `MAX_CHUNKS` in `tools::documents`)
 * before it produces anything, so every busy state here says so rather than
 * leaving a spinner to speak for itself.
 */

import { useEffect, useState } from "react";
import type { ReactNode } from "react";

import { open } from "@tauri-apps/plugin-dialog";

import * as api from "@/shared/api";
import { Button, Callout, Spinner } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

function basename(path: string): string {
  return path.split("/").pop() || path;
}

export function DocumentsPage({ onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Documents"), [onSetTitle]);

  // --- PDF ---------------------------------------------------------------
  const [pdfPath, setPdfPath] = useState<string | null>(null);
  const [pdfQuestion, setPdfQuestion] = useState("");
  const [pdfBusy, setPdfBusy] = useState<"summary" | "ask" | null>(null);
  const [pdfResult, setPdfResult] = useState<string | null>(null);
  const [pdfError, setPdfError] = useState<string | null>(null);

  const pickPdf = async () => {
    const picked = await open({ multiple: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (typeof picked === "string") {
      setPdfPath(picked);
      setPdfResult(null);
      setPdfError(null);
    }
  };

  const summarisePdf = async () => {
    if (!pdfPath) return;
    setPdfBusy("summary");
    setPdfError(null);
    setPdfResult(null);
    try {
      setPdfResult(await api.pdfSummary(pdfPath));
    } catch (error) {
      setPdfError(api.errorMessage(error));
    } finally {
      setPdfBusy(null);
    }
  };

  const askPdf = async () => {
    if (!pdfPath || !pdfQuestion.trim()) return;
    setPdfBusy("ask");
    setPdfError(null);
    setPdfResult(null);
    try {
      setPdfResult(await api.pdfAsk(pdfPath, pdfQuestion.trim()));
    } catch (error) {
      setPdfError(api.errorMessage(error));
    } finally {
      setPdfBusy(null);
    }
  };

  // --- article -------------------------------------------------------------
  const [articleUrl, setArticleUrl] = useState("");
  const [articleBusy, setArticleBusy] = useState(false);
  const [articleResult, setArticleResult] = useState<string | null>(null);
  const [articleError, setArticleError] = useState<string | null>(null);

  const summariseArticle = async () => {
    if (!articleUrl.trim()) return;
    setArticleBusy(true);
    setArticleError(null);
    setArticleResult(null);
    try {
      setArticleResult(await api.articleSummary(articleUrl.trim()));
    } catch (error) {
      setArticleError(api.errorMessage(error));
    } finally {
      setArticleBusy(false);
    }
  };

  // --- youtube ---------------------------------------------------------------
  const [youtubeUrl, setYoutubeUrl] = useState("");
  const [youtubeBusy, setYoutubeBusy] = useState(false);
  const [youtubeResult, setYoutubeResult] = useState<string | null>(null);
  const [youtubeError, setYoutubeError] = useState<string | null>(null);

  const summariseYoutube = async () => {
    if (!youtubeUrl.trim()) return;
    setYoutubeBusy(true);
    setYoutubeError(null);
    setYoutubeResult(null);
    try {
      setYoutubeResult(await api.youtubeSummary(youtubeUrl.trim()));
    } catch (error) {
      setYoutubeError(api.errorMessage(error));
    } finally {
      setYoutubeBusy(false);
    }
  };

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Documents</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Summarise a PDF or a web article, or ask a question about one, through whichever AI backend
          is configured in Settings.
        </p>
      </div>

      <Section title="PDF">
        <div className="row flex-wrap items-center gap-2">
          <Button size="sm" onClick={() => void pickPdf()}>
            {pdfPath ? "Choose another" : "Choose a PDF…"}
          </Button>
          {pdfPath && <span className="truncate text-2xs text-ink-faint">{basename(pdfPath)}</span>}
        </div>

        {pdfPath && (
          <div className="mt-3 space-y-2">
            <Button tone="primary" onClick={() => void summarisePdf()} disabled={pdfBusy !== null}>
              {pdfBusy === "summary" ? "Summarising…" : "Summarise"}
            </Button>

            <div className="row gap-2">
              <input
                value={pdfQuestion}
                onChange={(e) => setPdfQuestion(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void askPdf();
                }}
                spellCheck={false}
                placeholder="Ask a question about it…"
                className="min-w-[200px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-1.5 text-[13px] text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
              />
              <Button onClick={() => void askPdf()} disabled={pdfBusy !== null || !pdfQuestion.trim()}>
                {pdfBusy === "ask" ? "Asking…" : "Ask"}
              </Button>
            </div>
          </div>
        )}

        {pdfBusy && (
          <p className="mt-2 row gap-2 text-2xs text-ink-faint">
            <Spinner className="text-accent" />
            Long PDFs are read in sections, so this can take a little while.
          </p>
        )}
        {pdfError && <p className="mt-2 text-2xs text-danger">{pdfError}</p>}
        {pdfResult && <ResultBlock text={pdfResult} />}
      </Section>

      <Section title="Web article">
        <div className="row gap-2">
          <input
            value={articleUrl}
            onChange={(e) => setArticleUrl(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void summariseArticle();
            }}
            spellCheck={false}
            placeholder="https://example.com/an-article"
            className="min-w-[200px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-1.5 text-[13px] text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
          <Button tone="primary" onClick={() => void summariseArticle()} disabled={articleBusy || !articleUrl.trim()}>
            {articleBusy ? "Reading…" : "Summarise"}
          </Button>
        </div>
        <p className="mt-2 text-2xs text-ink-faint">
          Works on ordinary article pages. A page that renders its text with JavaScript after load can
          come back thin or empty — that is reported honestly rather than guessed at.
        </p>

        {articleBusy && (
          <p className="mt-2 row gap-2 text-2xs text-ink-faint">
            <Spinner className="text-accent" />
            Fetching and reading the page…
          </p>
        )}
        {articleError && <p className="mt-2 text-2xs text-danger">{articleError}</p>}
        {articleResult && <ResultBlock text={articleResult} />}
      </Section>

      <Section title="YouTube video">
        <Callout tone="warn" title="Does not currently work">
          As of mid-2026, YouTube blocks caption downloads from anything that is not a signed-in
          browser session, so this fails for essentially every video. It is left here — rather than
          removed — because the mechanism worked for years before that changed and may again; if it
          fails for you, that is the known, current state, not a bug in this build.
        </Callout>

        <div className="row mt-3 gap-2">
          <input
            value={youtubeUrl}
            onChange={(e) => setYoutubeUrl(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void summariseYoutube();
            }}
            spellCheck={false}
            placeholder="https://youtube.com/watch?v=…"
            className="min-w-[200px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-1.5 text-[13px] text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
          <Button onClick={() => void summariseYoutube()} disabled={youtubeBusy || !youtubeUrl.trim()}>
            {youtubeBusy ? "Trying…" : "Try anyway"}
          </Button>
        </div>

        {youtubeBusy && (
          <p className="mt-2 row gap-2 text-2xs text-ink-faint">
            <Spinner className="text-accent" />
            Asking YouTube for captions…
          </p>
        )}
        {youtubeError && <p className="mt-2 text-2xs text-danger">{youtubeError}</p>}
        {youtubeResult && <ResultBlock text={youtubeResult} />}
      </Section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mb-5">
      <p className="eyebrow mb-2">{title}</p>
      <div className="rounded-cad border border-line bg-surface/50 p-3">{children}</div>
    </section>
  );
}

function ResultBlock({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="mt-3 rounded-lg border border-line bg-raised/40 p-3">
      <div className="row justify-end">
        <Button
          size="sm"
          tone="ghost"
          onClick={() => {
            navigator.clipboard
              .writeText(text)
              .then(() => setCopied(true))
              .catch(() => setCopied(false));
          }}
        >
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
      <p className="whitespace-pre-wrap text-[13px] leading-relaxed text-ink-soft">{text}</p>
    </div>
  );
}
