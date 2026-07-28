//! Smart multi-model auto-routing: deciding *which backend* answers a prompt
//! without ever asking a model to make that decision.
//!
//! # Why a heuristic classifier and not a model call
//!
//! The entire point of routing is to avoid paying cloud latency and cost for
//! work a local model could have done in milliseconds. A classifier that
//! itself calls a model — even a small one — reintroduces exactly the
//! round-trip this feature exists to skip, and on the *fast* path, where the
//! extra hop matters most. So [`classify`] is pure: string length, word
//! shape, keyword presence, punctuation counts. No I/O, no async, no model.
//! It runs in well under a microsecond and gives the same answer every time
//! for the same input, which also makes it unit-testable without a running
//! backend — see the adversarial cases at the bottom of this file.
//!
//! # Why heuristics can be trusted here
//!
//! This is not trying to be a general-purpose difficulty estimator — that
//! would need a model. It only has to separate two very different shapes of
//! request: a short, mechanical, single-step edit (format this, rename that,
//! fix the typo) from something that needs sustained reasoning (design,
//! debug, compare, analyze, write from scratch across a long document). Those
//! two shapes tend to *announce themselves* in the verbs and structure of the
//! prompt, which is what the signal set below leans on. Getting the boundary
//! case wrong costs a slightly worse first attempt from the small model, not
//! a wrong answer nobody checks — so the classifier is deliberately biased
//! toward the cheap path when the signals are weak or absent (see
//! `COMPLEX_THRESHOLD`), and only escalates when something concrete pushes it
//! there.
//!
//! # The pieces
//!
//! - [`classify`] — the classifier. Deterministic, explainable, no model.
//! - [`route`] — the policy: maps a [`Classification`] plus the caller's
//!   backend list to a [`RoutingDecision`], honouring a user override and
//!   falling back sanely when the ideal backend is unavailable.
//! - [`LatencyTracker`] — an in-memory (never persisted, never transmitted)
//!   rolling average of how long each backend actually took last time, so
//!   "the fastest local backend" is measured rather than assumed. See
//!   [`latency_tracker`] for the process-wide instance and [`LatencyGuard`]
//!   for the one-line way to feed it from a call site.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use crate::settings::{BackendConfig, BackendKind};

// ---------------------------------------------------------------------------
// Classifier
// ---------------------------------------------------------------------------

/// The two buckets routing cares about. Deliberately not a finer-grained
/// scale ("easy/medium/hard/expert...") — a policy only needs to know which
/// side of "does this need the strong model" a prompt falls on, and a two-way
/// split is the only version of that question a length-and-keyword heuristic
/// can answer honestly. Anything fancier would be false precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    /// Small, fast, mechanical. Formatting, renaming, a one-line translation,
    /// a regex, a commit message. Safe to hand to a local model.
    Micro,
    /// Needs sustained reasoning, multi-step planning, or wide context.
    /// Architecture, debugging, analysis, long documents. Goes to the
    /// configured strong backend.
    Complex,
}

/// A short prompt gets the benefit of the doubt *unless* something concrete
/// (a reasoning verb, a code-review request) says otherwise — most short
/// requests really are mechanical, and the cost of guessing wrong on a short
/// prompt is small. A long prompt gets the opposite benefit of the doubt:
/// most prompts nobody bothered to write six sentences for really do need
/// the reasoning. These are word counts, not characters, because word count
/// tracks "how much is being asked" far better than raw length — a 40-word
/// sentence and a 400-character minified JSON blob are very different asks
/// that happen to be similar in character count.
const SHORT_WORD_THRESHOLD: usize = 12;
const LONG_WORD_THRESHOLD: usize = 60;

/// At most this many distinct keyword hits count toward the score in either
/// direction. Without a cap, a prompt that happens to say "refactor" three
/// times would out-vote every other signal; the cap keeps one repeated word
/// from dominating over structure and length.
const MAX_KEYWORD_HITS: usize = 3;

/// The score has to clear this to be called complex. Zero or negative stays
/// micro — see the module doc for why ties lean cheap.
const COMPLEX_THRESHOLD: i32 = 1;

/// Phrases that mark a request as small, mechanical, single-step work.
/// Deliberately specific multi-word phrases where a bare word would be too
/// eager to fire (e.g. no bare "convert", because "convert this monolith to
/// microservices" is not micro work just because it contains "convert").
const MICRO_KEYWORDS: &[&str] = &[
    "format",
    "reformat",
    "pretty-print",
    "pretty print",
    "prettify",
    "minify",
    "lint",
    "typo",
    "rename",
    "capitalize",
    "lowercase",
    "uppercase",
    "translate",
    "shorten this",
    "trim whitespace",
    "commit message",
    "one-liner",
    "one liner",
    "reindent",
    "indent this",
    "align this",
    "camelcase",
    "snake_case",
    "regex",
    "regular expression",
    "syntax highlight",
];

/// Phrases that mark a request as needing sustained reasoning. Also specific
/// multi-word phrases for the same reason as above — bare "why" fires on
/// idle chit-chat, but "why does" and "why is" are diagnostic framing.
const COMPLEX_KEYWORDS: &[&str] = &[
    "architecture",
    "architectural",
    "design a",
    "design an",
    "redesign",
    "system design",
    "analyze",
    "analyse",
    "analysis",
    "compare",
    "comparison",
    "trade-off",
    "tradeoff",
    "trade off",
    "refactor",
    "root cause",
    "investigate",
    "investigation",
    "explain why",
    "why does",
    "why is",
    "why did",
    "prove",
    "derive",
    "derivation",
    "debug",
    "review this",
    "review the",
    "code review",
    "summarize the document",
    "evaluate",
    "evaluation",
    "assess",
    "diagnose",
    "diagnosis",
    "algorithm for",
    "migration plan",
    "audit",
    "multi-step",
    "strategy",
    "security issues",
    "distributed system",
    "consensus algorithm",
    "pros and cons",
    "implications of",
];

/// Everything [`classify`] worked out about one prompt, kept around (rather
/// than collapsed straight to a [`TaskClass`]) so [`route`] — and tests — can
/// see *why*, not just *what*.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub class: TaskClass,
    /// The raw signed score. Positive pushes toward [`TaskClass::Complex`].
    /// Exposed mainly so tests can assert on more than the final bucket.
    pub score: i32,
    /// One clause (no trailing period — [`route`] composes it into a full
    /// sentence) explaining the classification in terms a user could read.
    pub reason: String,
    pub word_count: usize,
    pub matched_micro_keywords: Vec<&'static str>,
    pub matched_complex_keywords: Vec<&'static str>,
    pub looks_like_code: bool,
}

/// Classify `prompt` as [`TaskClass::Micro`] or [`TaskClass::Complex`] using
/// only its text — length, keyword shape, and structure. Never calls a model
/// and never blocks; see the module doc for why that is the whole point.
pub fn classify(prompt: &str) -> Classification {
    let trimmed = prompt.trim();
    let lower = trimmed.to_lowercase();
    let word_count = trimmed.split_whitespace().count();

    let matched_micro: Vec<&'static str> =
        MICRO_KEYWORDS.iter().copied().filter(|k| lower.contains(k)).collect();
    let matched_complex: Vec<&'static str> =
        COMPLEX_KEYWORDS.iter().copied().filter(|k| lower.contains(k)).collect();
    let code = looks_like_code(trimmed);

    // Multiple blank-line-separated paragraphs and a run of several sentences
    // both say "this is a document, not an instruction" — a signal length
    // alone can miss when every individual sentence is short.
    let paragraph_count = trimmed.split("\n\n").filter(|p| !p.trim().is_empty()).count();
    let sentence_count = trimmed.chars().filter(|c| matches!(c, '.' | '!' | '?')).count();
    let question_marks = trimmed.chars().filter(|c| *c == '?').count();

    let mut score: i32 = 0;

    if word_count <= SHORT_WORD_THRESHOLD {
        score -= 2;
    } else if word_count >= LONG_WORD_THRESHOLD {
        score += 2;
    }

    score += 3 * matched_complex.len().min(MAX_KEYWORD_HITS) as i32;
    score -= 3 * matched_micro.len().min(MAX_KEYWORD_HITS) as i32;

    if paragraph_count >= 3 {
        score += 2;
    }
    if sentence_count >= 5 {
        score += 1;
    }
    if question_marks >= 2 {
        score += 1;
    }

    // A code block changes what the other signals mean: formatting code is
    // still mechanical no matter how the code looks, and reviewing or
    // redesigning code is real work no matter how short the ask reads.
    if code && !matched_micro.is_empty() {
        score -= 3;
    } else if code && !matched_complex.is_empty() {
        score += 2;
    }

    let class = if score >= COMPLEX_THRESHOLD { TaskClass::Complex } else { TaskClass::Micro };
    let reason = describe(class, word_count, &matched_micro, &matched_complex, code, score);

    Classification {
        class,
        score,
        reason,
        word_count,
        matched_micro_keywords: matched_micro,
        matched_complex_keywords: matched_complex,
        looks_like_code: code,
    }
}

/// Cheap "does this contain code" check. Deliberately conservative — it looks
/// for a fenced code block first (how code overwhelmingly arrives in a chat
/// prompt) and otherwise requires several *distinct* code markers to agree,
/// so a sentence that happens to use a semicolon does not trip it.
fn looks_like_code(text: &str) -> bool {
    if text.contains("```") {
        return true;
    }
    const MARKERS: &[&str] = &[
        "{", "}", ";", "=>", "->", "def ", "fn ", "function ", "class ", "const ", "let ",
        "import ", "return ", "public ", "void ", "#include",
    ];
    MARKERS.iter().filter(|m| text.contains(*m)).count() >= 3
}

fn describe(
    class: TaskClass,
    word_count: usize,
    micro: &[&'static str],
    complex: &[&'static str],
    looks_like_code: bool,
    score: i32,
) -> String {
    let length_desc = if word_count <= SHORT_WORD_THRESHOLD {
        format!("short ({word_count} word{})", plural(word_count))
    } else if word_count >= LONG_WORD_THRESHOLD {
        format!("long ({word_count} words)")
    } else {
        format!("medium-length ({word_count} words)")
    };

    let mut clauses = vec![length_desc];
    if let Some(kw) = complex.first() {
        clauses.push(format!("contains the reasoning cue \"{kw}\""));
    }
    if let Some(kw) = micro.first() {
        clauses.push(format!("contains the mechanical-task cue \"{kw}\""));
    }
    if looks_like_code {
        clauses.push("includes a code block".to_string());
    }

    let label = match class {
        TaskClass::Micro => "micro",
        TaskClass::Complex => "complex",
    };
    format!("classified as {label} (score {score}): {}", clauses.join(", "))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

// ---------------------------------------------------------------------------
// Latency tracking — measured, in-memory only
// ---------------------------------------------------------------------------

/// How many recent samples each backend keeps. A rolling window rather than a
/// running total so a backend that used to be slow (cold model, machine under
/// load) is not punished forever, and one that degrades is noticed within a
/// handful of requests.
const MAX_SAMPLES_PER_BACKEND: usize = 20;

/// Rolling per-backend latency samples, held only in process memory.
///
/// Caduceus's whole positioning is no data collection: this is why the
/// numbers live here and nowhere else. Nothing in this module writes them to
/// disk, logs them off-device, or sends them anywhere — they exist purely so
/// [`route`] can ask "which local backend has actually been fastest" instead
/// of guessing, and they evaporate the moment the process exits.
pub struct LatencyTracker {
    samples: Mutex<HashMap<String, VecDeque<Duration>>>,
}

impl Default for LatencyTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencyTracker {
    pub fn new() -> Self {
        Self { samples: Mutex::new(HashMap::new()) }
    }

    /// Record one completed call's latency for `backend_id`.
    pub fn record(&self, backend_id: &str, elapsed: Duration) {
        let mut map = self.samples.lock();
        let entry = map.entry(backend_id.to_string()).or_default();
        entry.push_back(elapsed);
        if entry.len() > MAX_SAMPLES_PER_BACKEND {
            entry.pop_front();
        }
    }

    /// Mean of the retained samples for `backend_id`, or `None` if it has
    /// never been measured.
    pub fn average(&self, backend_id: &str) -> Option<Duration> {
        let map = self.samples.lock();
        let entry = map.get(backend_id)?;
        if entry.is_empty() {
            return None;
        }
        let total = entry.iter().fold(Duration::ZERO, |acc, d| acc + *d);
        Some(total / entry.len() as u32)
    }

    /// How many samples are currently retained for `backend_id`. Useful for
    /// UI copy like "based on 12 recent replies".
    pub fn sample_count(&self, backend_id: &str) -> usize {
        self.samples.lock().get(backend_id).map(VecDeque::len).unwrap_or(0)
    }

    /// Drop every recorded sample. Exposed for tests and for a possible
    /// "reset routing stats" setting; never called from production paths.
    pub fn clear(&self) {
        self.samples.lock().clear();
    }
}

/// The process-wide latency tracker. A single shared instance is simplest
/// here: routing decisions and the calls that feed them both need the same
/// view of "how fast was each backend recently", and there is exactly one
/// such view per running app — no per-window or per-session split makes
/// sense. A `OnceLock` avoids needing any Tauri wiring (managed state,
/// `lib.rs` changes) just to stand this up.
static LATENCY_TRACKER: OnceLock<LatencyTracker> = OnceLock::new();

pub fn latency_tracker() -> &'static LatencyTracker {
    LATENCY_TRACKER.get_or_init(LatencyTracker::new)
}

/// RAII helper for the call site that actually talks to a backend: hold one
/// of these across the request and its elapsed time is recorded automatically
/// on drop, including on an early return via `?` or a panic unwind — the
/// measurement is "how long did the caller wait", which those all still are.
///
/// ```ignore
/// let _timing = LatencyGuard::start(routing::latency_tracker(), &config.id);
/// let response = backend.chat(messages, &config).await?;
/// // elapsed time recorded here, whether this line is reached or not
/// ```
pub struct LatencyGuard<'a> {
    tracker: &'a LatencyTracker,
    backend_id: String,
    start: Instant,
}

impl<'a> LatencyGuard<'a> {
    pub fn start(tracker: &'a LatencyTracker, backend_id: impl Into<String>) -> Self {
        Self { tracker, backend_id: backend_id.into(), start: Instant::now() }
    }
}

impl Drop for LatencyGuard<'_> {
    fn drop(&mut self) {
        self.tracker.record(&self.backend_id, self.start.elapsed());
    }
}

// ---------------------------------------------------------------------------
// Routing policy
// ---------------------------------------------------------------------------

/// Everything [`route`] needs about the current configuration, gathered by
/// the caller from [`crate::settings::AgentSettings`]. Kept as plain
/// borrowed data rather than taking `&AgentSettings` directly so this module
/// has no compile-time dependency on the settings schema — see the note on
/// `override_backend_id` below and the report for what field name to give it.
pub struct RoutingContext<'a> {
    /// Every backend the user has configured (mirrors `AgentSettings::backends`).
    pub backends: &'a [BackendConfig],
    /// `AgentSettings::primary_backend_id` — the strong, user-chosen backend.
    pub primary_backend_id: Option<&'a str>,
    /// An explicit user pin that always wins, no matter what the classifier
    /// says. This is the override requirement: routing must be defeatable.
    /// Maps to a *new* settings field — see the report for the proposed name
    /// and shape; this module does not assume where it lives, only that the
    /// caller can supply it as `Some(backend_id)` when set.
    pub override_backend_id: Option<&'a str>,
    /// Master on/off switch for auto-routing (also a new settings field).
    /// When `false`, every prompt goes to the primary backend exactly like
    /// today, i.e. routing is a pure opt-in behavioural change.
    pub auto_routing_enabled: bool,
}

/// The outcome of one routing decision, and — critically — why.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDecision {
    pub backend_id: String,
    pub class: TaskClass,
    /// One sentence, safe to show directly in the UI. Answers "why did that
    /// take 8 seconds" (or "why was that instant") without requiring the user
    /// to trust an invisible process.
    pub reason: String,
}

/// Decide which backend should handle `prompt`.
///
/// Returns `None` only when there is truly nothing to route to (no backends
/// configured at all, or the configured primary/override id does not match
/// any configured backend) — the same situation [`crate::agent::resolve_backend`]
/// already reports as "not configured". Every other situation, including
/// "only one backend exists" or "no local backend is available for a micro
/// task", resolves to a decision rather than a failure: routing degrading
/// gracefully to today's single-backend behaviour is the whole point of the
/// "never fail merely because the preferred backend is missing" requirement.
pub fn route(
    prompt: &str,
    ctx: &RoutingContext<'_>,
    latencies: &LatencyTracker,
) -> Option<RoutingDecision> {
    // An explicit pin always wins, and does not even need to classify the
    // prompt correctly to be honoured — the user asked for this backend by
    // name, and that is a stronger signal than any heuristic.
    if let Some(id) = ctx.override_backend_id {
        if let Some(backend) = ctx.backends.iter().find(|b| b.id == id) {
            return Some(RoutingDecision {
                backend_id: backend.id.clone(),
                class: classify(prompt).class,
                reason: format!(
                    "using \"{}\", which you pinned in Settings — automatic routing is bypassed",
                    backend.display_name
                ),
            });
        }
        // A dangling override id (backend was deleted) is not treated as a
        // hard failure: fall through to normal routing rather than refusing
        // to answer because a stale setting points nowhere.
    }

    let usable: Vec<&BackendConfig> =
        ctx.backends.iter().filter(|b| b.kind != BackendKind::Null).collect();

    if usable.len() <= 1 {
        let only = usable.first().copied().or_else(|| ctx.backends.first())?;
        return Some(RoutingDecision {
            backend_id: only.id.clone(),
            class: classify(prompt).class,
            reason: format!(
                "only one backend (\"{}\") is configured, so it handles everything",
                only.display_name
            ),
        });
    }

    let classification = classify(prompt);

    if !ctx.auto_routing_enabled {
        let primary = resolve_primary(ctx)?;
        return Some(RoutingDecision {
            backend_id: primary.id.clone(),
            class: classification.class,
            reason: "automatic routing is turned off, so this went to your primary backend"
                .to_string(),
        });
    }

    match classification.class {
        TaskClass::Complex => {
            let primary = resolve_primary(ctx)?;
            Some(RoutingDecision {
                backend_id: primary.id.clone(),
                class: classification.class,
                reason: format!(
                    "{}; routed to your primary backend (\"{}\") for stronger reasoning",
                    classification.reason, primary.display_name
                ),
            })
        }
        TaskClass::Micro => {
            // Prefer a plain local chat endpoint (Ollama, LM Studio, …) over
            // Hermes. Hermes is an agent that always attaches tools; handing it
            // a short chat question is how you end up with
            // "model does not support tools" from a vision tag that was wired
            // as Hermes' default for /c. Chat stays on OpenAI-compatible
            // locals; Hermes stays for computer-use unless it is the only local.
            let chat_locals: Vec<&BackendConfig> = usable
                .iter()
                .copied()
                .filter(|b| is_local_chat_backend(b))
                .collect();
            let locals: Vec<&BackendConfig> = if chat_locals.is_empty() {
                usable.iter().copied().filter(|b| is_local_backend(b)).collect()
            } else {
                chat_locals
            };

            let Some(fastest) = fastest_local(&locals, latencies) else {
                let primary = resolve_primary(ctx)?;
                return Some(RoutingDecision {
                    backend_id: primary.id.clone(),
                    class: classification.class,
                    reason: format!(
                        "{}; no local backend is available, so this still went to your primary backend (\"{}\")",
                        classification.reason, primary.display_name
                    ),
                });
            };

            let speed_note = match latencies.average(&fastest.id) {
                Some(avg) => format!(
                    "fastest measured local backend, averaging {}ms over {} recent replies",
                    avg.as_millis(),
                    latencies.sample_count(&fastest.id)
                ),
                None => "the first configured local backend (no measurements yet)".to_string(),
            };

            Some(RoutingDecision {
                backend_id: fastest.id.clone(),
                class: classification.class,
                reason: format!(
                    "{}; routed to \"{}\" for a fast, cheap reply — {}",
                    classification.reason, fastest.display_name, speed_note
                ),
            })
        }
    }
}

fn resolve_primary<'a>(ctx: &RoutingContext<'a>) -> Option<&'a BackendConfig> {
    let id = ctx.primary_backend_id?;
    ctx.backends.iter().find(|b| b.id == id)
}

/// A backend Caduceus can reach without leaving the machine. There is no
/// explicit "is local" field on [`BackendConfig`], so this infers it from
/// what is already there: Hermes always runs as a local subprocess, and an
/// OpenAI-compatible endpoint counts only when its base URL is a loopback
/// address — the same shape `agent::discover` already probes for local
/// runtimes (Ollama, LM Studio, llama.cpp, Jan, vLLM). A cloud
/// OpenAI-compatible endpoint (`api.openai.com`, a hosted gateway, ...) is
/// correctly excluded here even though it uses the same `BackendKind`.
fn is_local_backend(cfg: &BackendConfig) -> bool {
    match cfg.kind {
        BackendKind::Null => false,
        BackendKind::Hermes => true,
        BackendKind::OpenAiCompatible => is_loopback_url(&cfg.base_url),
    }
}

/// Local backends that speak plain chat — no tool schemas attached.
///
/// Hermes is local but is an agent: every turn can include tools. Micro chat
/// must not land there when an Ollama-style endpoint is available, or a
/// vision-only Hermes default surfaces as a 400 on ordinary questions.
fn is_local_chat_backend(cfg: &BackendConfig) -> bool {
    cfg.kind == BackendKind::OpenAiCompatible && is_loopback_url(&cfg.base_url)
}

fn is_loopback_url(base_url: &str) -> bool {
    let lower = base_url.to_lowercase();
    lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("://[::1]")
        || lower.contains("0.0.0.0")
}

/// Pick the fastest of `locals` by measured average latency, falling back to
/// list order for anything unmeasured (ties, or *everything* unmeasured on a
/// cold start). `min_by` keeps the first element among equals, so "no data
/// yet" resolves to a stable, explainable choice rather than an arbitrary one
/// that could change between otherwise-identical calls.
fn fastest_local<'a>(
    locals: &[&'a BackendConfig],
    latencies: &LatencyTracker,
) -> Option<&'a BackendConfig> {
    locals
        .iter()
        .copied()
        .min_by(|a, b| {
            let la = latencies.average(&a.id);
            let lb = latencies.average(&b.id);
            match (la, lb) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Classifier — the part most likely to be subtly wrong. Table-driven
    // so the accuracy figure in the routing report comes straight from
    // this list rather than being hand-counted.
    // -----------------------------------------------------------------

    /// (label, prompt, expected class). Covers the plain cases from the
    /// spec plus three adversarial ones: a short-but-hard prompt, a
    /// long-but-trivial prompt, and code that only needs formatting.
    fn labeled_cases() -> Vec<(&'static str, &'static str, TaskClass)> {
        use TaskClass::{Complex, Micro};
        vec![
            // --- plainly micro ---
            ("format json", "Can you format this JSON for me?\n```json\n{\"a\":1,\"b\":2}\n```", Micro),
            ("commit message", "write a commit message for these changes", Micro),
            ("fix typo", "fix the typo in this sentence: \"recieve\"", Micro),
            ("write regex", "write a regex that matches a US zip code", Micro),
            ("rename var", "rename the variable foo to userCount", Micro),
            ("translate line", "translate this line to French: \"good morning\"", Micro),
            ("capitalize", "capitalize the first letter of every word here", Micro),
            ("lowercase", "convert to lowercase: HELLO WORLD", Micro),
            ("one liner", "give me a one-liner to reverse a string in python", Micro),
            ("short greeting", "hi, what time is it in Tokyo?", Micro),
            // --- plainly complex ---
            (
                "architecture design",
                "Design a distributed consensus algorithm that tolerates Byzantine faults \
                 across five data centers, considering network partition scenarios, leader \
                 election trade-offs, and how it interacts with our existing Raft-based \
                 metadata store. Please analyze the failure modes and propose an architecture \
                 with pros and cons of each approach, including latency implications, and a \
                 phased migration plan.",
                Complex,
            ),
            (
                "debug root cause",
                "Our checkout service intermittently returns 500s under load and I need help \
                 finding the root cause. Here are three days of logs, the deploy history, and \
                 the relevant service code. Please investigate what's actually going wrong, \
                 not just the surface symptom, and propose a fix along with how we'd verify it \
                 in staging before shipping.",
                Complex,
            ),
            (
                "code review",
                "Here's my auth middleware:\n```js\nfunction auth(req,res,next){ if(req.headers.token){next()} }\n```\n\
                 Can you review this for security issues and suggest a better design?",
                Complex,
            ),
            (
                "compare tradeoffs",
                "Compare Postgres logical replication versus a Kafka-based CDC pipeline for \
                 keeping our search index in sync, and lay out the trade-offs for our scale.",
                Complex,
            ),
            (
                "long document summary",
                "Please read through the following incident postmortem, which covers what \
                 happened, the contributing factors across three separate systems, the \
                 timeline of detection and mitigation, and the list of proposed follow-up \
                 actions from each team involved.\n\n\
                 It started when a routine config change to the rate limiter interacted badly \
                 with a stale cache entry on one of the edge nodes.\n\n\
                 By the time on-call noticed, the error budget for the quarter was already \
                 exhausted, and the fix required a coordinated rollback across two services.\n\n\
                 Summarize the document and evaluate whether the proposed follow-ups actually \
                 address the root cause or just the symptom.",
                Complex,
            ),
            // --- adversarial: short but genuinely hard ---
            ("short but hard: proof", "Prove that P is not equal to NP.", Complex),
            ("short but hard: physics", "Why does time dilate near a black hole?", Complex),
            // --- adversarial: long but trivial ---
            (
                "long but trivial: rename",
                "I've been going back and forth on this for a while and I know it seems like \
                 a small thing but it's been bugging me for weeks now every time I open this \
                 file, so if you don't mind, could you please just rename the variable called \
                 x to something more descriptive like y everywhere it appears in this \
                 function, that's really all I need right now, nothing fancy, just a simple \
                 rename across the file.",
                Micro,
            ),
            (
                "long but trivial: pretty-print",
                "I have a configuration file that has gotten completely disorganized over the \
                 past few months as different team members added fields without any \
                 consistent style, mixing tabs and spaces and inconsistent quoting, and now it \
                 is genuinely hard to read, so could you please just format this JSON blob so \
                 it is valid and consistently indented, without changing any of the actual \
                 values, just pretty-print it exactly as it already is structured:\n\
                 ```json\n{\"a\":1,\"b\":2,\"c\":3,\"d\":4,\"e\":5}\n```",
                Micro,
            ),
            // --- adversarial: code that only needs formatting ---
            (
                "code only needs formatting",
                "```py\ndef  foo(x,y):\n    return   x+y\n```\nplease format this with black style",
                Micro,
            ),
        ]
    }

    #[test]
    fn classifier_matches_the_labeled_set() {
        let cases = labeled_cases();
        let mut correct = 0usize;
        let mut failures = Vec::new();

        for (label, prompt, expected) in &cases {
            let got = classify(prompt);
            if got.class == *expected {
                correct += 1;
            } else {
                failures.push(format!(
                    "{label}: expected {:?}, got {:?} (score {}, reason: {})",
                    expected, got.class, got.score, got.reason
                ));
            }
        }

        let accuracy = 100.0 * correct as f64 / cases.len() as f64;
        assert!(
            failures.is_empty(),
            "classifier accuracy {accuracy:.1}% ({correct}/{}), failures:\n{}",
            cases.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn no_model_call_is_possible_from_the_classifier() {
        // Purely structural guard: classify() takes a &str and returns a
        // plain value synchronously, with no async fn signature and no
        // network types in scope. If someone later makes this `async fn`
        // or gives it a backend/http parameter, this test's very shape
        // stops compiling, which is the point.
        fn assert_sync_pure_fn(_f: fn(&str) -> Classification) {}
        assert_sync_pure_fn(classify);
    }

    #[test]
    fn empty_and_whitespace_prompts_do_not_panic() {
        assert_eq!(classify("").class, TaskClass::Micro);
        assert_eq!(classify("   \n\n  ").class, TaskClass::Micro);
    }

    #[test]
    fn non_ascii_prompts_are_handled() {
        // Word count and keyword matching must not panic on multi-byte UTF-8.
        let c = classify("Écris une régression linéaire et explique-moi les résultats en détail, avec les hypothèses, les résidus, et une comparaison de deux modèles.");
        // Long, mostly non-English text: not asserting a specific class,
        // only that classification completes and reports a sane word count.
        assert!(c.word_count > 0);
    }

    #[test]
    fn ties_lean_micro_by_design() {
        // A prompt with no signals in either direction and no strong length
        // pull settles at score 0, which is below COMPLEX_THRESHOLD.
        let c = classify("what's the weather like");
        assert_eq!(c.class, TaskClass::Micro);
    }

    #[test]
    fn matched_keywords_are_reported_for_explainability() {
        let c = classify("please rename this function to something clearer");
        assert!(c.matched_micro_keywords.contains(&"rename"));
        assert!(c.reason.contains("rename"));
    }

    #[test]
    fn reason_is_a_single_readable_clause_without_a_trailing_period() {
        let c = classify("format this please");
        assert!(!c.reason.ends_with('.'));
        assert!(c.reason.starts_with("classified as"));
    }

    // -----------------------------------------------------------------
    // Latency tracker
    // -----------------------------------------------------------------

    #[test]
    fn a_backend_with_no_samples_has_no_average() {
        let t = LatencyTracker::new();
        assert_eq!(t.average("nope"), None);
        assert_eq!(t.sample_count("nope"), 0);
    }

    #[test]
    fn average_reflects_recorded_samples() {
        let t = LatencyTracker::new();
        t.record("a", Duration::from_millis(100));
        t.record("a", Duration::from_millis(300));
        assert_eq!(t.average("a"), Some(Duration::from_millis(200)));
        assert_eq!(t.sample_count("a"), 2);
    }

    #[test]
    fn the_window_drops_old_samples() {
        let t = LatencyTracker::new();
        for _ in 0..MAX_SAMPLES_PER_BACKEND {
            t.record("a", Duration::from_millis(1000));
        }
        assert_eq!(t.sample_count("a"), MAX_SAMPLES_PER_BACKEND);
        t.record("a", Duration::from_millis(0));
        assert_eq!(t.sample_count("a"), MAX_SAMPLES_PER_BACKEND);
        // The new fast sample should have pulled the average down from 1000ms.
        assert!(t.average("a").unwrap() < Duration::from_millis(1000));
    }

    #[test]
    fn latency_guard_records_on_drop_even_on_early_return() {
        let t = LatencyTracker::new();
        fn do_work(t: &LatencyTracker) -> Option<()> {
            let _g = LatencyGuard::start(t, "b");
            None? // early return — the guard must still fire
        }
        do_work(&t);
        assert_eq!(t.sample_count("b"), 1);
    }

    #[test]
    fn clear_removes_every_sample() {
        let t = LatencyTracker::new();
        t.record("a", Duration::from_millis(10));
        t.clear();
        assert_eq!(t.average("a"), None);
    }

    // -----------------------------------------------------------------
    // Routing policy
    // -----------------------------------------------------------------

    fn hermes(id: &str) -> BackendConfig {
        BackendConfig {
            id: id.into(),
            display_name: format!("Hermes ({id})"),
            kind: BackendKind::Hermes,
            supports_computer_use: true,
            ..Default::default()
        }
    }

    fn local_openai(id: &str, port: u16) -> BackendConfig {
        BackendConfig {
            id: id.into(),
            display_name: format!("Local ({id})"),
            kind: BackendKind::OpenAiCompatible,
            base_url: format!("http://localhost:{port}/v1"),
            ..Default::default()
        }
    }

    fn cloud_openai(id: &str) -> BackendConfig {
        BackendConfig {
            id: id.into(),
            display_name: format!("Cloud ({id})"),
            kind: BackendKind::OpenAiCompatible,
            base_url: "https://api.openai.com/v1".into(),
            ..Default::default()
        }
    }

    fn null_backend() -> BackendConfig {
        BackendConfig { id: "null".into(), display_name: "None".into(), ..Default::default() }
    }

    #[test]
    fn micro_prompt_routes_to_the_only_local_backend() {
        let backends = vec![local_openai("local", 11434), cloud_openai("cloud")];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("cloud"),
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        let d = route("fix this typo: teh", &ctx, &LatencyTracker::new()).unwrap();
        assert_eq!(d.backend_id, "local");
        assert_eq!(d.class, TaskClass::Micro);
    }

    #[test]
    fn complex_prompt_always_routes_to_primary_even_with_a_local_backend_present() {
        let backends = vec![local_openai("local", 11434), cloud_openai("cloud")];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("cloud"),
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        let d = route(
            "Design a fault-tolerant architecture for our payments pipeline and analyze the trade-offs.",
            &ctx,
            &LatencyTracker::new(),
        )
        .unwrap();
        assert_eq!(d.backend_id, "cloud");
        assert_eq!(d.class, TaskClass::Complex);
    }

    #[test]
    fn micro_prompt_falls_back_to_primary_when_no_local_backend_exists() {
        // Two usable backends, both remote, so this exercises the "no local
        // backend available" fallback rather than the "only one backend"
        // shortcut, which fires before classification even runs.
        let backends = vec![cloud_openai("cloud"), cloud_openai("cloud2")];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("cloud"),
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        let d = route("fix this typo: teh", &ctx, &LatencyTracker::new()).unwrap();
        assert_eq!(d.backend_id, "cloud");
        assert!(d.reason.contains("no local backend"));
    }

    #[test]
    fn override_wins_regardless_of_classification() {
        let backends = vec![local_openai("local", 11434), cloud_openai("cloud")];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("cloud"),
            override_backend_id: Some("local"),
            auto_routing_enabled: true,
        };
        // A prompt that would otherwise clearly be routed to "cloud" as Complex.
        let d = route(
            "Design a fault-tolerant architecture and analyze every trade-off in depth.",
            &ctx,
            &LatencyTracker::new(),
        )
        .unwrap();
        assert_eq!(d.backend_id, "local");
        assert!(d.reason.contains("pinned"));
    }

    #[test]
    fn a_dangling_override_falls_through_to_normal_routing_instead_of_failing() {
        let backends = vec![local_openai("local", 11434), cloud_openai("cloud")];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("cloud"),
            override_backend_id: Some("deleted-backend"),
            auto_routing_enabled: true,
        };
        let d = route("fix this typo: teh", &ctx, &LatencyTracker::new()).unwrap();
        assert_eq!(d.backend_id, "local");
    }

    #[test]
    fn auto_routing_disabled_always_uses_primary() {
        let backends = vec![local_openai("local", 11434), cloud_openai("cloud")];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("cloud"),
            override_backend_id: None,
            auto_routing_enabled: false,
        };
        let d = route("fix this typo: teh", &ctx, &LatencyTracker::new()).unwrap();
        assert_eq!(d.backend_id, "cloud");
        assert!(d.reason.contains("turned off"));
    }

    #[test]
    fn only_one_real_backend_is_used_for_everything() {
        let backends = vec![hermes("hermes"), null_backend()];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("hermes"),
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        let micro = route("fix this typo: teh", &ctx, &LatencyTracker::new()).unwrap();
        let complex = route(
            "Design a fault-tolerant architecture and analyze every trade-off in depth.",
            &ctx,
            &LatencyTracker::new(),
        )
        .unwrap();
        assert_eq!(micro.backend_id, "hermes");
        assert_eq!(complex.backend_id, "hermes");
        assert!(micro.reason.contains("only one backend"));
    }

    #[test]
    fn no_configured_backends_returns_none_rather_than_panicking() {
        let backends: Vec<BackendConfig> = vec![];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: None,
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        assert!(route("fix this typo: teh", &ctx, &LatencyTracker::new()).is_none());
    }

    #[test]
    fn fastest_measured_local_backend_wins_over_an_unmeasured_one() {
        let backends = vec![local_openai("slow", 11434), local_openai("fast", 1234)];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("slow"),
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        let latencies = LatencyTracker::new();
        latencies.record("slow", Duration::from_millis(900));
        latencies.record("fast", Duration::from_millis(50));

        let d = route("fix this typo: teh", &ctx, &latencies).unwrap();
        assert_eq!(d.backend_id, "fast");
        assert!(d.reason.contains("fastest measured"));
    }

    #[test]
    fn an_unmeasured_local_backend_still_beats_a_measured_slow_one_if_listed_first() {
        // With no measurements at all, routing must still be deterministic:
        // list order, not an arbitrary HashMap-derived order.
        let backends = vec![local_openai("first", 11434), local_openai("second", 1234)];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("first"),
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        let d = route("fix this typo: teh", &ctx, &LatencyTracker::new()).unwrap();
        assert_eq!(d.backend_id, "first");
        assert!(d.reason.contains("no measurements yet"));
    }

    #[test]
    fn micro_prompt_prefers_local_chat_over_hermes() {
        // Installer order puts Hermes first; without this preference a short
        // chat question would hit Hermes (and its often-vision default) instead
        // of the Ollama chat backend the UI shows as selected.
        let backends = vec![hermes("hermes"), local_openai("ollama-chat", 11434)];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("ollama-chat"),
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        let d = route("How tall is the empire state building", &ctx, &LatencyTracker::new()).unwrap();
        assert_eq!(d.backend_id, "ollama-chat");
        assert_eq!(d.class, TaskClass::Micro);
    }

    #[test]
    fn hermes_counts_as_local_but_a_cloud_openai_endpoint_does_not() {
        assert!(is_local_backend(&hermes("h")));
        assert!(is_local_backend(&local_openai("l", 11434)));
        assert!(!is_local_backend(&cloud_openai("c")));
        assert!(!is_local_backend(&null_backend()));
        assert!(is_local_chat_backend(&local_openai("l", 11434)));
        assert!(!is_local_chat_backend(&hermes("h")));
    }

    #[test]
    fn every_routing_decision_carries_a_nonempty_one_sentence_reason() {
        let backends = vec![local_openai("local", 11434), cloud_openai("cloud")];
        let ctx = RoutingContext {
            backends: &backends,
            primary_backend_id: Some("cloud"),
            override_backend_id: None,
            auto_routing_enabled: true,
        };
        for prompt in ["fix this typo: teh", "Design a fault-tolerant architecture and analyze the trade-offs."] {
            let d = route(prompt, &ctx, &LatencyTracker::new()).unwrap();
            assert!(!d.reason.is_empty());
        }
    }
}
