//! The messaging gateway: lets the owner reach their agent from their phone.
//!
//! This is what turns Caduceus from a desktop tool into something always
//! available. A [`GatewayAdapter`] speaks to one messaging platform; today
//! there is exactly one, [`telegram::TelegramAdapter`], reached over Telegram's
//! Bot API. Everything platform-independent — the allow-list, rate limiting,
//! the fail-closed approval gate, and the bridge into
//! [`crate::agent::run_tool_loop`] — lives here in [`process_inbound`], the
//! single place every adapter's inbound traffic must pass through before it
//! ever reaches the agent.
//!
//! ```text
//!  TelegramAdapter::connect()                 GatewayRuntime
//!  ───────────────────────────                ──────────────
//!   getUpdates (long poll) ──┐                 status: Stopped/Connecting/
//!                            │                         Connected/Error
//!                            ▼
//!                     InboundMessage ──channel──▶ dispatch_loop
//!                                                       │
//!                                                       ▼
//!                                                 process_inbound
//!                                            ┌──────────┼──────────┐
//!                                      allow-list   rate limit   agent::run_tool_loop
//!                                       (deny)        (drop)     (fail-closed approval)
//!                                                                       │
//!                                                            adapter.send(reply)
//! ```
//!
//! # The adapter trait, and why it is this small
//!
//! Modelled on the reference implementation this feature is inspired by
//! (Hermes Agent's `gateway/platforms/base.py`, MIT licensed): a platform
//! adapter has exactly four things about it that cannot have a sensible
//! generic default — connecting, disconnecting, sending, and describing a
//! chat. Everything else (allow-listing, rate limiting, how a reply is
//! produced) is policy that must behave identically no matter which platform
//! delivered the message, so it lives in this file once rather than in every
//! adapter. A second adapter (Signal, WhatsApp, whatever) implements
//! [`GatewayAdapter`] and gets all of that for free — it never has its own
//! copy of the allow-list check to accidentally get wrong or skip.
//!
//! # Security model
//!
//! This module accepts input from the public internet — anyone who has, or
//! guesses, the bot's username can send it a message — and hands surviving
//! input to an agent that can call tools and drive the desktop. That makes
//! the four rules below load-bearing, not decorative:
//!
//! 1. **The allow-list is mandatory and deny-by-default.**
//!    [`telegram::is_allowed`] is consulted on *every* inbound message inside
//!    [`process_inbound`] — the one function every adapter's dispatch path
//!    must route through — and it is read fresh from settings on every call
//!    rather than cached at startup, so an edit takes effect on the very next
//!    message. An empty allow-list (the default) means nobody is allowed,
//!    never everybody, and [`start_internal`] refuses to even start polling
//!    until at least one id is configured — a bot that is reachable but can
//!    answer no one is a confusing, useless state, not a safe one worth
//!    defaulting to. A message from a sender who fails the check is dropped
//!    silently, never answered — see point 3.
//!
//! 2. **The bot token is a secret, and it is the only one this feature has.**
//!    It lives in the OS keychain via [`crate::settings::secrets`]
//!    (`set_telegram_bot_token` / `get_telegram_bot_token_opt` /
//!    `has_telegram_bot_token` / `delete_telegram_bot_token`, added there
//!    following the exact shape of the existing STT/TTS keys), never in the
//!    settings JSON file — see that module's header and rule 3 at the top of
//!    `settings::model`. [`GatewayStatusInfo::has_bot_token`] is the only
//!    thing settings ever expose about it: a boolean, computed live from the
//!    keychain, the same pattern `BackendConfig::has_api_key` already
//!    established.
//!
//! 3. **Message content is untrusted data, never instructions.** An inbound
//!    Telegram message becomes exactly one thing: the user turn of a fresh
//!    [`agent::run_tool_loop`] conversation (see [`process_inbound`]). There
//!    is no in-chat admin command of any kind — no way to type something that
//!    edits the allow-list, flips a setting, or changes how approval works —
//!    so nothing arriving over the wire can ever escalate its own privileges
//!    or reach a path this module doesn't already gate. A disallowed sender
//!    or a flood over budget is dropped without a reply (point 1 and the rate
//!    limiter below): answering would spend a reply — itself a resource — on
//!    input this module has already decided not to trust, and for the flood
//!    case specifically, an automated "slow down" notice is exactly the kind
//!    of reply a flood could keep triggering.
//!
//! 4. **A remote message can never grant its own approval.** Caduceus's tool
//!    loop already gates any tool call behind [`agent::ApprovalGate`] when
//!    `confirm_before_first_action` is on — see `agent::toolloop`'s module
//!    doc. A desktop session asks the person sitting there; a gateway session
//!    has no such person to ask, and no reliable way to tell whether the
//!    owner happens to be watching. [`DenyApproval`] is what "fail closed"
//!    means concretely: every gateway-triggered approval request is denied,
//!    unconditionally, the instant it is asked — never left pending, never
//!    silently approved. This does **not** widen what a gateway message can
//!    do relative to a desktop `/` session with the same setting: when the
//!    owner has turned confirmation off entirely, a gateway session gets the
//!    same auto-approve a desktop session would, because the gateway is a new
//!    *entry point* into the agent, not a way around a trust decision the
//!    owner already made on their own machine. If a future UI wants to let
//!    the owner approve a gateway action from their phone or desktop in real
//!    time, that is new plumbing — a way to actually reach the owner — layered
//!    on top of [`agent::ApprovalAsker`]; nothing here should quietly grow a
//!    presence heuristic instead.
//!
//! On top of the four rules above, two more properties bound the blast radius
//! of a flood rather than a single bad message: [`RateLimiter`] caps how many
//! new agent sessions inbound traffic may start per minute, and every
//! adapter's inbound channel (see [`start_internal`]) is bounded and drained
//! by exactly one [`dispatch_loop`], so messages are handled one at a time —
//! there is no way for a burst to spawn concurrent agent sessions.
//!
//! # Lifecycle
//!
//! [`GatewayStatus`] is runtime-only — it always starts at `Stopped` on a
//! fresh process and is never written to disk, the same split MCP servers
//! already use between [`crate::mcp::ServerStatus`] (live, in-memory) and a
//! server's persisted `enabled` flag. What *is* persisted is intent:
//! [`crate::settings::TelegramGatewaySettings::enabled`], flipped by
//! [`gateway_start`]/[`gateway_stop`] and read once at launch by
//! [`autostart_if_enabled`] (wired into `lib.rs::setup`) so a restart resumes
//! whatever the owner last asked for, and shows `Error` with a reason rather
//! than silently doing nothing if it can't.
//!
//! # What this module deliberately does not do
//!
//! It does not give inbound messages any memory of earlier ones — each
//! message starts a fresh single-turn [`agent::run_tool_loop`] conversation,
//! not a thread. It does not send anything unprompted — [`GatewayAdapter::send`]
//! exists and a future scheduler-driven "daily briefing" feature is exactly
//! what the reference implementation this is modelled on uses it for, but
//! deciding *when* and *where* to push a proactive message is a separate
//! design question this pass leaves alone. Both are natural follow-ups, not
//! gaps in what was asked for here.

pub mod telegram;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::mpsc;

use crate::agent::{self, AgentLoopContext, ApprovalAsker, ApprovalGate, Message, StepSink};
use crate::settings::{self, secrets, SettingsManager};

type Res<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("no Telegram bot token is configured yet -- add one before starting")]
    NotConfigured,
    #[error("add at least one allowed Telegram user id before starting")]
    NoAllowedUsers,
    #[error("could not reach Telegram: {0}")]
    Transport(String),
    #[error("Telegram rejected the request ({code}): {message}")]
    Api { code: i64, message: String },
    #[error("could not understand Telegram's response: {0}")]
    Protocol(String),
    #[error("{0}")]
    Other(String),
}

pub type GatewayResult<T> = Result<T, GatewayError>;

// ---------------------------------------------------------------------------
// The adapter trait
// ---------------------------------------------------------------------------

/// A messaging platform Caduceus can be reached through. See the module doc
/// for why these four methods are the entire trait — every one of them is
/// something a new platform genuinely has to implement its own way, and
/// nothing about allow-listing, rate limiting or talking to the agent
/// belongs on this trait, because it must not vary by platform.
#[async_trait]
pub trait GatewayAdapter: Send + Sync {
    /// Authenticate and start receiving messages. An implementation that
    /// polls or holds a persistent connection should spawn that work here —
    /// after confirming it is actually live, e.g. one successful identity
    /// check — and return, rather than blocking for the adapter's whole
    /// lifetime.
    async fn connect(&self) -> GatewayResult<()>;

    /// Stop receiving messages and release whatever `connect` started.
    /// Infallible: the gateway is stopping either way, and there is nobody
    /// left to hand an error to once it has.
    async fn disconnect(&self);

    /// Send a text message to a chat. Implementations own their platform's
    /// length limits (see [`telegram::chunk_message`] for how the Telegram
    /// adapter splits a long reply across multiple messages).
    async fn send(&self, chat_id: &str, text: &str) -> GatewayResult<()>;

    /// Look up a chat's display name and kind — used to let the owner
    /// confirm an allow-listed id really resolves to the person they think
    /// it does, rather than trusting a bare number.
    async fn get_chat_info(&self, chat_id: &str) -> GatewayResult<ChatInfo>;
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatInfo {
    pub chat_id: String,
    pub name: String,
    /// Platform-native chat kind, e.g. Telegram's `"private"` / `"group"` /
    /// `"supergroup"` / `"channel"`, passed through rather than normalized —
    /// there is only one platform to normalize against so far.
    pub kind: String,
}

/// One inbound message, normalized from whatever wire shape the source
/// adapter speaks — the only shape [`process_inbound`] ever sees, which is
/// what makes the security rules in the module doc apply uniformly
/// regardless of which platform produced it.
#[derive(Debug, Clone)]
pub(crate) struct InboundMessage {
    /// Platform-native sender id, as a string (Telegram's is numeric; kept
    /// as text here so this type does not have to change shape for a future
    /// platform whose ids are not). Allow-list comparison is still done in
    /// each platform's own module against its own typed settings — see
    /// [`telegram::is_allowed`] — because "an allowed id" is inherently
    /// platform-shaped (a Telegram integer, a Signal phone number, ...).
    pub(crate) sender_id: String,
    pub(crate) chat_id: String,
    pub(crate) text: String,
}

/// Callback a background connection loop uses to report that it has died for
/// good (as opposed to a transient error it will retry through on its own —
/// see `telegram::poll_loop`'s backoff handling). Shaped exactly like
/// [`agent::backend::StepSink`] for the same reason: a plain `Fn` closure
/// rather than a channel keeps the loop itself simple, and the receiving end
/// decides what "died" should mean for [`GatewayStatus`].
pub(crate) type StatusSink = Arc<dyn Fn(String) + Send + Sync>;

// ---------------------------------------------------------------------------
// Lifecycle state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum GatewayStatus {
    #[default]
    Stopped,
    Connecting,
    Connected,
    /// Never retried automatically once reached — see `telegram::is_fatal`
    /// for exactly which failures land here versus being backed off and
    /// retried in place while `Connected` stays true.
    Error {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatusInfo {
    pub status: GatewayStatus,
    /// Fixed to `"telegram"` today. Not read from the adapter trait object
    /// (which has no such method — see the module doc on why the trait stays
    /// to exactly four methods); this is simply the one platform that exists.
    pub platform: &'static str,
    pub has_bot_token: bool,
    pub allowed_user_count: usize,
}

struct Inner {
    status: GatewayStatus,
    adapter: Option<Arc<dyn GatewayAdapter>>,
    /// Bumped every time the adapter changes (see `take_adapter`), so a
    /// [`StatusSink`] callback captured by an old session can recognise
    /// itself as stale and skip mutating a status that belongs to whatever
    /// session is current now. See `report_fatal`.
    generation: u64,
}

impl Default for Inner {
    fn default() -> Self {
        Self { status: GatewayStatus::default(), adapter: None, generation: 0 }
    }
}

/// The gateway's live state: current status, and the adapter instance (if
/// any) currently connected. Cheap to clone (an `Arc` underneath, the same
/// shape as [`SettingsManager`]) so a background task started by
/// [`start_internal`] can hold its own handle back to it without needing an
/// `AppHandle` for that purpose — see `report_fatal`'s caller in
/// `start_internal` for why that matters.
#[derive(Clone)]
pub struct GatewayRuntime {
    inner: Arc<RwLock<Inner>>,
}

impl GatewayRuntime {
    pub fn new() -> Self {
        Self { inner: Arc::new(RwLock::new(Inner::default())) }
    }

    pub fn status(&self) -> GatewayStatus {
        self.inner.read().status.clone()
    }

    fn set_status(&self, status: GatewayStatus) {
        self.inner.write().status = status;
    }

    fn set_adapter(&self, adapter: Arc<dyn GatewayAdapter>) {
        self.inner.write().adapter = Some(adapter);
    }

    /// Remove and return whatever adapter is currently registered, bumping
    /// the generation counter. Called at the start of every `stop`/`start`
    /// cycle (see `stop_internal`) whether or not anything was actually
    /// running, so the counter also advances on a no-op stop — cheap, and it
    /// means a [`StatusSink`] from *any* previous attempt is invalidated by
    /// the time a new one begins, not just when there happened to be a live
    /// adapter to replace.
    fn take_adapter(&self) -> Option<Arc<dyn GatewayAdapter>> {
        let mut inner = self.inner.write();
        inner.generation += 1;
        inner.adapter.take()
    }

    fn current_generation(&self) -> u64 {
        self.inner.read().generation
    }

    /// Move to `Error` and drop the adapter, but only if `generation` still
    /// matches the live one. Without this check, a fatal report from an old
    /// polling loop that raced a manual Stop (or a Stop-then-Start) could
    /// land after the newer session has already moved on, overwriting its
    /// `Connected`/`Connecting` status with a stale `Error` that describes a
    /// session which no longer exists.
    fn report_fatal(&self, generation: u64, reason: String) {
        let mut inner = self.inner.write();
        if inner.generation == generation {
            inner.status = GatewayStatus::Error { reason };
            inner.adapter = None;
        }
    }
}

impl Default for GatewayRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn status_info(runtime: &GatewayRuntime, settings: &SettingsManager) -> GatewayStatusInfo {
    let cfg = settings.get().gateway.telegram;
    GatewayStatusInfo {
        status: runtime.status(),
        platform: "telegram",
        has_bot_token: secrets::has_telegram_bot_token(),
        allowed_user_count: cfg.allowed_user_ids.len(),
    }
}

/// How many inbound messages may be buffered between the platform's own
/// receive loop and [`dispatch_loop`] before the platform loop has to wait.
/// Combined with `dispatch_loop` draining strictly one at a time, this is a
/// second, structural layer under [`RateLimiter`]: even if the limiter's
/// window budget were misconfigured, a flood cannot make this module hold
/// more than a small, fixed amount of unprocessed work in memory.
const INBOUND_CHANNEL_CAPACITY: usize = 16;

/// Tear down whatever is currently running (idempotent — a no-op if nothing
/// is), then, if configuration allows it, bring up a fresh
/// [`telegram::TelegramAdapter`] and start dispatching its messages. Always
/// stops-then-starts rather than reusing a live adapter, the same
/// "disconnect any previous instance under this name first" discipline
/// [`crate::mcp::connect_config`] uses for MCP servers — it is what keeps
/// this idempotent and guarantees at most one poll loop and one dispatcher
/// exist at a time.
///
/// Returns the resulting status either way; a configuration or connection
/// failure comes back as `GatewayStatus::Error` inside a normal `Ok`-shaped
/// response rather than an `Err`, matching how `mcp_add_server` reports a
/// failed connection — the caller (a command, or `autostart_if_enabled`) has
/// something to show either way.
async fn start_internal<R: Runtime>(
    app: &AppHandle<R>,
    runtime: &GatewayRuntime,
    settings: &SettingsManager,
) -> GatewayStatusInfo {
    stop_internal(runtime).await;

    let cfg = settings.get().gateway.telegram;
    if cfg.allowed_user_ids.is_empty() {
        runtime.set_status(GatewayStatus::Error { reason: GatewayError::NoAllowedUsers.to_string() });
        return status_info(runtime, settings);
    }
    let Some(token) = secrets::get_telegram_bot_token_opt() else {
        runtime.set_status(GatewayStatus::Error { reason: GatewayError::NotConfigured.to_string() });
        return status_info(runtime, settings);
    };

    runtime.set_status(GatewayStatus::Connecting);

    // Captured now, after `stop_internal`'s bump above, so it names exactly
    // the session being started here -- see `report_fatal`.
    let generation = runtime.current_generation();
    let runtime_for_fatal = runtime.clone();
    let on_fatal: StatusSink = Arc::new(move |reason: String| {
        runtime_for_fatal.report_fatal(generation, reason);
    });

    let (tx, rx) = mpsc::channel(INBOUND_CHANNEL_CAPACITY);
    let adapter = match telegram::TelegramAdapter::new(token, tx, on_fatal) {
        Ok(a) => Arc::new(a) as Arc<dyn GatewayAdapter>,
        Err(e) => {
            runtime.set_status(GatewayStatus::Error { reason: e.to_string() });
            return status_info(runtime, settings);
        }
    };

    if let Err(e) = adapter.connect().await {
        runtime.set_status(GatewayStatus::Error { reason: e.to_string() });
        return status_info(runtime, settings);
    }

    runtime.set_adapter(adapter.clone());
    runtime.set_status(GatewayStatus::Connected);

    // The dispatcher is the one place `AppHandle<R>` and the agent loop meet
    // the adapter; it is intentionally generic over `R` and spawned fresh
    // here rather than stored anywhere, the same reason `agent::start_tool_session`
    // captures `app.clone()` into its own spawned task instead of putting an
    // `AppHandle` on a long-lived, non-generic runtime struct like
    // `GatewayRuntime` -- see that function's module for the pattern this
    // follows.
    let app_for_dispatch = app.clone();
    let settings_for_dispatch = settings.clone();
    tauri::async_runtime::spawn(dispatch_loop(app_for_dispatch, settings_for_dispatch, adapter, rx));

    status_info(runtime, settings)
}

async fn stop_internal(runtime: &GatewayRuntime) {
    if let Some(adapter) = runtime.take_adapter() {
        adapter.disconnect().await;
    }
    runtime.set_status(GatewayStatus::Stopped);
}

/// Resume the gateway automatically if it was running before the last
/// restart. Called once from `lib.rs::setup`, fire-and-forget, the same way
/// `mcp::connect_enabled_servers` is documented to run — see that function's
/// doc. A failure here is not fatal to app startup; it just leaves
/// `GatewayStatus::Error` for the owner to find and fix in Settings.
pub async fn autostart_if_enabled<R: Runtime>(app: &AppHandle<R>) {
    let Some(settings_state) = app.try_state::<SettingsManager>() else { return };
    let settings = settings_state.inner().clone();
    if !settings.get().gateway.telegram.enabled {
        return;
    }
    let Some(runtime_state) = app.try_state::<GatewayRuntime>() else { return };
    let runtime = runtime_state.inner().clone();

    log::info!("gateway: resuming Telegram automatically (it was running before the last restart)");
    let info = start_internal(app, &runtime, &settings).await;
    if let GatewayStatus::Error { reason } = info.status {
        log::warn!("gateway: could not resume Telegram automatically: {reason}");
    }
}

// ---------------------------------------------------------------------------
// Inbound dispatch -- the shared policy layer every adapter's traffic
// passes through. See the module doc's security section.
// ---------------------------------------------------------------------------

/// Drain one adapter's inbound channel, one message at a time, for as long as
/// the channel stays open. The channel closes when the adapter's own receive
/// loop exits (`disconnect`, or a fatal error — see `telegram::poll_loop`),
/// which is what ends this loop too; there is no separate stop signal to
/// plumb through here.
async fn dispatch_loop<R: Runtime>(
    app: AppHandle<R>,
    settings: SettingsManager,
    adapter: Arc<dyn GatewayAdapter>,
    mut rx: mpsc::Receiver<InboundMessage>,
) {
    // One limiter per connection, not per runtime: a fresh Start gets a
    // fresh budget rather than inheriting whatever a previous, unrelated
    // session had already spent.
    let limiter = RateLimiter::default();
    while let Some(msg) = rx.recv().await {
        process_inbound(&app, &settings, &adapter, &limiter, msg).await;
    }
    log::info!("gateway: dispatcher stopped (the connection closed)");
}

/// Turn one already-received message into (at most) one reply. This is the
/// function the module doc's security rules describe — every rule is
/// enforced somewhere in this body, in order, before anything reaches the
/// agent.
async fn process_inbound<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
    adapter: &Arc<dyn GatewayAdapter>,
    limiter: &RateLimiter,
    msg: InboundMessage,
) {
    let snapshot = settings.get();

    // Rule 1 -- mandatory, deny-by-default, re-checked here on every message
    // regardless of anything the adapter itself may already have filtered.
    if !telegram::is_allowed(&snapshot.gateway.telegram.allowed_user_ids, &msg.sender_id) {
        log::warn!("gateway: dropped a message from a sender not on the allow-list (id {})", msg.sender_id);
        return;
    }

    // Flood control -- see the module doc. Dropped silently, same reasoning
    // as an unauthorized sender: a reply is a resource, and a rate-limit
    // notice could itself become part of the flood it exists to stop.
    if !limiter.check() {
        log::warn!("gateway: dropped a message -- inbound rate limit exceeded");
        return;
    }

    let config = match agent::resolve_backend(&snapshot, agent::BackendRole::Primary) {
        Ok(c) => c,
        Err(e) => {
            let _ = adapter
                .send(&msg.chat_id, &format!("Caduceus isn't set up to answer yet: {}", e.user_message()))
                .await;
            return;
        }
    };

    // Rule 4 -- fail closed. Mirrors `confirm_before_first_action` exactly
    // as a desktop session would; see `DenyApproval` and the module doc.
    let approval = if snapshot.agents.confirm_before_first_action {
        ApprovalGate::Ask(Arc::new(DenyApproval))
    } else {
        ApprovalGate::AutoApprove
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    let emit_app = app.clone();
    let on_step: StepSink = Arc::new(move |step: agent::AgentStep| {
        // The same event a desktop session emits, so a future UI can show
        // gateway activity in the same feed without any changes on its side.
        if let Err(e) = emit_app.emit(agent::AGENT_STEP_EVENT, &step) {
            log::warn!("gateway: could not emit an agent step: {e}");
        }
    });

    let ctx = AgentLoopContext {
        session_id: session_id.clone(),
        on_step,
        cancel: agent::CancelToken::default(),
        approval,
    };

    // Rule 3 -- the message text becomes the user turn and nothing else; see
    // the module doc. A fresh conversation every time, not a thread: see
    // "What this module deliberately does not do".
    log::info!("gateway: running an agent session for an allow-listed message (session {session_id})");
    let outcome = agent::run_tool_loop(app, &config, vec![Message::user(msg.text.clone())], ctx).await;

    let reply = match outcome {
        Ok(outcome) => reply_text_for(&outcome),
        Err(e) => {
            log::error!("gateway: session {session_id} failed: {e}");
            format!("Something went wrong answering that: {}", e.user_message())
        }
    };

    if let Err(e) = adapter.send(&msg.chat_id, &reply).await {
        log::error!("gateway: could not send the reply back: {e}");
    }
}

/// Turn a finished tool-loop session into the text actually sent back.
/// `final_message` is empty for every [`agent::StopReason`] except
/// `Completed` (and even then, if the model produced no closing text) — see
/// `toolloop::finish` — so every other reason gets an explanation here
/// instead of a blank message reaching the owner's phone.
fn reply_text_for(outcome: &agent::AgentOutcome) -> String {
    if !outcome.final_message.trim().is_empty() {
        return outcome.final_message.clone();
    }
    match outcome.stop_reason {
        agent::StopReason::Completed => "Done.".to_string(),
        agent::StopReason::MaxSteps => "I stopped after too many tool calls without reaching an \
             answer. Try breaking the request into smaller steps."
            .to_string(),
        agent::StopReason::UserStopped => "Stopped before finishing.".to_string(),
        agent::StopReason::Declined => "That would have required your approval, but nobody was \
             available on the desktop to grant it, so I stopped before doing anything. Open \
             Caduceus and turn off \u{201c}confirm before first action\u{201d} if remote messages \
             should be able to act without approval."
            .to_string(),
        agent::StopReason::Error => "Something went wrong before I could finish.".to_string(),
    }
}

/// The fail-closed [`ApprovalAsker`] every gateway-triggered session is wired
/// through when `confirm_before_first_action` is on. See the module doc,
/// rule 4.
struct DenyApproval;

#[async_trait]
impl ApprovalAsker for DenyApproval {
    async fn ask(&self, session_id: &str, summary: &str) -> bool {
        log::warn!(
            "gateway: session {session_id} needed approval to {summary:?} but was triggered \
             remotely with nobody at the desktop to ask -- denying (fail closed)"
        );
        false
    }
}

/// Bounds how many new agent sessions inbound traffic may start per minute.
/// Global rather than per-sender: the allow-list (rule 1) is already the
/// identity boundary, and Caduceus is a single-owner desktop app rather than
/// a multi-tenant service, so there is exactly one budget worth tracking.
///
/// Backed by [`Instant`] rather than wall-clock time so a system clock change
/// cannot confuse it, and `check_at` takes "now" explicitly (rather than
/// calling `Instant::now()` itself) so tests can simulate the window elapsing
/// without a real sleep — see the tests below.
pub(crate) struct RateLimiter {
    max: usize,
    window: Duration,
    hits: Mutex<VecDeque<Instant>>,
}

/// Generous enough that a real conversation (the owner going back and forth
/// with follow-ups) never trips it, tight enough that a flood cannot start
/// more than a handful of expensive tool-loop sessions before this kicks in.
pub(crate) const MAX_MESSAGES_PER_WINDOW: usize = 10;
pub(crate) const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(MAX_MESSAGES_PER_WINDOW, RATE_LIMIT_WINDOW)
    }
}

impl RateLimiter {
    pub(crate) fn new(max: usize, window: Duration) -> Self {
        Self { max, window, hits: Mutex::new(VecDeque::new()) }
    }

    /// Records an attempt "now" and reports whether it is within budget.
    pub(crate) fn check(&self) -> bool {
        self.check_at(Instant::now())
    }

    fn check_at(&self, now: Instant) -> bool {
        let mut hits = self.hits.lock();
        while let Some(&front) = hits.front() {
            if now.saturating_duration_since(front) > self.window {
                hits.pop_front();
            } else {
                break;
            }
        }
        if hits.len() >= self.max {
            false
        } else {
            hits.push_back(now);
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------
//
// Not registered via `commands.rs` -- following `mcp.rs`'s precedent, a
// module with its own well-scoped feature area owns its own `#[tauri::command]`s,
// registered directly by path in `lib.rs`'s `generate_handler!`.

/// Live status plus the non-secret facts about configuration the UI needs —
/// see `GatewayStatusInfo`.
#[tauri::command]
pub fn gateway_status(
    runtime: tauri::State<'_, GatewayRuntime>,
    settings: tauri::State<'_, SettingsManager>,
) -> GatewayStatusInfo {
    status_info(runtime.inner(), settings.inner())
}

/// Persist "should be running", then attempt to connect immediately —
/// submitting Start *is* the explicit consent to begin polling a public API
/// with real credentials, the same reasoning `mcp_add_server`'s doc gives for
/// connecting on submit rather than waiting for a second confirmation.
#[tauri::command]
pub async fn gateway_start<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, GatewayRuntime>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<GatewayStatusInfo> {
    enable_and_start(&app, runtime.inner(), settings.inner()).await
}

/// Behaviourally identical to [`gateway_start`] today — `start_internal`
/// always tears down any previous session before starting a new one — kept
/// as its own command because "retry after fixing a token" reads as Restart
/// in a UI, not Start, and the two are free to diverge later.
#[tauri::command]
pub async fn gateway_restart<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, GatewayRuntime>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<GatewayStatusInfo> {
    enable_and_start(&app, runtime.inner(), settings.inner()).await
}

async fn enable_and_start<R: Runtime>(
    app: &AppHandle<R>,
    runtime: &GatewayRuntime,
    settings: &SettingsManager,
) -> Res<GatewayStatusInfo> {
    let mut next = settings.get();
    next.gateway.telegram.enabled = true;
    settings::save(app, &next)?;
    Ok(start_internal(app, runtime, settings).await)
}

#[tauri::command]
pub async fn gateway_stop<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, GatewayRuntime>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<GatewayStatusInfo> {
    let mut next = settings.get();
    next.gateway.telegram.enabled = false;
    settings::save(&app, &next)?;

    stop_internal(runtime.inner()).await;
    Ok(status_info(runtime.inner(), settings.inner()))
}

/// Store a bot token in the OS keychain. One-way, like `set_backend_api_key`:
/// there is no command to read it back out, so a compromised webview cannot
/// exfiltrate it.
#[tauri::command]
pub fn gateway_set_telegram_token(token: String) -> Res<bool> {
    secrets::set_telegram_bot_token(token.trim()).map_err(|e| e.to_string())?;
    Ok(secrets::has_telegram_bot_token())
}

#[tauri::command]
pub fn gateway_delete_telegram_token() -> Res<()> {
    secrets::delete_telegram_bot_token().map_err(|e| e.to_string())
}

/// Validate a candidate token against Telegram's `getMe` before it is ever
/// saved, so the UI can offer "Test" separately from "Save". Takes the token
/// as a parameter rather than reading the stored one, so a candidate can be
/// checked before committing to it; whatever is currently in the keychain (if
/// anything) is untouched either way.
#[tauri::command]
pub async fn gateway_test_telegram_token(token: String) -> Res<String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("Enter a bot token first.".into());
    }
    let http = telegram::build_http_client(SHORT_CALL_TIMEOUT_SECS).map_err(|e| e.to_string())?;
    let me = telegram::get_me(&http, &token).await.map_err(|e| e.to_string())?;
    Ok(format!("Connected as @{}.", me.username.as_deref().unwrap_or("(this bot has no username)")))
}

/// Look up a chat's display name, e.g. so the owner can confirm an
/// allow-listed id really resolves to the person they think it does. Works
/// whether or not the gateway is currently running: it opens its own
/// short-lived connection rather than requiring a live poll loop.
#[tauri::command]
pub async fn gateway_chat_info(chat_id: String) -> Res<ChatInfo> {
    let token = secrets::get_telegram_bot_token_opt().ok_or_else(|| GatewayError::NotConfigured.to_string())?;
    let http = telegram::build_http_client(SHORT_CALL_TIMEOUT_SECS).map_err(|e| e.to_string())?;
    telegram::get_chat_info_via(&http, &token, &chat_id).await.map_err(|e| e.to_string())
}

/// Timeout for a one-off, user-initiated API call (test token, look up a
/// chat) — short, because a UI action should fail fast and clearly rather
/// than hang; contrast `telegram::POLL_TIMEOUT_SECS`, which the long-poll
/// connection is deliberately built around waiting out.
const SHORT_CALL_TIMEOUT_SECS: u64 = 15;

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // RateLimiter
    // -----------------------------------------------------------------

    #[test]
    fn allows_up_to_the_configured_budget_then_rejects() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        let now = Instant::now();
        assert!(limiter.check_at(now));
        assert!(limiter.check_at(now));
        assert!(limiter.check_at(now));
        assert!(!limiter.check_at(now), "a fourth hit inside the window must be rejected");
    }

    #[test]
    fn budget_replenishes_once_the_window_has_fully_elapsed() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.check_at(t0));
        assert!(!limiter.check_at(t0 + Duration::from_secs(30)), "still inside the window");
        assert!(limiter.check_at(t0 + Duration::from_secs(61)), "the window has elapsed, budget is back");
    }

    #[test]
    fn an_old_hit_is_forgotten_but_a_recent_one_still_counts() {
        // A sliding window, not a hard reset: one old hit ages out while a
        // more recent one (still inside the window) keeps counting against
        // the budget.
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.check_at(t0));
        assert!(limiter.check_at(t0 + Duration::from_secs(65)), "the first hit has aged out");
        assert!(!limiter.check_at(t0 + Duration::from_secs(90)), "the second hit is still within its own window");
    }

    // -----------------------------------------------------------------
    // DenyApproval -- rule 4, fail closed
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn deny_approval_always_denies() {
        let asker = DenyApproval;
        assert!(!asker.ask("session-1", "delete a file").await);
        assert!(!asker.ask("session-2", "").await);
    }

    // -----------------------------------------------------------------
    // reply_text_for
    // -----------------------------------------------------------------

    fn outcome(stop_reason: agent::StopReason, final_message: &str) -> agent::AgentOutcome {
        agent::AgentOutcome {
            session_id: "s".into(),
            completed: stop_reason == agent::StopReason::Completed,
            steps: 1,
            final_message: final_message.into(),
            stop_reason,
            usage: None,
        }
    }

    #[test]
    fn a_non_empty_final_message_always_wins_over_the_stop_reason() {
        let o = outcome(agent::StopReason::MaxSteps, "here is what I found anyway");
        assert_eq!(reply_text_for(&o), "here is what I found anyway");
    }

    #[test]
    fn declined_explains_the_fail_closed_approval_gate() {
        let o = outcome(agent::StopReason::Declined, "");
        let text = reply_text_for(&o);
        assert!(text.contains("approval"));
        assert!(text.contains("desktop"));
    }

    #[test]
    fn every_stop_reason_produces_non_empty_text_with_no_final_message() {
        for reason in [
            agent::StopReason::Completed,
            agent::StopReason::MaxSteps,
            agent::StopReason::UserStopped,
            agent::StopReason::Declined,
            agent::StopReason::Error,
        ] {
            let o = outcome(reason, "");
            assert!(!reply_text_for(&o).trim().is_empty(), "{reason:?} produced a blank reply");
        }
    }

    // -----------------------------------------------------------------
    // GatewayStatus / GatewayRuntime
    // -----------------------------------------------------------------

    #[test]
    fn status_serializes_with_a_discriminator() {
        let json = serde_json::to_value(GatewayStatus::Error { reason: "bad token".into() }).unwrap();
        assert_eq!(json["state"], "error");
        assert_eq!(json["reason"], "bad token");

        let json = serde_json::to_value(GatewayStatus::Connected).unwrap();
        assert_eq!(json["state"], "connected");
    }

    #[test]
    fn a_fresh_runtime_starts_stopped_with_no_adapter() {
        let rt = GatewayRuntime::new();
        assert_eq!(rt.status(), GatewayStatus::Stopped);
        assert!(rt.take_adapter().is_none());
    }

    #[test]
    fn a_stale_generations_fatal_report_is_ignored_but_the_current_ones_is_not() {
        let rt = GatewayRuntime::new();
        let gen0 = rt.current_generation();
        rt.set_status(GatewayStatus::Connected);

        // Simulate moving on to a new session: `take_adapter` (as
        // `stop_internal` always calls, even on a no-op stop) bumps the
        // generation.
        let _ = rt.take_adapter();
        let gen1 = rt.current_generation();
        assert_ne!(gen0, gen1);

        rt.set_status(GatewayStatus::Connected);
        rt.report_fatal(gen0, "stale, from the old session".into());
        assert_eq!(rt.status(), GatewayStatus::Connected, "a stale generation's report must not overwrite the current status");

        rt.report_fatal(gen1, "fresh, from the current session".into());
        assert_eq!(rt.status(), GatewayStatus::Error { reason: "fresh, from the current session".into() });
    }

    // -----------------------------------------------------------------
    // TelegramGatewaySettings defaults -- fail-safe out of the box
    // -----------------------------------------------------------------

    #[test]
    fn telegram_gateway_settings_default_to_off_and_nobody_allowed() {
        let s = crate::settings::TelegramGatewaySettings::default();
        assert!(!s.enabled);
        assert!(s.allowed_user_ids.is_empty(), "an empty allow-list must mean nobody, not everybody");
    }
}
