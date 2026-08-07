//! The Telegram adapter: [`TelegramAdapter`], talking to the Bot API over
//! long polling.
//!
//! # Why long polling, not a webhook
//!
//! A webhook needs a public URL Telegram can reach, which a desktop app
//! sitting behind a home router and no port forwarding does not have. Long
//! polling needs nothing but an outbound HTTPS connection, which is already
//! how every other backend in this codebase talks to the network — see
//! [`poll_loop`].
//!
//! # The offset
//!
//! `getUpdates` never re-delivers an update once the caller has asked for
//! everything *after* it — that "after" is the `offset` query parameter, and
//! it is the caller's job to track it (Telegram has no concept of
//! acknowledging one update at a time). [`next_offset`] is the whole of that
//! bookkeeping: after every poll, the offset advances past the highest
//! `update_id` seen, whether or not this module actually acted on that
//! update's content. That last part matters — an update from a
//! disallowed sender still has to move the offset forward, or Telegram keeps
//! redelivering it forever, exactly as if this module were stuck failing to
//! process it.
//!
//! # Backoff
//!
//! A poll that fails is retried after [`next_backoff`]'s delay, doubling up
//! to [`MAX_BACKOFF`] and resetting the moment a poll succeeds again — see
//! [`poll_loop`]. Two error codes are treated as unrecoverable rather than
//! retried: 401 (the token was rejected) and 404 (no such bot) — see
//! [`is_fatal`]. Retrying either forever would just hammer Telegram with a
//! request that can never succeed until the owner fixes the token by hand.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::Deserialize;
use tokio::sync::mpsc;

use super::{ChatInfo, GatewayAdapter, GatewayError, GatewayResult, InboundMessage, StatusSink};
use crate::agent;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Telegram's own long-poll window. Chosen well under Telegram's maximum
/// (50s) so `disconnect` never has to wait out too long a single in-flight
/// request before [`poll_once`]'s periodic cancellation check gets a turn
/// between requests -- and mid-request cancellation (the common case, since
/// most of this loop's time is spent inside one request) is handled by
/// racing the request itself, not by waiting for it to finish.
pub(crate) const POLL_TIMEOUT_SECS: u64 = 25;

/// How often a request in flight is interrupted to check whether `disconnect`
/// was asked for. Small enough that Stop feels immediate; large enough not to
/// matter for CPU.
const STOP_CHECK_INTERVAL: Duration = Duration::from_millis(300);

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Telegram enforces a 4096 UTF-16-code-unit cap per `sendMessage`. Counting
/// exact UTF-16 units is not worth the complexity here, so this chunks
/// conservatively well under the real limit -- leaving headroom for any
/// astral-plane characters, which cost two UTF-16 units per Rust `char` but
/// only count once against this budget -- rather than cutting it close and
/// occasionally overshooting.
const MAX_CHUNK_CHARS: usize = 3500;

// ---------------------------------------------------------------------------
// Wire shapes (only the fields this module actually reads)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RawUpdate {
    pub(crate) update_id: i64,
    /// `None` for every update type this module does not act on (edited
    /// messages, channel posts, callback queries, ...) -- Telegram puts each
    /// of those under a *different* top-level key, so simply not
    /// deserializing those keys is what makes them fall out here rather than
    /// needing to be matched and discarded explicitly.
    #[serde(default)]
    pub(crate) message: Option<RawMessage>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RawMessage {
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) from: Option<RawUser>,
    pub(crate) chat: RawChat,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RawUser {
    pub(crate) id: i64,
    #[serde(default)]
    pub(crate) first_name: String,
    #[serde(default)]
    pub(crate) username: Option<String>,
    #[serde(default)]
    pub(crate) is_bot: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RawChat {
    pub(crate) id: i64,
    #[serde(default, rename = "type")]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) first_name: Option<String>,
    #[serde(default)]
    pub(crate) username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BotIdentity {
    #[allow(dead_code)] // read by nothing today; kept because getMe returns it and dropping the field would silently swallow parse errors on it
    pub(crate) id: i64,
    #[serde(default)]
    pub(crate) username: Option<String>,
}

/// The envelope every Telegram Bot API response shares, success or failure:
/// `{"ok": true, "result": ...}` or `{"ok": false, "error_code": ..., "description": ...}`.
/// Telegram uses this same JSON shape regardless of HTTP status code, so
/// [`parse_envelope`] never needs to branch on the transport-level status at
/// all -- only on `ok`.
#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    ok: bool,
    // Deliberately no `#[serde(default)]` here: serde-derive's `default`
    // codegen for a generic field adds a `T: Default` bound to the whole
    // impl (even though `Option<T>` itself needs no such bound), which would
    // force every `T` this is ever called with to implement `Default` for no
    // real reason. Unnecessary anyway -- serde already treats a missing key
    // on an `Option<T>` field as `None` with no attribute required.
    result: Option<T>,
    #[serde(default)]
    error_code: Option<i64>,
    #[serde(default)]
    description: Option<String>,
}

fn parse_envelope<T: serde::de::DeserializeOwned>(body: &str) -> GatewayResult<T> {
    let envelope: ApiEnvelope<T> = serde_json::from_str(body)
        .map_err(|e| GatewayError::Protocol(format!("could not parse Telegram's response: {e}")))?;
    if !envelope.ok {
        return Err(GatewayError::Api {
            code: envelope.error_code.unwrap_or(0),
            message: envelope.description.unwrap_or_else(|| "no description given".into()),
        });
    }
    envelope
        .result
        .ok_or_else(|| GatewayError::Protocol("Telegram said ok but sent no result".into()))
}

/// 401 (token rejected) and 404 (no such bot) can never succeed on retry --
/// the credential itself is wrong, not the network or a transient server
/// hiccup -- so [`poll_loop`] treats these as the end of the session rather
/// than backing off and trying again. Every other API error (rate limited,
/// a 5xx, a malformed-but-parseable response) is assumed transient.
pub(crate) fn is_fatal(err: &GatewayError) -> bool {
    matches!(err, GatewayError::Api { code, .. } if matches!(code, 401 | 404))
}

// ---------------------------------------------------------------------------
// Pure helpers -- offset, parsing, allow-list, backoff, chunking
// ---------------------------------------------------------------------------

/// The `offset` to send on the *next* `getUpdates` call, given the current
/// one and whatever this call returned. See the module doc's "The offset"
/// section: it always advances past every update handed back, whether or not
/// this module acted on it, and never moves backwards on an empty response.
pub(crate) fn next_offset(current: Option<i64>, updates: &[RawUpdate]) -> Option<i64> {
    match updates.iter().map(|u| u.update_id).max() {
        Some(max_id) => Some(max_id + 1),
        None => current,
    }
}

/// Normalize one update into an [`InboundMessage`], or `None` for anything
/// this module does not act on: no `message` at all (an edited message, a
/// channel post, ...), a message with no text (a photo, a sticker, ...), or a
/// message from another bot -- ignored specifically to avoid two bots
/// replying to each other in an endless loop, a real failure mode with no
/// allow-list check that would prevent it (a bot cannot be "allow-listed" the
/// way a human user id can; the fix is simply to never treat one as a sender
/// worth answering).
pub(crate) fn to_inbound(update: &RawUpdate) -> Option<InboundMessage> {
    let msg = update.message.as_ref()?;
    let text = msg.text.as_deref()?.trim();
    if text.is_empty() {
        return None;
    }
    let from = msg.from.as_ref()?;
    if from.is_bot {
        return None;
    }
    Some(InboundMessage {
        sender_id: from.id.to_string(),
        chat_id: msg.chat.id.to_string(),
        text: text.to_string(),
    })
}

/// The allow-list check -- see the gateway module doc, security rule 1. Fails
/// closed on a malformed sender id (should never happen; Telegram ids are
/// always numeric) exactly the same way it fails closed on an id that
/// parses fine but is not on the list: anything short of an exact match on a
/// real, listed id is "no".
pub(crate) fn is_allowed(allowed: &[i64], sender_id: &str) -> bool {
    match sender_id.parse::<i64>() {
        Ok(id) => allowed.contains(&id),
        Err(_) => false,
    }
}

pub(crate) fn next_backoff(current: Duration) -> Duration {
    std::cmp::min(current.saturating_mul(2), MAX_BACKOFF)
}

/// Split `text` into pieces no longer than [`MAX_CHUNK_CHARS`], breaking on
/// line boundaries where possible so a paragraph is not cut mid-sentence
/// purely because the budget landed in the middle of it. A single line
/// longer than the whole budget (unusual, but not impossible for something
/// like a long URL or an unbroken code line) is hard-split character by
/// character rather than left to overflow one chunk.
pub(crate) fn chunk_message(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if text.chars().count() <= MAX_CHUNK_CHARS {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if line.chars().count() > MAX_CHUNK_CHARS {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            for ch in line.chars() {
                if current.chars().count() >= MAX_CHUNK_CHARS {
                    chunks.push(std::mem::take(&mut current));
                }
                current.push(ch);
            }
            continue;
        }
        if current.chars().count() + line.chars().count() > MAX_CHUNK_CHARS {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn chat_name(chat: &RawChat) -> String {
    if let Some(t) = chat.title.as_deref().filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    if let Some(u) = chat.username.as_deref().filter(|s| !s.is_empty()) {
        return format!("@{u}");
    }
    if let Some(f) = chat.first_name.as_deref().filter(|s| !s.is_empty()) {
        return f.to_string();
    }
    chat.id.to_string()
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

pub(crate) fn build_http_client(timeout_secs: u64) -> GatewayResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| GatewayError::Other(format!("could not build an HTTP client: {e}")))
}

/// The token lives in the URL path here because that is Telegram's actual
/// wire format (`/bot<token>/<method>`), not a Caduceus choice -- which is
/// exactly why every call site below converts a transport failure through
/// [`transport_err`] rather than a bare `.to_string()`: `reqwest::Error`'s own
/// `Display` includes the full request URL whenever it has one (see its
/// doc comment, and its own `without_url` escape hatch, which is exactly
/// what `transport_err` calls), so a plain `.to_string()` on a failed
/// request here would put the token straight into a warning log the moment
/// a poll hit a network blip -- not a hypothetical, but the single most
/// likely error this module logs in practice.
fn api_url(token: &str, method: &str) -> String {
    format!("{TELEGRAM_API_BASE}/bot{token}/{method}")
}

/// Convert a `reqwest::Error` to a [`GatewayError::Transport`] with the
/// request URL stripped first. See [`api_url`]'s doc for why this, and not
/// `.to_string()`, is the only correct way to log a transport failure
/// anywhere in this module.
fn transport_err(e: reqwest::Error) -> GatewayError {
    GatewayError::Transport(e.without_url().to_string())
}

pub(crate) async fn get_me(http: &reqwest::Client, token: &str) -> GatewayResult<BotIdentity> {
    let resp = http.get(api_url(token, "getMe")).send().await.map_err(transport_err)?;
    let body = resp.text().await.map_err(transport_err)?;
    parse_envelope(&body)
}

async fn send_one(http: &reqwest::Client, token: &str, chat_id: &str, text: &str) -> GatewayResult<()> {
    let resp = http
        .post(api_url(token, "sendMessage"))
        .json(&serde_json::json!({ "chat_id": chat_id, "text": text }))
        .send()
        .await
        .map_err(transport_err)?;
    let body = resp.text().await.map_err(transport_err)?;
    parse_envelope::<serde_json::Value>(&body)?;
    Ok(())
}

pub(crate) async fn get_chat_info_via(http: &reqwest::Client, token: &str, chat_id: &str) -> GatewayResult<ChatInfo> {
    let resp = http
        .post(api_url(token, "getChat"))
        .json(&serde_json::json!({ "chat_id": chat_id }))
        .send()
        .await
        .map_err(transport_err)?;
    let body = resp.text().await.map_err(transport_err)?;
    let raw: RawChat = parse_envelope(&body)?;
    Ok(ChatInfo { chat_id: raw.id.to_string(), name: chat_name(&raw), kind: raw.kind.clone() })
}

/// Build the `getUpdates` URL for one poll. Pure and separate from
/// [`fetch_updates`] specifically so the offset/timeout query-building logic
/// is unit-testable without a network call.
pub(crate) fn build_get_updates_url(token: &str, offset: Option<i64>, timeout_secs: u64) -> String {
    let mut url = format!("{}?timeout={timeout_secs}", api_url(token, "getUpdates"));
    if let Some(o) = offset {
        url.push_str(&format!("&offset={o}"));
    }
    url
}

async fn fetch_updates(http: &reqwest::Client, token: &str, offset: Option<i64>) -> GatewayResult<Vec<RawUpdate>> {
    let url = build_get_updates_url(token, offset, POLL_TIMEOUT_SECS);
    let resp = http.get(&url).send().await.map_err(transport_err)?;
    let body = resp.text().await.map_err(transport_err)?;
    parse_envelope(&body)
}

// ---------------------------------------------------------------------------
// The poll loop
// ---------------------------------------------------------------------------

/// Race one `getUpdates` call against a periodic check of `stop`, so a
/// request that happens to be sitting inside Telegram's up-to-25s long-poll
/// window does not delay `disconnect` by that long. Returns `None` if `stop`
/// fired before the request resolved -- at which point the in-flight request
/// future is simply dropped, which drops the underlying connection too.
async fn poll_once(
    http: &reqwest::Client,
    token: &str,
    offset: Option<i64>,
    stop: &agent::CancelToken,
) -> Option<GatewayResult<Vec<RawUpdate>>> {
    let fetch = fetch_updates(http, token, offset);
    tokio::pin!(fetch);
    loop {
        if stop.is_cancelled() {
            return None;
        }
        tokio::select! {
            result = &mut fetch => return Some(result),
            _ = tokio::time::sleep(STOP_CHECK_INTERVAL) => continue,
        }
    }
}

/// Sleep out a backoff delay, checking `stop` periodically so a long backoff
/// (up to [`MAX_BACKOFF`]) cannot itself delay `disconnect`. Returns `false`
/// if `stop` fired during the wait.
async fn sleep_or_stop(duration: Duration, stop: &agent::CancelToken) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if stop.is_cancelled() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        tokio::time::sleep(remaining.min(STOP_CHECK_INTERVAL)).await;
    }
}

/// The long-poll loop itself: fetch, hand text messages to `tx`, advance the
/// offset, repeat -- until `stop` fires, the channel's other end is dropped
/// (the dispatcher went away), or a fatal error ends the session for good.
/// Spawned once by [`TelegramAdapter::connect`] and never restarted in
/// place; a fresh `TelegramAdapter` (and thus a fresh loop) is what a
/// subsequent Start creates -- see `gateway::start_internal`.
async fn poll_loop(
    http: reqwest::Client,
    token: String,
    tx: mpsc::Sender<InboundMessage>,
    stop: agent::CancelToken,
    on_fatal: StatusSink,
) {
    let mut offset: Option<i64> = None;
    let mut backoff = INITIAL_BACKOFF;

    loop {
        if stop.is_cancelled() {
            break;
        }

        let Some(result) = poll_once(&http, &token, offset, &stop).await else {
            break;
        };

        match result {
            Ok(updates) => {
                backoff = INITIAL_BACKOFF;
                offset = next_offset(offset, &updates);
                for update in &updates {
                    if stop.is_cancelled() {
                        return;
                    }
                    if let Some(msg) = to_inbound(update) {
                        if tx.send(msg).await.is_err() {
                            // The dispatcher's Receiver is gone; nothing
                            // downstream can act on anything else either.
                            return;
                        }
                    }
                }
            }
            Err(e) if is_fatal(&e) => {
                log::error!("gateway: Telegram polling stopped for good: {e}");
                on_fatal(e.to_string());
                return;
            }
            Err(e) => {
                log::warn!("gateway: a Telegram poll failed, retrying in {backoff:?}: {e}");
                if !sleep_or_stop(backoff, &stop).await {
                    break;
                }
                backoff = next_backoff(backoff);
            }
        }
    }
    log::info!("gateway: Telegram poll loop stopped");
}

// ---------------------------------------------------------------------------
// The adapter
// ---------------------------------------------------------------------------

pub struct TelegramAdapter {
    token: String,
    http: reqwest::Client,
    /// Taken (moved into the spawned poll loop) by `connect`, leaving `None`
    /// behind -- which is also what makes a second `connect()` on the same
    /// instance a clean error instead of silently doing nothing. Once taken,
    /// this is the *only* sender for the channel, so when the poll loop
    /// exits and drops it, `gateway::dispatch_loop`'s `rx.recv()` sees the
    /// channel close and ends on its own -- no separate signal needed to
    /// stop the dispatcher once the receive side has stopped.
    inbound_tx: Mutex<Option<mpsc::Sender<InboundMessage>>>,
    stop: agent::CancelToken,
    on_fatal: StatusSink,
    poll_handle: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl TelegramAdapter {
    pub(crate) fn new(token: String, inbound_tx: mpsc::Sender<InboundMessage>, on_fatal: StatusSink) -> GatewayResult<Self> {
        // A generous ceiling covering the long-poll window plus slack, used
        // for every call this adapter makes (not just polling) -- see the
        // module doc on `build_http_client`'s dedicated short-timeout
        // sibling used for one-off, user-initiated calls in `gateway::mod`.
        let http = build_http_client(POLL_TIMEOUT_SECS + 15)?;
        Ok(Self {
            token,
            http,
            inbound_tx: Mutex::new(Some(inbound_tx)),
            stop: agent::CancelToken::default(),
            on_fatal,
            poll_handle: Mutex::new(None),
        })
    }
}

#[async_trait]
impl GatewayAdapter for TelegramAdapter {
    async fn connect(&self) -> GatewayResult<()> {
        // Fail fast on a bad token rather than starting a poll loop that
        // would immediately hit the same 401 and treat it as fatal anyway --
        // this way a bad token never round-trips through the loop at all.
        let me = get_me(&self.http, &self.token).await?;
        log::info!("gateway: connected to Telegram as @{}", me.username.as_deref().unwrap_or("(this bot has no username)"));

        let Some(tx) = self.inbound_tx.lock().take() else {
            return Err(GatewayError::Other("this adapter has already been connected once".into()));
        };

        let http = self.http.clone();
        let token = self.token.clone();
        let stop = self.stop.clone();
        let on_fatal = self.on_fatal.clone();
        let handle = tauri::async_runtime::spawn(poll_loop(http, token, tx, stop, on_fatal));
        *self.poll_handle.lock() = Some(handle);
        Ok(())
    }

    async fn disconnect(&self) {
        self.stop.cancel();
        if let Some(handle) = self.poll_handle.lock().take() {
            // `abort` is a hard guarantee `disconnect` does not return before
            // the loop is truly gone, on top of the cooperative flag above
            // which lets a healthy loop notice and exit on its own first.
            handle.abort();
        }
    }

    async fn send(&self, chat_id: &str, text: &str) -> GatewayResult<()> {
        for chunk in chunk_message(text) {
            send_one(&self.http, &self.token, chat_id, &chunk).await?;
        }
        Ok(())
    }

    async fn get_chat_info(&self, chat_id: &str) -> GatewayResult<ChatInfo> {
        get_chat_info_via(&self.http, &self.token, chat_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(id: i64, chat_id: i64, sender_id: i64, text: &str) -> RawUpdate {
        RawUpdate {
            update_id: id,
            message: Some(RawMessage {
                text: Some(text.to_string()),
                from: Some(RawUser { id: sender_id, first_name: "Ann".into(), username: None, is_bot: false }),
                chat: RawChat { id: chat_id, kind: "private".into(), title: None, first_name: None, username: None },
            }),
        }
    }

    // -----------------------------------------------------------------
    // Allow-list enforcement -- both directions
    // -----------------------------------------------------------------

    #[test]
    fn an_allow_listed_sender_is_allowed() {
        assert!(is_allowed(&[111, 222], "111"));
        assert!(is_allowed(&[111, 222], "222"));
    }

    #[test]
    fn a_sender_not_on_the_list_is_rejected() {
        assert!(!is_allowed(&[111, 222], "333"));
    }

    #[test]
    fn an_empty_allow_list_rejects_everyone_rather_than_no_one() {
        assert!(!is_allowed(&[], "111"));
    }

    #[test]
    fn a_malformed_sender_id_fails_closed_rather_than_erroring_open() {
        assert!(!is_allowed(&[111], "not-a-number"));
        assert!(!is_allowed(&[111], ""));
    }

    // -----------------------------------------------------------------
    // Offset handling
    // -----------------------------------------------------------------

    #[test]
    fn no_updates_leaves_the_offset_unchanged() {
        assert_eq!(next_offset(None, &[]), None);
        assert_eq!(next_offset(Some(5), &[]), Some(5));
    }

    #[test]
    fn updates_advance_the_offset_past_the_highest_id_seen() {
        let updates = vec![update(1, 1, 1, "a"), update(2, 1, 1, "b"), update(3, 1, 1, "c")];
        assert_eq!(next_offset(None, &updates), Some(4));
    }

    #[test]
    fn out_of_order_updates_still_advance_past_the_maximum_not_the_last() {
        let updates = vec![update(5, 1, 1, "a"), update(3, 1, 1, "b"), update(4, 1, 1, "c")];
        assert_eq!(next_offset(Some(1), &updates), Some(6));
    }

    #[test]
    fn build_get_updates_url_includes_the_offset_only_when_present() {
        let with_offset = build_get_updates_url("TEST_TOKEN", Some(42), 25);
        assert!(with_offset.contains("offset=42"));
        assert!(with_offset.contains("timeout=25"));

        let without_offset = build_get_updates_url("TEST_TOKEN", None, 25);
        assert!(!without_offset.contains("offset="));
        assert!(without_offset.contains("timeout=25"));
    }

    // -----------------------------------------------------------------
    // Update parsing
    // -----------------------------------------------------------------

    #[test]
    fn a_normal_text_message_parses_into_an_inbound_message() {
        let u = update(10, 555, 111, "hello there");
        let msg = to_inbound(&u).expect("a plain text message must parse");
        assert_eq!(msg.sender_id, "111");
        assert_eq!(msg.chat_id, "555");
        assert_eq!(msg.text, "hello there");
    }

    #[test]
    fn an_update_with_no_message_field_is_ignored() {
        // What an edited_message/channel_post/callback_query update looks
        // like once only `message` is deserialized: `message` is simply
        // absent.
        let u = RawUpdate { update_id: 1, message: None };
        assert!(to_inbound(&u).is_none());
    }

    #[test]
    fn a_message_with_no_text_is_ignored() {
        let mut u = update(1, 1, 1, "irrelevant");
        u.message.as_mut().unwrap().text = None;
        assert!(to_inbound(&u).is_none());
    }

    #[test]
    fn a_blank_text_message_is_ignored() {
        let u = update(1, 1, 1, "   ");
        assert!(to_inbound(&u).is_none());
    }

    #[test]
    fn a_message_from_another_bot_is_ignored() {
        let mut u = update(1, 1, 1, "beep boop");
        u.message.as_mut().unwrap().from.as_mut().unwrap().is_bot = true;
        assert!(to_inbound(&u).is_none());
    }

    #[test]
    fn a_message_with_no_sender_is_ignored() {
        let mut u = update(1, 1, 1, "hello");
        u.message.as_mut().unwrap().from = None;
        assert!(to_inbound(&u).is_none());
    }

    #[test]
    fn parse_envelope_reads_a_successful_getupdates_body() {
        let body = r#"{"ok":true,"result":[{"update_id":100,"message":{"message_id":1,"date":0,"chat":{"id":9,"type":"private"},"from":{"id":9,"is_bot":false,"first_name":"A"},"text":"hi"}}]}"#;
        let updates: Vec<RawUpdate> = parse_envelope(body).expect("a valid envelope must parse");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].update_id, 100);
        assert_eq!(updates[0].message.as_ref().unwrap().text.as_deref(), Some("hi"));
    }

    #[test]
    fn parse_envelope_surfaces_an_ok_false_response_as_an_api_error() {
        let body = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        let err = parse_envelope::<Vec<RawUpdate>>(body).unwrap_err();
        match err {
            GatewayError::Api { code, message } => {
                assert_eq!(code, 401);
                assert_eq!(message, "Unauthorized");
            }
            other => panic!("expected an Api error, got {other:?}"),
        }
    }

    #[test]
    fn parse_envelope_rejects_bodies_that_are_not_json() {
        let err = parse_envelope::<Vec<RawUpdate>>("<html>not json</html>").unwrap_err();
        assert!(matches!(err, GatewayError::Protocol(_)));
    }

    #[test]
    fn parse_envelope_handles_an_empty_update_batch() {
        let updates: Vec<RawUpdate> = parse_envelope(r#"{"ok":true,"result":[]}"#).unwrap();
        assert!(updates.is_empty());
    }

    // -----------------------------------------------------------------
    // Backoff
    // -----------------------------------------------------------------

    #[test]
    fn backoff_doubles_each_time() {
        let b1 = next_backoff(Duration::from_secs(1));
        assert_eq!(b1, Duration::from_secs(2));
        let b2 = next_backoff(b1);
        assert_eq!(b2, Duration::from_secs(4));
        let b3 = next_backoff(b2);
        assert_eq!(b3, Duration::from_secs(8));
    }

    #[test]
    fn backoff_is_capped_and_stays_capped() {
        let near_cap = next_backoff(Duration::from_secs(40)); // would be 80s uncapped
        assert_eq!(near_cap, MAX_BACKOFF);
        let still_capped = next_backoff(near_cap);
        assert_eq!(still_capped, MAX_BACKOFF);
    }

    // -----------------------------------------------------------------
    // is_fatal
    // -----------------------------------------------------------------

    #[test]
    fn unauthorized_and_not_found_are_fatal() {
        assert!(is_fatal(&GatewayError::Api { code: 401, message: "x".into() }));
        assert!(is_fatal(&GatewayError::Api { code: 404, message: "x".into() }));
    }

    #[test]
    fn other_api_errors_and_transport_failures_are_not_fatal() {
        assert!(!is_fatal(&GatewayError::Api { code: 429, message: "rate limited".into() }));
        assert!(!is_fatal(&GatewayError::Api { code: 500, message: "server error".into() }));
        assert!(!is_fatal(&GatewayError::Transport("timed out".into())));
        assert!(!is_fatal(&GatewayError::Protocol("bad json".into())));
    }

    // -----------------------------------------------------------------
    // transport_err -- the token must never reach a log line
    // -----------------------------------------------------------------

    /// `reqwest::Error`'s own `Display` embeds the full request URL when it
    /// has one (its doc comment says as much, and recommends exactly the
    /// `without_url()` call `transport_err` makes) -- which, for every
    /// request this module makes, means the bot token embedded in the URL
    /// path. This exercises that against a real (if instantly-refused, purely
    /// loopback, no internet required) connection failure rather than trusting
    /// the library's documentation by inspection alone.
    #[tokio::test]
    async fn transport_errors_never_carry_the_token_bearing_url() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback bind must succeed in a test sandbox");
        let port = listener.local_addr().unwrap().port();
        drop(listener); // now guaranteed closed: nothing answers on `port`

        let client = build_http_client(2).unwrap();
        let url = format!("http://127.0.0.1:{port}/botSECRET_TOKEN_VALUE/getMe");
        let raw_err = client.get(&url).send().await.expect_err("a closed local port must fail the request");

        // Confirms the premise: an unredacted error really does carry the
        // token, or the assertion below would be vacuous.
        assert!(
            raw_err.to_string().contains("SECRET_TOKEN_VALUE"),
            "test premise failed -- reqwest errors are expected to carry the request URL by default"
        );

        let GatewayError::Transport(message) = transport_err(raw_err) else {
            panic!("expected a Transport error");
        };
        assert!(!message.contains("SECRET_TOKEN_VALUE"), "a transport error must never carry the token-bearing URL: {message:?}");
    }

    // -----------------------------------------------------------------
    // chunk_message
    // -----------------------------------------------------------------

    #[test]
    fn short_text_is_a_single_chunk() {
        assert_eq!(chunk_message("hello"), vec!["hello".to_string()]);
    }

    #[test]
    fn blank_text_produces_no_chunks() {
        assert!(chunk_message("").is_empty());
        assert!(chunk_message("   ").is_empty());
    }

    #[test]
    fn long_text_is_split_under_the_chunk_budget() {
        let long = "line\n".repeat(2000); // ~10000 chars
        let chunks = chunk_message(&long);
        assert!(chunks.len() > 1, "text well over the budget must split into multiple chunks");
        for c in &chunks {
            assert!(c.chars().count() <= MAX_CHUNK_CHARS, "no chunk may exceed the budget");
        }
        // Nothing is dropped: rejoining every chunk reproduces the original
        // (trimmed) text.
        assert_eq!(chunks.concat(), long.trim());
    }

    #[test]
    fn a_single_line_longer_than_the_budget_is_hard_split() {
        let long_line = "x".repeat(MAX_CHUNK_CHARS * 2 + 10);
        let chunks = chunk_message(&long_line);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(c.chars().count() <= MAX_CHUNK_CHARS);
        }
        assert_eq!(chunks.concat(), long_line);
    }

    // -----------------------------------------------------------------
    // chat_name
    // -----------------------------------------------------------------

    #[test]
    fn chat_name_prefers_title_then_username_then_first_name_then_id() {
        let mut chat = RawChat { id: 42, kind: "group".into(), title: Some("Team".into()), first_name: None, username: Some("ignored".into()) };
        assert_eq!(chat_name(&chat), "Team");

        chat.title = None;
        assert_eq!(chat_name(&chat), "@ignored");

        chat.username = None;
        chat.first_name = Some("Ann".into());
        assert_eq!(chat_name(&chat), "Ann");

        chat.first_name = None;
        assert_eq!(chat_name(&chat), "42");
    }
}
