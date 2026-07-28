//! Semantic / natural-language local file search — the conceptual-search
//! upgrade over `tools::search_files` (which is a raw `mdfind -name`
//! filename substring match, and stays exactly that for the "find that file
//! named invoice-2026" case it is good at).
//!
//! # The constraint that shaped this
//!
//! No vector-database crate and no local embedding-model crate are available
//! here, and `cargo add` is off the table. That rules out the two "obvious"
//! implementations (`sqlite-vec`/`usearch`-style ANN index; an in-process
//! embedding model such as `fastembed` or `rust-bert`) before design even
//! starts. What *is* available: `reqwest` (so a local Ollama server can be
//! asked to embed text over loopback HTTP) and `rusqlite` (already used by
//! `clipboard::store` for exactly this "small local database of records"
//! shape).
//!
//! # Decision: BM25 always, embeddings layered on top when Ollama has one
//!
//! Two designs were weighed:
//!
//! 1. **Embeddings-only, via Ollama.** Best semantic recall when it works,
//!    but Caduceus does not require any AI backend to be configured — this
//!    module would silently stop being useful (or exist at all) for anyone
//!    without Ollama running and an embedding model pulled, which the
//!    "Configure AI" scan in `agent::discover` shows is not everyone.
//! 2. **BM25-only.** Always available, no network dependency, and — this is
//!    the part worth being honest about — already a *huge* upgrade over
//!    `mdfind -name`: it ranks by relevance across the whole document, not a
//!    filename substring, and survives synonyms of form (plurals, verb
//!    tenses) via light stemming. But it still cannot find a note about
//!    "designing the database" from a query that says "schema for storing
//!    data" — no token overlap, no match, however semantically close.
//!
//! The chosen design is both, layered: BM25 lexical search is the floor —
//! self-contained, instant, works offline, works for every user — and when
//! `agent::discover`'s same detection pattern finds a local Ollama serving
//! an embedding-shaped model, its vectors are folded in via Reciprocal Rank
//! Fusion (`fuse_results`) so pure-synonym queries also surface. Losing the
//! Ollama connection (or never having had one) degrades exactly one step,
//! back to BM25, never to nothing. That degrade path is exercised by every
//! test in this file — none of them talk to a network, on purpose (see the
//! module doc on `mod tests` below), and the whole point is that the module
//! behaves identically whether or not that is because Ollama is absent.
//!
//! BM25 itself (Okapi BM25, `bm25_term_score`/`bm25_idf`) is hand-rolled
//! against `rusqlite` tables (`files`, `postings`) rather than reached for as
//! a crate, for the same reason the rest of this codebase hand-rolls small
//! well-understood algorithms (see `tools::documents`'s HTML stripper): it is
//! two formulas and a couple of SQL queries, not something worth a
//! dependency for. Same reasoning for the stemmer (`stem`) — a small,
//! deliberately crude suffix-stripper, not a real Porter implementation.
//! It does not need to be linguistically correct, only *consistent*: the
//! same function stems both the indexed document and the incoming query, so
//! "designing"/"designed"/"design" collapse to one BM25 term regardless of
//! whether the stem itself is a real word (`stem("running") == "runn"`,
//! which is wrong Latin grammar and exactly right for retrieval).
//!
//! # Privacy
//!
//! This module indexes the contents of personal files. Nothing it does may
//! leave the machine. The only network calls anywhere in this file are to
//! [`OLLAMA_BASE_URL`], a hardcoded `http://localhost:11434` constant — never
//! a URL built from settings, configuration, or user input — for exactly two
//! things: listing locally-installed model names (`/api/tags`) and asking
//! for an embedding vector (`/api/embeddings`). If that probe fails (Ollama
//! not installed, not running, or running with no embedding-shaped model
//! pulled), every embedding code path is skipped and the module falls all
//! the way back to BM25 rather than trying anywhere else. There is no cloud
//! fallback and none should ever be added here — a user who never touches
//! Ollama gets a fully local, fully offline search feature by construction,
//! not by configuration.
//!
//! # Extraction
//!
//! `.pdf` text comes from [`super::documents::extract_pdf_text`] — the PDF
//! extractor another agent built alongside this one — rather than a second
//! implementation; see that function's own doc comment for why it shells out
//! to Spotlight's importer instead of a PDF-parsing crate. `.md`/`.txt` are
//! read directly. `.rtf` gets a small hand-rolled control-word stripper
//! ([`strip_rtf`]) in the same spirit as `documents::extract_readable_text`'s
//! HTML handling: RTF is old-format-index-file territory (a `{...}` group
//! syntax with named "destination" groups like `\fonttbl` to discard, and
//! `\'hh` hex escapes in Windows-1252), not something worth a crate for.
//!
//! # Bounds, incrementality, and interruptibility
//!
//! - **Incremental**: [`SemanticIndex::sync`] compares each file's mtime and
//!   size against what is already indexed for that exact path and does
//!   *nothing* for files that have not changed — no re-read, no
//!   re-extraction, no re-tokenizing. The only per-file work on an unchanged
//!   file is one `stat()` (from the directory walk, which every sync does
//!   regardless) and, if the active embedding model has changed since last
//!   time, a possible re-embed — see [`SemanticIndex::embed_if_stale`] for
//!   why that specific case cannot be skipped by mtime alone.
//! - **Never blocks the UI**: every filesystem walk and every text
//!   extraction runs inside `tokio::task::spawn_blocking`, because both can
//!   block on real I/O for a while (a PDF's `mdimport` subprocess, a slow
//!   external drive) and this all happens off the UI's async runtime
//!   threads as a result. SQLite writes are not wrapped the same way — they
//!   are WAL-mode, single-row-scoped, and consistently sub-millisecond in
//!   local testing (see the report), which is the same judgment call
//!   `clipboard::store` already makes for the same database engine.
//! - **Interruptible**: [`CancelFlag`] is a cheap `Clone`-able handle a
//!   caller can flip from anywhere (a "Stop indexing" button, app shutdown).
//!   `sync` checks it between every file and between every directory-walk
//!   step, and returns whatever partial [`IndexStats`] it has rather than
//!   erroring — a cancelled sync is a normal outcome, not a failure one.
//! - **Bounded**: a max per-file size ([`IndexConfig::max_file_bytes`]), a
//!   max document count and max on-disk database size
//!   ([`IndexConfig::max_documents`], [`IndexConfig::max_db_bytes`]), a max
//!   number of files touched and directory depth walked per `sync` call
//!   ([`IndexConfig::max_files_per_run`], [`IndexConfig::max_depth`]) so one
//!   call cannot become an unbounded disk crawl, and a wall-clock budget on
//!   every `search` call ([`MAX_QUERY_DURATION`]) so a pathological query
//!   against a large index degrades to partial results instead of hanging.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::documents;

// ---------------------------------------------------------------------------
// Tunables / bounds
// ---------------------------------------------------------------------------

/// A file bigger than this is skipped rather than extracted. 8 MB comfortably
/// covers any real note, transcript, or report in these formats; anything
/// past it is far more likely a data dump than reading material, and a huge
/// text file would otherwise dominate a single `sync` call's time budget.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Ceiling on how many documents the index will ever hold. Bounds both
/// memory (the `existing` map `sync` loads per run) and worst-case query
/// time, which scans postings/embeddings proportional to corpus size.
const MAX_INDEXED_DOCUMENTS: usize = 50_000;

/// Ceiling on how many *new-or-changed* files a single `sync` call will
/// touch. A first-ever index of a large ~/Documents needs several calls to
/// finish rather than one call that runs for however long that takes — each
/// call is cheap and interruptible, so the caller (a background timer, most
/// likely) just calls `sync` again and picks up where the last one left off.
const MAX_FILES_PER_RUN: usize = 20_000;

/// Extra safety valve under [`MAX_FILES_PER_RUN`]: caps total directory
/// entries *looked at* (not just matched), so a root containing millions of
/// irrelevant files (a misconfigured root pointing at a package cache, say)
/// cannot make one `sync` call scan forever before hitting the file cap.
const WALK_MAX_ENTRIES: u64 = 300_000;

/// How deep under a configured root the walk descends. 12 covers any
/// reasonable personal folder structure without following an unbounded
/// nesting of subfolders.
const MAX_WALK_DEPTH: usize = 12;

/// Ceiling on the index database's on-disk size. Checked (cheaply, via
/// `stat`) before every *new* document is added; existing documents can
/// still be updated past this point since updates do not grow the document
/// count, only the content of one existing row.
const MAX_INDEX_DB_BYTES: u64 = 512 * 1024 * 1024;

/// Characters of a document sent to Ollama for embedding. Small on purpose:
/// this is a *gist* embedding for retrieval, not a full-document summary, and
/// keeping the request small keeps embedding fast and comfortably inside
/// every local embedding model's context window (roughly 500 tokens at
/// ~4 chars/token, well under even the smallest common context of 512).
const EMBED_EXCERPT_CHARS: usize = 2_000;

/// Loopback-only, hardcoded, never built from configuration — see the
/// privacy section of the module doc comment.
const OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// How long the "is Ollama even running" probe gets. Short: this runs on
/// every `search` call, and a slow "no" must not make typing feel laggy.
const OLLAMA_PROBE_TIMEOUT_MS: u64 = 1_500;

/// How long a single embedding request gets. Longer than the probe because
/// this is real model inference, not a port check.
const EMBED_TIMEOUT_SECS: u64 = 15;

/// Wall-clock budget for a whole `search` call. Bounds the BM25 postings
/// scan and, more importantly, the embedding cosine-similarity scan (which
/// is the one that scales with corpus size) so a query against a large index
/// returns *something* promptly rather than hanging until it is exhaustive.
const MAX_QUERY_DURATION: Duration = Duration::from_millis(1_500);

/// Reciprocal Rank Fusion constant. 60 is the value from the original RRF
/// paper and the de facto default everywhere it is used since; it is not
/// sensitive enough to the exact corpus here to be worth tuning.
const RRF_K: f32 = 60.0;

/// Standard Okapi BM25 defaults (Robertson et al.), unmodified: `k1` controls
/// how quickly extra occurrences of a term stop adding score, `b` controls
/// how strongly document length is penalised.
const BM25_K1: f32 = 1.5;
const BM25_B: f32 = 0.75;

/// File extensions this module knows how to read text out of.
const SUPPORTED_EXTENSIONS: &[&str] = &["md", "markdown", "txt", "text", "rtf", "pdf"];

/// Directory names skipped outright, case-insensitively, wherever they
/// appear under a configured root — build output, dependency caches, and
/// version-control internals, none of which contain the kind of document
/// this index is for, all of which are large enough to blow the per-run file
/// budget on nothing useful.
const SKIP_DIR_NAMES: &[&str] = &[
    "node_modules",
    ".git",
    ".svn",
    ".hg",
    "library",
    "applications",
    ".trash",
    ".cache",
    "dist",
    "build",
    "target",
    "venv",
    ".venv",
    "__pycache__",
    "pods",
    "deriveddata",
    ".cargo",
    ".rustup",
    "site-packages",
    ".npm",
    ".yarn",
    "vendor",
];

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Everything a `sync` call needs to know about where to look and how much
/// work it is allowed to do.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// Folders to index, recursively (subject to [`Self::max_depth`]).
    pub roots: Vec<PathBuf>,
    pub max_file_bytes: u64,
    pub max_documents: usize,
    pub max_files_per_run: usize,
    pub max_depth: usize,
    pub max_db_bytes: u64,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            roots: default_roots(),
            max_file_bytes: MAX_FILE_BYTES,
            max_documents: MAX_INDEXED_DOCUMENTS,
            max_files_per_run: MAX_FILES_PER_RUN,
            max_depth: MAX_WALK_DEPTH,
            max_db_bytes: MAX_INDEX_DB_BYTES,
        }
    }
}

/// The default set of roots: the folders someone keeps personal documents in
/// by convention, plus a couple of common "Notes" locations. Deliberately
/// does *not* default to the whole of iCloud Drive or the whole home
/// directory — both would sweep in application data that has nothing to do
/// with "find the note where I discussed X", and a wide surprising default
/// is a worse privacy posture than a narrow one the user can widen in
/// Settings (once a settings surface for this exists — see the report for
/// what still needs wiring up outside this file).
pub fn default_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let candidates = [
        home.join("Documents"),
        home.join("Desktop"),
        home.join("Downloads"),
        home.join("Documents").join("Notes"),
        home.join("Notes"),
        home.join("Library")
            .join("Mobile Documents")
            .join("com~apple~CloudDocs")
            .join("Notes"),
    ];
    candidates.into_iter().filter(|p| p.is_dir()).collect()
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

/// A cheap, `Clone`-able flag a caller holds onto and flips to interrupt an
/// in-progress [`SemanticIndex::sync`]. Checked between every file and every
/// directory-walk step; a cancelled sync returns its partial [`IndexStats`]
/// rather than an error, because "the user closed the window" is not a
/// failure of indexing.
#[derive(Clone)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Re-arm the flag before a new run.
    ///
    /// Without this a cancel latches forever: the flag is process-wide and
    /// shared with whatever starts the next sync, so one cancelled indexing run
    /// would quietly kill every future one until the app restarted — with the
    /// UI showing a Build button that appeared to do nothing.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tokenizing, stopwords, stemming
// ---------------------------------------------------------------------------

/// Split text into lowercase runs of letters/digits — Unicode-aware (so
/// accented text is not silently dropped), and deliberately keeps mixed
/// alphanumeric tokens like `"q3"` intact as one token rather than splitting
/// on the digit boundary, which matters for exactly the "Q3 budget" kind of
/// query this module exists for.
fn tokenize_raw(text: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[\p{L}\p{N}]+").expect("static regex is valid"));
    re.find_iter(text)
        .map(|m| m.as_str().to_lowercase())
        .filter(|w| w.chars().count() >= 2)
        .collect()
}

/// Tokenize, drop stopwords, and stem — the exact pipeline used for both
/// indexed document text and incoming queries, which is what makes the
/// stemmer's lack of linguistic correctness harmless (see the module doc).
fn tokenize_and_stem(text: &str) -> Vec<String> {
    tokenize_raw(text)
        .into_iter()
        .filter(|w| !is_stopword(w))
        .map(|w| stem(&w))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Standard English stopwords, plus the short fragments the regex tokenizer
/// leaves behind when it splits a contraction on the apostrophe (`"don't"` →
/// `"don"`, `"t"` — the latter is already dropped by the length-2 filter in
/// [`tokenize_raw`], but `"re"`/`"ve"`/`"ll"` and the un-contracted stems of
/// `"isn't"`/`"aren't"`/etc. are two letters or more and need to be listed
/// explicitly or they would count as real search terms).
const STOPWORDS: &[&str] = &[
    "a", "about", "above", "after", "again", "against", "all", "am", "an", "and", "any", "are", "aren", "as", "at",
    "be", "because", "been", "before", "being", "below", "between", "both", "but", "by", "can", "cannot", "could",
    "couldn", "did", "didn", "do", "does", "doesn", "doing", "don", "down", "during", "each", "few", "for", "from",
    "further", "had", "hadn", "has", "hasn", "have", "haven", "having", "he", "her", "here", "hers", "herself",
    "him", "himself", "his", "how", "if", "in", "into", "is", "isn", "it", "its", "itself", "just", "let", "ll",
    "me", "more", "most", "mustn", "my", "myself", "no", "nor", "not", "of", "off", "on", "once", "only", "or",
    "other", "ought", "our", "ours", "ourselves", "out", "over", "own", "re", "same", "shan", "she", "should",
    "shouldn", "so", "some", "such", "than", "that", "the", "their", "theirs", "them", "themselves", "then",
    "there", "these", "they", "this", "those", "through", "to", "too", "under", "until", "up", "very", "was",
    "wasn", "we", "were", "weren", "what", "when", "where", "which", "while", "who", "whom", "why", "will", "with",
    "won", "would", "wouldn", "ve", "you", "your", "yours", "yourself", "yourselves",
];

fn is_stopword(word: &str) -> bool {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| STOPWORDS.iter().copied().collect()).contains(word)
}

/// Suffix-stripping rules, longest/most-specific first so a word matching a
/// longer suffix is never left half-stripped by a shorter rule that also
/// happens to match its tail. `min_stem_len` guards against stripping a
/// short word down to nothing or near-nothing (`"as"` must not become `""`
/// via the `"s"` rule).
///
/// This is *not* a real Porter stemmer — see the module doc for why that is
/// a deliberate, documented simplification rather than a gap.
const STEM_RULES: &[(&str, &str, usize)] = &[
    ("ational", "ate", 3),
    ("tional", "tion", 3),
    ("ization", "ize", 3),
    ("fulness", "ful", 3),
    ("iveness", "ive", 3),
    ("ousness", "ous", 3),
    ("ities", "ity", 3),
    ("ing", "", 3),
    ("edly", "", 3),
    ("ies", "y", 2),
    ("ied", "y", 2),
    ("ed", "", 3),
    ("es", "", 3),
    ("ly", "", 3),
    ("ment", "", 4),
    ("ness", "", 3),
    ("tion", "te", 3),
    ("sion", "se", 3),
    ("er", "", 3),
    ("s", "", 3),
];

fn stem(word: &str) -> String {
    if word.chars().count() <= 3 {
        return word.to_string();
    }
    for (suffix, replacement, min_stem_len) in STEM_RULES {
        if let Some(stripped) = word.strip_suffix(suffix) {
            if stripped.chars().count() >= *min_stem_len {
                return format!("{stripped}{replacement}");
            }
        }
    }
    word.to_string()
}

// ---------------------------------------------------------------------------
// Text extraction
// ---------------------------------------------------------------------------

/// Read whatever text a supported file holds, dispatching by extension.
/// `.pdf` is handed to `documents::extract_pdf_text` rather than
/// re-implemented — see the module doc.
fn extract_text(path: &Path, ext: &str) -> Result<String, String> {
    match ext {
        "md" | "markdown" | "txt" | "text" => {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            // Lossy rather than `read_to_string`: a stray non-UTF-8 byte in
            // an otherwise-fine note must not make the whole file unindexable.
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
        "rtf" => {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            Ok(strip_rtf(&String::from_utf8_lossy(&bytes)))
        }
        "pdf" => documents::extract_pdf_text(&path.to_string_lossy()).map(|doc| doc.text),
        other => Err(format!("unsupported extension: {other}")),
    }
}

/// RTF "destination" groups whose entire contents are formatting metadata,
/// never document text: font/color/style tables, revision tracking, embedded
/// objects. Skipped wholesale, nested groups included.
const RTF_SKIP_DESTINATIONS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "generator",
    "pict",
    "object",
    "footnote",
    "header",
    "footer",
    "listtable",
    "listoverridetable",
    "revtbl",
    "xmlnstbl",
    "latentstyles",
    "themedata",
    "colorschememapping",
    "datastore",
    "filetbl",
    "rsidtbl",
];

/// Strip an RTF document down to its plain text.
///
/// A small hand-written scanner over RTF's `{\controlword ...}` group syntax
/// — not a full parser, in the same spirit as `documents.rs`'s HTML
/// stripper: it tracks brace depth to skip known-uninteresting "destination"
/// groups (fonttbl, colortbl, ...) wholesale, converts `\par`/`\line` to
/// newlines and `\tab` to a tab, decodes `\'hh` Windows-1252 hex escapes and
/// `\uN` Unicode escapes (including the one plain fallback character RTF
/// requires after a `\uN`, which is skipped rather than emitted twice), and
/// otherwise passes plain characters through once past the header. It does
/// not implement destination-group detection via `\*` (the "ignorable
/// destination" marker) separately from the named list above — every
/// destination this module needs to skip is named explicitly, and an
/// unrecognised `\*` group is passed through as if it were body text, which
/// is a visible artifact rather than lost content on the rare document that
/// hits it.
fn strip_rtf(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let mut depth: i32 = 0;
    let mut skip_depth: Option<i32> = None;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '{' => {
                depth += 1;
                i += 1;
            }
            '}' => {
                if skip_depth == Some(depth) {
                    skip_depth = None;
                }
                depth = (depth - 1).max(0);
                i += 1;
            }
            '\\' => {
                i += 1;
                if i >= chars.len() {
                    break;
                }
                let c2 = chars[i];
                if c2 == '\\' || c2 == '{' || c2 == '}' {
                    if skip_depth.is_none() && depth >= 1 {
                        out.push(c2);
                    }
                    i += 1;
                } else if c2 == '\'' {
                    i += 1;
                    let end = (i + 2).min(chars.len());
                    let hex: String = chars[i..end].iter().collect();
                    i = end;
                    if skip_depth.is_none() && depth >= 1 {
                        if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                            out.push(cp1252_char(byte));
                        }
                    }
                } else if c2.is_ascii_alphabetic() {
                    let start = i;
                    while i < chars.len() && chars[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word: String = chars[start..i].iter().collect();

                    let mut negative = false;
                    if i < chars.len() && chars[i] == '-' {
                        negative = true;
                        i += 1;
                    }
                    let digits_start = i;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    let num: Option<i32> = if i > digits_start {
                        chars[digits_start..i]
                            .iter()
                            .collect::<String>()
                            .parse::<i32>()
                            .ok()
                            .map(|n| if negative { -n } else { n })
                    } else {
                        None
                    };

                    // A single space after a control word is the word's own
                    // delimiter, not document text.
                    if i < chars.len() && chars[i] == ' ' {
                        i += 1;
                    }

                    let lower = word.to_ascii_lowercase();
                    if RTF_SKIP_DESTINATIONS.contains(&lower.as_str()) {
                        skip_depth = Some(depth);
                    } else if skip_depth.is_none() && depth >= 1 {
                        match lower.as_str() {
                            "par" | "line" => out.push('\n'),
                            "tab" => out.push('\t'),
                            "u" => {
                                if let Some(n) = num {
                                    let code = if n < 0 { (n + 65536) as u32 } else { n as u32 };
                                    if let Some(ch) = char::from_u32(code) {
                                        out.push(ch);
                                    }
                                    // RTF requires one fallback character after
                                    // \uN for readers that cannot decode it;
                                    // consumed here, not emitted twice.
                                    if i < chars.len() && !matches!(chars[i], '\\' | '{' | '}') {
                                        i += 1;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    // A control symbol (`\~`, `\_`, `\*`, ...): consumed, not
                    // emitted — none of them are document text.
                    i += 1;
                }
            }
            _ => {
                if skip_depth.is_none() && depth >= 1 {
                    out.push(c);
                }
                i += 1;
            }
        }
    }

    out
}

/// Decode one Windows-1252 byte, used for RTF's `\'hh` escapes. Windows-1252
/// and Latin-1 (`byte as char`) agree everywhere except 0x80-0x9F, which in
/// Windows-1252 holds the punctuation word processors actually export in
/// this range — curly quotes, en/em dashes, ellipsis — and in Latin-1 holds
/// unprintable C1 control codes. Using plain Latin-1 there would turn every
/// smart quote in an exported document into an invisible control character.
fn cp1252_char(byte: u8) -> char {
    match byte {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        other => other as char,
    }
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_ws = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(ch);
            last_ws = false;
        }
    }
    out.trim().to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{}\u{2026}", truncated.trim_end())
}

/// A title: the first non-empty line (Markdown `#` markers stripped), or the
/// filename if the document has no readable first line.
fn make_title(path: &Path, text: &str) -> String {
    for line in text.lines() {
        let cleaned = line.trim().trim_start_matches('#').trim();
        if !cleaned.is_empty() {
            return truncate_chars(cleaned, 140);
        }
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn make_snippet(text: &str) -> String {
    truncate_chars(&collapse_ws(text), 280)
}

fn embedding_excerpt(text: &str) -> String {
    let collapsed = collapse_ws(text);
    if collapsed.chars().count() <= EMBED_EXCERPT_CHARS {
        collapsed
    } else {
        collapsed.chars().take(EMBED_EXCERPT_CHARS).collect()
    }
}

// ---------------------------------------------------------------------------
// BM25
// ---------------------------------------------------------------------------

/// Okapi BM25 inverse document frequency, the "+1" (BM25+) variant that
/// stays positive even when a term appears in more than half the corpus,
/// where the classic formula can go negative and actively *hurt* a
/// document's score for containing a common-but-not-stopword term.
fn bm25_idf(total_docs: f64, doc_freq: f64) -> f32 {
    (((total_docs - doc_freq + 0.5) / (doc_freq + 0.5)) + 1.0).ln() as f32
}

/// One term's contribution to a document's BM25 score.
fn bm25_term_score(tf: f32, doc_len: f32, avg_doc_len: f32, idf: f32) -> f32 {
    let norm = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * (doc_len / avg_doc_len));
    idf * (tf * (BM25_K1 + 1.0)) / norm.max(f32::EPSILON)
}

/// Rank every document containing at least one query term by summed BM25
/// score, highest first, truncated to `candidate_n`. Checks `deadline`
/// before starting each term's postings scan — see [`MAX_QUERY_DURATION`].
fn bm25_search(
    conn: &Connection,
    terms: &[String],
    candidate_n: usize,
    deadline: Instant,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    let total_docs: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    if total_docs == 0 {
        return Ok(Vec::new());
    }
    let avg_doc_len: f64 = conn
        .query_row("SELECT AVG(doc_len) FROM files WHERE doc_len > 0", [], |r| r.get(0))
        .unwrap_or(0.0);
    let avg_doc_len = if avg_doc_len <= 0.0 { 1.0 } else { avg_doc_len as f32 };

    let mut scores: HashMap<i64, f32> = HashMap::new();
    for term in terms {
        if Instant::now() >= deadline {
            break;
        }

        let df: i64 = conn.query_row("SELECT COUNT(*) FROM postings WHERE term = ?1", params![term], |r| r.get(0))?;
        if df == 0 {
            continue;
        }
        let idf = bm25_idf(total_docs as f64, df as f64);

        let mut stmt = conn.prepare(
            "SELECT postings.file_id, postings.tf, files.doc_len
             FROM postings JOIN files ON files.id = postings.file_id
             WHERE postings.term = ?1",
        )?;
        let rows = stmt.query_map(params![term], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })?;
        for row in rows {
            let (file_id, tf, doc_len) = row?;
            let score = bm25_term_score(tf as f32, doc_len as f32, avg_doc_len, idf);
            *scores.entry(file_id).or_insert(0.0) += score;
        }
    }

    let mut ranked: Vec<(i64, f32)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(candidate_n);
    Ok(ranked)
}

// ---------------------------------------------------------------------------
// Embeddings (Ollama)
// ---------------------------------------------------------------------------

/// Substrings that show up in the names of embedding-shaped Ollama models
/// (`nomic-embed-text`, `mxbai-embed-large`, `all-minilm`, `bge-small`,
/// `gte-base`, `e5-large`, ...). Heuristic rather than an exhaustive list —
/// Ollama does not currently expose "this model is for embeddings" as
/// structured metadata from `/api/tags`, only the name — but every embedding
/// model on the Ollama library as of this writing matches one of these.
const EMBEDDING_MODEL_HINTS: &[&str] = &["embed", "minilm", "bge", "gte", "e5-"];

fn looks_like_embedding_model(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    EMBEDDING_MODEL_HINTS.iter().any(|hint| lower.contains(hint))
}

/// Pick the first embedding-shaped model name out of whatever Ollama has
/// installed, or `None` if it has nothing embedding-shaped (a chat-only
/// install, most commonly). Pure and network-free on purpose — this is the
/// part of embedding-model detection that is worth unit testing directly,
/// separately from the HTTP call that feeds it real data in
/// [`detect_embedder`].
fn choose_embedding_model<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    names.filter(|n| looks_like_embedding_model(n)).map(str::to_string).next()
}

struct OllamaEmbedder {
    model: String,
}

/// Probe the local Ollama server for an embedding-shaped model. `None`
/// covers every reason that could fail — not installed, not running, no
/// matching model — uniformly, because every one of those means the same
/// thing to a caller: fall back to BM25.
async fn detect_embedder() -> Option<OllamaEmbedder> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(OLLAMA_PROBE_TIMEOUT_MS))
        .connect_timeout(Duration::from_millis(OLLAMA_PROBE_TIMEOUT_MS))
        .build()
        .ok()?;
    let resp = client.get(format!("{OLLAMA_BASE_URL}/api/tags")).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let names: Vec<String> = json
        .get("models")?
        .as_array()?
        .iter()
        .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    let model = choose_embedding_model(names.iter().map(String::as_str))?;
    Some(OllamaEmbedder { model })
}

impl OllamaEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(EMBED_TIMEOUT_SECS))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .post(format!("{OLLAMA_BASE_URL}/api/embeddings"))
            .json(&serde_json::json!({ "model": self.model, "prompt": text }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Ollama embeddings returned {}", resp.status()));
        }
        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let arr = json
            .get("embedding")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "Ollama response had no \"embedding\" field".to_string())?;
        Ok(arr.iter().filter_map(|v| v.as_f64()).map(|f| f as f32).collect())
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

fn encode_vec(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

fn decode_vec(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Cosine-rank every stored embedding for the currently active `model`
/// against `query_vec`, highest first, truncated to `candidate_n`. Skips
/// embeddings from a *different* model outright — vectors from two models
/// live in unrelated spaces and comparing them is meaningless, not just
/// less accurate. Checked against `deadline` periodically (not every row:
/// the check itself has a cost, and this loop can run tens of thousands of
/// times per query).
fn embedding_search(
    conn: &Connection,
    query_vec: &[f32],
    model: &str,
    candidate_n: usize,
    deadline: Instant,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    let mut stmt = conn.prepare("SELECT file_id, vector FROM embeddings WHERE model = ?1")?;
    let rows = stmt.query_map(params![model], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;

    let mut scored: Vec<(i64, f32)> = Vec::new();
    for (i, row) in rows.enumerate() {
        if i % 256 == 0 && Instant::now() >= deadline {
            break;
        }
        let (file_id, bytes) = row?;
        let vector = decode_vec(&bytes);
        if vector.len() != query_vec.len() {
            continue;
        }
        scored.push((file_id, cosine_similarity(query_vec, &vector)));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(candidate_n);
    Ok(scored)
}

// ---------------------------------------------------------------------------
// Result fusion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// Found only by BM25 term overlap.
    Lexical,
    /// Found only by embedding similarity — no query term appears in the
    /// document, but its meaning is close.
    Semantic,
    /// Found by both.
    Hybrid,
}

/// Merge a BM25 ranking and an embedding-similarity ranking into one list
/// via Reciprocal Rank Fusion: `score(doc) = Σ 1/(k + rank)` over every list
/// the document appears in. RRF is used instead of normalizing and summing
/// the two raw scores because BM25 scores and cosine similarities live on
/// unrelated, unbounded-vs-bounded scales with no principled way to weight
/// one against the other — rank position is the one thing both lists agree
/// on the meaning of.
///
/// When `embed` is empty (no embedder available, or the query embedding
/// call failed), this returns the BM25 ranking unchanged rather than running
/// RRF against nothing, which would otherwise just relabel BM25's own scores
/// without changing their order.
fn fuse_results(bm25: &[(i64, f32)], embed: &[(i64, f32)], limit: usize) -> Vec<(i64, f32, MatchKind)> {
    if embed.is_empty() {
        return bm25.iter().take(limit).map(|&(id, score)| (id, score, MatchKind::Lexical)).collect();
    }

    let mut rrf: HashMap<i64, f32> = HashMap::new();
    let mut in_bm25: HashSet<i64> = HashSet::new();
    let mut in_embed: HashSet<i64> = HashSet::new();

    for (rank, (id, _)) in bm25.iter().enumerate() {
        *rrf.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
        in_bm25.insert(*id);
    }
    for (rank, (id, _)) in embed.iter().enumerate() {
        *rrf.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
        in_embed.insert(*id);
    }

    let mut ranked: Vec<(i64, f32)> = rrf.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(limit);

    ranked
        .into_iter()
        .map(|(id, score)| {
            let matched_via = match (in_bm25.contains(&id), in_embed.contains(&id)) {
                (true, true) => MatchKind::Hybrid,
                (true, false) => MatchKind::Lexical,
                (false, true) => MatchKind::Semantic,
                (false, false) => MatchKind::Lexical, // unreachable: id came from one of the two lists
            };
            (id, score, matched_via)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Directory walking
// ---------------------------------------------------------------------------

struct WalkResult {
    files: Vec<(PathBuf, i64, i64)>, // path, mtime (unix secs), size (bytes)
    truncated: bool,
    roots: Vec<PathBuf>,
}

impl WalkResult {
    fn root_for(&self, path: &Path) -> Option<&Path> {
        self.roots.iter().map(PathBuf::as_path).find(|r| path.starts_with(r))
    }
}

/// Drop any configured root that is itself inside another configured root
/// (e.g. `~/Documents/Notes` when `~/Documents` is also configured), so a
/// file under it is never walked, tokenized, and written twice in the same
/// pass. Canonicalizes so this comparison is robust to `..`/symlink
/// differences between two ways of writing the same path; falls back to the
/// given path unchanged if canonicalization fails (permissions, a path that
/// briefly stopped existing) rather than dropping the root outright.
fn dedupe_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut canon: Vec<PathBuf> = roots
        .iter()
        .filter(|r| r.is_dir())
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.clone()))
        .collect();
    canon.sort_by_key(|p| p.as_os_str().len());

    let mut kept: Vec<PathBuf> = Vec::new();
    for r in canon {
        if !kept.iter().any(|k| r.starts_with(k)) {
            kept.push(r);
        }
    }
    kept
}

fn to_unix_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A cheap `stat`-only walk of every configured root: no file content is
/// read here, which is what keeps this "incremental and cheap" per the
/// design brief — `sync` decides what to actually extract by comparing this
/// walk's mtime/size against what is already indexed, *after* this returns.
///
/// Synchronous by design: this is meant to be run inside
/// `tokio::task::spawn_blocking` by its caller, not awaited directly — see
/// [`SemanticIndex::sync`].
fn walk_roots(config: &IndexConfig, cancel: &CancelFlag) -> WalkResult {
    let roots = dedupe_roots(&config.roots);
    let mut files = Vec::new();
    let mut truncated = false;
    let mut visited_entries: u64 = 0;

    'roots: for root in &roots {
        let mut stack: Vec<(PathBuf, usize)> = vec![(root.clone(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            if cancel.is_cancelled() {
                truncated = true;
                break 'roots;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                visited_entries += 1;
                if visited_entries >= WALK_MAX_ENTRIES || files.len() >= config.max_files_per_run {
                    truncated = true;
                    break;
                }

                let Ok(file_type) = entry.file_type() else { continue };
                // Symlinks are skipped rather than followed, both for files
                // and directories — following directory symlinks risks an
                // unbounded (or cyclic) walk outside the configured roots.
                if file_type.is_symlink() {
                    continue;
                }

                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with('.') {
                    continue;
                }

                if file_type.is_dir() {
                    if depth >= config.max_depth {
                        continue;
                    }
                    if SKIP_DIR_NAMES.contains(&name_str.to_ascii_lowercase().as_str()) {
                        continue;
                    }
                    stack.push((entry.path(), depth + 1));
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }

                let path = entry.path();
                let Some(ext) = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) else {
                    continue;
                };
                if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
                    continue;
                }

                let Ok(meta) = entry.metadata() else { continue };
                let Ok(modified) = meta.modified() else { continue };
                files.push((path, to_unix_secs(modified), meta.len() as i64));
            }
            if truncated {
                break 'roots;
            }
        }
    }

    WalkResult { files, truncated, roots }
}

fn is_under_any_root(path: &str, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| Path::new(path).starts_with(r))
}

// ---------------------------------------------------------------------------
// SQLite schema
// ---------------------------------------------------------------------------

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS files (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            path        TEXT NOT NULL UNIQUE,
            root        TEXT NOT NULL,
            ext         TEXT NOT NULL,
            mtime       INTEGER NOT NULL,
            size        INTEGER NOT NULL,
            title       TEXT NOT NULL,
            snippet     TEXT NOT NULL,
            doc_len     INTEGER NOT NULL,
            indexed_at  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_files_root ON files(root);

        CREATE TABLE IF NOT EXISTS postings (
            term    TEXT NOT NULL,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            tf      INTEGER NOT NULL,
            PRIMARY KEY (term, file_id)
        );
        CREATE INDEX IF NOT EXISTS idx_postings_file ON postings(file_id);

        CREATE TABLE IF NOT EXISTS embeddings (
            file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
            model   TEXT NOT NULL,
            dim     INTEGER NOT NULL,
            vector  BLOB NOT NULL
        );

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---------------------------------------------------------------------------
// Public result / stats types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub path: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
    pub matched_via: MatchKind,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    /// Files seen by the directory walk (changed or not).
    pub scanned: usize,
    pub indexed: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped_too_large: usize,
    /// New documents rejected because [`IndexConfig::max_documents`] or
    /// [`IndexConfig::max_db_bytes`] was already reached.
    pub skipped_index_full: usize,
    pub errors: usize,
    pub embedded: usize,
    /// True if the walk or the file loop stopped early — a per-run budget
    /// was hit, or [`CancelFlag`] was raised. When true, deleted-file
    /// reconciliation is skipped for this run (see [`SemanticIndex::sync`]),
    /// since an incomplete walk cannot tell "deleted" apart from
    /// "not reached yet".
    pub truncated: bool,
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// SemanticIndex
// ---------------------------------------------------------------------------

/// The result of (re)indexing one changed file, carrying whether an
/// embedding was actually written (not just attempted — an Ollama request
/// can fail independently of the lexical write succeeding).
enum ReindexOutcome {
    InsertedNew(bool),
    UpdatedExisting(bool),
    NoExtractableText,
    ExtractionFailed,
}

/// A local search index over the configured roots: BM25 postings always,
/// per-document embeddings when a local Ollama with an embedding model is
/// present. One SQLite database file, one connection behind a mutex — the
/// same shape `clipboard::store::ClipboardStore` uses for the same reason:
/// writes come from a single background sync at a time, reads come from
/// interactive search, both at human speed, so connection pooling would be
/// complexity without a workload to justify it.
#[derive(Clone)]
pub struct SemanticIndex {
    conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
    embeddings_enabled: Arc<AtomicBool>,
}

impl SemanticIndex {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| e.to_string())?;
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(|e| e.to_string())?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(|e| e.to_string())?;
        migrate(&conn).map_err(|e| e.to_string())?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
            embeddings_enabled: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    /// Off by default for a caller that wants a guaranteed-offline index
    /// (or, as every test in this file does, a guaranteed-network-free run).
    /// On by default otherwise — an available local embedder should not
    /// require an extra opt-in to actually get used.
    pub fn set_embeddings_enabled(&self, on: bool) {
        self.embeddings_enabled.store(on, Ordering::Relaxed);
    }

    pub fn document_count(&self) -> Result<usize, String> {
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0))
            .map(|n| n as usize)
            .map_err(|e| e.to_string())
    }

    /// Bring the index up to date with the filesystem: walk the configured
    /// roots, (re)index anything new or changed, remove anything deleted,
    /// and refresh any embedding left stale by an embedding-model switch.
    /// See the module doc for the incrementality/bounds/interruptibility
    /// guarantees this provides.
    pub async fn sync(&self, config: &IndexConfig, cancel: CancelFlag) -> Result<IndexStats, String> {
        let start = Instant::now();
        let mut stats = IndexStats::default();

        if config.roots.is_empty() {
            stats.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(stats);
        }

        let walk_config = config.clone();
        let cancel_for_walk = cancel.clone();
        let walk = tokio::task::spawn_blocking(move || walk_roots(&walk_config, &cancel_for_walk))
            .await
            .map_err(|e| format!("index walk panicked: {e}"))?;

        stats.scanned = walk.files.len();
        stats.truncated = walk.truncated;
        if cancel.is_cancelled() {
            stats.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(stats);
        }

        let existing: HashMap<String, (i64, i64, i64)> = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare("SELECT path, id, mtime, size FROM files").map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))
                })
                .map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };

        // One detection call for the whole run rather than one per file —
        // an Ollama round trip is not free, and its answer will not change
        // in the seconds a `sync` call takes.
        let embedder = if self.embeddings_enabled.load(Ordering::Relaxed) {
            detect_embedder().await
        } else {
            None
        };

        let seen: HashSet<String> = walk.files.iter().map(|(p, _, _)| p.to_string_lossy().to_string()).collect();

        for (path, mtime, size) in &walk.files {
            if cancel.is_cancelled() {
                stats.truncated = true;
                break;
            }

            let path_str = path.to_string_lossy().to_string();
            let prior = existing.get(&path_str).copied();
            let unchanged = matches!(prior, Some((_, m, s)) if m == *mtime && s == *size);

            if unchanged {
                if let (Some(embedder), Some((file_id, _, _))) = (embedder.as_ref(), prior) {
                    if self.embed_if_stale(embedder, file_id, path, *size, config.max_file_bytes).await {
                        stats.embedded += 1;
                    }
                }
                continue;
            }

            if *size as u64 > config.max_file_bytes {
                stats.skipped_too_large += 1;
                continue;
            }

            let is_new = prior.is_none();
            if is_new {
                let doc_count = self.document_count().unwrap_or(0);
                let db_bytes = std::fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);
                if doc_count >= config.max_documents || db_bytes >= config.max_db_bytes {
                    stats.skipped_index_full += 1;
                    continue;
                }
            }

            let root = walk.root_for(path);
            match self.reindex_file(path, *mtime, *size, root, prior.map(|(id, _, _)| id), embedder.as_ref()).await {
                ReindexOutcome::InsertedNew(embedded) => {
                    stats.indexed += 1;
                    if embedded {
                        stats.embedded += 1;
                    }
                }
                ReindexOutcome::UpdatedExisting(embedded) => {
                    stats.updated += 1;
                    if embedded {
                        stats.embedded += 1;
                    }
                }
                ReindexOutcome::NoExtractableText => {}
                ReindexOutcome::ExtractionFailed => stats.errors += 1,
            }
        }

        // Deletion reconciliation only runs after a walk that saw every file
        // under every configured root — a truncated walk cannot distinguish
        // "this file was deleted" from "the per-run budget ran out before
        // reaching it", and guessing wrong there would silently drop a
        // document that is still on disk.
        if !stats.truncated {
            let conn = self.conn.lock();
            for (path_str, (id, _, _)) in &existing {
                if seen.contains(path_str) {
                    continue;
                }
                if !is_under_any_root(path_str, &walk.roots) {
                    continue;
                }
                if Path::new(path_str).exists() {
                    continue;
                }
                if conn.execute("DELETE FROM files WHERE id = ?1", params![id]).is_ok() {
                    stats.removed += 1;
                }
            }
        }

        {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT INTO meta (key, value) VALUES ('last_sync_at', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![unix_now().to_string()],
            );
        }

        stats.duration_ms = start.elapsed().as_millis() as u64;
        Ok(stats)
    }

    /// For a file whose content has *not* changed (mtime/size match), check
    /// whether it still needs a fresh embedding — which happens exactly
    /// when the active embedding model has changed since it was last
    /// embedded (the user switched, uninstalled, or newly installed a model
    /// in Ollama). This is the one case mtime-based incrementality cannot
    /// answer by itself: the file is identical, but the vector space its
    /// embedding lives in is not.
    async fn embed_if_stale(&self, embedder: &OllamaEmbedder, file_id: i64, path: &Path, size: i64, max_file_bytes: u64) -> bool {
        let has_current = {
            let conn = self.conn.lock();
            conn.query_row(
                "SELECT 1 FROM embeddings WHERE file_id = ?1 AND model = ?2",
                params![file_id, embedder.model],
                |_| Ok(()),
            )
            .optional()
            .unwrap_or(None)
            .is_some()
        };
        if has_current || size as u64 > max_file_bytes {
            return false;
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let p = path.to_path_buf();
        let text = match tokio::task::spawn_blocking(move || extract_text(&p, &ext)).await {
            Ok(Ok(t)) if !t.trim().is_empty() => t,
            _ => return false,
        };

        let Ok(vector) = embedder.embed(&embedding_excerpt(&text)).await else {
            return false;
        };
        if vector.is_empty() {
            return false;
        }

        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO embeddings (file_id, model, dim, vector) VALUES (?1,?2,?3,?4)",
            params![file_id, embedder.model, vector.len() as i64, encode_vec(&vector)],
        )
        .is_ok()
    }

    /// Extract, tokenize, and write one new-or-changed file: the lexical
    /// (BM25 postings) write always happens; the embedding write happens
    /// too if `embedder` is available. The SQLite transaction covers only
    /// the lexical write — it fully commits (or fails) before the embedding
    /// HTTP call starts, so the database lock is never held across an
    /// `.await`.
    async fn reindex_file(
        &self,
        path: &Path,
        mtime: i64,
        size: i64,
        root: Option<&Path>,
        prior_id: Option<i64>,
        embedder: Option<&OllamaEmbedder>,
    ) -> ReindexOutcome {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let p = path.to_path_buf();
        let ext_for_extract = ext.clone();
        let text = match tokio::task::spawn_blocking(move || extract_text(&p, &ext_for_extract)).await {
            Ok(Ok(t)) if !t.trim().is_empty() => t,
            Ok(Ok(_)) => return ReindexOutcome::NoExtractableText,
            _ => return ReindexOutcome::ExtractionFailed,
        };

        let tokens = tokenize_and_stem(&text);
        let doc_len = tokens.len() as i64;
        let mut term_freq: HashMap<String, i64> = HashMap::new();
        for t in tokens {
            *term_freq.entry(t).or_insert(0) += 1;
        }
        let title = make_title(path, &text);
        let snippet = make_snippet(&text);
        let now = unix_now();
        let path_str = path.to_string_lossy().to_string();
        let root_str = root.map(|r| r.to_string_lossy().to_string()).unwrap_or_default();
        let is_new = prior_id.is_none();

        let write_result: Result<i64, String> = {
            let mut conn = self.conn.lock();
            (|| {
                let tx = conn.transaction().map_err(|e| e.to_string())?;
                let file_id = if let Some(id) = prior_id {
                    tx.execute("DELETE FROM postings WHERE file_id = ?1", params![id]).map_err(|e| e.to_string())?;
                    tx.execute(
                        "UPDATE files SET mtime=?1,size=?2,ext=?3,title=?4,snippet=?5,doc_len=?6,indexed_at=?7,root=?8
                         WHERE id=?9",
                        params![mtime, size, ext, title, snippet, doc_len, now, root_str, id],
                    )
                    .map_err(|e| e.to_string())?;
                    id
                } else {
                    tx.execute(
                        "INSERT INTO files (path,root,ext,mtime,size,title,snippet,doc_len,indexed_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                        params![path_str, root_str, ext, mtime, size, title, snippet, doc_len, now],
                    )
                    .map_err(|e| e.to_string())?;
                    tx.last_insert_rowid()
                };
                {
                    let mut ins = tx
                        .prepare("INSERT INTO postings (term,file_id,tf) VALUES (?1,?2,?3)")
                        .map_err(|e| e.to_string())?;
                    for (term, tf) in &term_freq {
                        ins.execute(params![term, file_id, tf]).map_err(|e| e.to_string())?;
                    }
                }
                tx.commit().map_err(|e| e.to_string())?;
                Ok(file_id)
            })()
        };

        let file_id = match write_result {
            Ok(id) => id,
            Err(_) => return ReindexOutcome::ExtractionFailed,
        };

        let mut embedded = false;
        if let Some(embedder) = embedder {
            if let Ok(vector) = embedder.embed(&embedding_excerpt(&text)).await {
                if !vector.is_empty() {
                    let conn = self.conn.lock();
                    embedded = conn
                        .execute(
                            "INSERT OR REPLACE INTO embeddings (file_id, model, dim, vector) VALUES (?1,?2,?3,?4)",
                            params![file_id, embedder.model, vector.len() as i64, encode_vec(&vector)],
                        )
                        .is_ok();
                }
            }
        }

        if is_new {
            ReindexOutcome::InsertedNew(embedded)
        } else {
            ReindexOutcome::UpdatedExisting(embedded)
        }
    }

    /// Search the index. BM25 always runs; embedding-based semantic search
    /// layers on top of it when [`Self::set_embeddings_enabled`] has not
    /// disabled it and a local Ollama with an embedding model answers in
    /// time — see [`fuse_results`] for how the two are combined, and the
    /// module doc for why this degrades to lexical-only rather than failing.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, String> {
        let limit = limit.clamp(1, 200);
        let deadline = Instant::now() + MAX_QUERY_DURATION;

        let mut terms: Vec<String> = tokenize_and_stem(query).into_iter().collect::<HashSet<_>>().into_iter().collect();
        terms.sort();
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        let candidate_n = (limit * 6).clamp(30, 400);

        let bm25_ranked: Vec<(i64, f32)> = {
            let conn = self.conn.lock();
            bm25_search(&conn, &terms, candidate_n, deadline).map_err(|e| e.to_string())?
        };

        let embedder = if self.embeddings_enabled.load(Ordering::Relaxed) {
            detect_embedder().await
        } else {
            None
        };

        let embed_ranked: Vec<(i64, f32)> = if let Some(embedder) = &embedder {
            match embedder.embed(query).await {
                Ok(qvec) if !qvec.is_empty() => {
                    let conn = self.conn.lock();
                    embedding_search(&conn, &qvec, &embedder.model, candidate_n, deadline).unwrap_or_default()
                }
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let fused = fuse_results(&bm25_ranked, &embed_ranked, limit);
        if fused.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock();
        let mut hits = Vec::with_capacity(fused.len());
        for (file_id, score, matched_via) in fused {
            let row = conn
                .query_row("SELECT path, title, snippet FROM files WHERE id = ?1", params![file_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                })
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some((path, title, snippet)) = row {
                hits.push(SearchHit { path, title, snippet, score, matched_via });
            }
        }
        Ok(hits)
    }
}

// ---------------------------------------------------------------------------
// Tests
//
// Nothing here touches the network or requires Ollama — every test that
// exercises `SemanticIndex::sync`/`search` calls `set_embeddings_enabled(false)`
// first, which short-circuits both `detect_embedder` call sites before any
// `reqwest` call is made. The tokenizer, stemmer, BM25 math, cosine
// similarity, RRF fusion, RTF stripping, and directory-walk/incremental
// logic are otherwise all pure functions over local data and are tested
// directly against temp directories, per the brief for this module.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod cancel_flag_tests {
    use super::CancelFlag;

    /// The flag is shared and long-lived, so a cancel that could not be undone
    /// would take every later run down with it.
    #[test]
    fn a_cancelled_flag_can_be_re_armed_for_the_next_run() {
        let flag = CancelFlag::new();
        assert!(!flag.is_cancelled());

        flag.cancel();
        assert!(flag.is_cancelled());

        flag.reset();
        assert!(!flag.is_cancelled(), "a cancel must not latch across runs");
    }

    /// Clones share one flag — that is how cancel reaches a sync already
    /// running — so a reset through one handle must clear all of them.
    #[test]
    fn clones_share_the_same_flag() {
        let flag = CancelFlag::new();
        let clone = flag.clone();
        clone.cancel();
        assert!(flag.is_cancelled());
        flag.reset();
        assert!(!clone.is_cancelled());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("caduceus-semantic-test-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_db_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("caduceus-semantic-test-{label}-{}.db", uuid::Uuid::new_v4()))
    }

    fn offline_index(label: &str) -> SemanticIndex {
        let index = SemanticIndex::open(temp_db_path(label)).unwrap();
        index.set_embeddings_enabled(false);
        index
    }

    // -- tokenizing -------------------------------------------------------

    #[test]
    fn tokenize_raw_lowercases_and_keeps_alphanumeric_runs_together() {
        assert_eq!(
            tokenize_raw("Hello, World! Q3 2026 budget."),
            vec!["hello", "world", "q3", "2026", "budget"]
        );
    }

    #[test]
    fn tokenize_raw_drops_single_characters() {
        assert_eq!(tokenize_raw("a b of cats"), vec!["of", "cats"]);
    }

    #[test]
    fn tokenize_and_stem_removes_stopwords() {
        let tokens = tokenize_and_stem("The quick brown fox jumps over the lazy dog");
        assert!(!tokens.contains(&"the".to_string()));
        assert!(!tokens.contains(&"over".to_string()));
        assert!(tokens.contains(&"quick".to_string()));
    }

    #[test]
    fn tokenize_and_stem_handles_contractions_without_leaking_fragments() {
        let tokens = tokenize_and_stem("I don't think it's ready");
        assert!(!tokens.iter().any(|t| t == "t" || t == "s" || t == "don" || t == "isn"));
    }

    // -- stemming -----------------------------------------------------------

    #[test]
    fn stem_collapses_common_verb_and_noun_forms() {
        assert_eq!(stem("design"), "design");
        assert_eq!(stem("designs"), "design");
        assert_eq!(stem("designed"), "design");
        assert_eq!(stem("designing"), "design");
    }

    #[test]
    fn stem_collapses_plurals() {
        assert_eq!(stem("schema"), "schema");
        assert_eq!(stem("schemas"), "schema");
        assert_eq!(stem("boxes"), "box");
    }

    #[test]
    fn stem_leaves_short_words_alone() {
        assert_eq!(stem("was"), "was");
        assert_eq!(stem("as"), "as");
    }

    #[test]
    fn stem_is_deterministic() {
        for word in ["running", "database", "notes", "quickly", "reports"] {
            assert_eq!(stem(word), stem(word));
        }
    }

    // -- BM25 ---------------------------------------------------------------

    #[test]
    fn bm25_idf_is_higher_for_rarer_terms() {
        let rare = bm25_idf(1000.0, 2.0);
        let common = bm25_idf(1000.0, 500.0);
        assert!(rare > common, "rare={rare} common={common}");
    }

    #[test]
    fn bm25_idf_stays_positive_for_a_very_common_term() {
        // Classic (non-"+1") BM25 IDF goes negative once a term appears in
        // more than half the corpus; this variant must not.
        let idf = bm25_idf(1000.0, 900.0);
        assert!(idf > 0.0, "idf={idf}");
    }

    #[test]
    fn bm25_term_score_increases_with_term_frequency() {
        let low = bm25_term_score(1.0, 100.0, 100.0, 2.0);
        let high = bm25_term_score(5.0, 100.0, 100.0, 2.0);
        assert!(high > low);
    }

    #[test]
    fn bm25_term_score_penalises_longer_documents_at_equal_term_frequency() {
        let short_doc = bm25_term_score(2.0, 50.0, 100.0, 2.0);
        let long_doc = bm25_term_score(2.0, 400.0, 100.0, 2.0);
        assert!(short_doc > long_doc);
    }

    #[test]
    fn bm25_search_ranks_the_document_with_more_query_term_overlap_first() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        insert_test_doc(&conn, "/a.md", "alpha alpha alpha beta", 4);
        insert_test_doc(&conn, "/b.md", "alpha gamma delta epsilon", 4);

        let terms = vec!["alpha".to_string(), "beta".to_string()];
        let ranked = bm25_search(&conn, &terms, 10, Instant::now() + Duration::from_secs(5)).unwrap();

        assert_eq!(ranked.len(), 2);
        let path_of = |id: i64| -> String {
            conn.query_row("SELECT path FROM files WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
        };
        assert_eq!(path_of(ranked[0].0), "/a.md", "document matching both query terms should rank first");
    }

    #[test]
    fn bm25_search_on_an_empty_index_returns_nothing() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let ranked = bm25_search(&conn, &["anything".to_string()], 10, Instant::now() + Duration::from_secs(1)).unwrap();
        assert!(ranked.is_empty());
    }

    fn insert_test_doc(conn: &Connection, path: &str, body: &str, doc_len: i64) {
        conn.execute(
            "INSERT INTO files (path,root,ext,mtime,size,title,snippet,doc_len,indexed_at)
             VALUES (?1,'/','md',0,0,'t','s',?2,0)",
            params![path, doc_len],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        let mut tf: HashMap<&str, i64> = HashMap::new();
        for word in body.split_whitespace() {
            *tf.entry(word).or_insert(0) += 1;
        }
        for (term, count) in tf {
            conn.execute("INSERT INTO postings (term,file_id,tf) VALUES (?1,?2,?3)", params![term, file_id, count])
                .unwrap();
        }
    }

    // -- cosine similarity / vector encoding ---------------------------------

    #[test]
    fn cosine_similarity_of_identical_vectors_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_of_orthogonal_vectors_is_zero() {
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_of_opposite_vectors_is_negative_one() {
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_of_mismatched_lengths_is_zero_not_a_panic() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn vector_encoding_round_trips() {
        let v = vec![0.5f32, -1.25, 3.0, 0.0, 1e-6];
        assert_eq!(decode_vec(&encode_vec(&v)), v);
    }

    // -- embedding model selection (pure, no network) ------------------------

    #[test]
    fn embedding_model_hints_match_common_ollama_embedding_models() {
        for name in ["nomic-embed-text:latest", "mxbai-embed-large", "all-minilm", "bge-small-en", "gte-base"] {
            assert!(looks_like_embedding_model(name), "{name} should look like an embedding model");
        }
    }

    #[test]
    fn embedding_model_hints_reject_chat_models() {
        for name in ["llama3:8b", "qwen3:1.7b", "mistral", "phi3"] {
            assert!(!looks_like_embedding_model(name), "{name} should not look like an embedding model");
        }
    }

    #[test]
    fn choose_embedding_model_picks_the_first_match_or_none() {
        let names = vec!["llama3:8b", "nomic-embed-text:latest", "qwen3:1.7b"];
        assert_eq!(choose_embedding_model(names.into_iter()), Some("nomic-embed-text:latest".to_string()));
        assert_eq!(choose_embedding_model(vec!["llama3:8b", "qwen3:1.7b"].into_iter()), None);
    }

    // -- result fusion --------------------------------------------------------

    #[test]
    fn fuse_results_is_bm25_only_when_no_embeddings_are_available() {
        let bm25 = vec![(1, 3.0), (2, 1.0)];
        let fused = fuse_results(&bm25, &[], 10);
        assert_eq!(fused.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(), vec![1, 2]);
        assert!(fused.iter().all(|(_, _, k)| *k == MatchKind::Lexical));
    }

    #[test]
    fn fuse_results_marks_docs_found_by_both_lists_as_hybrid() {
        let bm25 = vec![(1, 5.0), (2, 1.0)];
        let embed = vec![(2, 0.9), (3, 0.8)];
        let fused = fuse_results(&bm25, &embed, 10);

        let kind_of = |id: i64| fused.iter().find(|(fid, _, _)| *fid == id).map(|(_, _, k)| *k);
        assert_eq!(kind_of(1), Some(MatchKind::Lexical));
        assert_eq!(kind_of(2), Some(MatchKind::Hybrid));
        assert_eq!(kind_of(3), Some(MatchKind::Semantic));
    }

    #[test]
    fn fuse_results_respects_the_limit() {
        let bm25: Vec<(i64, f32)> = (0..10).map(|i| (i, 10.0 - i as f32)).collect();
        let embed: Vec<(i64, f32)> = (0..10).map(|i| (i, 1.0 - i as f32 * 0.01)).collect();
        let fused = fuse_results(&bm25, &embed, 3);
        assert_eq!(fused.len(), 3);
    }

    // -- RTF stripping --------------------------------------------------------

    #[test]
    fn strip_rtf_extracts_body_text_and_drops_font_and_color_tables() {
        let rtf = r#"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fnil\fcharset0 Calibri;}}
{\colortbl ;\red255\green0\blue0;}
\viewkind4\uc1\pard\f0\fs22 Hello \b world\b0 , this is a test.\par
Second line.\par
}"#;
        let text = strip_rtf(rtf);
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(text.contains("Second line"));
        assert!(!text.contains("fonttbl"));
        assert!(!text.contains("Calibri"));
        assert!(!text.contains("colortbl"));
    }

    #[test]
    fn strip_rtf_converts_par_to_newlines_and_tab_to_a_tab() {
        let rtf = r#"{\rtf1 First\par Second\tab Third}"#;
        let text = strip_rtf(rtf);
        assert!(text.contains("First"));
        assert!(text.contains('\n'));
        assert!(text.contains('\t'));
    }

    #[test]
    fn strip_rtf_decodes_windows_1252_smart_quotes() {
        let rtf = r#"{\rtf1 \'93quoted\'94}"#;
        let text = strip_rtf(rtf);
        assert!(text.contains('\u{201C}'), "text was: {text:?}");
        assert!(text.contains('\u{201D}'), "text was: {text:?}");
    }

    #[test]
    fn strip_rtf_decodes_a_unicode_escape_and_skips_its_ascii_fallback() {
        // The RTF control word `\uN` followed by one ASCII fallback char
        // ("?") — 8364 is the codepoint for U+20AC EURO SIGN.
        let rtf = "{\\rtf1 \\u8364?}";
        let text = strip_rtf(rtf);
        assert_eq!(text.trim(), "\u{20AC}");
    }

    #[test]
    fn cp1252_char_matches_ascii_below_0x80() {
        assert_eq!(cp1252_char(b'A'), 'A');
    }

    // -- title / snippet helpers ------------------------------------------

    #[test]
    fn make_title_strips_a_markdown_heading_marker() {
        let title = make_title(Path::new("/notes/x.md"), "# Database Schema Design\n\nBody text.");
        assert_eq!(title, "Database Schema Design");
    }

    #[test]
    fn make_title_falls_back_to_the_filename_when_the_file_has_no_text() {
        let title = make_title(Path::new("/notes/empty-file.md"), "   \n  \n");
        assert_eq!(title, "empty-file");
    }

    #[test]
    fn make_snippet_collapses_whitespace_and_truncates() {
        let snippet = make_snippet(&"word ".repeat(200));
        assert!(snippet.chars().count() <= 281);
        assert!(snippet.ends_with('\u{2026}'));
    }

    // -- extraction dispatch -------------------------------------------------

    #[test]
    fn extract_text_reads_markdown_and_txt_directly() {
        let dir = temp_dir("extract");
        let md = dir.join("note.md");
        std::fs::write(&md, "# Title\n\nSome body text.").unwrap();
        assert_eq!(extract_text(&md, "md").unwrap(), "# Title\n\nSome body text.");
    }

    #[test]
    fn extract_text_strips_rtf() {
        let dir = temp_dir("extract-rtf");
        let rtf = dir.join("note.rtf");
        std::fs::write(&rtf, r#"{\rtf1 Plain text here.}"#).unwrap();
        assert!(extract_text(&rtf, "rtf").unwrap().contains("Plain text here."));
    }

    #[test]
    fn extract_text_rejects_unsupported_extensions() {
        assert!(extract_text(Path::new("/tmp/whatever.exe"), "exe").is_err());
    }

    #[test]
    fn extract_text_dispatches_pdf_to_the_shared_extractor() {
        // A nonexistent path is enough to prove the dispatch happens without
        // needing a real PDF or `mdimport`: `documents::extract_pdf_text`
        // checks the file exists before doing anything else.
        let err = extract_text(Path::new("/definitely/not/a/real/file.pdf"), "pdf").unwrap_err();
        assert!(err.to_lowercase().contains("not exist") || err.to_lowercase().contains("pdf"), "err was: {err}");
    }

    // -- directory walking -----------------------------------------------

    #[test]
    fn walk_roots_finds_supported_extensions_and_skips_others() {
        let dir = temp_dir("walk-basic");
        std::fs::write(dir.join("a.md"), "hello").unwrap();
        std::fs::write(dir.join("b.txt"), "hello").unwrap();
        std::fs::write(dir.join("c.png"), "not text").unwrap();

        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };
        let result = walk_roots(&config, &CancelFlag::new());

        let names: HashSet<String> =
            result.files.iter().map(|(p, _, _)| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert!(names.contains("a.md"));
        assert!(names.contains("b.txt"));
        assert!(!names.contains("c.png"));
    }

    #[test]
    fn walk_roots_skips_hidden_and_denylisted_directories() {
        let dir = temp_dir("walk-skip");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("secret.md"), "x").unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("node_modules").join("readme.md"), "x").unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join(".hidden").join("note.md"), "x").unwrap();
        std::fs::write(dir.join("visible.md"), "x").unwrap();

        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };
        let result = walk_roots(&config, &CancelFlag::new());

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].0.file_name().unwrap().to_string_lossy(), "visible.md");
    }

    #[test]
    fn walk_roots_respects_max_depth() {
        let dir = temp_dir("walk-depth");
        let deep = dir.join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(dir.join("a").join("shallow.md"), "x").unwrap();
        std::fs::write(deep.join("deep.md"), "x").unwrap();

        let config = IndexConfig { roots: vec![dir.clone()], max_depth: 1, ..Default::default() };
        let result = walk_roots(&config, &CancelFlag::new());

        let names: HashSet<String> =
            result.files.iter().map(|(p, _, _)| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert!(names.contains("shallow.md"));
        assert!(!names.contains("deep.md"), "a file past max_depth must not be walked");
    }

    #[test]
    fn walk_roots_truncates_at_the_per_run_file_budget() {
        let dir = temp_dir("walk-budget");
        for i in 0..10 {
            std::fs::write(dir.join(format!("f{i}.md")), "x").unwrap();
        }
        let config = IndexConfig { roots: vec![dir.clone()], max_files_per_run: 3, ..Default::default() };
        let result = walk_roots(&config, &CancelFlag::new());
        assert!(result.truncated);
        assert!(result.files.len() <= 3);
    }

    #[test]
    fn walk_roots_honours_a_cancel_flag() {
        let dir = temp_dir("walk-cancel");
        std::fs::write(dir.join("a.md"), "x").unwrap();
        let cancel = CancelFlag::new();
        cancel.cancel();
        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };
        let result = walk_roots(&config, &cancel);
        assert!(result.truncated);
    }

    #[test]
    fn dedupe_roots_drops_a_root_nested_inside_another_root() {
        let dir = temp_dir("dedupe");
        let nested = dir.join("notes");
        std::fs::create_dir_all(&nested).unwrap();
        let kept = dedupe_roots(&[dir.clone(), nested.clone()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].canonicalize().unwrap(), dir.canonicalize().unwrap());
    }

    // -- incremental sync (integration, network-free) ------------------------

    #[tokio::test]
    async fn sync_indexes_new_files_and_is_a_no_op_on_the_second_call() {
        let dir = temp_dir("sync-basic");
        std::fs::write(dir.join("one.md"), "database schema design notes").unwrap();
        std::fs::write(dir.join("two.txt"), "grocery list: milk, eggs, bread").unwrap();

        let index = offline_index("sync-basic");
        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };

        let first = index.sync(&config, CancelFlag::new()).await.unwrap();
        assert_eq!(first.indexed, 2);
        assert_eq!(first.updated, 0);
        assert_eq!(index.document_count().unwrap(), 2);

        let second = index.sync(&config, CancelFlag::new()).await.unwrap();
        assert_eq!(second.indexed, 0, "an unchanged file must not be re-indexed");
        assert_eq!(second.updated, 0);
        assert_eq!(second.scanned, 2, "the walk itself still runs — only reprocessing is skipped");
    }

    #[tokio::test]
    async fn sync_reprocesses_a_file_whose_content_changed() {
        let dir = temp_dir("sync-change");
        let path = dir.join("note.md");
        std::fs::write(&path, "original content").unwrap();

        let index = offline_index("sync-change");
        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };
        index.sync(&config, CancelFlag::new()).await.unwrap();

        // A size-changing edit, so this is detected regardless of the
        // filesystem's mtime resolution (which can be coarser than the time
        // this test takes to run).
        std::fs::write(&path, "original content, now with considerably more text appended to it").unwrap();
        let second = index.sync(&config, CancelFlag::new()).await.unwrap();
        assert_eq!(second.updated, 1);
        assert_eq!(second.indexed, 0);
        assert_eq!(index.document_count().unwrap(), 1);
    }

    #[tokio::test]
    async fn sync_removes_a_deleted_file_from_the_index() {
        let dir = temp_dir("sync-delete");
        let path = dir.join("note.md");
        std::fs::write(&path, "temporary content").unwrap();

        let index = offline_index("sync-delete");
        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };
        index.sync(&config, CancelFlag::new()).await.unwrap();
        assert_eq!(index.document_count().unwrap(), 1);

        std::fs::remove_file(&path).unwrap();
        let second = index.sync(&config, CancelFlag::new()).await.unwrap();
        assert_eq!(second.removed, 1);
        assert_eq!(index.document_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn sync_skips_files_over_the_size_limit() {
        let dir = temp_dir("sync-oversize");
        std::fs::write(dir.join("big.txt"), "x".repeat(1000)).unwrap();

        let index = offline_index("sync-oversize");
        let config = IndexConfig { roots: vec![dir.clone()], max_file_bytes: 100, ..Default::default() };
        let stats = index.sync(&config, CancelFlag::new()).await.unwrap();

        assert_eq!(stats.skipped_too_large, 1);
        assert_eq!(index.document_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn sync_stops_adding_new_documents_once_the_document_cap_is_reached() {
        let dir = temp_dir("sync-cap");
        std::fs::write(dir.join("a.md"), "first document").unwrap();
        std::fs::write(dir.join("b.md"), "second document").unwrap();

        let index = offline_index("sync-cap");
        let config = IndexConfig { roots: vec![dir.clone()], max_documents: 1, ..Default::default() };
        let stats = index.sync(&config, CancelFlag::new()).await.unwrap();

        assert_eq!(index.document_count().unwrap(), 1);
        assert_eq!(stats.indexed, 1);
        assert_eq!(stats.skipped_index_full, 1);
    }

    #[tokio::test]
    async fn sync_never_deletes_a_file_whose_root_is_no_longer_configured() {
        let dir_a = temp_dir("sync-root-a");
        let dir_b = temp_dir("sync-root-b");
        std::fs::write(dir_a.join("keep-me.md"), "content under root a").unwrap();

        let index = offline_index("sync-root-safety");
        index.sync(&IndexConfig { roots: vec![dir_a.clone()], ..Default::default() }, CancelFlag::new()).await.unwrap();
        assert_eq!(index.document_count().unwrap(), 1);

        // Root A is no longer configured at all — this must not be treated
        // as "everything under it was deleted".
        let stats = index
            .sync(&IndexConfig { roots: vec![dir_b.clone()], ..Default::default() }, CancelFlag::new())
            .await
            .unwrap();

        assert_eq!(stats.removed, 0);
        assert_eq!(index.document_count().unwrap(), 1, "a file under an unconfigured root must survive");
    }

    #[tokio::test]
    async fn sync_honours_cancellation_mid_run() {
        let dir = temp_dir("sync-cancel");
        for i in 0..5 {
            std::fs::write(dir.join(format!("f{i}.md")), format!("document number {i}")).unwrap();
        }
        let index = offline_index("sync-cancel");
        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };

        let cancel = CancelFlag::new();
        cancel.cancel();
        let stats = index.sync(&config, cancel).await.unwrap();
        assert!(stats.truncated);
        assert_eq!(index.document_count().unwrap(), 0, "a pre-cancelled sync must do no indexing work");
    }

    #[tokio::test]
    async fn search_finds_a_document_via_stemmed_term_overlap() {
        let dir = temp_dir("search-basic");
        std::fs::write(dir.join("schema.md"), "Notes on designing the database schema for the new app").unwrap();
        std::fs::write(dir.join("groceries.md"), "Buy milk, eggs, and bread this weekend").unwrap();

        let index = offline_index("search-basic");
        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };
        index.sync(&config, CancelFlag::new()).await.unwrap();

        let hits = index.search("designed databases", 10).await.unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].path.ends_with("schema.md"), "top hit was {:?}", hits.first().map(|h| &h.path));
        assert_eq!(hits[0].matched_via, MatchKind::Lexical);
    }

    #[tokio::test]
    async fn search_on_an_empty_index_returns_no_results_not_an_error() {
        let index = offline_index("search-empty");
        let hits = index.search("anything at all", 10).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_with_only_stopwords_returns_no_results() {
        let dir = temp_dir("search-stopwords");
        std::fs::write(dir.join("a.md"), "the quick brown fox").unwrap();
        let index = offline_index("search-stopwords");
        index.sync(&IndexConfig { roots: vec![dir], ..Default::default() }, CancelFlag::new()).await.unwrap();

        let hits = index.search("the and of", 10).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn perf_measure_sync_and_search() {
        let dir = temp_dir("perf");
        for i in 0..1000 {
            let body = format!(
                "Document number {i}\n\nThis note discusses topic {t} in some detail, covering database schema design, \
                 budget planning for Q{q} 2026, grocery lists, and various other subjects that come up in day to day \
                 notes. Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt.",
                t = i % 37,
                q = (i % 4) + 1,
            );
            std::fs::write(dir.join(format!("note{i}.md")), body).unwrap();
        }

        let index = offline_index("perf");
        let config = IndexConfig { roots: vec![dir.clone()], ..Default::default() };

        let start = std::time::Instant::now();
        let stats = index.sync(&config, CancelFlag::new()).await.unwrap();
        let first_sync = start.elapsed();
        eprintln!("first sync (1000 new files): {:?} stats={:?}", first_sync, stats);

        let start = std::time::Instant::now();
        let stats2 = index.sync(&config, CancelFlag::new()).await.unwrap();
        let second_sync = start.elapsed();
        eprintln!("second sync (0 changed): {:?} stats={:?}", second_sync, stats2);

        let start = std::time::Instant::now();
        let hits = index.search("database schema design", 10).await.unwrap();
        let query_time = start.elapsed();
        eprintln!("query time: {:?} hits={}", query_time, hits.len());

        let db_size = std::fs::metadata(index.path()).map(|m| m.len()).unwrap_or(0);
        eprintln!("db file size after 1000 docs: {} bytes ({:.2} MB)", db_size, db_size as f64 / (1024.0 * 1024.0));
    }
}
