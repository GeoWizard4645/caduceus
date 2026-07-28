//! Document and media comprehension: PDF extraction + ask-me-anything, web
//! article extraction + summary, and (a documented gap) YouTube transcripts.
//!
//! Three different "get readable text out of something" problems, funnelled
//! into two shared primitives once the text is in hand: [`chunk_text`] to
//! split it small enough for a model, and the map-reduce helpers at the
//! bottom to summarise or answer questions about it through whichever AI
//! backend the user has configured. Nothing here hardcodes a provider — that
//! decision belongs to `agent::chat_with_history` alone, so this module reads
//! the same "no backend configured" message every other AI-backed tool does.
//!
//! No new crates were added for any of this. PDF text comes from Spotlight's
//! own PDF importer (already on every Mac, no OCR needed for a text-layer
//! PDF); web articles are read with `reqwest`, already a dependency, and
//! stripped down by hand; YouTube transcripts turned out not to be gettable
//! without a signed-in browser session as of this writing, which is reported
//! below rather than papered over.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::agent::{self, Message};
use crate::settings::SettingsManager;

// ---------------------------------------------------------------------------
// Shared: chunking long text for a model with a finite context window
// ---------------------------------------------------------------------------

/// Characters per chunk, not tokens — this module never sees which model is
/// on the other end, and a character count is a conservative enough proxy
/// for "small enough" across every tokenizer in practice (English text runs
/// roughly 4 characters per token, so this lands well under most context
/// windows even before the rest of the prompt and the model's own answer are
/// counted in).
const CHUNK_CHARS: usize = 12_000;

/// Characters carried over from the end of one chunk to the start of the
/// next. A fact or a sentence that happens to fall right on a chunk boundary
/// would otherwise be split in half and lost to both halves; repeating a
/// little text costs nothing an LLM call doesn't already dwarf.
const CHUNK_OVERLAP: usize = 800;

/// How many chunks a single summarise-or-ask call will process before it
/// stops and says so, rather than turning a 500-page PDF into 500 sequential
/// model calls no one asked for. This is a product decision, not a technical
/// limit: raise it if "read the whole thing, however long" turns out to be
/// what people want badly enough to wait for.
const MAX_CHUNKS: usize = 24;

/// Split `text` into pieces of at most `chunk_chars` characters, each
/// overlapping the previous by roughly `overlap_chars`.
///
/// Operates on `char`s throughout, not bytes — this runs on PDF and web text
/// of unknown provenance, and a naive byte slice panics the moment a
/// multi-byte character (an accented letter, an em dash, anything outside
/// ASCII) sits on the cut point. A 50-page PDF is exactly the kind of input
/// where that eventually happens rather than might.
pub fn chunk_text(text: &str, chunk_chars: usize, overlap_chars: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n <= chunk_chars {
        return vec![text.to_string()];
    }

    // Never let the overlap swallow (or exceed) a whole chunk — that would
    // stop `start` from making forward progress.
    let overlap = overlap_chars.min(chunk_chars.saturating_sub(1));
    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < n {
        let hard_end = (start + chunk_chars).min(n);
        let end = if hard_end >= n {
            n
        } else {
            // Only look for a break in the back half of the chunk. A break
            // found near `start` would produce a tiny chunk and immediately
            // hand back most of the same text next iteration.
            best_break(&chars, start + chunk_chars / 2, hard_end).unwrap_or(hard_end)
        };

        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim();
        if !piece.is_empty() {
            chunks.push(piece.to_string());
        }

        if end >= n {
            break;
        }
        // Guards against a degenerate input (e.g. a chunk with no
        // whitespace at all) where the overlap would otherwise land back on
        // or before `start` and spin in place forever.
        start = end.saturating_sub(overlap).max(start + 1);
    }

    chunks
}

/// The rightmost paragraph break, sentence end, or plain whitespace inside
/// `[from, to)`, in that preference order — a chunk boundary mid-word is the
/// one outcome worse than a boundary mid-sentence, and a boundary mid-sentence
/// is worse than one between paragraphs.
fn best_break(chars: &[char], from: usize, to: usize) -> Option<usize> {
    let from = from.min(to);

    for i in (from..to).rev() {
        if chars[i] == '\n' && i > 0 && chars[i - 1] == '\n' {
            return Some(i + 1);
        }
    }
    for i in (from..to).rev() {
        if matches!(chars[i], '.' | '!' | '?') && matches!(chars.get(i + 1), Some(' ') | Some('\n')) {
            return Some(i + 1);
        }
    }
    for i in (from..to).rev() {
        if chars[i].is_whitespace() {
            return Some(i + 1);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Shared: HTML entities and tag stripping (used by both the article reader
// and the YouTube caption parser — neither pulls in an HTML crate for it)
// ---------------------------------------------------------------------------

/// Decode the handful of HTML entities that show up in article bodies and
/// caption tracks, plus arbitrary numeric entities.
///
/// Not exhaustive against the full named-entity table (there are over 2,000
/// of them) — the ones listed are what actually appears in running prose;
/// anything else is left as literal text, which is a visible glitch rather
/// than a wrong character silently substituted in its place.
fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < input.len() {
        if bytes[i] == b'&' {
            if let Some(semi_rel) = input[i..].find(';') {
                // Entities are short; a `;` more than ~12 bytes away is
                // almost certainly an unrelated one later in the text, not
                // this `&` being part of an entity at all.
                if semi_rel <= 12 {
                    let entity = &input[i + 1..i + semi_rel];
                    if let Some(ch) = named_entity(entity) {
                        out.push_str(ch);
                        i += semi_rel + 1;
                        continue;
                    }
                    if let Some(rest) = entity.strip_prefix('#') {
                        let code_point = if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X'))
                        {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            rest.parse::<u32>().ok()
                        };
                        if let Some(c) = code_point.and_then(char::from_u32) {
                            out.push(c);
                            i += semi_rel + 1;
                            continue;
                        }
                    }
                }
            }
        }
        let ch = input[i..].chars().next().expect("i is a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" | "#39" => "'",
        "nbsp" => " ",
        "mdash" => "\u{2014}",
        "ndash" => "\u{2013}",
        "hellip" => "\u{2026}",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "copy" => "\u{a9}",
        "reg" => "\u{ae}",
        "trade" => "\u{2122}",
        _ => return None,
    })
}

/// Remove `<...>` markup with nothing smarter than "ignore everything
/// between angle brackets" — good enough for the small inline tags
/// (`<i>`, `<font>`) that occasionally wrap a `<title>` or a caption line,
/// which is the only place this is used.
fn strip_simple_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for ch in s.chars() {
        match ch {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// PDF text extraction
// ---------------------------------------------------------------------------

/// How long Spotlight's PDF importer gets before it is assumed wedged.
///
/// Longer than [`super::TOOL_TIMEOUT`] on purpose: everything else in
/// `tools/` is a quick system query, but running a PDF through a text
/// importer scales with page count, and a few hundred pages of a technical
/// document is a real thing people open. In testing, a 32-page 10&nbsp;MB
/// scanned-and-OCR'd textbook chapter took under a second — this bound only
/// matters for the pathological case.
const PDF_EXTRACT_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug)]
pub struct PdfText {
    pub pages: Option<u32>,
    pub text: String,
}

/// Run a command with a deadline, draining stdout and stderr concurrently.
///
/// `super::output_with_timeout` looks like it should do this job, but it
/// polls `try_wait()` in a loop and only reads the child's pipes afterward,
/// via `wait_with_output`. That is fine for the short, small-output commands
/// it was written for, and wrong here: `mdimport`/`plutil` on a genuinely
/// long PDF (a scanned-and-OCR'd 32-page textbook chapter measured at this)
/// write well over the ~64 KB an OS pipe buffers before the writer blocks.
/// With nothing draining the pipe, the child blocks mid-write, `try_wait`
/// never observes it exit, and every call ran out the clock and was killed —
/// discovered by running extraction against real files during development,
/// not by inspection. `std::process::Child::wait_with_output` avoids exactly
/// this by reading both pipes on their own threads while it waits; this
/// function does the same thing by hand so a deadline can still be enforced
/// around it.
fn run_bounded(mut command: Command, timeout: Duration, wedged: &str) -> Result<std::process::Output, String> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start it: {e}"))?;

    let mut stdout_pipe = child.stdout.take().expect("stdout was requested as piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was requested as piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(format!("Could not wait for it: {e}")),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // The reader threads are still blocked on their now-orphaned
            // pipe ends; they will unblock once the killed process's pipes
            // close, which happens as part of the kill/wait above. Not
            // joined here so a wedged reader can never make a *timeout*
            // path hang too.
            return Err(wedged.to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(std::process::Output { status, stdout, stderr })
}

/// Pull the text layer out of a PDF using Spotlight's own importer.
///
/// # Why this and not a PDF-parsing crate
///
/// macOS already ships a PDF text extractor and runs it on every PDF on disk
/// to make it searchable in Spotlight — `PDF.mdimporter`, backed by the same
/// PDFKit that also handles the `.pdf` icon and QuickLook preview. `mdimport
/// -t` asks it to run on one file without touching the index, and `-o` writes
/// every attribute the importer produced — including `kMDItemTextContent`,
/// which is the one this needs and is *not* part of the smaller `-d2` debug
/// summary — to a plist rather than only printing it. `plutil -convert json
/// -o -` turns that into JSON on stdout; it is also stock macOS, and it is
/// what actually makes this reliable: the plist uses Objective-C's `\Uxxxx`
/// escaping and unbalanced-looking nested braces for arrays of dictionaries,
/// which is a parser someone would have to write and get exactly right,
/// whereas `plutil`'s conversion already handles it (verified below on real
/// files with accented text, non-Latin scripts, and umlauts, all of which
/// round-tripped correctly through the JSON).
///
/// `-d1` (rather than `-d3`) is deliberate and was not the first thing
/// tried: `-d3` additionally prints every attribute to *stdout* as well as
/// the `-o` file, and for a large scanned textbook chapter that is well over
/// 64&nbsp;KB of text — more than a pipe buffer holds. `output_with_timeout`
/// does not drain stdout until the process exits, so `mdimport` blocked
/// writing to a full pipe while nothing was reading it, and every call
/// timed out. `-d1`'s few-line summary never approaches that, and `-o`
/// captures the full text either way — confirmed by writing both `-d1` and
/// `-d3`, converting each through `plutil`, and diffing the resulting JSON.
///
/// # What this cannot do
///
/// If the PDF has no text layer — a scan with no OCR pass ever run on it —
/// `kMDItemTextContent` comes back empty, and that is reported as an error
/// rather than a silently empty summary. Caduceus's own Vision OCR
/// (`tools::native::ocr_image`) reads an image, not a PDF, and the only
/// macOS tool available here for rendering a PDF page to an image (`sips`)
/// only ever produces the first page — running OCR through it would silently
/// summarise page one of a scanned PDF as if it were the whole document,
/// which is worse than refusing. That gap is left for a future pass that can
/// add a proper multi-page PDF renderer.
pub fn extract_pdf_text(path: &str) -> Result<PdfText, String> {
    let file = Path::new(path);
    if !file.is_file() {
        return Err("That PDF does not exist.".into());
    }
    let is_pdf = file
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return Err("That is not a PDF file.".into());
    }

    let plist_path = std::env::temp_dir().join(format!("caduceus-pdf-{}.plist", uuid::Uuid::new_v4()));
    let cleanup = || {
        let _ = std::fs::remove_file(&plist_path);
    };

    let mut mdimport = Command::new("mdimport");
    mdimport.args(["-t", "-d1", "-o"]).arg(&plist_path).arg(file);
    let import = run_bounded(mdimport, PDF_EXTRACT_TIMEOUT, "Reading that PDF is taking too long.");
    let import = match import {
        Ok(out) => out,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    if !import.status.success() || !plist_path.is_file() {
        cleanup();
        let reason = String::from_utf8_lossy(&import.stderr).trim().to_string();
        return Err(if reason.is_empty() {
            "macOS could not read that PDF.".to_string()
        } else {
            format!("macOS could not read that PDF: {reason}")
        });
    }

    let mut plutil = Command::new("plutil");
    plutil.args(["-convert", "json", "-o", "-"]).arg(&plist_path);
    let converted = run_bounded(plutil, PDF_EXTRACT_TIMEOUT, "Reading that PDF is taking too long.");
    cleanup();
    let converted = converted?;
    if !converted.status.success() {
        return Err("macOS extracted the PDF's contents but could not convert them to a readable form.".into());
    }

    let json: serde_json::Value = serde_json::from_slice(&converted.stdout)
        .map_err(|e| format!("Could not read what Spotlight found in that PDF: {e}"))?;

    // `plutil -convert json` on an *old-style* (NeXTSTEP) plist — which is
    // what `mdimport -o` writes — carries no type information beyond
    // "string" and "array of these", so every scalar comes out as a JSON
    // string, page count included. `"1"` here, never `1`.
    let pages = json
        .get("kMDItemNumberOfPages")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u32>().ok());
    let text = json
        .get("kMDItemTextContent")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if text.is_empty() {
        let page_note = match pages {
            Some(1) => " (1 page)".to_string(),
            Some(p) => format!(" ({p} pages)"),
            None => String::new(),
        };
        return Err(format!(
            "That PDF{page_note} has no extractable text — it is most likely a scan with no OCR text \
             layer. Caduceus's on-device OCR reads individual images (Command Center → OCR), but does \
             not yet run across every page of a scanned PDF."
        ));
    }

    Ok(PdfText { pages, text })
}

// ---------------------------------------------------------------------------
// Web article extraction
// ---------------------------------------------------------------------------

/// Long enough for a slow news site's server-side rendering, short enough
/// that a wedged host does not hang the request indefinitely. Matches the
/// spirit of `extensions::net`'s bound without borrowing its constant, since
/// that one is sized for API calls and this is sized for HTML documents.
const ARTICLE_TIMEOUT: Duration = Duration::from_secs(30);

/// A refused response past this size is a document not meant to be read as
/// an article (a video, a large single-page app's bundle, an infinite feed) —
/// erroring is more honest than truncating mid-tag and extracting garbage.
const ARTICLE_MAX_BODY: usize = 15 * 1024 * 1024;

#[derive(Debug)]
pub struct Article {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
}

/// Fetch a web page and return its readable text.
///
/// # Why hand-rolled extraction rather than a crate
///
/// The brief for this module was explicit: fetch with `reqwest` (already a
/// dependency) and write the stripping heuristics by hand rather than adding
/// an HTML-parsing crate. What follows is a small state machine over the raw
/// markup, not a DOM: it removes script/style/nav/etc. blocks by name
/// (tracking the matching close tag by hand, since Rust's `regex` crate is
/// intentionally linear-time and has no backreferences to match `<div>` to
/// its own `</div>`), prefers the contents of `<article>` or `<main>` when
/// either is present, turns block-level tags into paragraph breaks before
/// stripping the rest, and decodes entities last. It is a heuristic, not a
/// browser — a page that hides its content behind JavaScript rendering will
/// come back thin or empty, and that is reported as "no readable text"
/// rather than guessed at.
pub async fn fetch_article(url: &str) -> Result<Article, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| format!("\u{201c}{}\u{201d} is not a URL.", url.trim()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http and https pages can be read as articles.".into());
    }

    let client = reqwest::Client::builder()
        .timeout(ARTICLE_TIMEOUT)
        // A generic browser UA: a number of publishers serve a near-empty
        // page (or an outright block) to anything that looks like a bot,
        // which would otherwise turn every fetch into "no readable text".
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
        .build()
        .map_err(|e| format!("Could not start the request: {e}"))?;

    let response = client.get(parsed).send().await.map_err(describe_fetch_error)?;
    if let Some(len) = response.content_length() {
        if len as usize > ARTICLE_MAX_BODY {
            return Err(article_too_big());
        }
    }
    let final_url = response.url().to_string();
    let bytes = response.bytes().await.map_err(describe_fetch_error)?;
    if bytes.len() > ARTICLE_MAX_BODY {
        return Err(article_too_big());
    }

    let html = String::from_utf8_lossy(&bytes);
    let (title, text) = extract_readable_text(&html);
    if text.trim().is_empty() {
        return Err("Could not find any readable article text on that page.".into());
    }

    Ok(Article { url: final_url, title, text })
}

fn article_too_big() -> String {
    format!("That page is over {} MB — too large to be an article.", ARTICLE_MAX_BODY / (1024 * 1024))
}

fn describe_fetch_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        return "That page took too long to load.".into();
    }
    if error.is_connect() {
        return "Could not reach that page. Check the address and your connection.".into();
    }
    error.to_string()
}

/// Tags whose entire subtree is noise for reading comprehension: chrome
/// (nav, forms, embedded players), not-yet-rendered script/style/template
/// content, and sidebars, which are far more often related-links widgets and
/// ad units than article body.
const NOISE_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "iframe", "form", "select", "textarea", "button", "nav",
    "aside", "object", "embed", "video", "audio",
];

fn extract_readable_text(html: &str) -> (Option<String>, String) {
    let title = extract_title(html);
    let without_comments = strip_html_comments(html);
    let without_noise = strip_noise_elements(&without_comments);

    let body = extract_element(&without_noise, "article")
        .or_else(|| extract_element(&without_noise, "main"))
        // Falling all the way back to the raw document would pull in
        // `<head>` — titles, meta descriptions, inline JSON-LD — as if it
        // were body text. `<body>` is still a heuristic guess at "the
        // readable part" compared to `<article>`/`<main>`, but it is a
        // strictly better one than the whole document.
        .or_else(|| extract_element(&without_noise, "body"))
        .unwrap_or(without_noise.as_str());

    let with_breaks = block_tags_to_newlines(body);
    let bare = strip_remaining_tags(&with_breaks);
    let text = collapse_whitespace(&decode_entities(&bare));

    (title, text)
}

fn extract_title(html: &str) -> Option<String> {
    let inner = extract_element(html, "title")?;
    let text = decode_entities(&strip_simple_tags(inner)).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn strip_html_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out, // unterminated comment runs to end of document
        }
    }
    out.push_str(rest);
    out
}

/// True if the byte at `pos` (if any) cannot be part of a longer tag name —
/// i.e. `<nav` really is the start of `<nav>` or `<nav class=...>`, not
/// `<navigation>`.
fn tag_name_boundary(bytes: &[u8], pos: usize) -> bool {
    bytes.get(pos).is_none_or(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'))
}

/// Remove every element in [`NOISE_TAGS`], contents included.
///
/// Matches openings to closings by name and byte position rather than with a
/// regex backreference — the `regex` crate deliberately does not support
/// those (it stays linear-time by construction), so "find the `</div>` that
/// belongs to *this* `<div>`" is done by hand here. It is a nearest-match,
/// not a nesting-aware one: the first `</tag>` after the opening tag is
/// taken as its close. That is exact for every tag in the noise list (none
/// of `nav`, `script`, `aside`, etc. legitimately nest inside themselves in
/// real-world HTML), which is what keeps this simple rather than needing a
/// depth counter.
fn strip_noise_elements(html: &str) -> String {
    let lower = html.to_ascii_lowercase(); // same byte length as `html`: ASCII-only case folding never changes UTF-8 layout
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;

    while i < html.len() {
        let Some(rel) = lower[i..].find('<') else {
            out.push_str(&html[i..]);
            break;
        };
        let lt = i + rel;
        out.push_str(&html[i..lt]);

        match skip_noise_element(html, &lower, lt) {
            Some(next) => i = next,
            None => {
                out.push('<');
                i = lt + 1;
            }
        }
    }

    out
}

/// If `at` (the index of a `<`) opens one of [`NOISE_TAGS`], return the index
/// just past its matching close tag; otherwise `None`.
fn skip_noise_element(html: &str, lower: &str, at: usize) -> Option<usize> {
    let after = &lower[at + 1..];
    for tag in NOISE_TAGS {
        if !after.starts_with(tag) || !tag_name_boundary(after.as_bytes(), tag.len()) {
            continue;
        }

        let open_end_rel = after.find('>')?;
        let open_tag_body = &after[..open_end_rel];
        let open_end = at + 1 + open_end_rel + 1;

        // A self-closing `<nav/>` (invalid HTML, but seen in generated
        // markup) has nothing to skip past the tag itself.
        if open_tag_body.trim_end().ends_with('/') {
            return Some(open_end);
        }

        let close_needle = format!("</{tag}");
        return match lower[open_end..].find(&close_needle) {
            Some(close_rel) => {
                let close_at = open_end + close_rel;
                match lower[close_at..].find('>') {
                    Some(gt_rel) => Some(close_at + gt_rel + 1),
                    None => Some(html.len()), // truncated document: nothing more to keep
                }
            }
            // No closing tag at all — treat the rest of the document as part
            // of this (malformed) element rather than leaving it dangling.
            None => Some(html.len()),
        };
    }
    None
}

/// The contents of the first `<tag>...</tag>`, by the same nearest-match
/// rule as [`strip_noise_elements`].
fn extract_element<'a>(html: &'a str, tag: &str) -> Option<&'a str> {
    let lower = html.to_ascii_lowercase();
    let open_needle = format!("<{tag}");
    let mut search_from = 0usize;
    loop {
        let rel = lower[search_from..].find(&open_needle)?;
        let start = search_from + rel;
        if tag_name_boundary(lower.as_bytes(), start + open_needle.len()) {
            let open_end = lower[start..].find('>')? + start + 1;
            let close_needle = format!("</{tag}>");
            let close = lower[open_end..].find(&close_needle)? + open_end;
            return Some(&html[open_end..close]);
        }
        // Matched a longer tag name sharing this prefix (e.g. `<article-x>`
        // if such a thing existed); keep looking past it.
        search_from = start + open_needle.len();
    }
}

fn block_tags_to_newlines(html: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?is)</?(p|div|section|article|header|footer|li|ul|ol|h[1-6]|tr|table|thead|tbody|blockquote|pre|br)\b[^>]*/?>")
            .expect("static regex is valid")
    });
    re.replace_all(html, "\n").into_owned()
}

fn strip_remaining_tags(html: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?s)<[^>]*>").expect("static regex is valid"));
    re.replace_all(html, "").into_owned()
}

/// Collapse runs of blank lines to at most one, and trim trailing whitespace
/// off every line — the difference between "readable paragraphs" and a wall
/// of the single blank line every stripped `<div>` leaves behind.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_blank = true; // suppresses leading blank lines too
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !last_was_blank {
                out.push('\n');
            }
            last_was_blank = true;
        } else {
            out.push_str(line);
            out.push('\n');
            last_was_blank = false;
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// YouTube transcripts — investigated, verified against live YouTube, and
// found not to work without a signed-in browser session. Documented rather
// than shipped as a silent failure; see `YOUTUBE_BLOCKED_MESSAGE`.
// ---------------------------------------------------------------------------

const YOUTUBE_TIMEOUT: Duration = Duration::from_secs(20);

/// What every verification attempt against live YouTube produced.
///
/// Checked on 2026-07-27: three different videos, their signed `timedtext`
/// URL taken straight from the `captionTracks` list embedded in that video's
/// own watch page (the same mechanism every no-API-key transcript tool has
/// used for years), requested immediately afterward — so the URL's signature
/// and expiry could not be the problem — with a browser user agent and a
/// matching `Referer` header, over both HTTP/1.1 and HTTP/2. Every request
/// came back `200 OK` with `Content-Length: 0`. That is consistent with
/// YouTube's anti-scraping posture since 2024: the caption endpoint now
/// expects a "proof of origin" token minted by a real browser session
/// executing YouTube's own JavaScript, which nothing running outside an
/// actual browser can produce. The YouTube Data API (which does need a key)
/// does not close this gap either — it exposes caption *metadata*, not
/// caption *text*; downloading the track itself needs OAuth scoped to the
/// video's owner, which is a different feature for a different audience than
/// "summarise this video I'm watching".
///
/// The fetch below is still attempted rather than skipped outright, on the
/// chance a particular video, region, or future change in YouTube's
/// enforcement makes it work again — but every attempt so far ends here.
const YOUTUBE_BLOCKED_MESSAGE: &str = "YouTube did not return any caption data for that video. As of \
    mid-2026, YouTube blocks this kind of request unless it comes from a signed-in browser session — \
    there is currently no reliable way for Caduceus to fetch a transcript without an API key (and the \
    official API does not expose caption text either) or a real browser. This is a known gap, not a \
    silent failure.";

/// Fetch a YouTube video's transcript, if YouTube will hand one over.
///
/// See [`YOUTUBE_BLOCKED_MESSAGE`] for why that is currently "no" for every
/// video tested. Kept as a real attempt rather than a stub: the mechanism is
/// exactly what worked for years before YouTube tightened this, so it costs
/// nothing to leave it wired up for the day that changes (or for a request
/// that, unlike the ones tested, happens to get through).
pub async fn youtube_transcript(url: &str) -> Result<String, String> {
    let video_id = extract_video_id(url).ok_or("That does not look like a YouTube video URL.")?;

    let client = reqwest::Client::builder()
        .timeout(YOUTUBE_TIMEOUT)
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15")
        .build()
        .map_err(|e| format!("Could not start the request: {e}"))?;

    let watch_url = format!("https://www.youtube.com/watch?v={video_id}");
    let page = client
        .get(&watch_url)
        .send()
        .await
        .map_err(|e| format!("Could not reach YouTube: {e}"))?
        .text()
        .await
        .map_err(|e| format!("Could not read YouTube's response: {e}"))?;

    let Some(track_url) = find_caption_track_url(&page) else {
        return Err("That video does not appear to have captions available.".into());
    };

    let xml = client
        .get(&track_url)
        .header("Referer", &watch_url)
        .send()
        .await
        .map_err(|e| format!("Could not reach YouTube's caption service: {e}"))?
        .text()
        .await
        .map_err(|e| format!("Could not read the caption response: {e}"))?;

    let transcript = parse_timedtext_xml(&xml);
    if transcript.trim().is_empty() {
        return Err(YOUTUBE_BLOCKED_MESSAGE.into());
    }
    Ok(transcript)
}

/// Pull a YouTube video ID out of any of the URL shapes people actually
/// paste: `watch?v=`, `youtu.be/`, `/embed/`, `/shorts/`, `/live/`.
fn extract_video_id(url: &str) -> Option<String> {
    let url = url.trim();
    let is_valid = |s: &str| s.len() == 11 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');

    if let Some(pos) = url.find("v=") {
        let candidate: String = url[pos + 2..].chars().take_while(|&c| c != '&' && c != '#').collect();
        if is_valid(&candidate) {
            return Some(candidate);
        }
    }
    for marker in ["youtu.be/", "youtube.com/embed/", "youtube.com/shorts/", "youtube.com/live/"] {
        if let Some(pos) = url.find(marker) {
            let candidate: String = url[pos + marker.len()..]
                .chars()
                .take_while(|&c| c != '?' && c != '&' && c != '#' && c != '/')
                .collect();
            if is_valid(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// The first caption track's `baseUrl` out of the `captionTracks` array
/// embedded (as JSON, inside a `<script>`) in a YouTube watch page.
///
/// String-scanned rather than parsed as JSON: the surrounding document is
/// not JSON (it is a `var ytInitialPlayerResponse = {...};` assignment
/// inside HTML), and pulling just this one field out avoids needing to
/// balance the enclosing object, which is not otherwise interesting here.
fn find_caption_track_url(html: &str) -> Option<String> {
    let start = html.find("\"captionTracks\":")? + "\"captionTracks\":".len();
    let end = html[start..].find(']').map(|i| start + i).unwrap_or(html.len());
    let region = &html[start..end];

    let needle = "\"baseUrl\":\"";
    let value_start = region.find(needle)? + needle.len();
    let value_end = region[value_start..].find('"')? + value_start;
    Some(unescape_js_string(&region[value_start..value_end]))
}

/// Undo JSON/JS string escaping (`&` → `&`, …) in the `baseUrl` pulled
/// out by [`find_caption_track_url`], which is lifted out of a JS literal
/// rather than parsed by a JSON library.
fn unescape_js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => out.push(decoded),
                    None => out.push_str(&hex),
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Turn a YouTube `timedtext` XML transcript (`<transcript><text start="…"
/// dur="…">…</text>…</transcript>`) into plain text, one caption line per
/// line.
///
/// String-scanned like the rest of this module's HTML/XML handling, for the
/// same reason: it is three field names in a fixed, well-documented format,
/// not a document worth a general XML parser for.
fn parse_timedtext_xml(xml: &str) -> String {
    let mut lines = Vec::new();
    let mut rest = xml;

    while let Some(open_rel) = rest.find("<text") {
        rest = &rest[open_rel..];
        let Some(tag_end) = rest.find('>') else { break };
        let Some(close_start) = rest.find("</text>") else { break };
        if close_start < tag_end {
            break; // malformed: a later `</text>` belongs to an earlier tag
        }

        let inner = &rest[tag_end + 1..close_start];
        let text = decode_entities(&strip_simple_tags(inner));
        let text = text.trim();
        if !text.is_empty() {
            lines.push(text.to_string());
        }

        rest = &rest[close_start + "</text>".len()..];
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Summarisation and Q&A, through the user's configured AI backend
// ---------------------------------------------------------------------------

/// Summarise a long piece of text via the primary AI backend, chunking and
/// map-reducing it first if it does not fit in one call.
///
/// `subject` names what `text` is ("PDF document", "web article", …) purely
/// for the wording of the prompts sent to the model — it has no effect on
/// the mechanics.
async fn summarize_text(settings: &SettingsManager, text: &str, subject: &str) -> Result<String, String> {
    let mut chunks = chunk_text(text, CHUNK_CHARS, CHUNK_OVERLAP);
    if chunks.is_empty() {
        return Err(format!("There is no text in that {subject} to summarise."));
    }

    let truncated = chunks.len() > MAX_CHUNKS;
    if truncated {
        chunks.truncate(MAX_CHUNKS);
    }
    let total = chunks.len();

    // A document that fits in one chunk skips the map step entirely and is
    // summarised directly — running it through "summarise this part, then
    // summarise the summary" would only blur a short document that a single
    // call already handles well.
    let source = if total == 1 {
        format!(
            "Summarise this {subject} in a few clear paragraphs, covering the main points a reader would \
             want to know:\n\n{}",
            chunks[0]
        )
    } else {
        let mut partials = Vec::with_capacity(total);
        for (i, chunk) in chunks.iter().enumerate() {
            let prompt = format!(
                "This is part {}/{} of a longer {subject}. Summarise the key points of this excerpt in \
                 three to six sentences, so it can later be combined with summaries of the other parts. \
                 State the content directly — do not refer to \"this excerpt\" or \"this part\".\n\n{chunk}",
                i + 1,
                total
            );
            let response = agent::chat_with_history(settings, vec![Message::user(prompt)])
                .await
                .map_err(|e| e.user_message())?;
            partials.push(response.text);
        }
        format!(
            "Here are summaries of {total} consecutive sections of a longer {subject}. Write one coherent \
             summary that reads as a single piece, not a list of parts:\n\n{}",
            partials.join("\n\n")
        )
    };

    let response = agent::chat_with_history(settings, vec![Message::user(source)])
        .await
        .map_err(|e| e.user_message())?;

    let mut result = response.text;
    if truncated {
        result.push_str(&format!(
            "\n\n(This {subject} was long enough that only the first {MAX_CHUNKS} sections were summarised.)"
        ));
    }
    Ok(result)
}

/// Answer a question about a long piece of text, gathering relevant excerpts
/// chunk by chunk first when the whole thing does not fit one call.
///
/// This is the map-reduce shape of retrieval without an embeddings index —
/// deliberately: adding one would mean a vector-search crate for a single
/// feature, which the brief for this module rules out. Asking the model
/// "does this excerpt matter, and if so what does it say" per chunk is
/// slower and more model calls than real retrieval, but needs nothing beyond
/// what [`summarize_text`] already uses.
async fn ask_document(settings: &SettingsManager, text: &str, question: &str, subject: &str) -> Result<String, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("Type a question to ask about it.".into());
    }

    let mut chunks = chunk_text(text, CHUNK_CHARS, CHUNK_OVERLAP);
    if chunks.is_empty() {
        return Err(format!("There is no text in that {subject} to answer from."));
    }

    let truncated = chunks.len() > MAX_CHUNKS;
    if truncated {
        chunks.truncate(MAX_CHUNKS);
    }
    let total = chunks.len();

    if total == 1 {
        let prompt = format!(
            "Answer the question using only the following {subject}. If the answer is not in the text, \
             say so honestly rather than guessing.\n\nQuestion: {question}\n\n{subject}:\n{}",
            chunks[0]
        );
        let response = agent::chat_with_history(settings, vec![Message::user(prompt)])
            .await
            .map_err(|e| e.user_message())?;
        return Ok(response.text);
    }

    let mut excerpts = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let prompt = format!(
            "This is part {}/{} of a longer {subject}. The reader's question is: \"{question}\"\n\nIf this \
             excerpt contains information relevant to the question, extract the relevant facts or quotes \
             concisely. If it has nothing relevant, reply with exactly: NOTHING RELEVANT\n\nExcerpt:\n{chunk}",
            i + 1,
            total
        );
        let response = agent::chat_with_history(settings, vec![Message::user(prompt)])
            .await
            .map_err(|e| e.user_message())?;
        let text = response.text.trim().to_string();
        if !text.eq_ignore_ascii_case("nothing relevant") {
            excerpts.push(format!("From part {}/{}: {text}", i + 1, total));
        }
    }

    if excerpts.is_empty() {
        return Ok(format!("Nothing in this {subject} appears to answer that question."));
    }

    let prompt = format!(
        "Answer the question using only the excerpts gathered from a longer {subject}. If the excerpts do \
         not contain enough to answer confidently, say so honestly rather than guessing.\n\nQuestion: \
         {question}\n\nExcerpts:\n{}",
        excerpts.join("\n\n")
    );
    let response = agent::chat_with_history(settings, vec![Message::user(prompt)])
        .await
        .map_err(|e| e.user_message())?;

    let mut result = response.text;
    if truncated {
        result.push_str(&format!(
            "\n\n(This {subject} was long enough that only the first {MAX_CHUNKS} sections were searched.)"
        ));
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Public entry points — the wrappers a `commands.rs` IPC layer would call
// ---------------------------------------------------------------------------

/// Summarise a PDF on disk.
pub async fn pdf_summary(settings: &SettingsManager, path: &str) -> Result<String, String> {
    let doc = extract_pdf_text(path)?;
    summarize_text(settings, &doc.text, "PDF document").await
}

/// Ask a question about a PDF on disk.
pub async fn pdf_ask(settings: &SettingsManager, path: &str, question: &str) -> Result<String, String> {
    let doc = extract_pdf_text(path)?;
    ask_document(settings, &doc.text, question, "PDF document").await
}

/// Fetch and summarise a web article.
pub async fn article_summary(settings: &SettingsManager, url: &str) -> Result<String, String> {
    let article = fetch_article(url).await?;
    let subject = match &article.title {
        Some(title) => format!("web article (\u{201c}{title}\u{201d})"),
        None => "web article".to_string(),
    };
    summarize_text(settings, &article.text, &subject).await
}

/// Summarise a YouTube video's transcript.
///
/// Will currently fail with [`YOUTUBE_BLOCKED_MESSAGE`] for every video —
/// see the comment on [`youtube_transcript`] for the investigation behind
/// that. Left wired up rather than removed so it starts working the moment
/// the underlying fetch does, with no code change needed here.
pub async fn youtube_summary(settings: &SettingsManager, url: &str) -> Result<String, String> {
    let transcript = youtube_transcript(url).await?;
    summarize_text(settings, &transcript, "YouTube video transcript").await
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- chunk_text -----------------------------------------------------

    #[test]
    fn short_text_is_a_single_chunk() {
        let chunks = chunk_text("just a short sentence.", 1000, 100);
        assert_eq!(chunks, vec!["just a short sentence.".to_string()]);
    }

    #[test]
    fn empty_text_produces_no_chunks() {
        assert!(chunk_text("   ", 1000, 100).is_empty());
        assert!(chunk_text("", 1000, 100).is_empty());
    }

    #[test]
    fn long_text_is_split_on_paragraph_breaks_when_possible() {
        let para_a = "Alpha ".repeat(200); // well past the chunk size on its own
        let para_b = "Beta ".repeat(200);
        let text = format!("{}\n\n{}", para_a.trim(), para_b.trim());

        // Overlap is zero here so the split lands exactly on the paragraph
        // break; `consecutive_chunks_overlap` below covers the non-zero case.
        let chunks = chunk_text(&text, para_a.len() + 20, 0);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].trim_end().ends_with("Alpha"));
        assert!(chunks[1].trim_start().starts_with("Beta"));
    }

    #[test]
    fn consecutive_chunks_overlap() {
        let text = (0..500).map(|n| format!("sentence number {n}.")).collect::<Vec<_>>().join(" ");
        let chunks = chunk_text(&text, 500, 100);
        assert!(chunks.len() > 1, "expected multiple chunks from {} characters", text.len());

        // The end of one chunk and the start of the next should share some
        // text — that is the overlap doing its job, not a coincidence.
        for pair in chunks.windows(2) {
            let tail: String = pair[0].chars().rev().take(30).collect::<String>().chars().rev().collect();
            let tail_words: Vec<&str> = tail.split_whitespace().collect();
            let next_has_overlap = tail_words.iter().any(|w| pair[1].contains(w));
            assert!(next_has_overlap, "no shared text between {:?} and {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn a_pathological_single_run_of_non_whitespace_still_terminates() {
        // No whitespace anywhere to break on, so every chunk is a hard cut —
        // this is the case that would spin forever if the overlap logic ever
        // let `start` stand still or go backward.
        let text: String = std::iter::repeat('x').take(50_000).collect();
        let chunks = chunk_text(&text, 1000, 900);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().map(|c| c.len()).sum::<usize>() >= text.len());
    }

    #[test]
    fn chunk_size_below_overlap_does_not_hang() {
        let text: String = std::iter::repeat('y').take(10_000).collect();
        // overlap_chars > chunk_chars is a caller mistake; must not hang.
        let chunks = chunk_text(&text, 50, 500);
        assert!(!chunks.is_empty());
    }

    // -- HTML entity / tag helpers ---------------------------------------

    #[test]
    fn named_and_numeric_entities_decode() {
        assert_eq!(decode_entities("Tom &amp; Jerry"), "Tom & Jerry");
        assert_eq!(decode_entities("caf&#233;"), "caf\u{e9}");
        assert_eq!(decode_entities("caf&#xe9;"), "caf\u{e9}");
        assert_eq!(decode_entities("&ldquo;quoted&rdquo;"), "\u{201c}quoted\u{201d}");
        // An ampersand that is not an entity at all is left alone rather
        // than swallowed looking for a `;` that will never come.
        assert_eq!(decode_entities("Q&A session"), "Q&A session");
    }

    #[test]
    fn strip_simple_tags_removes_inline_markup_only() {
        assert_eq!(strip_simple_tags("<i>hello</i> <b>world</b>"), "hello world");
    }

    // -- extract_readable_text -------------------------------------------

    const SAMPLE_ARTICLE_HTML: &str = r#"
        <html>
        <head>
            <title>How Caduceus Reads Articles &amp; PDFs</title>
            <style>.hidden { display: none; }</style>
            <script>trackEverything();</script>
        </head>
        <body>
            <nav><ul><li><a href="/">Home</a></li><li><a href="/about">About</a></li></ul></nav>
            <header><div class="logo">Caduceus</div></header>
            <div class="ad-banner"><iframe src="https://ads.example.com"></iframe></div>
            <article>
                <h1>How Caduceus Reads Articles</h1>
                <p>This is the first paragraph of the real article text.</p>
                <p>This is the second paragraph, with a &ldquo;quoted&rdquo; word.</p>
            </article>
            <aside class="related">
                <h3>Related</h3>
                <p>Some unrelated sidebar content that should not appear.</p>
            </aside>
            <footer>Copyright 2026</footer>
        </body>
        </html>
    "#;

    #[test]
    fn readable_text_keeps_the_article_and_drops_the_chrome() {
        let (title, text) = extract_readable_text(SAMPLE_ARTICLE_HTML);

        assert_eq!(title.as_deref(), Some("How Caduceus Reads Articles & PDFs"));
        assert!(text.contains("first paragraph of the real article text"));
        assert!(text.contains("\u{201c}quoted\u{201d} word"));

        assert!(!text.contains("Home"), "nav content leaked into the article text");
        assert!(!text.contains("trackEverything"), "script content leaked into the article text");
        assert!(!text.contains("display: none"), "style content leaked into the article text");
        assert!(!text.contains("unrelated sidebar"), "aside content leaked into the article text");
        assert!(!text.contains("Copyright"), "footer content leaked outside the <article>");
    }

    #[test]
    fn readable_text_falls_back_to_main_without_an_article_tag() {
        let html = r#"<html><body><nav>skip me</nav><main><p>Main content here.</p></main></body></html>"#;
        let (_, text) = extract_readable_text(html);
        assert!(text.contains("Main content here."));
        assert!(!text.contains("skip me"));
    }

    #[test]
    fn readable_text_on_a_page_with_no_body_text_is_empty_not_panicking() {
        let (_, text) = extract_readable_text("<html><head><title>Empty</title></head><body></body></html>");
        assert!(text.is_empty());
    }

    // -- YouTube: video ID extraction, JS-string unescaping, XML parsing --

    #[test]
    fn video_ids_are_read_from_every_common_url_shape() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=30s"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(extract_video_id("https://youtu.be/dQw4w9WgXcQ?t=5"), Some("dQw4w9WgXcQ".to_string()));
        assert_eq!(
            extract_video_id("https://www.youtube.com/embed/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            extract_video_id("https://www.youtube.com/shorts/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(extract_video_id("https://example.com/not-youtube"), None);
    }

    #[test]
    fn js_unicode_escapes_are_unescaped() {
        assert_eq!(unescape_js_string("a\\u0026b"), "a&b");
        assert_eq!(unescape_js_string("no escapes here"), "no escapes here");
    }

    /// A real `captionTracks` fragment, captured 2026-07-27 from a live
    /// YouTube watch page (`curl -A "<browser UA>" https://www.youtube.com/watch?v=dQw4w9WgXcQ`)
    /// — recorded rather than hand-written so the parser is checked against
    /// YouTube's actual current field layout and its `&`-escaped
    /// `baseUrl`, not an assumption about it. This is the same fragment
    /// whose `baseUrl`, when actually requested, is the empty response
    /// documented on `YOUTUBE_BLOCKED_MESSAGE` — the parsing here works; the
    /// network fetch is what YouTube currently refuses.
    const RECORDED_CAPTION_TRACKS_FRAGMENT: &str = r#""captionTracks":[{"baseUrl":"https://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ&ei=oyRoapXLAp2DkucP_qHnkQs&caps=asr&opi=112496729&exp=xpe&xoaf=5&xowf=1&hl=en&ip=0.0.0.0&ipbits=0&expire=1785235219&sparams=ip,ipbits,expire,v,ei,caps,opi,exp,xoaf&signature=7B2A3E6A49A4CF605DC11280C772A819273E0FA9.D7084F9091CCB59562DD2B8294BE57541C9E3D41&key=yt8&lang=en","name":{"simpleText":"English"},"vssId":".en","languageCode":"en","isTranslatable":true,"trackName":""}]"#;

    #[test]
    fn caption_track_url_is_found_and_unescaped_from_a_recorded_watch_page_fragment() {
        let url = find_caption_track_url(RECORDED_CAPTION_TRACKS_FRAGMENT).expect("a track URL");
        assert!(url.starts_with("https://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ"));
        assert!(url.contains('&'), "escaped \\u0026 should have become a literal &: {url}");
        assert!(!url.contains("\\u0026"), "escape sequence should not survive unescaping: {url}");
    }

    #[test]
    fn caption_track_url_is_none_when_the_field_is_absent() {
        assert!(find_caption_track_url("<html>no captions mentioned here</html>").is_none());
    }

    /// The documented `timedtext` XML shape (`fmt` unspecified / `srv1`):
    /// `<transcript><text start="…" dur="…">…</text>…</transcript>`, with
    /// ordinary HTML entities inside caption text and one line carrying an
    /// inline `<i>` tag, both of which real caption tracks do contain.
    const SAMPLE_TIMEDTEXT_XML: &str = r#"<?xml version="1.0" encoding="utf-8" ?><transcript>
        <text start="0.5" dur="3.2">Never gonna give you up</text>
        <text start="3.7" dur="2.1">Never gonna let you &amp; down</text>
        <text start="5.9" dur="2.8"><i>music playing</i></text>
        <text start="8.0" dur="1.0">   </text>
    </transcript>"#;

    #[test]
    fn timedtext_xml_becomes_plain_lines_with_entities_decoded_and_blanks_dropped() {
        let text = parse_timedtext_xml(SAMPLE_TIMEDTEXT_XML);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            vec!["Never gonna give you up", "Never gonna let you & down", "music playing"]
        );
    }

    #[test]
    fn empty_timedtext_xml_parses_to_empty_text() {
        assert_eq!(parse_timedtext_xml(""), "");
        assert_eq!(parse_timedtext_xml("<transcript></transcript>"), "");
    }

    // `extract_pdf_text` itself is exercised by hand against real PDFs
    // rather than by a checked-in test: it shells out to `mdimport` and
    // `plutil`, which is exactly the kind of environment-dependent
    // (Spotlight availability, macOS version) integration the brief for
    // this module asked to keep out of the automated suite. See the final
    // report for what was run and the results.
}
