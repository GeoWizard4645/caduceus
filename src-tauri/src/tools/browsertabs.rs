//! Browser awareness: open-tab search/switch, bookmark search, recent downloads.
//!
//! # Why AppleScript for tabs, and not BrowserClaw
//!
//! The product ask named [BrowserClaw](https://github.com/idan-rubin/browserclaw)
//! as the library to use here. It is a Node/TypeScript package built on
//! `playwright-core`, driving a browser over the Chrome DevTools Protocol. That
//! is the right tool when you need to *act inside* a page — click things, read
//! the DOM, wait on navigation. It is the wrong tool for "what tabs are open"
//! and "switch to this one": those need no Node runtime (Caduceus ships none),
//! no CDP debug port (Chrome does not expose one by default, and turning it on
//! for every browser install just to answer a search query is a real attack
//! surface for a feature that should be instant and read-only), and no
//! `playwright-core` process to launch and keep alive. AppleScript already
//! answers both questions in one round trip through `osascript`, which macOS
//! ships, with no port to open and nothing to keep running.
//!
//! The module is still shaped so a future automation backend — CDP-based or
//! otherwise — could sit behind the same functions ([`search_tabs`],
//! [`switch_tab`]) without the caller changing: the AppleScript is entirely
//! contained in the two `*_script` builders below, and everything above them
//! deals only in [`TabHit`] and plain arguments. Swapping the transport later
//! means rewriting those two functions, not this module's public shape.
//!
//! # The "never launches a browser" guard
//!
//! [`super::qr::front_tab_url`] already solved this once: `tell application
//! "Safari" to get URL of front document` — with no guard — *launches* Safari
//! if it is not running, because sending any Apple Event to an app is itself
//! what starts it. The fix there, which every script in this file reuses
//! verbatim, is `tell application "X" to if it is running then …`: `is
//! running` is answered by Launch Services from the process list and does not
//! send an Apple Event, so asking "what tabs do you have open" — or "search my
//! bookmarks" — can never be the reason a browser appears in the Dock. Every
//! `tell` block below is wrapped in that guard, and the fixture tests assert
//! the guard text is present in the generated script rather than trusting it
//! by inspection, because a script that silently drops the guard is a bug that
//! would otherwise only show up on the one browser a manual test happened to
//! have quit.
//!
//! # AppleScript injection
//!
//! The only strings this module interpolates into a `tell application "…"`
//! block are browser display names (`"Google Chrome"`, `"Safari"`, …). They
//! come from a fixed list in this file today, but [`tab_script`] and
//! [`switch_script`] treat that name exactly as if it were attacker-controlled
//! — passing it through [`crate::shortcuts::escape_applescript`] before
//! interpolation — for the same reason `qr.rs` does: this codebase has shipped
//! the unescaped version of this exact bug before (see `notes.rs`,
//! `timekeeping.rs`, `calendar.rs`), and "it's just a constant today" is
//! precisely the reasoning that made those bugs ship, because the call site
//! that later starts passing a caller-supplied value in rarely goes back to
//! add the escape. The tests below build the actual script text with a
//! malicious name (`"Evil" & do shell script "…" & "`) and assert the payload
//! never appears unescaped in it — not just that the escaper itself works.
//!
//! Window ids and tab indices are `i64`, formatted with `{}` and never touch
//! `escape_applescript` — there is nothing to escape in a formatted integer,
//! and routing a number through a string escaper would only be decoration.
//! [`switch_tab`] does still reject anything not a positive integer before it
//! reaches the formatter, and rejects any `browser` not on the known
//! Safari/Chromium list, so a caller cannot use this path to run a `tell
//! application` against an arbitrary app name either.
//!
//! # Tab identity is (window id, tab index), and that can go stale
//!
//! Safari and Chrome both give a `window` a persistent `id`, but neither
//! exposes a persistent id for an individual *tab* in a form this module can
//! rely on across every fork in the Chromium list — `index of window`
//! addressing is the one thing common to all of them. That means a tab found
//! by [`search_tabs`] and then closed or dragged to another window before
//! [`switch_tab`] runs will switch to whatever now sits at that index, or fail
//! if the window itself is gone. This is the same tradeoff every AppleScript
//! tab switcher makes; the alternative (re-matching by URL at switch time) can
//! silently jump to the wrong tab of several with the same URL, which is a
//! worse failure than an occasional stale-index error.
//!
//! # Bookmarks live in files, not AppleScript
//!
//! Neither Safari nor Chrome expose bookmarks through their scripting
//! dictionary, so this half of the module reads the files the browsers write
//! bookmarks to directly:
//!
//! * **Chromium family** (Chrome, Brave, Edge, Vivaldi, Arc, Chromium) keep a
//!   JSON file called `Bookmarks` per profile, no extension, under
//!   `~/Library/Application Support/<Vendor>/<Product>/<Profile>/Bookmarks`.
//!   Verified present on this machine: **Chrome**, with nine profiles (`Default`,
//!   `Profile 1`, `2`, `3`, `9`, `13`, `15`, `16`, `17`), each with a real
//!   `Bookmarks` file (`roots.bookmark_bar` / `roots.other` / `roots.synced`,
//!   nested `folder`/`url` nodes — confirmed by reading one). Brave, Edge and
//!   Arc are not installed here (their Application Support folders exist —
//!   likely created by some other process probing for them — but are empty and
//!   there is no matching `.app` in `/Applications`), so their code paths are
//!   written and tested against fixtures but unverified against a real
//!   profile; they degrade to "nothing found" rather than erroring, same as
//!   every browser [`crate::shortcuts::browser`] does not find installed.
//! * **Safari** uses a *binary* plist at `~/Library/Safari/Bookmarks.plist` —
//!   confirmed to exist on this machine (82 KB). Reading it needs `plutil
//!   -convert json -o -` (the `-o -` writes the conversion to stdout; the file
//!   on disk is never touched) because there is no pure-Rust binary-plist
//!   reader in this crate's dependency list and adding one is not worth it for
//!   a format `plutil` already converts for free. Actually reading the file's
//!   *bytes* failed in this sandbox with "couldn't be opened because you don't
//!   have permission" — `~/Library/Safari` has been TCC-protected (Full Disk
//!   Access) since Catalina, for every process, sandboxed or not. That is
//!   expected to also gate the shipped app the first time this runs, so a
//!   `plutil` failure whose stderr mentions permission is translated into a
//!   sentence naming Full Disk Access rather than "plutil exited 1", mirroring
//!   how `apple::translate` names Automation instead of `-1743`.
//! * **Firefox** keeps a SQLite database, `places.sqlite`, one per profile
//!   under `~/Library/Application Support/Firefox/Profiles/<profile>/`, listed
//!   in `profiles.ini` next to it. Not installed on this machine — no
//!   `~/Library/Application Support/Firefox` at all — so the schema below
//!   (`moz_bookmarks` joined to `moz_places`) is written from Mozilla's own
//!   documented, years-stable schema and exercised only against an in-memory
//!   fixture database built by the tests, never a real profile. `rusqlite` can
//!   open the file directly, but Firefox holds an exclusive lock on
//!   `places.sqlite` for as long as it runs, so the real path always copies it
//!   to a temp file first — SQLite happily opens a copy of a WAL-mode database
//!   read-only even mid-write, which is exactly why browsers themselves ship a
//!   "bookmarks backup" feature built the same way.
//!
//! # Downloads
//!
//! Every browser on macOS writes downloads to `~/Downloads` by default and
//! there is no per-browser index worth reading instead — Chrome's own download
//! history is itself just another `History` SQLite row pointing at that same
//! path. So "recent downloads" is a directory listing of `~/Downloads`,
//! newest first, skipping the in-progress extensions (`.crdownload`,
//! `.download`, `.part`, `.partial`) a paused or active download leaves
//! behind — offering someone a half-written file as their "latest download"
//! is never what they meant.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::shortcuts::escape_applescript;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// One open tab, as returned by [`search_tabs`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabHit {
    /// The exact AppleScript application name (`"Safari"`, `"Google Chrome"`,
    /// …). [`switch_tab`] takes this straight back as its `browser` argument.
    pub browser: String,
    pub window_id: i64,
    /// 1-based, matching AppleScript's own indexing — see the module doc
    /// comment on why this, and not a tab id, is the addressing scheme.
    pub tab_index: i64,
    pub title: String,
    pub url: String,
}

/// One bookmark, as returned by [`search_bookmarks`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkHit {
    /// Human label for where this came from: `"Safari"`, `"Chrome — Work"`
    /// (profile name appended when a Chromium browser has more than one
    /// profile), `"Firefox"`.
    pub source: String,
    pub title: String,
    pub url: String,
    /// The bookmark's containing folder path, `" / "`-joined, when known.
    pub folder: Option<String>,
}

/// One file in `~/Downloads`, as returned by [`recent_downloads`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadHit {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    /// RFC 3339, e.g. `2026-07-27T14:03:00-07:00`. A string rather than a
    /// number so the frontend never has to decide whether an integer is
    /// seconds or milliseconds.
    pub modified: String,
    /// The file extension, uppercased (`"PDF"`, `"PNG"`), or `"File"` when
    /// there isn't one — never blank, since a blank chip in a list reads as
    /// missing data rather than as "no extension".
    pub kind: String,
}

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

/// Every Chromium-family browser this module knows how to script.
///
/// Kept in sync by hand with [`crate::shortcuts::browser`]'s private
/// `CANDIDATES` table and with `qr.rs::front_tab_url`'s `CHROMIUM` list — none
/// of the three is public in a form the others could import, and duplicating
/// eleven string literals is a smaller cost than exposing internal tables
/// across module boundaries for it.
const CHROMIUM: &[&str] = &[
    "Google Chrome",
    "Arc",
    "Brave Browser",
    "Microsoft Edge",
    "Vivaldi",
    "Chromium",
    "Dia",
    "Comet",
];

/// Field / record separators for the AppleScript enumeration output.
///
/// Control characters rather than a comma or tab: a page title cannot contain
/// them (browsers strip control characters from `document.title` before
/// AppleScript ever sees it), so there is no real page whose title could be
/// mistaken for a field boundary. Even if one somehow did, the failure mode is
/// a garbled *title* in a search result — not a script-injection path, since
/// this is parsing output, not building further script from it.
const UNIT_SEP: char = '\u{1f}';
const RECORD_SEP: char = '\u{1e}';

/// Build the AppleScript that lists every tab of every window of `browser`.
///
/// Pure and side-effect-free on purpose: it is the thing the injection tests
/// below inspect directly, without needing `osascript` or a running browser.
fn tab_script(browser: &str, safari: bool) -> String {
    let name = escape_applescript(browser);
    let title_prop = if safari { "name" } else { "title" };
    format!(
        "tell application \"{name}\"\n\
         \tif it is running then\n\
         \t\trepeat with w in windows\n\
         \t\t\tset wid to id of w\n\
         \t\t\trepeat with t in tabs of w\n\
         \t\t\t\tset output to output & wid & US & (index of t) & US & ({title_prop} of t) & \
         US & (URL of t) & RS\n\
         \t\t\tend repeat\n\
         \t\tend repeat\n\
         \tend if\n\
         end tell\n"
    )
}

/// Parse one browser's enumeration output (from [`tab_wrapper_script`]) into
/// [`TabHit`]s. `browser` is supplied by the caller — who is the one that
/// knows which `tell` block produced `raw` — rather than read out of the
/// record, since a single call only ever scripts one browser at a time (see
/// [`search_tabs`] for why one subprocess per browser, not a combined one).
fn parse_tabs_for(browser: &str, raw: &str) -> Vec<TabHit> {
    raw.split(RECORD_SEP)
        .filter(|record| !record.trim().is_empty())
        .filter_map(|record| {
            let mut fields = record.split(UNIT_SEP);
            let window_id = fields.next()?.trim().parse::<i64>().ok()?;
            let tab_index = fields.next()?.trim().parse::<i64>().ok()?;
            let title = fields.next()?.trim().to_string();
            let url = fields.next()?.trim().to_string();
            if url.is_empty() {
                return None;
            }
            Some(TabHit { browser: browser.to_string(), window_id, tab_index, title, url })
        })
        .collect()
}

/// Search every open tab across every scriptable browser (Safari + the
/// Chromium family — Firefox exposes no tab-enumeration scripting dictionary
/// at all, so it is absent here even though it appears in bookmark search).
///
/// An empty or whitespace-only `query` returns every open tab, ranked by
/// recency-of-nothing (i.e. window/tab enumeration order) — useful for "list
/// my open tabs" rather than a search. A non-empty query ranks by
/// [`score_text`] against the title and URL and drops non-matches.
///
/// Never launches a browser: see the module doc comment on the `is running`
/// guard, which every generated `tell` block carries.
pub fn search_tabs(query: &str) -> Vec<TabHit> {
    let query = query.trim();

    // One browser at a time rather than the combined script from
    // `all_tabs_script`, because the combined script's output cannot say
    // *which* `tell` block a given record came from without also emitting the
    // browser name per-record (which reintroduces exactly the ambiguity
    // `parse_tabs_for` exists to avoid). Firing one small `osascript` per
    // scriptable, running browser is a handful of subprocesses at most — most
    // machines have one or two browsers open — and keeps parsing unambiguous.
    let mut hits = Vec::new();
    for browser in std::iter::once("Safari").chain(CHROMIUM.iter().copied()) {
        let safari = browser == "Safari";
        let script = tab_wrapper_script(browser, safari);
        let Ok(raw) = super::apple::run_script(&script) else { continue };
        hits.extend(parse_tabs_for(browser, &raw));
    }

    rank(hits, query, |h| &h.title, |h| &h.url)
}

/// [`tab_script`] wrapped with the `US`/`RS` variable declarations and a
/// `return`, so it can be run standalone for one browser.
fn tab_wrapper_script(browser: &str, safari: bool) -> String {
    format!(
        "set US to (ASCII character 31)\nset RS to (ASCII character 30)\nset output to \"\"\n{}return output\n",
        tab_script(browser, safari)
    )
}

/// Build the AppleScript that focuses one already-open tab.
///
/// Pure, for the same reason `tab_script` is — the injection tests call this
/// directly and never touch `osascript`.
fn switch_script(browser: &str, safari: bool, window_id: i64, tab_index: i64) -> String {
    let name = escape_applescript(browser);
    let focus = if safari {
        format!(
            "set current tab of window id {window_id} to tab {tab_index} of window id {window_id}"
        )
    } else {
        format!("set active tab index of window id {window_id} to {tab_index}")
    };
    format!(
        "tell application \"{name}\"\n\
         \tif it is running then\n\
         \t\t{focus}\n\
         \t\tset index of window id {window_id} to 1\n\
         \t\tactivate\n\
         \tend if\n\
         end tell\n"
    )
}

/// Bring one open tab to the front.
///
/// `browser` must be `"Safari"` or one of [`CHROMIUM`] — anything else is
/// refused before a script is ever built, both because switching to a tab in
/// an app with no tab-scripting dictionary cannot work and because it keeps
/// this function from ever being a way to run `tell application` against an
/// arbitrary, caller-chosen name. `window_id` and `tab_index` must be
/// positive, matching what [`search_tabs`] hands back.
///
/// `activate` here does bring the target app forward — unlike `search_tabs`,
/// that is the entire point of "switch to this tab", not an accidental
/// launch. It is still guarded by `is running`, so a browser that quit
/// between the search and this call is reported rather than started fresh
/// into an empty window.
pub fn switch_tab(browser: &str, window_id: i64, tab_index: i64) -> super::ToolOutcome {
    let safari = browser == "Safari";
    if !safari && !CHROMIUM.contains(&browser) {
        return super::ToolOutcome::err(format!("Caduceus does not know how to script “{browser}”."));
    }
    if window_id <= 0 || tab_index <= 0 {
        return super::ToolOutcome::err("That tab no longer looks valid — try searching again.");
    }

    let script = switch_script(browser, safari, window_id, tab_index);
    match super::apple::run_script(&script) {
        Ok(_) => super::ToolOutcome::ok(format!("Switched to the tab in {browser}.")),
        Err(e) if e.contains("-1728") => super::ToolOutcome::err(
            "That tab or window is gone — it may have been closed or moved. Try searching again.",
        ),
        Err(e) => super::ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/// Score `text` against `query`, case-insensitively. Higher is better; `0`
/// means "does not match" and the caller should drop it.
///
/// Deliberately not fuzzy (no transposition/typo tolerance): a tab or
/// bookmark list is short enough that a plain substring match is fast and,
/// more importantly, predictable — a fuzzy matcher surfacing an unrelated
/// result for a three-letter query is worse here than it just returning
/// nothing.
fn score_text(text: &str, query: &str) -> u32 {
    if query.is_empty() {
        return 1;
    }
    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();
    if text_lower == query_lower {
        100
    } else if text_lower.starts_with(&query_lower) {
        80
    } else if text_lower.contains(&query_lower) {
        60
    } else {
        0
    }
}

/// Rank `items` against `query` by title (weighted higher) then URL, dropping
/// anything that matches neither when `query` is non-empty.
fn rank<T>(
    items: Vec<T>,
    query: &str,
    title: impl Fn(&T) -> &str,
    url: impl Fn(&T) -> &str,
) -> Vec<T> {
    let mut scored: Vec<(u32, T)> = items
        .into_iter()
        .filter_map(|item| {
            let title_score = score_text(title(&item), query);
            // A URL match is real but weaker than a title match — someone
            // searching "docs" almost always means the tab titled "Docs", not
            // the unrelated tab whose URL happens to contain "docs.".
            let url_score = score_text(url(&item), query) / 2;
            let score = title_score.max(url_score);
            if score == 0 && !query.is_empty() {
                None
            } else {
                Some((score, item))
            }
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, item)| item).collect()
}

// ---------------------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------------------

/// Search bookmarks across every browser this module knows how to read:
/// Safari, the Chromium family, and Firefox. `limit` bounds the result count
/// after ranking, not before — a low limit should still return the *best*
/// matches, not just the first ones a particular browser happened to yield.
pub fn search_bookmarks(query: &str, limit: usize) -> Vec<BookmarkHit> {
    let query = query.trim();
    let mut all = Vec::new();
    all.extend(safari_bookmarks());
    all.extend(chromium_bookmarks());
    all.extend(firefox_bookmarks());

    let ranked = rank(all, query, |b| &b.title, |b| &b.url);
    ranked.into_iter().take(limit.max(1)).collect()
}

// -- Chromium family ---------------------------------------------------------

/// `(display name, path segments under Application Support)` for every
/// Chromium fork this module reads bookmarks from. A subset of
/// `shortcuts::browser`'s private candidate table — duplicated for the same
/// reason [`CHROMIUM`] above is: that table exposes profile *metadata*
/// (`BrowserInstall`), never the filesystem path a caller would need to open
/// `Bookmarks` directly.
const CHROMIUM_PATHS: &[(&str, &[&str])] = &[
    ("Chrome", &["Google", "Chrome"]),
    ("Brave", &["BraveSoftware", "Brave-Browser"]),
    ("Edge", &["Microsoft Edge"]),
    ("Vivaldi", &["Vivaldi"]),
    ("Chromium", &["Chromium"]),
    ("Arc", &["Arc", "User Data"]),
];

fn chromium_bookmarks() -> Vec<BookmarkHit> {
    let Some(support) = dirs::home_dir().map(|h| h.join("Library/Application Support")) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (label, segments) in CHROMIUM_PATHS {
        let mut root = support.clone();
        for seg in *segments {
            root.push(seg);
        }
        if !root.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        // Multiple profiles are common (a work and a personal Google login,
        // say); every profile directory with a `Bookmarks` file is read, not
        // just `Default`, or half of someone's bookmarks would silently never
        // show up in search.
        for entry in entries.flatten() {
            let profile_dir = entry.path();
            let bookmarks_file = profile_dir.join("Bookmarks");
            if !bookmarks_file.is_file() {
                continue;
            }
            let profile_name = entry.file_name().to_string_lossy().to_string();
            let source = if profile_name == "Default" {
                label.to_string()
            } else {
                format!("{label} — {profile_name}")
            };
            if let Ok(text) = std::fs::read_to_string(&bookmarks_file) {
                out.extend(parse_chromium_bookmarks(&text, &source));
            }
        }
    }
    out
}

/// Parse one Chromium `Bookmarks` JSON file's text into [`BookmarkHit`]s.
///
/// A free function taking the file's *contents* rather than a path, so the
/// fixture tests can exercise it against recorded JSON without touching disk.
fn parse_chromium_bookmarks(text: &str, source: &str) -> Vec<BookmarkHit> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let Some(roots) = json.get("roots").and_then(|r| r.as_object()) else { return Vec::new() };

    let mut out = Vec::new();
    for (_, root) in roots {
        walk_chromium_node(root, source, &mut Vec::new(), &mut out);
    }
    out
}

fn walk_chromium_node(
    node: &serde_json::Value,
    source: &str,
    path: &mut Vec<String>,
    out: &mut Vec<BookmarkHit>,
) {
    let node_type = node.get("type").and_then(|v| v.as_str()).unwrap_or_default();
    let name = node.get("name").and_then(|v| v.as_str()).unwrap_or_default();

    match node_type {
        "url" => {
            let Some(url) = node.get("url").and_then(|v| v.as_str()) else { return };
            if url.is_empty() {
                return;
            }
            out.push(BookmarkHit {
                source: source.to_string(),
                title: if name.is_empty() { url.to_string() } else { name.to_string() },
                url: url.to_string(),
                folder: if path.is_empty() { None } else { Some(path.join(" / ")) },
            });
        }
        "folder" => {
            let Some(children) = node.get("children").and_then(|v| v.as_array()) else { return };
            // The three synthetic roots ("Bookmarks Bar", "Other Bookmarks",
            // "Mobile Bookmarks") are noise as a folder label — nobody thinks
            // of a bookmark as being "in" Bookmarks Bar, they think of it as
            // top-level — so only real, user-created folders are pushed onto
            // the path.
            let is_synthetic_root = path.is_empty()
                && matches!(name, "Bookmarks Bar" | "Other Bookmarks" | "Mobile Bookmarks");
            if !is_synthetic_root && !name.is_empty() {
                path.push(name.to_string());
            }
            for child in children {
                walk_chromium_node(child, source, path, out);
            }
            if !is_synthetic_root && !name.is_empty() {
                path.pop();
            }
        }
        _ => {}
    }
}

// -- Safari -------------------------------------------------------------------

fn safari_bookmarks_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/Safari/Bookmarks.plist"))
}

fn safari_bookmarks() -> Vec<BookmarkHit> {
    let Some(path) = safari_bookmarks_path() else { return Vec::new() };
    if !path.is_file() {
        return Vec::new();
    }
    match read_safari_plist_json(&path) {
        Ok(text) => parse_safari_bookmarks(&text),
        // Full Disk Access is the overwhelmingly likely cause (see the module
        // doc comment) but this is a best-effort background read for a search
        // box, not a user-facing action with somewhere to put an error, so it
        // degrades to "no Safari bookmarks found" rather than surfacing
        // "grant Full Disk Access" from deep inside a ranked list. The
        // permission-wall pattern (`apple::translate`) is for actions the
        // user directly triggered and can act on immediately; a silent
        // background search result is not that.
        Err(_) => Vec::new(),
    }
}

/// Convert Safari's binary-plist bookmarks file to JSON via `plutil`.
///
/// `-o -` writes the conversion to stdout; the file on disk is opened
/// read-only by `plutil` itself and never rewritten. Goes through
/// [`super::output_with_timeout`] directly (not `apple::run_script`, which is
/// for `osascript`) because a real bookmarks file — hundreds of folders and
/// URLs — produces JSON well past the 64 KB a pipe buffer holds, which is
/// exactly the case `output_with_timeout` was fixed for.
fn read_safari_plist_json(path: &Path) -> Result<String, String> {
    let mut command = Command::new("plutil");
    command.arg("-convert").arg("json").arg("-o").arg("-").arg(path);
    let output = super::output_with_timeout(
        &mut command,
        Duration::from_secs(10),
        "plutil did not answer in time.",
    )?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Parse Safari's converted plist JSON into [`BookmarkHit`]s.
///
/// Structure (stable since Safari's bookmarks format was introduced, and
/// documented independently by forensic/export tools since no Apple reference
/// covers it): a tree of dictionaries. `WebBookmarkType` is
/// `"WebBookmarkTypeLeaf"` for an actual bookmark (`URLString` plus, usually,
/// `URIDictionary.title`) or `"WebBookmarkTypeList"` for a folder (`Title`
/// plus `Children`). `"WebBookmarkTypeProxy"` covers synthetic nodes —
/// Reading List, History, synced-device folders — which are skipped
/// entirely: they are not bookmarks a person filed themselves, and Reading
/// List in particular can be large enough to drown out real results.
fn parse_safari_bookmarks(text: &str) -> Vec<BookmarkHit> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let mut out = Vec::new();
    walk_safari_node(&json, &mut Vec::new(), &mut out);
    out
}

fn walk_safari_node(node: &serde_json::Value, path: &mut Vec<String>, out: &mut Vec<BookmarkHit>) {
    let node_type = node.get("WebBookmarkType").and_then(|v| v.as_str()).unwrap_or_default();

    match node_type {
        "WebBookmarkTypeLeaf" => {
            let Some(url) = node.get("URLString").and_then(|v| v.as_str()) else { return };
            if url.is_empty() {
                return;
            }
            let title = node
                .get("URIDictionary")
                .and_then(|d| d.get("title"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(url);
            out.push(BookmarkHit {
                source: "Safari".to_string(),
                title: title.to_string(),
                url: url.to_string(),
                folder: if path.is_empty() { None } else { Some(path.join(" / ")) },
            });
        }
        "WebBookmarkTypeList" => {
            let Some(children) = node.get("Children").and_then(|v| v.as_array()) else { return };
            let name = node.get("Title").and_then(|v| v.as_str()).unwrap_or_default();
            // The root list itself is untitled; only named children become
            // path segments, same reasoning as the Chromium synthetic roots.
            let pushed = !name.is_empty();
            if pushed {
                path.push(name.to_string());
            }
            for child in children {
                walk_safari_node(child, path, out);
            }
            if pushed {
                path.pop();
            }
        }
        // WebBookmarkTypeProxy and anything else: skipped, see doc comment.
        _ => {}
    }
}

// -- Firefox ------------------------------------------------------------------

fn firefox_bookmarks() -> Vec<BookmarkHit> {
    let Some(support) = dirs::home_dir().map(|h| h.join("Library/Application Support/Firefox"))
    else {
        return Vec::new();
    };
    if !support.is_dir() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for places_path in firefox_places_paths(&support) {
        if let Ok(hits) = read_firefox_places(&places_path) {
            out.extend(hits);
        }
    }
    out
}

/// Every `places.sqlite` this Firefox install has, one per profile.
///
/// Prefers `profiles.ini` (the documented source of truth for where a
/// profile lives) and falls back to scanning `Profiles/` for any directory
/// that directly contains a `places.sqlite`, the same "trust the file, not
/// the index" fallback `shortcuts::browser::scan_profile_dirs` uses when a
/// Chromium `Local State` is missing or unparseable.
fn firefox_places_paths(firefox_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(ini) = std::fs::read_to_string(firefox_dir.join("profiles.ini")) {
        for line in ini.lines() {
            let line = line.trim();
            if let Some(rel) = line.strip_prefix("Path=") {
                let candidate = firefox_dir.join(rel);
                let places = candidate.join("places.sqlite");
                if places.is_file() {
                    paths.push(places);
                }
            }
        }
    }

    if paths.is_empty() {
        let profiles_dir = firefox_dir.join("Profiles");
        if let Ok(entries) = std::fs::read_dir(&profiles_dir) {
            for entry in entries.flatten() {
                let places = entry.path().join("places.sqlite");
                if places.is_file() {
                    paths.push(places);
                }
            }
        }
    }

    paths
}

/// Read one Firefox profile's bookmarks.
///
/// `places.sqlite` is copied to a temp file first because Firefox holds it
/// open (and, in WAL mode, mid-write) for as long as the browser runs;
/// opening the original directly risks `SQLITE_BUSY` or a lock timeout for no
/// reason, when a copy reads exactly the same committed data safely. The copy
/// is removed on every exit path via the guard below, not just the success
/// path — a search box that leaks a temp file per query on someone with
/// Firefox open would fill `/tmp` over a long-running session.
fn read_firefox_places(places_path: &Path) -> Result<Vec<BookmarkHit>, String> {
    let tmp = std::env::temp_dir()
        .join(format!("caduceus-firefox-places-{}.sqlite", uuid::Uuid::new_v4()));
    std::fs::copy(places_path, &tmp).map_err(|e| e.to_string())?;
    struct RemoveOnDrop<'a>(&'a Path);
    impl Drop for RemoveOnDrop<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _cleanup = RemoveOnDrop(&tmp);

    let conn = rusqlite::Connection::open_with_flags(
        &tmp,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;

    query_firefox_bookmarks(&conn).map_err(|e| e.to_string())
}

/// The `moz_bookmarks`/`moz_places` query, factored out so the fixture tests
/// can run it against an in-memory database instead of a real profile.
///
/// `moz_bookmarks.type = 1` is Firefox's own constant for a bookmark (as
/// opposed to `2`, a folder, or `3`, a separator) — documented in Mozilla's
/// Places schema and unchanged since it was introduced. `fk` is the foreign
/// key into `moz_places`, which holds the URL and Firefox's own idea of the
/// page title; `moz_bookmarks.title` is the *user's* title when they renamed
/// the bookmark, and takes priority when present.
fn query_firefox_bookmarks(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<BookmarkHit>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(b.title, p.title, p.url), p.url \
         FROM moz_bookmarks b JOIN moz_places p ON b.fk = p.id \
         WHERE b.type = 1 AND p.url IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        let title: String = row.get(0)?;
        let url: String = row.get(1)?;
        Ok((title, url))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (title, url) = row?;
        if url.is_empty() {
            continue;
        }
        out.push(BookmarkHit {
            source: "Firefox".to_string(),
            title,
            url,
            // Folder path is left out: `moz_bookmarks.parent` chains up to a
            // root and Firefox's own folder titles need the same recursive
            // walk the Chromium/Safari trees get, which is real work for a
            // browser that is not installed on the machine this shipped from.
            // The field is `Option` for exactly this — "unknown", not "no
            // folder" — and a later pass can fill it in against a real
            // profile.
            folder: None,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------

/// Extensions a browser uses for a download that has not finished yet.
const IN_PROGRESS_EXTENSIONS: &[&str] = &["crdownload", "download", "part", "partial"];

/// The most recently modified files in `~/Downloads`, newest first.
pub fn recent_downloads(limit: usize) -> Vec<DownloadHit> {
    let Some(dir) = dirs::home_dir().map(|h| h.join("Downloads")) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };

    let mut hits: Vec<(std::time::SystemTime, DownloadHit)> = entries
        .flatten()
        .filter_map(|entry| download_hit(&entry))
        .collect();

    hits.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    hits.into_iter().take(limit.max(1)).map(|(_, hit)| hit).collect()
}

fn download_hit(entry: &std::fs::DirEntry) -> Option<(std::time::SystemTime, DownloadHit)> {
    let path = entry.path();
    let name = entry.file_name().to_string_lossy().to_string();
    if name.starts_with('.') {
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
    if IN_PROGRESS_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let meta = entry.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta.modified().ok()?;
    let modified_str: chrono::DateTime<chrono::Local> = modified.into();

    Some((
        modified,
        DownloadHit {
            name,
            path: path.to_string_lossy().to_string(),
            size_bytes: meta.len(),
            modified: modified_str.to_rfc3339(),
            kind: if ext.is_empty() { "File".to_string() } else { ext.to_uppercase() },
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- AppleScript injection ------------------------------------------------
    //
    // These build the real script text and inspect it — the thing the module
    // doc comment promises — rather than only exercising `escape_applescript`
    // in isolation. None of them run `osascript` or touch a browser.

    const EVIL_NAME: &str = "Evil\" & do shell script \"rm -rf ~\" & \"";

    #[test]
    fn a_malicious_browser_name_cannot_break_out_of_the_tab_script_literal() {
        let script = tab_script(EVIL_NAME, false);
        assert!(!script.contains("do shell script \"rm -rf ~\""));
        // The escaped form must still be present — proving the name made it
        // into the script at all, just safely, not that it was dropped.
        assert!(script.contains(&escape_applescript(EVIL_NAME)));
    }

    #[test]
    fn a_malicious_browser_name_cannot_break_out_of_the_switch_script_literal() {
        let script = switch_script(EVIL_NAME, false, 1, 1);
        assert!(!script.contains("do shell script \"rm -rf ~\""));
        assert!(script.contains(&escape_applescript(EVIL_NAME)));
    }

    #[test]
    fn a_name_with_only_a_backslash_is_still_escaped() {
        // Quotes are the injection vector, but an unescaped backslash before
        // a later, legitimate quote can still shift what the parser sees.
        let script = tab_script(r"back\slash", false);
        assert!(script.contains(r#"tell application "back\\slash""#));
    }

    #[test]
    fn switch_tab_refuses_an_unknown_browser_before_building_a_script() {
        let out = switch_tab("Definitely Not A Browser", 1, 1);
        assert!(!out.ok);
    }

    #[test]
    fn switch_tab_refuses_a_non_positive_window_or_tab() {
        assert!(!switch_tab("Safari", 0, 1).ok);
        assert!(!switch_tab("Safari", 1, -1).ok);
    }

    // -- The "never launches a browser" guard ----------------------------------

    #[test]
    fn every_tab_script_carries_the_is_running_guard() {
        for browser in std::iter::once("Safari").chain(CHROMIUM.iter().copied()) {
            let script = tab_script(browser, browser == "Safari");
            assert!(
                script.contains("if it is running then"),
                "{browser}'s enumeration script is missing the is-running guard"
            );
        }
    }

    #[test]
    fn the_switch_script_also_carries_the_guard() {
        let script = switch_script("Safari", true, 1, 1);
        assert!(script.contains("if it is running then"));
    }

    #[test]
    fn safari_and_chromium_scripts_ask_for_different_title_properties() {
        // Safari's Tab class calls it `name`; Chrome's calls it `title`. Using
        // the wrong one is a silent empty-title bug, not a compile error, so
        // it is worth pinning down explicitly.
        assert!(tab_script("Safari", true).contains("(name of t)"));
        assert!(tab_script("Google Chrome", false).contains("(title of t)"));
    }

    // -- Tab output parsing -----------------------------------------------------

    #[test]
    fn parses_a_well_formed_tab_listing() {
        let raw = format!(
            "1{u}1{u}Example{u}https://example.com{r}1{u}2{u}Docs{u}https://docs.example.com{r}",
            u = UNIT_SEP,
            r = RECORD_SEP
        );
        let hits = parse_tabs_for("Safari", &raw);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0], TabHit {
            browser: "Safari".into(),
            window_id: 1,
            tab_index: 1,
            title: "Example".into(),
            url: "https://example.com".into(),
        });
        assert_eq!(hits[1].tab_index, 2);
    }

    #[test]
    fn empty_output_parses_to_no_tabs() {
        assert!(parse_tabs_for("Safari", "").is_empty());
    }

    #[test]
    fn a_malformed_record_is_skipped_rather_than_panicking() {
        let raw = format!("not-a-number{u}1{u}Title{u}https://x.com{r}", u = UNIT_SEP, r = RECORD_SEP);
        assert!(parse_tabs_for("Safari", &raw).is_empty());
    }

    // -- Ranking ------------------------------------------------------------

    #[test]
    fn an_exact_title_match_outranks_a_prefix_which_outranks_a_substring() {
        assert!(score_text("Docs", "docs") > score_text("Docs Overview", "docs"));
        assert!(score_text("Docs Overview", "docs") > score_text("Google Docs", "docs"));
    }

    #[test]
    fn a_url_only_match_still_surfaces_but_below_a_title_match() {
        let items = vec![
            TabHit { browser: "Safari".into(), window_id: 1, tab_index: 1, title: "Homepage".into(), url: "https://docs.example.com".into() },
            TabHit { browser: "Safari".into(), window_id: 1, tab_index: 2, title: "Docs".into(), url: "https://example.com".into() },
        ];
        let ranked = rank(items, "docs", |h| &h.title, |h| &h.url);
        assert_eq!(ranked[0].title, "Docs");
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        let items = vec![TabHit { browser: "Safari".into(), window_id: 1, tab_index: 1, title: "Homepage".into(), url: "https://example.com".into() }];
        assert!(rank(items, "zzzznotfound", |h| &h.title, |h| &h.url).is_empty());
    }

    #[test]
    fn an_empty_query_returns_everything_unfiltered() {
        let items = vec![
            TabHit { browser: "Safari".into(), window_id: 1, tab_index: 1, title: "A".into(), url: "https://a.com".into() },
            TabHit { browser: "Safari".into(), window_id: 1, tab_index: 2, title: "B".into(), url: "https://b.com".into() },
        ];
        assert_eq!(rank(items, "", |h| &h.title, |h| &h.url).len(), 2);
    }

    // -- Chromium bookmark parsing (recorded JSON fixture) -----------------

    /// A trimmed but structurally real fixture — recorded from the shape
    /// confirmed against this machine's actual Chrome `Bookmarks` file
    /// (`roots.bookmark_bar` / `roots.other`, `folder`/`url` nodes), not
    /// invented from memory.
    const CHROMIUM_FIXTURE: &str = r#"{
        "checksum": "abc",
        "roots": {
            "bookmark_bar": {
                "type": "folder",
                "name": "Bookmarks Bar",
                "children": [
                    { "type": "url", "name": "Example", "url": "https://example.com/" },
                    {
                        "type": "folder",
                        "name": "Work",
                        "children": [
                            { "type": "url", "name": "Docs", "url": "https://docs.example.com/" }
                        ]
                    }
                ]
            },
            "other": {
                "type": "folder",
                "name": "Other Bookmarks",
                "children": [
                    { "type": "url", "name": "", "url": "https://untitled.example.com/" }
                ]
            }
        }
    }"#;

    #[test]
    fn parses_top_level_and_nested_chromium_bookmarks() {
        let hits = parse_chromium_bookmarks(CHROMIUM_FIXTURE, "Chrome");
        assert_eq!(hits.len(), 3);
        let example = hits.iter().find(|h| h.url == "https://example.com/").unwrap();
        assert_eq!(example.title, "Example");
        assert_eq!(example.folder, None, "synthetic root should not become a folder label");

        let docs = hits.iter().find(|h| h.url == "https://docs.example.com/").unwrap();
        assert_eq!(docs.folder.as_deref(), Some("Work"));
        assert_eq!(docs.source, "Chrome");
    }

    #[test]
    fn a_nameless_chromium_bookmark_falls_back_to_its_url_as_the_title() {
        let hits = parse_chromium_bookmarks(CHROMIUM_FIXTURE, "Chrome");
        let untitled = hits.iter().find(|h| h.url == "https://untitled.example.com/").unwrap();
        assert_eq!(untitled.title, "https://untitled.example.com/");
    }

    #[test]
    fn malformed_chromium_json_yields_no_bookmarks_not_a_panic() {
        assert!(parse_chromium_bookmarks("not json", "Chrome").is_empty());
    }

    // -- Safari bookmark parsing (recorded JSON fixture) --------------------

    /// Shape based on Safari's long-stable, independently-documented plist
    /// schema (see the module doc comment) — not verified against this
    /// machine's real file, since reading it needs Full Disk Access this
    /// sandbox does not have.
    const SAFARI_FIXTURE: &str = r#"{
        "WebBookmarkType": "WebBookmarkTypeList",
        "Children": [
            {
                "WebBookmarkType": "WebBookmarkTypeLeaf",
                "URLString": "https://example.com/",
                "URIDictionary": { "title": "Example" }
            },
            {
                "WebBookmarkType": "WebBookmarkTypeList",
                "Title": "Reading List",
                "WebBookmarkFileVersion": 1,
                "Children": [
                    { "WebBookmarkType": "WebBookmarkTypeProxy", "WebBookmarkProxyType": "ReadingList" }
                ]
            },
            {
                "WebBookmarkType": "WebBookmarkTypeList",
                "Title": "Work",
                "Children": [
                    {
                        "WebBookmarkType": "WebBookmarkTypeLeaf",
                        "URLString": "https://docs.example.com/",
                        "URIDictionary": { "title": "Docs" }
                    }
                ]
            }
        ]
    }"#;

    #[test]
    fn parses_top_level_and_nested_safari_bookmarks() {
        let hits = parse_safari_bookmarks(SAFARI_FIXTURE);
        let example = hits.iter().find(|h| h.url == "https://example.com/").unwrap();
        assert_eq!(example.title, "Example");
        assert_eq!(example.source, "Safari");

        let docs = hits.iter().find(|h| h.url == "https://docs.example.com/").unwrap();
        assert_eq!(docs.folder.as_deref(), Some("Work"));
    }

    #[test]
    fn a_reading_list_proxy_node_contributes_no_bookmarks() {
        let hits = parse_safari_bookmarks(SAFARI_FIXTURE);
        assert_eq!(hits.len(), 2, "the Reading List folder's proxy child must be skipped");
    }

    #[test]
    fn a_leaf_with_no_uri_title_falls_back_to_its_url() {
        let fixture = r#"{
            "WebBookmarkType": "WebBookmarkTypeList",
            "Children": [
                { "WebBookmarkType": "WebBookmarkTypeLeaf", "URLString": "https://bare.example.com/" }
            ]
        }"#;
        let hits = parse_safari_bookmarks(fixture);
        assert_eq!(hits[0].title, "https://bare.example.com/");
    }

    #[test]
    fn malformed_safari_json_yields_no_bookmarks_not_a_panic() {
        assert!(parse_safari_bookmarks("{not json").is_empty());
    }

    // -- Firefox bookmark parsing (in-memory SQLite fixture) -----------------
    //
    // No real Firefox profile exists on this machine (see module doc
    // comment), so the schema is exercised against a database built here with
    // Mozilla's documented column layout — never a real, on-disk profile.

    fn firefox_fixture_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT);
             CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER, \
                 parent INTEGER, title TEXT);
             INSERT INTO moz_places (id, url, title) VALUES \
                 (1, 'https://example.com/', 'Example (page title)'), \
                 (2, 'https://docs.example.com/', 'Docs (page title)'), \
                 (3, 'https://folder-only.example.com/', NULL);
             -- type 1 = bookmark, type 2 = folder (Mozilla's own constants).
             INSERT INTO moz_bookmarks (id, type, fk, parent, title) VALUES \
                 (10, 1, 1, 1, 'My Renamed Bookmark'), \
                 (11, 1, 2, 1, NULL), \
                 (12, 2, NULL, 1, 'A Folder, Not A Bookmark');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_user_renamed_bookmark_title_wins_over_the_pages_own_title() {
        let conn = firefox_fixture_db();
        let hits = query_firefox_bookmarks(&conn).unwrap();
        let renamed = hits.iter().find(|h| h.url == "https://example.com/").unwrap();
        assert_eq!(renamed.title, "My Renamed Bookmark");
    }

    #[test]
    fn a_bookmark_with_no_custom_title_falls_back_to_the_pages_title() {
        let conn = firefox_fixture_db();
        let hits = query_firefox_bookmarks(&conn).unwrap();
        let fallback = hits.iter().find(|h| h.url == "https://docs.example.com/").unwrap();
        assert_eq!(fallback.title, "Docs (page title)");
    }

    #[test]
    fn folders_type_2_are_not_returned_as_bookmarks() {
        let conn = firefox_fixture_db();
        let hits = query_firefox_bookmarks(&conn).unwrap();
        assert_eq!(hits.len(), 2, "only the two type=1 rows should come back");
        assert!(hits.iter().all(|h| h.title != "A Folder, Not A Bookmark"));
    }

    // -- Downloads ------------------------------------------------------------

    #[test]
    fn in_progress_download_extensions_are_recognised() {
        for ext in IN_PROGRESS_EXTENSIONS {
            assert!(IN_PROGRESS_EXTENSIONS.contains(ext));
        }
        assert!(!IN_PROGRESS_EXTENSIONS.contains(&"pdf"));
    }

    #[test]
    fn recent_downloads_on_a_scratch_directory_skips_partial_files_and_sorts_by_recency() {
        let dir = std::env::temp_dir()
            .join(format!("caduceus-downloads-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("report.pdf"), b"pdf bytes").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("photo.png"), b"png bytes").unwrap();
        std::fs::write(dir.join("still-downloading.crdownload"), b"partial").unwrap();
        std::fs::write(dir.join(".hidden"), b"dotfile").unwrap();

        let mut hits: Vec<(std::time::SystemTime, DownloadHit)> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| download_hit(&e))
            .collect();
        hits.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        let names: Vec<String> = hits.into_iter().map(|(_, h)| h.name).collect();

        assert_eq!(names, vec!["photo.png", "report.pdf"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_download_hit_reports_size_and_an_uppercased_kind() {
        let dir = std::env::temp_dir()
            .join(format!("caduceus-downloads-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("archive.zip"), b"12345").unwrap();

        let entry = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap();
        let (_, hit) = download_hit(&entry).unwrap();
        assert_eq!(hit.kind, "ZIP");
        assert_eq!(hit.size_bytes, 5);
        assert!(!hit.modified.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
