//! The context-length guard: detecting, and fixing, the trap where a local
//! model silently runs with far less context than tool calling needs.
//!
//! # The trap
//!
//! Most Ollama models default to a small runtime context window (2-4K tokens
//! depending on the server's own default) regardless of how large the model
//! was actually trained for. Below roughly 64K, a tool-calling turn — system
//! prompt, tool schemas, conversation so far — does not fit, and the failure
//! mode is **not an error**: the model just answers as if it has no tools at
//! all, and the person watching has no idea why. The reference implementation
//! this app was inspired by (Hermes Agent) hard-codes exactly this floor
//! (`MINIMUM_CONTEXT_LENGTH`) and refuses to start a tool-calling session
//! below it. [`super::toolloop`] gives Caduceus the same trap to fall into,
//! so it gets the same guard.
//!
//! # Why detection probes the server instead of trusting a table
//!
//! A hard-coded "model name -> context length" table is always one release
//! behind reality: it cannot know a locally re-quantized model, a Modelfile
//! someone hand-edited, or a context length Ollama itself changed the default
//! for between versions. Every number this module reports instead comes from
//! asking the server directly — see [`check`].
//!
//! # What "the real number" means for Ollama, precisely
//!
//! This took live probing against a running Ollama 0.32.1 instance to get
//! right (see the task notes this module was built from), because the
//! obvious source — a model's GGUF training-time context ceiling, exposed at
//! `model_info.*.context_length` via `POST /api/show` — **overclaims**. It is
//! the most a model could ever support, not what it is actually running
//! with. Concretely, on this machine:
//!
//! * `qwen3.5:0.8b` reports a GGUF ceiling of 262,144 tokens.
//! * Loaded with no explicit override, `GET /api/ps` shows it is actually
//!   running with a **4,096**-token window — Ollama's own small runtime
//!   default, unrelated to what the model could support.
//! * A request through `POST /v1/chat/completions` (Caduceus's own call
//!   path — see [`super::openai`]) carrying `options.num_ctx` in *any* wire
//!   shape tried (bare top-level, nested `options`, nested under a literal
//!   `extra_body` key) **does not change this**: the model reloads back to
//!   the 4,096 default regardless. This is not a bug in this probe — Ollama
//!   maintainers rejected adding OpenAI-compatible `num_ctx` support
//!   upstream (ollama/ollama#6137: "this does not follow OpenAI's API
//!   spec"), so no wire shape fixes it on this endpoint.
//! * The **native** `GET /api/chat` endpoint *does* honour `options.num_ctx`
//!   (confirmed: it both changes the active context and validates the
//!   field's type, 500ing on a bad one) — but Caduceus does not call that
//!   endpoint; see [`super::openai`]'s module doc for why one OpenAI-dialect
//!   backend covers every local runtime instead of a native client per one.
//! * The one thing that *does* survive every call path, because it stops
//!   being a per-request override at all, is baking `num_ctx` into the
//!   model's own Modelfile and giving it a new tag — exactly what the
//!   hand-made `gemma4-64k`, `qwen3vl-64k` and `qwen-vl-64k` tags on this
//!   machine already are. [`remediate`] automates making one of those.
//!
//! So: an *active* number from `GET /api/ps` (a model already loaded) or an
//! *explicit* Modelfile override from `POST /api/show`'s `parameters` are
//! both trustworthy. A bare GGUF ceiling with neither is not — reporting it
//! as "sufficient" would silently reproduce the exact trap this module
//! exists to catch (and the hand-made `-64k` tags above are direct evidence
//! that Ollama users hit this even on models with huge GGUF ceilings). See
//! [`check`] and [`ContextCheck`].
//!
//! # Why [`super::openai`] still sends a best-effort `num_ctx` override
//!
//! Given the above, sending `options.num_ctx` on every Ollama request (see
//! `openai::build_payload`) is *not* the load-bearing fix — this module's
//! [`remediate`] is. It is sent anyway, matching the reference
//! implementation, because it is free (confirmed live: an endpoint that does
//! not read the field just ignores it, no 400) and because a different
//! server, proxy, or future Ollama version in this same "OpenAI-compatible"
//! family might honour it where this one does not.
//!
//! # Caching
//!
//! A live probe costs 1-3 local HTTP round trips; [`check`] caches a
//! **sufficient** result keyed `"<model>@<base_url>"` in
//! `caduceus-context-cache.json` (same `tauri-plugin-store` mechanism
//! [`crate::mcp`] uses for its own server list — see that module's
//! `STORE_FILE` for the naming precedent this follows). A sub-floor result is
//! deliberately never written: caching a *bad* number would mean the very
//! next read has to distrust it anyway (see below), so there is nothing
//! gained by persisting it, and something lost — a stale "insufficient" that
//! outlives the user fixing it by hand.
//!
//! Every read re-validates the cached number against the *current*
//! [`MINIMUM_CONTEXT_LENGTH`] rather than trusting anything found on disk —
//! a stale sub-floor value (a hand-edited file, or one written by a version
//! of Caduceus with a different floor) must never be trusted just because it
//! is present. Combined with "never write a sub-floor value", this means the
//! cache can only ever contain numbers that were true, and are still judged
//! sufficient, right now.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

/// Below this, Caduceus's tool-calling loop ([`super::toolloop`]) cannot
/// reliably fit a system prompt plus tool schemas plus real conversation.
/// Matches the reference implementation's own `MINIMUM_CONTEXT_LENGTH`
/// exactly, on purpose — this is not a Caduceus-specific number, it is
/// roughly "how much a tool-calling prompt's fixed overhead costs," which
/// does not change because the code driving it changed.
pub const MINIMUM_CONTEXT_LENGTH: u32 = 64_000;

/// The output-token floor `openai::build_payload` applies to `max_tokens`
/// for an Ollama-shaped backend (Task C item 1: Ollama otherwise falls back
/// to its internal `num_predict=128`, which a thinking-capable model can
/// burn entirely on its hidden reasoning trace before emitting any real
/// content — reproduced live against `qwen3.5:0.8b`: a 100-token budget came
/// back with empty `content`, `finish_reason: "length"`, all 100 tokens
/// spent on `reasoning`), *and* the `num_ctx` value requested on the same
/// request (Task C item 2). One constant serves both call sites rather than
/// two numbers that would only ever be kept in sync by hand: 65,536
/// comfortably clears [`MINIMUM_CONTEXT_LENGTH`] either way, and it is also
/// exactly what [`remediate`] bakes into a new Modelfile variant, so a
/// request behaves the same whether or not its own `num_ctx` override ever
/// takes effect (see the module doc).
pub const OLLAMA_REQUEST_FLOOR: u32 = 65_536;

/// How long a local probe gets before we give up on it. Mirrors
/// [`super::discover::PROBE_TIMEOUT_SECS`] deliberately: same class of
/// request (loopback, to a server that is either up or is not), same
/// justification (generous for localhost, short enough that an unreachable
/// server does not make a "check my model" button feel hung).
const PROBE_TIMEOUT_SECS: u64 = 3;

/// `/api/create` re-tags existing blobs rather than re-quantizing (confirmed
/// live: under a second even for a multi-GB base model), but a much longer
/// ceiling still beats failing outright on a slow disk or an unusually large
/// model.
const CREATE_TIMEOUT_SECS: u64 = 120;

const STORE_FILE: &str = "caduceus-context-cache.json";
const CACHE_KEY: &str = "context_lengths";

// ---------------------------------------------------------------------------
// Task A — detection
// ---------------------------------------------------------------------------

/// The answer to "does this model have enough context for tool calling?" —
/// deliberately three states rather than a bool, because "the probe failed"
/// and "the probe succeeded and the answer is no" call for different UI (and,
/// per the module doc, different trust): an [`Unknown`](Self::Unknown) is not
/// evidence of anything, while an [`Insufficient`](Self::Insufficient) is a
/// confirmed, actionable number.
#[derive(Debug, Clone, PartialEq, Serialize)]
// `rename_all` alone only affects the `status` tag's own casing
// ("sufficient/insufficient/unknown"); `rename_all_fields` is the separate
// attribute that reaches inside each struct variant, which is what actually
// turns `context_length` into `contextLength` on the wire — confirmed by a
// unit test below that inspects the real serialized JSON rather than trusting
// this.
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ContextCheck {
    /// A trustworthy source (see the module doc) reports at least
    /// [`MINIMUM_CONTEXT_LENGTH`].
    Sufficient { context_length: u32 },
    /// A trustworthy source reports fewer than [`MINIMUM_CONTEXT_LENGTH`] —
    /// the real, confirmed number, not a guess.
    Insufficient { context_length: u32 },
    /// No trustworthy number could be obtained: the server did not answer,
    /// was not recognisable as a server this module knows how to introspect,
    /// or (for Ollama specifically) the model is not currently loaded and
    /// has no explicit Modelfile override, so its next active context is
    /// genuinely unknown until it loads. [`explain`] and [`remediate`] both
    /// treat this the same as `Insufficient` — see their docs for why
    /// "we don't know" is not a reason to assume the best.
    Unknown,
}

impl ContextCheck {
    fn from_probe(len: u32) -> Self {
        if len >= MINIMUM_CONTEXT_LENGTH {
            ContextCheck::Sufficient { context_length: len }
        } else {
            ContextCheck::Insufficient { context_length: len }
        }
    }
}

/// Check whether `model` at `base_url` has enough context for tool calling.
///
/// Reads the cache first (see the module doc's caching section); on a miss
/// (including a stale sub-floor hit, which counts as a miss) probes the live
/// server and, if that probe clears the floor, remembers it for next time.
pub async fn check<R: Runtime>(app: &AppHandle<R>, model: &str, base_url: &str) -> ContextCheck {
    if let Some(len) = cached_context_length(app, model, base_url) {
        return ContextCheck::from_probe(len);
    }
    let Some(len) = probe_live(model, base_url).await else {
        return ContextCheck::Unknown;
    };
    // A no-op when `len` is sub-floor -- see `cache_context_length`'s doc.
    cache_context_length(app, model, base_url, len);
    ContextCheck::from_probe(len)
}

/// A plain-English explanation of `check`, ready to show verbatim, or `None`
/// when there is nothing to explain (the model is fine). Kept separate from
/// [`ContextCheck`] itself the same way [`super::types::AgentError`] keeps
/// its data separate from `AgentError::user_message()` — one type carries
/// the facts, a different function decides how to talk about them, so a
/// caller that only wants the facts (e.g. deciding whether to offer the
/// "fix it" button at all) is not forced to also parse prose.
pub fn explain(check: &ContextCheck, model: &str) -> Option<String> {
    match check {
        ContextCheck::Sufficient { .. } => None,
        ContextCheck::Insufficient { context_length } => Some(format!(
            "\u{201c}{model}\u{201d} is currently running with a {}-token context window, but Caduceus's \
             tool-calling loop needs at least {}. Below that, the model does not fail loudly \u{2014} it \
             just answers as if it has no tools at all. Caduceus can create a raised-context copy of this \
             model to fix it.",
            grouped(*context_length),
            grouped(MINIMUM_CONTEXT_LENGTH),
        )),
        ContextCheck::Unknown => Some(format!(
            "Caduceus could not confirm \u{201c}{model}\u{201d}'s active context window. Local models \
             typically default to only a few thousand tokens, which silently breaks tool calling \u{2014} \
             Caduceus needs at least {} \u{2014} rather than failing with an error. Caduceus can create a \
             raised-context copy of this model to be safe.",
            grouped(MINIMUM_CONTEXT_LENGTH),
        )),
    }
}

async fn probe_live(model: &str, base_url: &str) -> Option<u32> {
    match detect_server_kind(base_url).await {
        ServerKind::Ollama => probe_ollama(&server_root(base_url), model).await,
        ServerKind::LmStudio => probe_lmstudio(&lmstudio_root(base_url), model).await,
        // No introspection path this module knows for a vanilla
        // OpenAI-compatible server (vLLM, llama.cpp, a hosted proxy, ...) --
        // guessing here would be exactly the "hard-coded table" mistake the
        // module doc explains this is trying to avoid, so this is left
        // honestly `Unknown` rather than assumed either way.
        ServerKind::Generic => None,
    }
}

// ---------------------------------------------------------------------------
// Server-kind detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerKind {
    Ollama,
    LmStudio,
    Generic,
}

/// Probe two native (non-OpenAI-compatible) surfaces to tell these apart —
/// every server family in [`super::discover`]'s candidate list answers
/// `GET {base}/v1/models` identically, so that alone cannot distinguish them.
async fn detect_server_kind(base_url: &str) -> ServerKind {
    let root = server_root(base_url);
    // Ollama: `GET /api/tags` responds `{"models": [...]}`. LM Studio and a
    // vanilla OpenAI-compatible server either 404 here or serve something
    // else entirely, so finding the `models` array is a strong enough signal
    // on its own -- this is also the exact shape `super::discover::probe`
    // already treats as "found a real API", just at the native path instead
    // of `/v1/models`.
    if let Some(json) = get_json(&format!("{root}/api/tags")).await {
        if json.get("models").and_then(Value::as_array).is_some() {
            return ServerKind::Ollama;
        }
    }
    // LM Studio: `GET /api/v0/models` is LM Studio's own native surface
    // (distinct from the OpenAI-compatible `/v1/models` every server here
    // already exposes), returning `{"data": [...]}` with extra fields like
    // `loaded_context_length`. Untested against a live LM Studio instance —
    // this machine only has Ollama running -- so this is best-effort, per
    // published LM Studio docs, not independently verified the way the
    // Ollama path above is.
    let lm_root = lmstudio_root(base_url);
    if let Some(json) = get_json(&format!("{lm_root}/api/v0/models")).await {
        if json.get("data").and_then(Value::as_array).is_some() {
            return ServerKind::LmStudio;
        }
    }
    ServerKind::Generic
}

// ---------------------------------------------------------------------------
// Ollama probing
// ---------------------------------------------------------------------------

/// See the module doc's "What 'the real number' means for Ollama" section for
/// why this order (active, then explicit override, then nothing) is the
/// entire trustworthy set, and why GGUF training max is deliberately absent
/// from it.
async fn probe_ollama(root: &str, model: &str) -> Option<u32> {
    if let Some(active) = ollama_ps_context(root, model).await {
        return Some(active);
    }
    ollama_show(root, model).await?.explicit_num_ctx
}

/// `GET /api/ps`'s worth of "model -> its live, active context" for the one
/// `model` asked about — the ground truth for anything currently loaded.
async fn ollama_ps_context(root: &str, model: &str) -> Option<u32> {
    let json = get_json(&format!("{root}/api/ps")).await?;
    ps_context_for(&json, model)
}

/// Pure matching logic over an already-fetched `/api/ps` body, split out from
/// [`ollama_ps_context`] so it is testable without a live server.
fn ps_context_for(ps_json: &Value, model: &str) -> Option<u32> {
    let target = normalize_model_tag(model);
    let models = ps_json.get("models")?.as_array()?;
    models.iter().find_map(|entry| {
        let name = entry.get("model").or_else(|| entry.get("name")).and_then(Value::as_str)?;
        if normalize_model_tag(name) != target {
            return None;
        }
        entry.get("context_length").and_then(Value::as_u64).map(|v| v as u32)
    })
}

/// What `POST /api/show` tells us about one model's context.
struct OllamaShowInfo {
    /// An explicit `PARAMETER num_ctx` baked into the model's Modelfile --
    /// the *runtime* value it will load with, and the only prediction of a
    /// not-currently-loaded model's future context this module trusts. See
    /// the module doc.
    explicit_num_ctx: Option<u32>,
    /// The GGUF training-time ceiling. Never treated as "currently
    /// sufficient" (see the module doc) — its only use here is as a sanity
    /// check in [`remediate`]: a model whose own ceiling is already under
    /// the floor cannot be fixed by any `num_ctx`, baked-in or not.
    training_max: Option<u32>,
}

async fn ollama_show(root: &str, model: &str) -> Option<OllamaShowInfo> {
    let json = post_json(&format!("{root}/api/show"), &json!({ "model": model })).await?;
    Some(parse_show(&json))
}

/// Pure parse of an already-fetched `/api/show` body, split out for testing.
fn parse_show(show_json: &Value) -> OllamaShowInfo {
    let explicit_num_ctx =
        show_json.get("parameters").and_then(Value::as_str).and_then(parse_num_ctx);
    let training_max = show_json.get("model_info").and_then(extract_training_max);
    OllamaShowInfo { explicit_num_ctx, training_max }
}

/// Ollama's `parameters` field is Modelfile-style plain text, one
/// `key<whitespace>value` pair per line (e.g. `"top_k 64\nnum_ctx
/// 65536\n..."`) — not JSON, so this is a small line scan rather than a
/// `serde_json` lookup.
fn parse_num_ctx(parameters: &str) -> Option<u32> {
    parameters.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != "num_ctx" {
            return None;
        }
        parts.next()?.parse::<u32>().ok()
    })
}

/// `model_info` keys are architecture-prefixed (`"gemma4.context_length"`,
/// `"qwen2.context_length"`, `"llama.context_length"`, ...) because the key
/// names come straight from GGUF metadata, so this scans for *any* key
/// ending in `.context_length` rather than hard-coding an architecture list
/// that would go stale the moment a new model family shows up.
fn extract_training_max(model_info: &Value) -> Option<u32> {
    let map = model_info.as_object()?;
    map.iter().find_map(|(key, value)| {
        if key.ends_with(".context_length") {
            value.as_u64().map(|v| v as u32)
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// LM Studio probing (best-effort — see the module doc's caveat)
// ---------------------------------------------------------------------------

async fn probe_lmstudio(root: &str, model: &str) -> Option<u32> {
    let json = get_json(&format!("{root}/api/v0/models")).await?;
    lmstudio_context_for(&json, model)
}

/// Pure matching logic over an already-fetched `/api/v0/models` body. Per
/// LM Studio's docs, an entry looks like `{"id": ..., "state": "loaded" |
/// "not-loaded", "max_context_length": N, "loaded_context_length": N}` —
/// `loaded_context_length` is only meaningful (and, per LM Studio, only
/// present) once `state == "loaded"`, which is why an unloaded model is left
/// `None` here rather than falling back to `max_context_length`: that field
/// is the same kind of ceiling-not-guarantee as Ollama's GGUF training max,
/// and the module doc's whole point is that a ceiling is not a number worth
/// trusting on its own.
fn lmstudio_context_for(models_json: &Value, model: &str) -> Option<u32> {
    let target = normalize_model_tag(model);
    let entries = models_json.get("data")?.as_array()?;
    entries.iter().find_map(|entry| {
        let id = entry.get("id").and_then(Value::as_str)?;
        if id != model && normalize_model_tag(id) != target {
            return None;
        }
        if entry.get("state").and_then(Value::as_str) != Some("loaded") {
            return None;
        }
        entry.get("loaded_context_length").and_then(Value::as_u64).map(|v| v as u32)
    })
}

// ---------------------------------------------------------------------------
// discover.rs integration
// ---------------------------------------------------------------------------

/// One `/api/tags` call's worth of GGUF training-ceiling per model, for the
/// "Configure AI" scan (see `discover::probe`) to offer a cheap advisory.
///
/// Deliberately the training *ceiling*, not the active/effective number
/// [`check`] computes: a bulk scan has no specific model in mind yet and
/// cannot afford a `/api/show` round trip per model the way a single
/// backend's precise check can, so the honest thing it can say is "this
/// model could reach the floor with the right override," never "this model
/// currently has enough" — [`check`] is what a specific backend selection
/// should call for that answer.
pub async fn ollama_context_ceilings(base_url: &str) -> Option<Vec<(String, u32)>> {
    let root = server_root(base_url);
    let json = get_json(&format!("{root}/api/tags")).await?;
    Some(ceilings_from_tags(&json))
}

fn ceilings_from_tags(tags_json: &Value) -> Vec<(String, u32)> {
    let Some(models) = tags_json.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|entry| {
            let name = entry.get("model").or_else(|| entry.get("name")).and_then(Value::as_str)?;
            let ctx = entry.pointer("/details/context_length").and_then(Value::as_u64)?;
            Some((name.to_string(), ctx as u32))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Task B — remediation
// ---------------------------------------------------------------------------

/// What [`remediate`] actually did, for display.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemediationOutcome {
    /// The Ollama model tag that now has (or already had) enough context —
    /// either `model` itself, an existing variant that was reused, or a
    /// freshly created one. Whichever it is, this is what a backend should
    /// be pointed at.
    pub variant_model: String,
    pub context_length: u32,
    /// `true` when nothing was created — `model` itself, or an existing
    /// variant, already cleared the floor. `false` only when a brand new
    /// model tag was created via `POST /api/create`.
    pub reused_existing: bool,
    /// Plain-English summary, ready to show verbatim.
    pub message: String,
}

/// Fix a model whose context is under the floor: create (or, per "prefer an
/// existing suitable variant over creating a duplicate," reuse) an Ollama
/// model tag with `num_ctx` raised to [`OLLAMA_REQUEST_FLOOR`], the
/// same mechanism the hand-made `gemma4-64k` / `qwen3vl-64k` / `qwen-vl-64k`
/// tags on this machine already use — see the module doc for why baking it
/// into a Modelfile, rather than relying on a per-request override, is the
/// only fix confirmed to survive every call path.
///
/// Only meaningful for Ollama; every other server kind returns an `Err`
/// explaining that (LM Studio's equivalent is reloading the model with a
/// larger context in its own UI, which this module cannot do on someone's
/// behalf).
pub async fn remediate<R: Runtime>(
    app: &AppHandle<R>,
    model: &str,
    base_url: &str,
) -> Result<RemediationOutcome, String> {
    let root = server_root(base_url);
    if !matches!(detect_server_kind(base_url).await, ServerKind::Ollama) {
        return Err(format!(
            "\u{201c}{base_url}\u{201d} does not look like an Ollama server, so Caduceus cannot create a \
             raised-context variant here. For LM Studio, raise the context in the model's own load \
             settings and reload it; for other local servers, check their context-length launch flag."
        ));
    }

    // Ground truth first: a model already loaded with enough context needs
    // no remediation regardless of what its Modelfile says.
    if let Some(active) = ollama_ps_context(&root, model).await {
        if active >= MINIMUM_CONTEXT_LENGTH {
            cache_context_length(app, model, base_url, active);
            return Ok(RemediationOutcome {
                variant_model: model.to_string(),
                context_length: active,
                reused_existing: true,
                message: format!(
                    "\u{201c}{model}\u{201d} is already running with a {}-token context \u{2014} nothing \
                     to do.",
                    grouped(active)
                ),
            });
        }
    }

    let show = ollama_show(&root, model).await;

    if let Some(explicit) = show.as_ref().and_then(|s| s.explicit_num_ctx) {
        if explicit >= MINIMUM_CONTEXT_LENGTH {
            cache_context_length(app, model, base_url, explicit);
            return Ok(RemediationOutcome {
                variant_model: model.to_string(),
                context_length: explicit,
                reused_existing: true,
                message: format!(
                    "\u{201c}{model}\u{201d} already has an explicit {}-token override in its Modelfile \
                     \u{2014} nothing to do.",
                    grouped(explicit)
                ),
            });
        }
    }

    // A model whose own training ceiling cannot reach the floor is not
    // fixable by any num_ctx, baked-in or not -- creating a variant would
    // just be a second, equally broken model. Say so plainly instead.
    if let Some(max) = show.as_ref().and_then(|s| s.training_max) {
        if max < MINIMUM_CONTEXT_LENGTH {
            return Err(format!(
                "\u{201c}{model}\u{201d}'s own training context is only {} tokens \u{2014} below the {}K \
                 minimum no matter what context is requested. A different, larger-context model is needed \
                 here, not a variant of this one.",
                grouped(max),
                MINIMUM_CONTEXT_LENGTH / 1000,
            ));
        }
    }

    if let Some(existing) = find_existing_variant(&root, model).await {
        cache_context_length(app, &existing.name, base_url, existing.context_length);
        return Ok(RemediationOutcome {
            variant_model: existing.name.clone(),
            context_length: existing.context_length,
            reused_existing: true,
            message: format!(
                "Reusing \u{201c}{}\u{201d}, an existing {}-token variant of \u{201c}{model}\u{201d}, \
                 instead of creating a duplicate.",
                existing.name,
                grouped(existing.context_length),
            ),
        });
    }

    let variant_name = derive_variant_name(model);
    create_variant(&root, &variant_name, model, OLLAMA_REQUEST_FLOOR).await?;

    // Verify rather than assume `/api/create` did exactly what was asked --
    // trusting the request's own intent over the server's confirmed state is
    // exactly the "hard-coded table" mistake this whole module exists to
    // avoid. Falls back to the requested value only if the follow-up show
    // itself fails (the create already succeeded at this point).
    let confirmed = ollama_show(&root, &variant_name)
        .await
        .and_then(|s| s.explicit_num_ctx)
        .unwrap_or(OLLAMA_REQUEST_FLOOR);
    cache_context_length(app, &variant_name, base_url, confirmed);

    Ok(RemediationOutcome {
        variant_model: variant_name.clone(),
        context_length: confirmed,
        reused_existing: false,
        message: format!(
            "Created \u{201c}{variant_name}\u{201d}, a {}-token copy of \u{201c}{model}\u{201d}. Point this \
             backend at \u{201c}{variant_name}\u{201d} to use it.",
            grouped(confirmed)
        ),
    })
}

struct ExistingVariant {
    name: String,
    context_length: u32,
}

/// Look for an already-created variant of `base_model` that already clears
/// the floor, so [`remediate`] can reuse it instead of creating a
/// near-duplicate — exactly what a human would do, and exactly why
/// `gemma4-64k` / `qwen3vl-64k` / `qwen-vl-64k` already exist on this
/// machine as hand-made instances of this same pattern.
async fn find_existing_variant(root: &str, base_model: &str) -> Option<ExistingVariant> {
    let tags = get_json(&format!("{root}/api/tags")).await?;
    for name in variants_of(&tags, base_model) {
        if let Some(active) = ollama_ps_context(root, &name).await {
            if active >= MINIMUM_CONTEXT_LENGTH {
                return Some(ExistingVariant { name, context_length: active });
            }
            continue; // loaded, but too small -- not suitable; keep looking
        }
        if let Some(explicit) = ollama_show(root, &name).await.and_then(|s| s.explicit_num_ctx) {
            if explicit >= MINIMUM_CONTEXT_LENGTH {
                return Some(ExistingVariant { name, context_length: explicit });
            }
        }
    }
    None
}

/// Every model whose Ollama-reported `details.parent_model` is exactly
/// `base_model` — i.e. every existing variant of it — in whatever order
/// `/api/tags` returned them. Pure function over an already-fetched
/// `/api/tags` body so the matching logic is testable without a live server;
/// [`find_existing_variant`] is the async wrapper that confirms one actually
/// clears the floor.
fn variants_of(tags_json: &Value, base_model: &str) -> Vec<String> {
    let target = normalize_model_tag(base_model);
    let Some(models) = tags_json.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|entry| {
            let name = entry.get("model").or_else(|| entry.get("name")).and_then(Value::as_str)?;
            let parent = entry.pointer("/details/parent_model").and_then(Value::as_str).unwrap_or("");
            if parent.is_empty() || normalize_model_tag(parent) != target {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
}

/// A deterministic, collision-safe name for a *newly created* variant —
/// never used to find an *existing* one (that is [`variants_of`], via
/// `parent_model`, which works regardless of naming). Keeps the tag
/// (`"gemma4:12b"` -> `"gemma4-12b-64k"`) rather than dropping it the way the
/// hand-made tags on this machine do (`"gemma4-64k"`), so two differently
/// sized variants of the same family (`gemma4:2b` and `gemma4:12b`) can never
/// collide on the name this function produces.
fn derive_variant_name(base_model: &str) -> String {
    let (name, tag) = base_model.split_once(':').unwrap_or((base_model, "latest"));
    if tag.eq_ignore_ascii_case("latest") {
        format!("{name}-64k")
    } else {
        format!("{name}-{tag}-64k")
    }
}

/// `POST /api/create` with the structured (not raw-Modelfile-text) request
/// shape: `{"model": <new tag>, "from": <base>, "parameters": {"num_ctx":
/// N}}`. Verified live against Ollama 0.32.1: the created tag correctly
/// reports `parent_model` = `base_model` and `num_ctx` = N afterward.
async fn create_variant(root: &str, new_name: &str, base_model: &str, num_ctx: u32) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(CREATE_TIMEOUT_SECS))
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("could not create an HTTP client: {e}"))?;

    let body = json!({
        "model": new_name,
        "from": base_model,
        "parameters": { "num_ctx": num_ctx },
        "stream": false,
    });

    let response = client
        .post(format!("{root}/api/create"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("could not reach Ollama to create \u{201c}{new_name}\u{201d}: {e}"))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Ollama refused to create \u{201c}{new_name}\u{201d} ({status}): {}",
            super::http::extract_error_message(&text)
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Cache (persisted via tauri-plugin-store, same mechanism `crate::mcp` uses)
// ---------------------------------------------------------------------------

/// Read a cached context length, re-validated against the current floor —
/// see the module doc's caching section for why a stale sub-floor entry must
/// never be trusted just because it is on disk. Returns `None` on a genuine
/// miss *or* a stale hit; either way the caller's next move is the same
/// (probe live).
fn cached_context_length<R: Runtime>(app: &AppHandle<R>, model: &str, base_url: &str) -> Option<u32> {
    let map = load_cache(app);
    let hit = *map.get(&cache_map_key(model, base_url))?;
    (hit >= MINIMUM_CONTEXT_LENGTH).then_some(hit)
}

/// Persist a freshly probed length — but only when it clears the floor. See
/// the module doc: a sub-floor number is never worth remembering, since the
/// very next read would have to distrust it anyway.
fn cache_context_length<R: Runtime>(app: &AppHandle<R>, model: &str, base_url: &str, length: u32) {
    if length < MINIMUM_CONTEXT_LENGTH {
        return;
    }
    let mut map = load_cache(app);
    map.insert(cache_map_key(model, base_url), length);
    save_cache(app, &map);
}

/// `"<model>@<base_url>"`, trailing slash stripped so
/// `http://host:11434/v1` and `http://host:11434/v1/` share one entry.
fn cache_map_key(model: &str, base_url: &str) -> String {
    format!("{model}@{}", base_url.trim().trim_end_matches('/'))
}

fn load_cache<R: Runtime>(app: &AppHandle<R>) -> HashMap<String, u32> {
    let Ok(store) = app.store(STORE_FILE) else {
        return HashMap::new();
    };
    store.get(CACHE_KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save_cache<R: Runtime>(app: &AppHandle<R>, map: &HashMap<String, u32>) {
    let Ok(store) = app.store(STORE_FILE) else {
        log::warn!("could not open the context-length cache store");
        return;
    };
    store.set(CACHE_KEY, serde_json::to_value(map).unwrap_or_default());
    if let Err(e) = store.save() {
        log::warn!("could not persist the context-length cache: {e}");
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Strip a trailing `/v1` to get the bare server root Ollama's native
/// endpoints (`/api/tags`, `/api/show`, `/api/ps`, `/api/create`) hang off —
/// `BackendConfig::base_url` stores the *OpenAI-compatible* base (e.g.
/// `http://localhost:11434/v1`), which is one path segment short of where
/// these live.
fn server_root(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed).to_string()
}

/// Like [`server_root`], but for LM Studio's handful of historical API
/// prefixes — mirrors the reference implementation's own
/// `_lmstudio_server_root`, which strips the same three suffixes in the same
/// order for the same reason (an LM Studio base URL has been spelled all
/// three ways across versions).
fn lmstudio_root(base_url: &str) -> String {
    let mut root = base_url.trim().trim_end_matches('/');
    for suffix in ["/api/v1", "/api", "/v1"] {
        if let Some(stripped) = root.strip_suffix(suffix) {
            root = stripped;
            break;
        }
    }
    root.trim_end_matches('/').to_string()
}

/// Ollama (and LM Studio's `id`s, loosely) treat a bare model name as
/// implicitly tagged `:latest` — normalizing both sides this way before
/// comparing means a configured `"gemma4"` correctly matches a server-
/// reported `"gemma4:latest"`.
fn normalize_model_tag(model: &str) -> String {
    if model.contains(':') {
        model.to_string()
    } else {
        format!("{model}:latest")
    }
}

/// `12345` -> `"12,345"` — purely so user-facing numbers in [`explain`] and
/// [`RemediationOutcome::message`] are readable at a glance.
fn grouped(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

// ---------------------------------------------------------------------------
// HTTP plumbing
// ---------------------------------------------------------------------------
//
// Deliberately separate from `super::http`: that module's `client()` builds
// a caller-configurable-timeout client for real chat requests, which can
// legitimately run for minutes. Every probe here is a small local metadata
// call that should fail fast — the same reasoning `discover.rs` gives its
// own dedicated short-timeout client.

async fn get_json(url: &str) -> Option<Value> {
    let client = probe_client()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Value>().await.ok()
}

async fn post_json(url: &str, body: &Value) -> Option<Value> {
    let client = probe_client()?;
    let response = client.post(url).json(body).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Value>().await.ok()
}

fn probe_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // ContextCheck
    // -----------------------------------------------------------------

    #[test]
    fn from_probe_classifies_against_the_floor() {
        assert_eq!(
            ContextCheck::from_probe(MINIMUM_CONTEXT_LENGTH),
            ContextCheck::Sufficient { context_length: MINIMUM_CONTEXT_LENGTH }
        );
        assert_eq!(
            ContextCheck::from_probe(MINIMUM_CONTEXT_LENGTH - 1),
            ContextCheck::Insufficient { context_length: MINIMUM_CONTEXT_LENGTH - 1 }
        );
        assert_eq!(
            ContextCheck::from_probe(4096),
            ContextCheck::Insufficient { context_length: 4096 }
        );
    }

    #[test]
    fn serializes_with_a_status_discriminator_and_the_number() {
        let json = serde_json::to_value(ContextCheck::Insufficient { context_length: 4096 }).unwrap();
        assert_eq!(json["status"], "insufficient");
        assert_eq!(json["contextLength"], 4096);

        let json = serde_json::to_value(ContextCheck::Unknown).unwrap();
        assert_eq!(json["status"], "unknown");
    }

    // -----------------------------------------------------------------
    // explain
    // -----------------------------------------------------------------

    #[test]
    fn a_sufficient_model_has_nothing_to_explain() {
        assert!(explain(&ContextCheck::Sufficient { context_length: 100_000 }, "m").is_none());
    }

    #[test]
    fn insufficient_names_the_model_and_the_real_number() {
        let msg = explain(&ContextCheck::Insufficient { context_length: 4096 }, "qwen3.5:0.8b").unwrap();
        assert!(msg.contains("qwen3.5:0.8b"));
        assert!(msg.contains("4,096"));
        assert!(msg.contains("64,000"));
    }

    #[test]
    fn unknown_still_names_the_model_and_the_requirement() {
        let msg = explain(&ContextCheck::Unknown, "mystery-model").unwrap();
        assert!(msg.contains("mystery-model"));
        assert!(msg.contains("64,000"));
    }

    // -----------------------------------------------------------------
    // parse_num_ctx
    // -----------------------------------------------------------------

    #[test]
    fn finds_num_ctx_among_other_parameters() {
        let params = "top_k                          64\ntop_p                          0.95\n\
                       num_ctx                        65536\ntemperature                    1";
        assert_eq!(parse_num_ctx(params), Some(65536));
    }

    #[test]
    fn absent_num_ctx_is_none_not_a_panic() {
        assert_eq!(parse_num_ctx("top_k 64\ntemperature 1"), None);
        assert_eq!(parse_num_ctx(""), None);
    }

    #[test]
    fn does_not_match_a_parameter_that_merely_contains_num_ctx_as_a_substring() {
        // Guards the exact-token check: a hypothetical "other_num_ctx_thing 5"
        // line must not be mistaken for the real parameter.
        assert_eq!(parse_num_ctx("other_num_ctx_thing 5"), None);
    }

    // -----------------------------------------------------------------
    // extract_training_max
    // -----------------------------------------------------------------

    #[test]
    fn finds_the_architecture_prefixed_context_length_key() {
        let info = json!({
            "gemma4.attention.head_count": 16,
            "gemma4.context_length": 262144,
            "gemma4.embedding_length": 3840,
        });
        assert_eq!(extract_training_max(&info), Some(262144));
    }

    #[test]
    fn works_for_a_different_architecture_prefix() {
        let info = json!({ "qwen2.context_length": 32768 });
        assert_eq!(extract_training_max(&info), Some(32768));
    }

    #[test]
    fn missing_context_length_key_is_none() {
        let info = json!({ "llama.embedding_length": 4096 });
        assert_eq!(extract_training_max(&info), None);
    }

    // -----------------------------------------------------------------
    // parse_show (the real /api/show shape, captured live against
    // gemma4-64k on Ollama 0.32.1)
    // -----------------------------------------------------------------

    #[test]
    fn parses_a_real_captured_show_response_for_a_raised_context_variant() {
        let body = json!({
            "parameters": "top_k                          64\ntop_p                          0.95\n\
                            num_ctx                        65536\ntemperature                    1",
            "model_info": { "gemma4.context_length": 262144 },
            "details": { "parent_model": "gemma4:12b" },
        });
        let info = parse_show(&body);
        assert_eq!(info.explicit_num_ctx, Some(65536));
        assert_eq!(info.training_max, Some(262144));
    }

    #[test]
    fn parses_a_base_model_with_no_override() {
        let body = json!({
            "parameters": "",
            "model_info": { "qwen35.context_length": 262144 },
        });
        let info = parse_show(&body);
        assert_eq!(info.explicit_num_ctx, None);
        assert_eq!(info.training_max, Some(262144));
    }

    // -----------------------------------------------------------------
    // ps_context_for (the real /api/ps shape, captured live)
    // -----------------------------------------------------------------

    #[test]
    fn finds_the_active_context_of_a_loaded_model() {
        let ps = json!({ "models": [
            { "name": "qwen3.5:0.8b", "model": "qwen3.5:0.8b", "context_length": 65536 },
        ]});
        assert_eq!(ps_context_for(&ps, "qwen3.5:0.8b"), Some(65536));
    }

    #[test]
    fn a_bare_name_matches_an_implicitly_latest_tagged_entry() {
        let ps = json!({ "models": [
            { "name": "gemma4:latest", "model": "gemma4:latest", "context_length": 4096 },
        ]});
        assert_eq!(ps_context_for(&ps, "gemma4"), Some(4096));
    }

    #[test]
    fn nothing_loaded_is_none() {
        assert_eq!(ps_context_for(&json!({ "models": [] }), "qwen3.5:0.8b"), None);
    }

    #[test]
    fn a_different_loaded_model_does_not_match() {
        let ps = json!({ "models": [
            { "name": "other:latest", "model": "other:latest", "context_length": 65536 },
        ]});
        assert_eq!(ps_context_for(&ps, "qwen3.5:0.8b"), None);
    }

    // -----------------------------------------------------------------
    // ceilings_from_tags
    // -----------------------------------------------------------------

    #[test]
    fn extracts_every_models_training_ceiling() {
        let tags = json!({ "models": [
            { "name": "a:latest", "model": "a:latest", "details": { "context_length": 8192 } },
            { "name": "b:latest", "model": "b:latest", "details": { "context_length": 262144 } },
        ]});
        let ceilings = ceilings_from_tags(&tags);
        assert_eq!(ceilings, vec![("a:latest".to_string(), 8192), ("b:latest".to_string(), 262144)]);
    }

    #[test]
    fn a_model_with_no_details_is_skipped_not_a_panic() {
        let tags = json!({ "models": [{ "name": "a:latest", "model": "a:latest" }] });
        assert!(ceilings_from_tags(&tags).is_empty());
    }

    // -----------------------------------------------------------------
    // variants_of (the parent_model scan)
    // -----------------------------------------------------------------

    #[test]
    fn finds_every_variant_whose_parent_model_matches() {
        let tags = json!({ "models": [
            { "name": "gemma4:12b", "model": "gemma4:12b", "details": { "parent_model": "" } },
            { "name": "gemma4-64k:latest", "model": "gemma4-64k:latest",
              "details": { "parent_model": "gemma4:12b" } },
            { "name": "unrelated:latest", "model": "unrelated:latest",
              "details": { "parent_model": "some-other:latest" } },
        ]});
        assert_eq!(variants_of(&tags, "gemma4:12b"), vec!["gemma4-64k:latest".to_string()]);
    }

    #[test]
    fn a_base_model_itself_is_never_its_own_variant() {
        // A base model's own parent_model is empty, which must not be treated
        // as matching another empty-parent_model base model.
        let tags = json!({ "models": [
            { "name": "a:latest", "model": "a:latest", "details": { "parent_model": "" } },
        ]});
        assert!(variants_of(&tags, "a:latest").is_empty());
    }

    #[test]
    fn no_variants_is_an_empty_list_not_a_panic() {
        let tags = json!({ "models": [] });
        assert!(variants_of(&tags, "gemma4:12b").is_empty());
    }

    // -----------------------------------------------------------------
    // lmstudio_context_for
    // -----------------------------------------------------------------

    #[test]
    fn reads_the_loaded_context_of_a_loaded_lmstudio_model() {
        let body = json!({ "data": [
            { "id": "google/gemma-4-26b-a4b", "state": "loaded", "max_context_length": 262144,
              "loaded_context_length": 200000 },
        ]});
        assert_eq!(lmstudio_context_for(&body, "google/gemma-4-26b-a4b"), Some(200000));
    }

    #[test]
    fn an_unloaded_lmstudio_model_reports_no_active_number() {
        // Even though `max_context_length` is present, it is a ceiling, not a
        // guarantee -- see the module doc and `lmstudio_context_for`'s own doc.
        let body = json!({ "data": [
            { "id": "some/model", "state": "not-loaded", "max_context_length": 131072 },
        ]});
        assert_eq!(lmstudio_context_for(&body, "some/model"), None);
    }

    // -----------------------------------------------------------------
    // derive_variant_name
    // -----------------------------------------------------------------

    #[test]
    fn tagged_models_keep_a_disambiguating_size_suffix() {
        assert_eq!(derive_variant_name("gemma4:12b"), "gemma4-12b-64k");
        assert_eq!(derive_variant_name("qwen3.5:4b"), "qwen3.5-4b-64k");
    }

    #[test]
    fn an_implicit_latest_tag_is_dropped_rather_than_spelled_out() {
        assert_eq!(derive_variant_name("qwen3-coder:latest"), "qwen3-coder-64k");
        assert_eq!(derive_variant_name("qwen3-coder"), "qwen3-coder-64k");
    }

    #[test]
    fn two_different_sizes_of_the_same_family_cannot_collide() {
        let a = derive_variant_name("gemma4:2b");
        let b = derive_variant_name("gemma4:12b");
        assert_ne!(a, b);
    }

    // -----------------------------------------------------------------
    // normalize_model_tag / server_root / lmstudio_root
    // -----------------------------------------------------------------

    #[test]
    fn bare_names_are_normalized_to_latest() {
        assert_eq!(normalize_model_tag("gemma4"), "gemma4:latest");
        assert_eq!(normalize_model_tag("gemma4:12b"), "gemma4:12b");
    }

    #[test]
    fn server_root_strips_the_openai_compatible_suffix() {
        assert_eq!(server_root("http://localhost:11434/v1"), "http://localhost:11434");
        assert_eq!(server_root("http://localhost:11434/v1/"), "http://localhost:11434");
        // No `/v1` to strip -- already a bare root.
        assert_eq!(server_root("http://localhost:11434"), "http://localhost:11434");
    }

    #[test]
    fn lmstudio_root_strips_any_of_its_historical_suffixes() {
        assert_eq!(lmstudio_root("http://localhost:1234/api/v1"), "http://localhost:1234");
        assert_eq!(lmstudio_root("http://localhost:1234/v1"), "http://localhost:1234");
        assert_eq!(lmstudio_root("http://localhost:1234/api"), "http://localhost:1234");
        assert_eq!(lmstudio_root("http://localhost:1234"), "http://localhost:1234");
    }

    // -----------------------------------------------------------------
    // grouped
    // -----------------------------------------------------------------

    #[test]
    fn groups_digits_in_threes() {
        assert_eq!(grouped(4096), "4,096");
        assert_eq!(grouped(64_000), "64,000");
        assert_eq!(grouped(262_144), "262,144");
        assert_eq!(grouped(65_536), "65,536");
    }

    #[test]
    fn short_numbers_are_left_alone() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(999), "999");
    }

    // -----------------------------------------------------------------
    // cache_map_key
    // -----------------------------------------------------------------

    #[test]
    fn cache_key_is_model_at_base_url() {
        assert_eq!(
            cache_map_key("qwen3.5:4b", "http://localhost:11434/v1"),
            "qwen3.5:4b@http://localhost:11434/v1"
        );
    }

    #[test]
    fn cache_key_ignores_a_trailing_slash_on_the_base_url() {
        assert_eq!(
            cache_map_key("m", "http://localhost:11434/v1/"),
            cache_map_key("m", "http://localhost:11434/v1")
        );
    }

    // -----------------------------------------------------------------
    // Live verification (ignored by default — see `tools::promptopt`'s
    // `against_a_real_local_model` for the same pattern this follows).
    //
    // `#[ignore]`d because the build must not depend on a server being up on
    // somebody's machine, but kept and checked in because everything above
    // this section tests the *parsing* of an already-fetched response, and
    // nothing above it proves this module reads the real, live shapes those
    // parsers assume — which is exactly the thing "probe rather than trust a
    // table" is supposed to guarantee. These were how the module doc's own
    // claims about Ollama 0.32.1's behaviour were established in the first
    // place.
    //
    // Run with:
    //   cargo test --lib agent::context::tests::live -- --ignored --nocapture
    //
    // Needs Ollama running locally, with `gemma4-64k` and its base
    // `gemma4:12b` both present (`ollama list`) — both already exist on the
    // machine this module was built against, as the hand-made evidence the
    // module doc and `remediate` both reference. `create_variant` /
    // `remediate` are deliberately not covered here: unlike everything else
    // in this section, actually creating a model is a real side effect on
    // whoever runs `--ignored`, which is not something a checked-in test
    // should do unprompted.
    // -----------------------------------------------------------------

    const LIVE_OLLAMA: &str = "http://localhost:11434/v1";

    #[tokio::test]
    #[ignore = "needs Ollama running locally"]
    async fn live_detect_server_kind_recognises_ollama() {
        assert_eq!(detect_server_kind(LIVE_OLLAMA).await, ServerKind::Ollama);
    }

    #[tokio::test]
    #[ignore = "needs Ollama running locally"]
    async fn live_ollama_context_ceilings_returns_real_data() {
        let ceilings = ollama_context_ceilings(LIVE_OLLAMA)
            .await
            .expect("a live Ollama server must answer /api/tags");
        assert!(!ceilings.is_empty(), "this machine has models installed");
        for (name, ctx) in &ceilings {
            println!("{name}: {ctx} tokens (GGUF training ceiling)");
            assert!(*ctx > 0, "{name} reported a zero-token ceiling");
        }
    }

    #[tokio::test]
    #[ignore = "needs Ollama running locally with gemma4-64k (see this section's doc)"]
    async fn live_ollama_show_reads_the_baked_in_override_on_a_raised_context_variant() {
        // gemma4-64k's Modelfile override is on disk and readable via
        // /api/show regardless of whether the model is currently loaded --
        // unlike an /api/ps-reported active context, this does not depend on
        // transient load state, which is what makes it a stable thing to
        // assert on in a test that runs whenever someone happens to invoke
        // it.
        let root = server_root(LIVE_OLLAMA);
        let info = ollama_show(&root, "gemma4-64k")
            .await
            .expect("gemma4-64k must exist on this machine (see this section's doc)");
        assert_eq!(info.explicit_num_ctx, Some(OLLAMA_REQUEST_FLOOR));
        assert!(
            info.training_max.unwrap_or(0) >= MINIMUM_CONTEXT_LENGTH,
            "gemma4's own GGUF ceiling is far above the floor"
        );
    }

    #[tokio::test]
    #[ignore = "needs Ollama running locally with gemma4-64k and gemma4:12b"]
    async fn live_find_existing_variant_discovers_the_hand_made_gemma4_64k() {
        // The exact scenario `remediate` exists to shortcut: asked to fix
        // gemma4:12b, it must find and reuse the tag a human already made by
        // hand, via `details.parent_model`, rather than being blind to
        // anything not named by convention.
        let root = server_root(LIVE_OLLAMA);
        let found = find_existing_variant(&root, "gemma4:12b")
            .await
            .expect("gemma4-64k should be discovered as a suitable existing variant");
        assert_eq!(found.name, "gemma4-64k:latest");
        assert_eq!(found.context_length, OLLAMA_REQUEST_FLOOR);
    }

    #[tokio::test]
    #[ignore = "needs Ollama running locally"]
    async fn live_probe_ollama_never_trusts_a_bare_gguf_ceiling() {
        // The module doc's central claim, checked directly: a model with a
        // huge GGUF ceiling and no explicit override must not be reported as
        // sufficient just because it *could* reach the floor. `nomic-embed-
        // text` is never loaded for chat, so this also exercises the "not
        // currently loaded, no override" path specifically rather than
        // whatever happens to already be resident in memory.
        let root = server_root(LIVE_OLLAMA);
        let result = probe_ollama(&root, "nomic-embed-text").await;
        if let Some(active) = result {
            // If something else has it loaded right now, the only trustworthy
            // claim is its *actual* active context -- which had better still
            // be the small one nothing in this repo ever raises it to.
            assert!(active < MINIMUM_CONTEXT_LENGTH);
        }
        // `None` (no active load, no explicit override) is equally a pass:
        // both outcomes are "not confidently sufficient", never `Some(huge)`.
    }
}
