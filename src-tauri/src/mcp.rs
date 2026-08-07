//! A native MCP (Model Context Protocol) host: launches locally configured
//! MCP servers as child processes, speaks JSON-RPC 2.0 to them over stdio,
//! and aggregates whatever tools they expose into one namespaced list the
//! agent layer can hand to a model. This is what lets any configured model —
//! local Ollama or a cloud key — call real local tools it does not know
//! about at compile time.
//!
//! # Protocol, in brief (verified against the 2025-06-18 spec at
//! modelcontextprotocol.io)
//!
//! * Transport is newline-delimited JSON-RPC 2.0 over the child's stdin/
//!   stdout. Messages **MUST NOT** contain embedded newlines and the server
//!   **MUST NOT** write anything to stdout that is not a valid MCP message —
//!   `serde_json` never emits a raw newline byte inside a string (it escapes
//!   `\n`), so framing on the write side is automatic; framing on the read
//!   side means anything that fails to parse as one JSON value per line is
//!   treated as a protocol violation, not skipped over leniently.
//! * The session opens with `initialize` (client sends `protocolVersion`,
//!   `capabilities`, `clientInfo`; server replies in kind and may pick a
//!   different, mutually supported `protocolVersion`), followed by a
//!   one-way `notifications/initialized` — a notification, so it carries no
//!   `id` and gets no reply. Only after that does either side send anything
//!   else (`tools/list`, `tools/call`, ...).
//! * `tools/list` returns `{ tools: [...], nextCursor? }`; `tools/call`
//!   takes `{ name, arguments }` and returns `{ content: [...], isError? }`.
//!   Content blocks are typed (`text`, `image`, `audio`, `resource_link`,
//!   `resource`) but this client never branches on the type beyond finding
//!   `text` blocks for a display string — see the security section below.
//! * There are two, deliberately different, ways a tool call can fail: a
//!   **protocol error** (a JSON-RPC `error` object — unknown tool, bad
//!   arguments, a broken server) versus a **tool execution error** (a
//!   normal `result` with `isError: true` — the tool ran and the *server*
//!   is reporting failure, e.g. an API timeout). [`McpError::Protocol`] is
//!   the former; [`ToolCallResultRaw::is_error`] /
//!   [`McpToolCallOutcome::is_error`] is the latter. Collapsing these into
//!   one "it failed" boolean would lose exactly the distinction the spec
//!   asks a client to preserve, so this module never does.
//!
//! # Security model
//!
//! An MCP server is an arbitrary executable the user points Caduceus at,
//! and once running its tools can do anything a normal program on this Mac
//! can do. That makes this module the most security-sensitive surface in
//! the app, and three things are non-negotiable:
//!
//! **(a) Nothing is launched that the user did not explicitly configure.**
//! The only way a [`McpServerConfig`] comes to exist is [`mcp_add_server`]
//! or [`mcp_update_server`] — both driven by a human typing a command and
//! arguments into a form — or hand-editing the store file directly, which
//! is the same trust boundary [`crate::extensions`] already accepts for its
//! `.js` files: it is the user's file, on their own disk. There is no
//! discovery, no fetching a list of servers from anywhere, and no path by
//! which a model's own output can add or launch a server — the aggregated
//! tool list a model sees is built exclusively from servers a human already
//! turned on. [`connect_enabled_servers`] — the one thing that runs
//! automatically, once, at app launch — only ever iterates that same
//! persisted, user-authored list. On top of that, spawning never goes
//! through a shell: [`spawn_server`] execs the configured program directly
//! via [`tokio::process::Command`], so a command or argument cannot smuggle
//! in `; rm -rf` style shell metacharacters even if the user's own input
//! were somehow hostile to itself.
//!
//! One narrow, deliberate exception: [`register_server_if_absent`], added
//! for [`crate::computeruse`]'s cua-driver auto-registration. It still never
//! *launches* anything without persisted, on-disk consent — it writes a
//! config exactly [`mcp_add_server`] would, then calls the identical
//! [`connect_config`] — but it is invoked from Caduceus's own startup path
//! rather than from a human filling in a form. The consent it relies on is
//! different in *shape*, not absent: every caller is required to build its
//! [`McpServerConfig`] from a detection step over the local filesystem for
//! one specific, hardcoded binary name known at compile time, never from a
//! model's output, a network response, or anything else untrusted — so
//! there is still no path by which an agent's own behaviour can add or
//! choose what gets registered. The user consented by installing that exact
//! binary, the same way they consent to any other locally-installed tool
//! Caduceus discovers rather than downloads. See that function's own docs
//! for the rest of the reasoning, including why an existing entry — enabled
//! or not — is never touched.
//!
//! **(b) A tool call is inspectable before it runs.** [`mcp_list_tools`]
//! hands back every tool's full JSON-Schema `inputSchema` up front, and
//! [`mcp_call_tool`] takes the fully-formed `arguments` object as a plain,
//! loggable [`serde_json::Value`] — nothing about what is being sent is
//! hidden or pre-serialized into something opaque. What this module
//! deliberately does **not** do is gate the call behind its own
//! confirmation prompt. That discipline already exists one layer up, in
//! [`crate::agent::types::AgentStep::AwaitingApproval`] for computer use —
//! "show the human what is about to happen before it happens" is a policy
//! of the agent loop, not of any one tool source, and an MCP call should go
//! through the identical show-then-approve step before it ever reaches
//! [`mcp_call_tool`]. Re-implementing that gate down here would just be a
//! second, inevitably-inconsistent copy of the same policy.
//!
//! **(c) A server's output is data, never instructions.** Every tool result
//! is carried as opaque [`serde_json::Value`] content blocks
//! ([`ToolCallResultRaw::content`] / [`McpToolCallOutcome::content`]). This
//! module never interprets a `text` block as anything other than a string
//! to show, never fetches a `resource_link` URI on its own initiative, and
//! never acts on a server's `instructions` field or a tool's `description`
//! as anything other than words to display. The one convenience it performs
//! — [`extract_text`], joining `text` blocks into a display string — is
//! exactly that: a rendering convenience, never consulted to make a
//! decision, and callers downstream (the agent loop, a model's context)
//! must keep treating it as untrusted conversation content, the same rule
//! this file itself has to follow for everything *it* reads from tools.
//! Anything a server writes to stdout that is not a valid MCP message drops
//! the connection rather than being parsed around; stderr — where a server
//! can write anything at all — is captured only as opaque display strings
//! in a small ring buffer for a "why is this unhealthy" log, and is never
//! read back as data. The same discipline extends to environment
//! variables: a server's child process gets a minimal, explicit environment
//! (`PATH`/`HOME`/`USER`/`SHELL`/`TMPDIR`/`LANG` plus whatever the user
//! typed into that server's own `env` config) rather than Caduceus's full
//! process environment, so pointing Caduceus at a new server can never be a
//! way to exfiltrate secrets — cloud credentials, tokens for unrelated
//! tools — that have nothing to do with it. See [`base_environment`].
//!
//! # Lifecycle and deadlines
//!
//! Every network-shaped wait here has a deadline: [`HANDSHAKE_TIMEOUT`] on
//! `initialize`, [`TOOLS_LIST_TIMEOUT`] on each `tools/list` page (capped at
//! [`MAX_TOOL_LIST_PAGES`] pages total — an endless `nextCursor` chain from
//! a broken or hostile server is a hang by another name), and
//! [`TOOL_CALL_TIMEOUT`] on `tools/call`. A server that never launches,
//! never answers, answers with garbage, or answers with an unsupported
//! `protocolVersion` all land in [`ServerStatus::Unhealthy`] with a reason a
//! person can read — never a panic, never an indefinite wait. A background
//! poll (started once a server reaches [`ServerStatus::Ready`], see
//! [`spawn_death_watcher`]) also catches a server that dies with nobody
//! actively calling it, rather than waiting for the next call to discover
//! the pipe is gone.
//!
//! One connection serializes its own requests — [`ServerHandle::conn`] is a
//! [`tokio::sync::Mutex`], not a multiplexed table of in-flight calls keyed
//! by id. Real MCP servers overwhelmingly expect one request outstanding at
//! a time anyway, and a client that cannot possibly have two requests
//! racing on the same connection is a great deal easier to read, to audit,
//! and to trust with exactly the job this module has — a worthwhile trade
//! against a little throughput.
//!
//! # Why the wire engine is generic over the transport
//!
//! [`Connection`] speaks JSON-RPC over any `AsyncBufRead + AsyncWrite`, not
//! specifically over a child process's pipes. Production code
//! ([`spawn_server`]) wires it to a real `Child`'s stdin/stdout; the test
//! module wires the exact same [`Connection::initialize`] /
//! [`Connection::list_all_tools`] / [`Connection::call_tool`] methods to an
//! in-memory [`tokio::io::duplex`] pair standing in for a fake server. That
//! is what makes the framing, the handshake ordering, the timeout paths and
//! the error-shape tests real coverage of the exact code that talks to a
//! real process, without any test spawning a real MCP server or touching
//! the network.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex as SyncMutex, RwLock as SyncRwLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::StoreExt;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command as TokioCommand};
use tokio::sync::Mutex as AsyncMutex;

type Res<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// The protocol version this client asks for. Per spec this should be the
/// *latest* version the client supports; a server that only speaks an older
/// one is expected to say so in its response, which [`SUPPORTED_PROTOCOL_VERSIONS`]
/// is checked against.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Versions this client will accept a server replying with. Per spec, if the
/// server's chosen version is not one the client supports, the client
/// **SHOULD** disconnect — [`Connection::initialize`] does exactly that,
/// surfacing [`McpError::UnsupportedVersion`] rather than pressing on with a
/// version nobody agreed to.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const CLIENT_NAME: &str = "Caduceus";

/// How long `initialize` gets to answer before the server is unhealthy.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long any single `tools/list` page gets.
const TOOLS_LIST_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a `tools/call` gets. Generous relative to the others because a
/// tool might do real work (a web request, a database query) — but still
/// bounded, per the module's "every wait needs a deadline" rule.
const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Hard cap on `tools/list` pages followed in one listing. Not a realistic
/// number of pages for any real tool catalogue — it exists purely so a
/// server that hands back an endless `nextCursor` chain (broken or hostile)
/// produces a readable error instead of a loop that never returns.
const MAX_TOOL_LIST_PAGES: usize = 50;

/// How many trailing stderr lines are kept per server for diagnostics.
const STDERR_RING_CAPACITY: usize = 20;

/// How often the death watcher polls a connected server's process for exit.
const DEATH_WATCH_INTERVAL: Duration = Duration::from_millis(750);

/// Longest a configured server name may be, and the only characters it may
/// use — keeps namespaced tool ids (`{server}__{tool}`) clean identifiers a
/// model's function-calling API is happy with.
const MAX_SERVER_NAME_LEN: usize = 40;

const STORE_FILE: &str = "caduceus-mcp.json";
const SERVERS_KEY: &str = "servers";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, thiserror::Error)]
pub enum McpError {
    #[error("could not start the process: {0}")]
    Spawn(String),
    #[error("the connection closed before it answered")]
    Closed,
    #[error("{method} did not answer within {after:?}")]
    Timeout { method: String, after: Duration },
    #[error("it sent invalid JSON-RPC ({detail}){}", raw_suffix(.raw))]
    InvalidJson { raw: String, detail: String },
    /// A JSON-RPC `error` response — a *protocol*-level failure (unknown
    /// tool, bad arguments, a broken server), distinct from a tool that ran
    /// and reported its own failure via `isError` (see
    /// [`ToolCallResultRaw::is_error`]).
    #[error("protocol error {code}: {message}")]
    Protocol { code: i64, message: String },
    #[error("unsupported protocol version: it offered {offered}")]
    UnsupportedVersion { offered: String },
    #[error("{0}")]
    Io(String),
}

fn raw_suffix(raw: &str) -> String {
    if raw.is_empty() {
        String::new()
    } else {
        format!(": {raw}")
    }
}

impl From<McpError> for String {
    fn from(e: McpError) -> String {
        e.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}\u{2026}", s.chars().take(max).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Wire types (JSON-RPC envelope)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RpcMessageIn {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcErrorObj>,
}

#[derive(Debug, Clone, Deserialize)]
struct RpcErrorObj {
    code: i64,
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    data: Option<Value>,
}

/// What an incoming line turned out to be, per the base JSON-RPC framing
/// rules: a response has `id` + (`result` xor `error`) and no `method`; a
/// notification has `method` and no `id`; a request has both. This client
/// only ever sends requests, so an incoming *request* (`id` + `method`) is
/// something we did not ask for — the spec has no server-to-client requests
/// this client implements (no `sampling`/`roots` capability declared, see
/// [`Connection::initialize`]), so those are read past rather than acted on.
enum Incoming {
    Response(u64),
    Notification(#[allow(dead_code)] String),
    ServerRequest(#[allow(dead_code)] String),
    Malformed,
}

fn classify(msg: &RpcMessageIn) -> Incoming {
    match (&msg.id, &msg.method) {
        (Some(id), None) => match id.as_u64() {
            Some(n) => Incoming::Response(n),
            None => Incoming::Malformed,
        },
        (Some(_), Some(m)) => Incoming::ServerRequest(m.clone()),
        (None, Some(m)) => Incoming::Notification(m.clone()),
        (None, None) => Incoming::Malformed,
    }
}

fn finish_response(msg: RpcMessageIn) -> Result<Value, McpError> {
    match (msg.result, msg.error) {
        (Some(r), None) => Ok(r),
        (None, Some(e)) => Err(McpError::Protocol { code: e.code, message: e.message }),
        (Some(_), Some(_)) => Err(McpError::InvalidJson {
            raw: String::new(),
            detail: "a response carried both a result and an error".into(),
        }),
        (None, None) => Err(McpError::InvalidJson {
            raw: String::new(),
            detail: "a response carried neither a result nor an error".into(),
        }),
    }
}

// ---------------------------------------------------------------------------
// MCP data shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ServerInfoRaw {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResultRaw {
    protocol_version: String,
    #[serde(default)]
    capabilities: Value,
    #[serde(default)]
    server_info: Option<ServerInfoRaw>,
    #[serde(default)]
    instructions: Option<String>,
}

/// Does a negotiated `capabilities` object declare the `tools` capability?
/// Servers that omit it (resources- or prompts-only servers) are still
/// legitimately connected — they simply contribute nothing to the tool
/// registry, so `tools/list` is skipped for them rather than sent and
/// treated as an error when it (correctly) fails.
fn declares_tools(capabilities: &Value) -> bool {
    capabilities.get("tools").is_some()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTool {
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_input_schema")]
    input_schema: Value,
    #[serde(default)]
    #[allow(dead_code)]
    output_schema: Option<Value>,
}

fn default_input_schema() -> Value {
    json!({ "type": "object" })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolsListResult {
    #[serde(default)]
    tools: Vec<RawTool>,
    #[serde(default)]
    next_cursor: Option<String>,
}

/// The raw `tools/call` result, before it is shaped into
/// [`McpToolCallOutcome`] for the frontend.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallResultRaw {
    /// Untouched content blocks, exactly as the server sent them — see the
    /// module header's security section on why this is never more than
    /// opaque data as far as this file is concerned.
    #[serde(default)]
    content: Vec<Value>,
    /// A *tool execution* failure (the call reached the tool and the tool
    /// reported failure), as opposed to a JSON-RPC `error` response, which
    /// surfaces as [`McpError::Protocol`] and never reaches this struct at
    /// all. See the module header for why the distinction matters.
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    #[allow(dead_code)]
    structured_content: Option<Value>,
}

/// Join every `text` content block into one display string. A convenience
/// for a UI or log line — never consulted by this module to make a
/// decision, and never should be by anything downstream either: it is
/// exactly the untrusted server output the header comment describes.
fn extract_text(content: &[Value]) -> String {
    content
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// The wire engine: JSON-RPC framing over any async byte stream
// ---------------------------------------------------------------------------

/// A live JSON-RPC 2.0 session over a byte stream, generic in the transport
/// so the exact same request/response/handshake logic can be driven by a
/// real child process or by an in-memory pipe in tests. See the module
/// header for why this split exists.
struct Connection<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
}

impl<R: AsyncBufRead + Unpin + Send, W: AsyncWrite + Unpin + Send> Connection<R, W> {
    fn new(reader: R, writer: W) -> Self {
        Self { reader, writer, next_id: 0 }
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    async fn write_value(&mut self, value: &Value) -> Result<(), McpError> {
        let mut bytes = serde_json::to_vec(value).map_err(|e| McpError::Io(e.to_string()))?;
        // Messages are newline-delimited and must never contain an embedded
        // newline — `serde_json` escapes `\n` inside strings rather than
        // emitting a raw byte, so appending exactly one here is sufficient
        // framing, never split across what we write.
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.map_err(|e| McpError::Io(e.to_string()))?;
        self.writer.flush().await.map_err(|e| McpError::Io(e.to_string()))
    }

    async fn read_line(&mut self) -> Result<Option<String>, McpError>
    where
        R: AsyncBufReadExt,
    {
        let mut buf = String::new();
        let n = self.reader.read_line(&mut buf).await.map_err(|e| McpError::Io(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(buf))
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let mut msg = serde_json::Map::new();
        msg.insert("jsonrpc".into(), json!("2.0"));
        msg.insert("method".into(), json!(method));
        if let Some(p) = params {
            msg.insert("params".into(), p);
        }
        self.write_value(&Value::Object(msg)).await
    }

    async fn request(&mut self, method: &str, params: Option<Value>, deadline: Duration) -> Result<Value, McpError>
    where
        R: AsyncBufReadExt,
    {
        let id = self.next_id();
        let mut msg = serde_json::Map::new();
        msg.insert("jsonrpc".into(), json!("2.0"));
        msg.insert("id".into(), json!(id));
        msg.insert("method".into(), json!(method));
        if let Some(p) = params {
            msg.insert("params".into(), p);
        }
        self.write_value(&Value::Object(msg)).await?;

        let method = method.to_string();
        tokio::time::timeout(deadline, self.await_response(id))
            .await
            .map_err(|_| McpError::Timeout { method, after: deadline })?
    }

    /// Read lines until the response for `id` arrives, skipping past
    /// anything else — a stray late response, a notification, a
    /// server-to-client request this client does not implement. See
    /// [`Incoming`]'s docs for why every one of those is safe to read past
    /// rather than a reason to stop.
    async fn await_response(&mut self, id: u64) -> Result<Value, McpError>
    where
        R: AsyncBufReadExt,
    {
        loop {
            let Some(line) = self.read_line().await? else {
                return Err(McpError::Closed);
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: RpcMessageIn = serde_json::from_str(trimmed).map_err(|e| McpError::InvalidJson {
                raw: truncate(trimmed, 200),
                detail: e.to_string(),
            })?;
            match classify(&msg) {
                Incoming::Response(rid) if rid == id => return finish_response(msg),
                Incoming::Response(_) | Incoming::Notification(_) | Incoming::ServerRequest(_) => continue,
                Incoming::Malformed => {
                    return Err(McpError::InvalidJson {
                        raw: truncate(trimmed, 200),
                        detail: "neither a response, a request, nor a notification".into(),
                    })
                }
            }
        }
    }

    /// The `initialize` handshake: send our `protocolVersion`/`capabilities`/
    /// `clientInfo`, reject a version the client cannot speak, then send the
    /// one-way `notifications/initialized` that lets normal operation begin.
    /// Caduceus declares no client capabilities (no `roots`, `sampling`, or
    /// `elicitation`) — v1 only calls tools, so there is nothing to offer a
    /// server back yet.
    async fn initialize(&mut self, deadline: Duration) -> Result<InitializeResultRaw, McpError>
    where
        R: AsyncBufReadExt,
    {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
        });
        let result = self.request("initialize", Some(params), deadline).await?;
        let init: InitializeResultRaw = serde_json::from_value(result)
            .map_err(|e| McpError::InvalidJson { raw: String::new(), detail: format!("initialize result: {e}") })?;

        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&init.protocol_version.as_str()) {
            return Err(McpError::UnsupportedVersion { offered: init.protocol_version });
        }

        self.notify("notifications/initialized", None).await?;
        Ok(init)
    }

    /// Every tool across every `tools/list` page, following `nextCursor`
    /// until the server stops sending one — bounded by
    /// [`MAX_TOOL_LIST_PAGES`] so a broken or hostile cursor chain cannot
    /// turn this into an unbounded loop.
    async fn list_all_tools(&mut self, deadline: Duration) -> Result<Vec<RawTool>, McpError>
    where
        R: AsyncBufReadExt,
    {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_LIST_PAGES {
            let params = cursor.take().map(|c| json!({ "cursor": c }));
            let result = self.request("tools/list", params, deadline).await?;
            let page: ToolsListResult = serde_json::from_value(result)
                .map_err(|e| McpError::InvalidJson { raw: String::new(), detail: format!("tools/list result: {e}") })?;
            tools.extend(page.tools);
            match page.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => return Ok(tools),
            }
        }
        Err(McpError::Protocol {
            code: 0,
            message: "kept paginating tools/list past the page limit — the server may be sending an endless cursor".into(),
        })
    }

    async fn call_tool(&mut self, name: &str, arguments: Value, deadline: Duration) -> Result<ToolCallResultRaw, McpError>
    where
        R: AsyncBufReadExt,
    {
        let params = json!({ "name": name, "arguments": arguments });
        let result = self.request("tools/call", Some(params), deadline).await?;
        serde_json::from_value(result)
            .map_err(|e| McpError::InvalidJson { raw: String::new(), detail: format!("tools/call result: {e}") })
    }
}

// ---------------------------------------------------------------------------
// Configuration and persistence
// ---------------------------------------------------------------------------
//
// Follows the pattern in `widgets.rs`: its own store file, never
// `crate::settings::Settings` — an MCP server can be added or removed
// without touching the shared settings schema or its version.

/// A user-configured MCP server: what to run and how. This is the entire
/// trust boundary described in the module header's point (a) — nothing in
/// this file ever constructs one of these on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Unique, user-chosen key. Also the namespace prefix for every tool
    /// this server exposes — see [`namespaced_id`].
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the child process, merged over the
    /// minimal base in [`base_environment`]. This — not Caduceus's own
    /// environment — is the explicit channel for a server that needs an API
    /// key or a working directory hint.
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Server names become part of a namespaced tool id handed to a model's
/// function-calling API, so they are held to the same conservative charset
/// most such APIs require: letters, digits, `-`, `_`.
fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SERVER_NAME_LEN
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn load_configs<R: Runtime>(app: &AppHandle<R>) -> Vec<McpServerConfig> {
    let Ok(store) = app.store(STORE_FILE) else {
        return Vec::new();
    };
    store.get(SERVERS_KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save_configs<R: Runtime>(app: &AppHandle<R>, configs: &[McpServerConfig]) -> Res<()> {
    let store = app.store(STORE_FILE).map_err(|e| format!("could not open the MCP server store: {e}"))?;
    let value = serde_json::to_value(configs).map_err(|e| e.to_string())?;
    store.set(SERVERS_KEY, value);
    store.save().map_err(|e| format!("could not write MCP server config: {e}"))
}

// ---------------------------------------------------------------------------
// Process wiring
// ---------------------------------------------------------------------------

type ProcConnection = Connection<BufReader<tokio::process::ChildStdout>, tokio::process::ChildStdin>;

/// Minimal environment passed to every server's child process — deliberately
/// not Caduceus's own environment. See the module header, point (c): a
/// third-party executable the user pointed at for a couple of tools has no
/// business inheriting whatever secrets happen to be sitting in this
/// process's environment for unrelated reasons.
fn base_environment() -> Vec<(String, String)> {
    const CARRY: &[&str] = &["PATH", "HOME", "USER", "SHELL", "TMPDIR", "LANG"];
    CARRY.iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v))).collect()
}

/// Runtime status of one configured server, as shown to the user.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ServerStatus {
    /// Process launched, handshake in flight.
    Connecting,
    /// Handshake succeeded; tools (if any) are listed.
    Ready,
    /// Never launched, never finished the handshake, sent something that
    /// didn't parse, or exited — always with a reason a person can read.
    Unhealthy { reason: String },
    /// Configured but not currently running (never connected, or explicitly
    /// disconnected).
    Disconnected,
}

/// What the negotiated `initialize` exchange told us about the server.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerIdentity {
    pub protocol_version: String,
    pub server_name: String,
    pub server_version: String,
    pub instructions: Option<String>,
}

/// A live (or once-live, or attempted) server: the process, its connection,
/// and everything about it a caller might want to inspect. Held behind an
/// `Arc` in [`McpRuntime`] so the death watcher, the connection, and the
/// commands that read status can all share one without cloning the world.
struct ServerHandle {
    config: McpServerConfig,
    /// `None` once disconnected, or if the process never spawned at all.
    child: AsyncMutex<Option<Child>>,
    /// `None` under the same conditions as `child`. A `tokio::sync::Mutex`
    /// (not `parking_lot`) because request/response round trips hold it
    /// across `.await` — see the module header on why one connection
    /// serializes its own calls rather than multiplexing them.
    conn: AsyncMutex<Option<ProcConnection>>,
    status: SyncRwLock<ServerStatus>,
    identity: SyncRwLock<Option<ServerIdentity>>,
    tools: SyncRwLock<Vec<RawTool>>,
    /// Last [`STDERR_RING_CAPACITY`] lines of stderr, kept only as inert
    /// display text for a "why is this unhealthy" log — never parsed, never
    /// treated as data. See the module header, point (c).
    stderr_log: Arc<SyncMutex<VecDeque<String>>>,
}

impl ServerHandle {
    /// A handle for a server whose process never even started — still
    /// registered (rather than silently discarded) so its reason stays
    /// visible until the user retries or reconfigures it.
    fn failed(config: &McpServerConfig, reason: String) -> Self {
        Self {
            config: config.clone(),
            child: AsyncMutex::new(None),
            conn: AsyncMutex::new(None),
            status: SyncRwLock::new(ServerStatus::Unhealthy { reason }),
            identity: SyncRwLock::new(None),
            tools: SyncRwLock::new(Vec::new()),
            stderr_log: Arc::new(SyncMutex::new(VecDeque::new())),
        }
    }
}

fn spawn_stderr_drain(stderr: tokio::process::ChildStderr, log: Arc<SyncMutex<VecDeque<String>>>) {
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut log = log.lock();
            if log.len() >= STDERR_RING_CAPACITY {
                log.pop_front();
            }
            log.push_back(line);
        }
    });
}

/// Launch a server's process and wire up its stdio, without performing the
/// handshake yet — that is [`connect_one`]'s job, so a caller can register
/// the handle (and thus its status) before the handshake's own deadline
/// starts ticking.
async fn spawn_server(config: &McpServerConfig) -> Result<ServerHandle, McpError> {
    let mut cmd = TokioCommand::new(&config.command);
    cmd.args(&config.args);
    // Never inherit this process's environment, and never run through a
    // shell — `Command::new` execs the configured program's argv directly.
    // See the module header, points (a) and (c).
    cmd.env_clear();
    for (k, v) in base_environment() {
        cmd.env(k, v);
    }
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Belt-and-suspenders: if this `Child` is ever dropped without going
    // through `disconnect` (a bug, a crash), tokio kills the process rather
    // than leaving it orphaned.
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| McpError::Spawn(e.to_string()))?;
    let stdin = child.stdin.take().ok_or_else(|| McpError::Spawn("no stdin pipe".into()))?;
    let stdout = child.stdout.take().ok_or_else(|| McpError::Spawn("no stdout pipe".into()))?;
    let stderr_log = Arc::new(SyncMutex::new(VecDeque::with_capacity(STDERR_RING_CAPACITY)));
    if let Some(stderr) = child.stderr.take() {
        spawn_stderr_drain(stderr, stderr_log.clone());
    }

    let conn = Connection::new(BufReader::new(stdout), stdin);
    Ok(ServerHandle {
        config: config.clone(),
        child: AsyncMutex::new(Some(child)),
        conn: AsyncMutex::new(Some(conn)),
        status: SyncRwLock::new(ServerStatus::Connecting),
        identity: SyncRwLock::new(None),
        tools: SyncRwLock::new(Vec::new()),
        stderr_log,
    })
}

/// Run the handshake and (if the server declares the `tools` capability)
/// list its tools, updating `handle`'s status in place. Never returns an
/// error itself — a failure at any step becomes
/// [`ServerStatus::Unhealthy`] with that step's reason, which is the whole
/// point: the caller always gets a status back, never a bare `Result` to
/// propagate past a UI that wants to keep showing the server either way.
async fn connect_one(handle: &ServerHandle) {
    let outcome: Result<(InitializeResultRaw, Vec<RawTool>), McpError> = async {
        let mut guard = handle.conn.lock().await;
        let conn = guard.as_mut().ok_or(McpError::Closed)?;
        let init = conn.initialize(HANDSHAKE_TIMEOUT).await?;
        let tools = if declares_tools(&init.capabilities) {
            conn.list_all_tools(TOOLS_LIST_TIMEOUT).await?
        } else {
            Vec::new()
        };
        Ok((init, tools))
    }
    .await;

    match outcome {
        Ok((init, tools)) => {
            let InitializeResultRaw { protocol_version, server_info, instructions, .. } = init;
            let server_info = server_info.unwrap_or_default();
            *handle.identity.write() = Some(ServerIdentity {
                protocol_version,
                server_name: server_info.name,
                server_version: server_info.version,
                instructions,
            });
            *handle.tools.write() = tools;
            *handle.status.write() = ServerStatus::Ready;
        }
        Err(e) => {
            *handle.status.write() = ServerStatus::Unhealthy { reason: e.to_string() };
        }
    }
}

/// Close a server's connection and stop its process. Closes stdin first
/// (dropping the connection's writer half) and gives the process a moment
/// to exit on its own — the graceful half of the spec's stdio shutdown
/// sequence — before escalating straight to `SIGKILL` rather than following
/// with a separate `SIGTERM` step: a process that ignored the graceful
/// window is exactly the "hostile or wedged" case this module has to assume
/// throughout, and a well-behaved one already had its chance.
async fn disconnect(handle: &ServerHandle) {
    *handle.status.write() = ServerStatus::Disconnected;
    *handle.conn.lock().await = None;

    let taken = {
        let mut guard = handle.child.lock().await;
        guard.take()
    };
    let Some(mut child) = taken else { return };

    if tokio::time::timeout(Duration::from_secs(2), child.wait()).await.is_ok() {
        return;
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
}

/// Poll a connected server's process for exit so a server that dies with no
/// one actively calling it still becomes visibly unhealthy, rather than
/// silently sitting in `Ready` until the next call happens to discover the
/// pipe is gone. A poll, not a blocking wait — see the module header on why
/// this does not need a deadline the way a request/response round trip
/// does. Self-terminates once the server is disconnected or gone, so it
/// never accumulates as a leak across repeated connect/disconnect cycles.
fn spawn_death_watcher(handle: Arc<ServerHandle>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(DEATH_WATCH_INTERVAL).await;

            if matches!(*handle.status.read(), ServerStatus::Disconnected) {
                return;
            }

            let exited = {
                let mut guard = handle.child.lock().await;
                match guard.as_mut() {
                    Some(child) => child.try_wait().ok().flatten(),
                    None => return,
                }
            };
            if let Some(status) = exited {
                *handle.status.write() = ServerStatus::Unhealthy { reason: format!("the server process exited ({status})") };
                return;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Every connected (or connecting, or unhealthy-but-registered) server,
/// keyed by its configured name. Lazily self-managed the same way
/// `widgets::WidgetRuntime` is — see [`ensure_managed`] — so this file needs
/// no changes to `lib.rs::setup` to work.
#[derive(Default)]
pub struct McpRuntime {
    servers: SyncRwLock<HashMap<String, Arc<ServerHandle>>>,
}

fn ensure_managed<R: Runtime>(app: &AppHandle<R>) {
    if app.try_state::<McpRuntime>().is_none() {
        app.manage(McpRuntime::default());
    }
}

/// Namespace a server's own tool name so two servers exposing a same-named
/// tool (`search`, say) never collide in the aggregated list. Resolution
/// (see [`find_tool`]) always *recomputes* this from a known `(server,
/// tool)` pair rather than splitting the id string back apart, so it is
/// never ambiguous in practice even in the case that two different pairs
/// would happen to render an identical string — see
/// [`find_tool_resolves_by_recomputed_id_not_by_splitting_the_string`] for
/// exactly that scenario.
fn namespaced_id(server: &str, tool: &str) -> String {
    format!("{server}__{tool}")
}

/// Every tool exposed to the agent layer: one entry per `(server, tool)`
/// pair, namespaced. This — not [`RawTool`] — is the shape callers outside
/// this module see.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub id: String,
    pub server: String,
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
}

/// Flatten `(server name, that server's tools)` pairs into the namespaced
/// list the agent layer consumes. A free function over borrowed slices
/// (rather than a method that reaches into [`McpRuntime`]'s locks) so it is
/// trivially unit-testable without any `AppHandle`.
fn aggregate_tools<'a>(servers: impl IntoIterator<Item = (&'a str, &'a [RawTool])>) -> Vec<McpTool> {
    let mut out = Vec::new();
    for (server, tools) in servers {
        for tool in tools {
            out.push(McpTool {
                id: namespaced_id(server, &tool.name),
                server: server.to_string(),
                name: tool.name.clone(),
                title: tool.title.clone(),
                description: tool.description.clone().unwrap_or_default(),
                input_schema: tool.input_schema.clone(),
            });
        }
    }
    out
}

/// Resolve a namespaced tool id back to the `(server name, tool)` that
/// produced it — by recomputing [`namespaced_id`] for every candidate, never
/// by parsing the id string apart. See [`namespaced_id`]'s docs for why that
/// distinction is the whole safety property.
fn find_tool<'a>(servers: impl IntoIterator<Item = (&'a str, &'a [RawTool])>, tool_id: &str) -> Option<(String, RawTool)> {
    for (server, tools) in servers {
        for tool in tools {
            if namespaced_id(server, &tool.name) == tool_id {
                return Some((server.to_string(), tool.clone()));
            }
        }
    }
    None
}

/// A server's config plus its live status, for the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub status: ServerStatus,
    pub tool_count: usize,
    pub identity: Option<ServerIdentity>,
    pub recent_log: Vec<String>,
}

impl McpServerInfo {
    fn from_config_only(c: &McpServerConfig) -> Self {
        Self {
            name: c.name.clone(),
            command: c.command.clone(),
            args: c.args.clone(),
            enabled: c.enabled,
            status: ServerStatus::Disconnected,
            tool_count: 0,
            identity: None,
            recent_log: Vec::new(),
        }
    }
}

fn server_info(handle: &ServerHandle) -> McpServerInfo {
    McpServerInfo {
        name: handle.config.name.clone(),
        command: handle.config.command.clone(),
        args: handle.config.args.clone(),
        enabled: handle.config.enabled,
        status: handle.status.read().clone(),
        tool_count: handle.tools.read().len(),
        identity: handle.identity.read().clone(),
        recent_log: handle.stderr_log.lock().iter().cloned().collect(),
    }
}

/// The outcome of one `tools/call`, shaped for the frontend/agent layer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallOutcome {
    pub server: String,
    pub tool: String,
    /// A tool-execution failure reported by the server itself — see the
    /// module header on why this is not the same thing as this command
    /// returning `Err`, which instead means the *call itself* could not be
    /// completed (protocol error, disconnected server, unknown tool).
    pub is_error: bool,
    /// Raw content blocks, exactly as the server sent them. Untrusted data —
    /// see the module header, point (c).
    pub content: Vec<Value>,
    /// Convenience join of any `text` blocks. Never used internally to
    /// decide anything; see [`extract_text`].
    pub text: String,
}

/// Remove a server from the registry (if present) and stop its process.
/// Idempotent: calling it for a server that was never connected, or already
/// disconnected, is a no-op.
async fn forget<R: Runtime>(app: &AppHandle<R>, name: &str) {
    ensure_managed(app);
    let handle = {
        let rt = app.state::<McpRuntime>();
        let mut servers = rt.servers.write();
        servers.remove(name)
    };
    if let Some(handle) = handle {
        disconnect(&handle).await;
    }
}

/// Spawn, register, and connect one server from its config — the single
/// path every "make this server live" entry point funnels through.
/// Disconnects any previous instance under the same name first, so
/// reconnecting (or updating a config) can never leave two processes
/// running for one configured server.
async fn connect_config<R: Runtime>(app: &AppHandle<R>, config: &McpServerConfig) -> McpServerInfo {
    forget(app, &config.name).await;

    let handle = Arc::new(match spawn_server(config).await {
        Ok(h) => h,
        Err(e) => ServerHandle::failed(config, e.to_string()),
    });

    {
        let rt = app.state::<McpRuntime>();
        rt.servers.write().insert(config.name.clone(), handle.clone());
    }

    let has_process = handle.child.lock().await.is_some();
    if has_process {
        spawn_death_watcher(handle.clone());
        connect_one(&handle).await;
    }

    server_info(&handle)
}

/// Connect every enabled, persisted server. Meant to be called once at
/// launch — the MCP equivalent of [`crate::widgets::restore_saved_widgets`],
/// except each connection is a handshake with an external process rather
/// than a window, so this is `async`: call it via
/// `tauri::async_runtime::spawn(mcp::connect_enabled_servers(handle))` from
/// `setup()` rather than blocking startup on however many servers are slow
/// to answer. Only ever iterates the persisted, user-authored config list —
/// see the module header, point (a).
pub async fn connect_enabled_servers<R: Runtime>(app: &AppHandle<R>) {
    ensure_managed(app);
    for config in load_configs(app).into_iter().filter(|c| c.enabled) {
        connect_config(app, &config).await;
    }
}

/// The actual idempotency rule behind [`register_server_if_absent`]: add
/// `candidate` only when nothing already uses its name. Pulled out as a
/// plain function over a borrowed slice — rather than inlined into the
/// `AppHandle`-shaped function below — for the same reason [`aggregate_tools`]
/// and [`find_tool`] are: it is the entire decision, and it is trivially
/// unit-testable without standing up a Tauri app just to prove that an
/// existing entry (enabled or not) is left alone.
fn should_register(existing: &[McpServerConfig], candidate: &McpServerConfig) -> bool {
    !existing.iter().any(|c| c.name == candidate.name)
}

/// Register `config` as a new server if, and only if, no server with that
/// name is already configured. Returns `Ok(true)` when a new config was
/// written (and connected), `Ok(false)` when a server with this name already
/// existed and was left completely untouched — not reconnected, not
/// re-enabled, not compared against `config` for drift.
///
/// # Why "already exists" always wins, no matter what it says
///
/// This must never overwrite an existing entry — not its command, not its
/// args, and especially not `enabled`. A user who removed or disabled a
/// server made a choice, and a background detector re-enabling it on the
/// next launch would silently undo that choice. [`should_register`] is the
/// whole rule, and it does not look past the name.
///
/// One honest gap this does *not* close: if a user deletes the entry
/// outright (rather than disabling it), presence-vs-absence in the store is
/// the only signal this function — or anything else — has, so the next
/// launch's detection will see "no server named this" and add it back. Only
/// *disabling* sticks; a full "the user declined this, never offer it
/// again" memory would need a persisted marker this store does not have
/// today. Solving that felt like scope the caller did not ask for; this
/// comment is so the gap is a documented decision rather than a surprise.
///
/// # Why this is not a violation of the module header's point (a)
///
/// See the module header's addendum to point (a) — this function is the
/// exception it describes, and the reasoning lives there rather than being
/// duplicated here.
pub async fn register_server_if_absent<R: Runtime>(app: &AppHandle<R>, config: McpServerConfig) -> Res<bool> {
    ensure_managed(app);
    let mut configs = load_configs(app);
    if !should_register(&configs, &config) {
        return Ok(false);
    }
    configs.push(config.clone());
    save_configs(app, &configs)?;
    if config.enabled {
        connect_config(app, &config).await;
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------
//
// Not registered in `generate_handler!` from this file — see the crate
// owner's notes at the top of the module. Each one below is otherwise a
// complete, ordinary Tauri command.

/// Every configured server, connected or not, config merged with live
/// status where a server is currently registered.
#[tauri::command]
pub async fn mcp_list_servers<R: Runtime>(app: AppHandle<R>) -> Res<Vec<McpServerInfo>> {
    ensure_managed(&app);
    let configs = load_configs(&app);
    let rt = app.state::<McpRuntime>();
    let servers = rt.servers.read();
    Ok(configs
        .into_iter()
        .map(|c| servers.get(&c.name).map(|h| server_info(h)).unwrap_or_else(|| McpServerInfo::from_config_only(&c)))
        .collect())
}

/// A single server's current status, log tail included. Meant for polling
/// a "connecting…" state in the UI without re-fetching every server.
#[tauri::command]
pub async fn mcp_server_status<R: Runtime>(app: AppHandle<R>, name: String) -> Res<McpServerInfo> {
    ensure_managed(&app);
    {
        let rt = app.state::<McpRuntime>();
        let servers = rt.servers.read();
        if let Some(handle) = servers.get(&name) {
            return Ok(server_info(handle));
        }
    }
    load_configs(&app)
        .iter()
        .find(|c| c.name == name)
        .map(McpServerInfo::from_config_only)
        .ok_or_else(|| format!("No server named \"{name}\" is configured."))
}

/// Add a new server: persist its config, then connect it immediately — the
/// act of submitting this form *is* the explicit user consent the module
/// header's point (a) requires, the same way `widgets_create` shows its
/// window immediately rather than waiting for a second confirmation.
/// A failure to connect does not fail this call; it comes back as
/// [`ServerStatus::Unhealthy`] with a reason, exactly like any other
/// connect failure, so the UI has something to show either way.
#[tauri::command]
pub async fn mcp_add_server<R: Runtime>(
    app: AppHandle<R>,
    name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> Res<McpServerInfo> {
    ensure_managed(&app);
    let name = name.trim().to_string();
    if !valid_server_name(&name) {
        return Err(format!(
            "Server names may only use letters, numbers, `-` and `_`, up to {MAX_SERVER_NAME_LEN} characters."
        ));
    }
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("A server needs a command to run.".into());
    }

    let mut configs = load_configs(&app);
    if configs.iter().any(|c| c.name == name) {
        return Err(format!("A server named \"{name}\" already exists."));
    }
    let config = McpServerConfig { name, command, args, env, enabled: true };
    configs.push(config.clone());
    save_configs(&app, &configs)?;

    Ok(connect_config(&app, &config).await)
}

/// Update a server's command, arguments, environment, or enabled flag.
/// Always disconnects the previous process first — a config change (a
/// different command, different arguments) must never be applied to an
/// already-running process silently, since the whole point of showing a
/// server's command and arguments in the UI is that they describe what is
/// actually running.
#[tauri::command]
pub async fn mcp_update_server<R: Runtime>(
    app: AppHandle<R>,
    name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    enabled: bool,
) -> Res<McpServerInfo> {
    ensure_managed(&app);
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("A server needs a command to run.".into());
    }

    let mut configs = load_configs(&app);
    let Some(existing) = configs.iter_mut().find(|c| c.name == name) else {
        return Err(format!("No server named \"{name}\" is configured."));
    };
    existing.command = command;
    existing.args = args;
    existing.env = env;
    existing.enabled = enabled;
    let config = existing.clone();
    save_configs(&app, &configs)?;

    forget(&app, &name).await;
    if config.enabled {
        Ok(connect_config(&app, &config).await)
    } else {
        Ok(McpServerInfo::from_config_only(&config))
    }
}

/// Forget a server entirely: stop it if running, and delete its config.
#[tauri::command]
pub async fn mcp_remove_server<R: Runtime>(app: AppHandle<R>, name: String) -> Res<()> {
    ensure_managed(&app);
    forget(&app, &name).await;
    let mut configs = load_configs(&app);
    configs.retain(|c| c.name != name);
    save_configs(&app, &configs)
}

/// Explicitly (re)connect a configured server — a "Retry" action for one
/// that is unhealthy, or a way to bring an `enabled: false` server up
/// without flipping its config.
#[tauri::command]
pub async fn mcp_connect_server<R: Runtime>(app: AppHandle<R>, name: String) -> Res<McpServerInfo> {
    ensure_managed(&app);
    let config = load_configs(&app)
        .into_iter()
        .find(|c| c.name == name)
        .ok_or_else(|| format!("No server named \"{name}\" is configured."))?;
    Ok(connect_config(&app, &config).await)
}

/// Stop a server without forgetting its config, so it can be reconnected
/// later without re-entering its command and arguments.
#[tauri::command]
pub async fn mcp_disconnect_server<R: Runtime>(app: AppHandle<R>, name: String) -> Res<McpServerInfo> {
    ensure_managed(&app);
    forget(&app, &name).await;
    load_configs(&app)
        .iter()
        .find(|c| c.name == name)
        .map(McpServerInfo::from_config_only)
        .ok_or_else(|| format!("No server named \"{name}\" is configured."))
}

/// Every tool exposed by every currently-`Ready` server, namespaced by
/// server — what the agent layer builds its function-calling list from.
#[tauri::command]
pub async fn mcp_list_tools<R: Runtime>(app: AppHandle<R>) -> Res<Vec<McpTool>> {
    ensure_managed(&app);
    let rt = app.state::<McpRuntime>();
    let servers = rt.servers.read();
    let ready: Vec<(String, Vec<RawTool>)> = servers
        .values()
        .filter(|h| matches!(*h.status.read(), ServerStatus::Ready))
        .map(|h| (h.config.name.clone(), h.tools.read().clone()))
        .collect();
    drop(servers);

    Ok(aggregate_tools(ready.iter().map(|(name, tools)| (name.as_str(), tools.as_slice()))))
}

/// Call one namespaced tool. `tool_id` is whatever [`mcp_list_tools`]
/// handed back as [`McpTool::id`]; `arguments` is sent to the server
/// verbatim. Per the module header's point (b), this does not itself gate
/// the call behind a confirmation prompt — the caller (the agent loop) is
/// expected to have already shown the tool and these exact arguments to the
/// user, the same discipline `AgentStep::AwaitingApproval` already applies
/// to computer-use actions.
#[tauri::command]
pub async fn mcp_call_tool<R: Runtime>(app: AppHandle<R>, tool_id: String, arguments: Option<Value>) -> Res<McpToolCallOutcome> {
    ensure_managed(&app);
    let rt = app.state::<McpRuntime>();

    let (handle, original_name) = {
        let servers = rt.servers.read();
        let snapshot: Vec<(String, Vec<RawTool>)> = servers.values().map(|h| (h.config.name.clone(), h.tools.read().clone())).collect();
        let Some((server_name, tool)) = find_tool(snapshot.iter().map(|(n, t)| (n.as_str(), t.as_slice())), &tool_id) else {
            return Err(format!("\"{tool_id}\" is not a known tool — the server it belongs to may be disconnected."));
        };
        let Some(handle) = servers.get(&server_name).cloned() else {
            return Err(format!("\"{tool_id}\" is not a known tool."));
        };
        (handle, tool.name)
    };

    let ready = matches!(*handle.status.read(), ServerStatus::Ready);
    if !ready {
        return Err(format!("\"{}\" is not connected right now.", handle.config.name));
    }

    let mut conn_guard = handle.conn.lock().await;
    let Some(conn) = conn_guard.as_mut() else {
        return Err(format!("\"{}\" is not connected right now.", handle.config.name));
    };
    let result = conn.call_tool(&original_name, arguments.unwrap_or_else(|| json!({})), TOOL_CALL_TIMEOUT).await;
    drop(conn_guard);

    match result {
        Ok(raw) => Ok(McpToolCallOutcome {
            server: handle.config.name.clone(),
            tool: original_name,
            is_error: raw.is_error,
            text: extract_text(&raw.content),
            content: raw.content,
        }),
        Err(e) => Err(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// No test here spawns a real process or touches the network. `Connection`
// is generic over the transport specifically so its handshake, framing, and
// error-mapping logic can be driven by an in-memory `tokio::io::duplex`
// pair playing the part of a fake server — see the module header.

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, split, DuplexStream, ReadHalf, WriteHalf};

    type TestConn = Connection<BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>>;

    /// Wire up a `Connection` on one end of an in-memory pipe and hand back
    /// the other end's reader/writer halves for a test to play "fake
    /// server" with.
    fn harness() -> (TestConn, BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>) {
        let (client_io, server_io) = duplex(8192);
        let (c_read, c_write) = split(client_io);
        let (s_read, s_write) = split(server_io);
        (Connection::new(BufReader::new(c_read), c_write), BufReader::new(s_read), s_write)
    }

    async fn recv_line(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read a line from the fake server's inbox");
        serde_json::from_str(line.trim()).expect("the client always writes valid JSON")
    }

    async fn send_line(writer: &mut WriteHalf<DuplexStream>, v: Value) {
        let mut bytes = serde_json::to_vec(&v).unwrap();
        bytes.push(b'\n');
        writer.write_all(&bytes).await.unwrap();
        writer.flush().await.unwrap();
    }

    fn blank_tool(name: &str) -> RawTool {
        RawTool { name: name.into(), title: None, description: Some("d".into()), input_schema: json!({"type":"object"}), output_schema: None }
    }

    // -- Framing --------------------------------------------------------

    #[test]
    fn a_value_containing_a_newline_still_serializes_to_one_physical_line() {
        // The spec forbids embedded newlines inside a message; this is what
        // guarantees it rather than assuming it.
        let v = json!({"jsonrpc":"2.0","id":1,"method":"x","params":{"note":"line one\nline two"}});
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s.matches('\n').count(), 0, "serde_json escapes embedded newlines, never emits a raw byte");
    }

    #[test]
    fn classify_recognises_a_response_by_id_without_method() {
        let msg: RpcMessageIn = serde_json::from_value(json!({"jsonrpc":"2.0","id":7,"result":{}})).unwrap();
        assert!(matches!(classify(&msg), Incoming::Response(7)));
    }

    #[test]
    fn classify_recognises_a_notification_by_method_without_id() {
        let msg: RpcMessageIn = serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})).unwrap();
        assert!(matches!(classify(&msg), Incoming::Notification(_)));
    }

    #[test]
    fn classify_recognises_a_server_request_by_both_id_and_method() {
        let msg: RpcMessageIn = serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"sampling/createMessage"})).unwrap();
        assert!(matches!(classify(&msg), Incoming::ServerRequest(_)));
    }

    #[test]
    fn classify_treats_neither_id_nor_method_as_malformed() {
        let msg: RpcMessageIn = serde_json::from_value(json!({"jsonrpc":"2.0"})).unwrap();
        assert!(matches!(classify(&msg), Incoming::Malformed));
    }

    #[test]
    fn a_string_id_does_not_masquerade_as_a_matching_numeric_one() {
        // This client only ever sends numeric ids; a server that echoes a
        // string back is misbehaving, and that must surface as an error
        // rather than silently failing to match anything.
        let msg: RpcMessageIn = serde_json::from_value(json!({"jsonrpc":"2.0","id":"7","result":{}})).unwrap();
        assert!(matches!(classify(&msg), Incoming::Malformed));
    }

    // -- Error mapping ----------------------------------------------------

    #[test]
    fn a_result_only_response_finishes_ok() {
        let msg: RpcMessageIn = serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}})).unwrap();
        assert_eq!(finish_response(msg).unwrap(), json!({"tools":[]}));
    }

    #[test]
    fn an_error_only_response_becomes_a_protocol_error_not_ok() {
        let msg: RpcMessageIn =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"Unknown tool: ghost"}})).unwrap();
        match finish_response(msg).unwrap_err() {
            McpError::Protocol { code, message } => {
                assert_eq!(code, -32602);
                assert!(message.contains("Unknown tool"));
            }
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[test]
    fn a_response_with_both_result_and_error_is_rejected_rather_than_guessed_at() {
        let msg: RpcMessageIn = serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"m"}})).unwrap();
        assert!(matches!(finish_response(msg).unwrap_err(), McpError::InvalidJson { .. }));
    }

    #[test]
    fn declares_tools_reads_the_capabilities_object_correctly() {
        assert!(declares_tools(&json!({"tools": {"listChanged": true}})));
        assert!(declares_tools(&json!({"tools": {}})));
        assert!(!declares_tools(&json!({"resources": {}})));
        assert!(!declares_tools(&Value::Null));
    }

    // -- Handshake sequencing ---------------------------------------------

    #[tokio::test]
    async fn handshake_then_tools_list_follows_the_spec_sequence_with_pagination() {
        let (mut conn, mut srv_r, mut srv_w) = harness();

        let server = async {
            let init_req = recv_line(&mut srv_r).await;
            assert_eq!(init_req["method"], "initialize");
            assert_eq!(init_req["params"]["protocolVersion"], PROTOCOL_VERSION);
            send_line(
                &mut srv_w,
                json!({
                    "jsonrpc":"2.0","id": init_req["id"],
                    "result": {
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name":"fake","version":"0.1"}
                    }
                }),
            )
            .await;

            // Per spec the client MUST send `notifications/initialized`
            // next, with no `id` at all.
            let initialized = recv_line(&mut srv_r).await;
            assert_eq!(initialized["method"], "notifications/initialized");
            assert!(initialized.get("id").is_none());

            let list_req = recv_line(&mut srv_r).await;
            assert_eq!(list_req["method"], "tools/list");
            send_line(
                &mut srv_w,
                json!({
                    "jsonrpc":"2.0","id": list_req["id"],
                    "result": {"tools":[{"name":"search","description":"Search","inputSchema":{"type":"object"}}], "nextCursor":"page2"}
                }),
            )
            .await;

            let list_req2 = recv_line(&mut srv_r).await;
            assert_eq!(list_req2["params"]["cursor"], "page2");
            send_line(
                &mut srv_w,
                json!({
                    "jsonrpc":"2.0","id": list_req2["id"],
                    "result": {"tools":[{"name":"fetch","description":"Fetch","inputSchema":{"type":"object"}}]}
                }),
            )
            .await;
        };

        let client = async {
            let identity = conn.initialize(HANDSHAKE_TIMEOUT).await.unwrap();
            assert_eq!(identity.protocol_version, PROTOCOL_VERSION);
            conn.list_all_tools(TOOLS_LIST_TIMEOUT).await.unwrap()
        };

        let (_, tools) = futures::join!(server, client);
        let names: Vec<_> = tools.into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["search".to_string(), "fetch".to_string()]);
    }

    #[tokio::test]
    async fn a_server_without_the_tools_capability_is_never_asked_to_list_them() {
        let (mut conn, mut srv_r, mut srv_w) = harness();

        let server = async {
            let init_req = recv_line(&mut srv_r).await;
            send_line(
                &mut srv_w,
                json!({
                    "jsonrpc":"2.0","id": init_req["id"],
                    "result": {"protocolVersion": PROTOCOL_VERSION, "capabilities": {"resources": {}}, "serverInfo": {"name":"fake","version":"0"}}
                }),
            )
            .await;
            // If the client sent tools/list here despite no `tools`
            // capability, this recv would hang forever and the test's own
            // outer timeout below would catch it.
        };
        let client = async {
            let init = conn.initialize(HANDSHAKE_TIMEOUT).await.unwrap();
            assert!(!declares_tools(&init.capabilities));
        };
        futures::join!(server, client);
    }

    #[tokio::test]
    async fn an_unsupported_protocol_version_is_rejected_not_silently_accepted() {
        let (mut conn, mut srv_r, mut srv_w) = harness();
        let server = async {
            let req = recv_line(&mut srv_r).await;
            send_line(
                &mut srv_w,
                json!({"jsonrpc":"2.0","id": req["id"], "result": {"protocolVersion": "1999-01-01", "capabilities": {}}}),
            )
            .await;
        };
        let client = async { conn.initialize(HANDSHAKE_TIMEOUT).await };
        let (_, result) = futures::join!(server, client);
        match result.unwrap_err() {
            McpError::UnsupportedVersion { offered } => assert_eq!(offered, "1999-01-01"),
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_server_that_never_answers_times_out_instead_of_hanging() {
        let (mut conn, _srv_r, _srv_w) = harness();
        // `_srv_r`/`_srv_w` stay in scope so the pipe is open but silent —
        // this is specifically testing the timeout path, not EOF.
        let started = std::time::Instant::now();
        let err = conn.initialize(Duration::from_millis(50)).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2), "a deadline must actually be enforced, not just documented");
        assert!(matches!(err, McpError::Timeout { .. }));
    }

    #[tokio::test]
    async fn the_server_vanishing_mid_handshake_is_reported_as_closed_not_a_hang() {
        let (mut conn, mut srv_r, srv_w) = harness();
        let server = async move {
            let _req = recv_line(&mut srv_r).await;
            drop(srv_r);
            drop(srv_w);
        };
        let client = async { conn.initialize(Duration::from_secs(2)).await };
        let (_, result) = futures::join!(server, client);
        assert!(matches!(result.unwrap_err(), McpError::Closed));
    }

    #[tokio::test]
    async fn garbage_from_the_server_is_reported_not_panicked_on() {
        let (mut conn, mut srv_r, mut srv_w) = harness();
        let server = async {
            let _req = recv_line(&mut srv_r).await;
            srv_w.write_all(b"not json at all\n").await.unwrap();
            srv_w.flush().await.unwrap();
        };
        let client = async { conn.initialize(Duration::from_secs(2)).await };
        let (_, result) = futures::join!(server, client);
        assert!(matches!(result.unwrap_err(), McpError::InvalidJson { .. }));
    }

    #[tokio::test]
    async fn a_stray_notification_before_the_real_response_is_skipped_not_mistaken_for_it() {
        let (mut conn, mut srv_r, mut srv_w) = harness();
        let server = async {
            let req = recv_line(&mut srv_r).await;
            send_line(&mut srv_w, json!({"jsonrpc":"2.0","method":"notifications/message","params":{"data":"hi"}})).await;
            send_line(
                &mut srv_w,
                json!({"jsonrpc":"2.0","id": req["id"], "result": {"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}}),
            )
            .await;
        };
        let client = async { conn.initialize(Duration::from_secs(2)).await };
        let (_, result) = futures::join!(server, client);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn a_server_that_never_stops_paginating_is_capped_not_looped_forever() {
        let (mut conn, mut srv_r, mut srv_w) = harness();
        // Exactly `MAX_TOOL_LIST_PAGES`, not more.
        //
        // Every page this serves still advertises another `nextCursor`, so from
        // the client's side the chain is endless and the cap is what stops it —
        // which is the property under test. Serving *extra* pages would leave
        // this future blocked in `recv_line` waiting for requests a correctly
        // capped client will never send, and `join!` waits for both halves, so
        // the test would hang on its own fixture rather than fail. It did: this
        // one test took over a minute and made the whole suite unusable.
        let server = async {
            for _ in 0..MAX_TOOL_LIST_PAGES {
                let req = recv_line(&mut srv_r).await;
                send_line(
                    &mut srv_w,
                    json!({"jsonrpc":"2.0","id": req["id"], "result": {"tools": [], "nextCursor": "more"}}),
                )
                .await;
            }
        };
        let client = async { conn.list_all_tools(Duration::from_secs(2)).await };
        let (_, result) = futures::join!(server, client);
        assert!(result.is_err(), "an endless cursor chain must be capped, not followed forever");
    }

    #[tokio::test]
    async fn an_unknown_tool_call_is_a_protocol_error_distinct_from_a_tool_failure() {
        let (mut conn, mut srv_r, mut srv_w) = harness();
        let server = async {
            let req = recv_line(&mut srv_r).await;
            assert_eq!(req["method"], "tools/call");
            send_line(&mut srv_w, json!({"jsonrpc":"2.0","id": req["id"], "error": {"code": -32602, "message": "Unknown tool: ghost"}})).await;
        };
        let client = async { conn.call_tool("ghost", json!({}), Duration::from_secs(2)).await };
        let (_, result) = futures::join!(server, client);
        match result.unwrap_err() {
            McpError::Protocol { code, .. } => assert_eq!(code, -32602),
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_tool_that_fails_reports_is_error_rather_than_a_protocol_error() {
        let (mut conn, mut srv_r, mut srv_w) = harness();
        let server = async {
            let req = recv_line(&mut srv_r).await;
            send_line(
                &mut srv_w,
                json!({"jsonrpc":"2.0","id": req["id"], "result": {"content":[{"type":"text","text":"boom"}], "isError": true}}),
            )
            .await;
        };
        let client = async { conn.call_tool("flaky", json!({}), Duration::from_secs(2)).await };
        let (_, result) = futures::join!(server, client);
        let raw = result.unwrap();
        assert!(raw.is_error);
        assert_eq!(extract_text(&raw.content), "boom");
    }

    // -- Namespacing --------------------------------------------------------

    #[test]
    fn namespacing_keeps_same_named_tools_on_different_servers_distinct() {
        let weather = [blank_tool("search")];
        let files = [blank_tool("search")];
        let tools = aggregate_tools([("weather", weather.as_slice()), ("files", files.as_slice())]);
        let ids: Vec<_> = tools.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"weather__search".to_string()));
        assert!(ids.contains(&"files__search".to_string()));
    }

    #[test]
    fn find_tool_resolves_by_recomputed_id_not_by_splitting_the_string() {
        // A server named "a" with a tool called "b__c" renders the same
        // joined string ("a__b__c") that a server named "a__b" with a tool
        // called "c" would. Resolution must not care: it recomputes the id
        // from each candidate pair rather than parsing the id apart, so it
        // only ever matches the pair that actually produced it.
        let odd = [blank_tool("b__c")];
        let entries = [("a", odd.as_slice())];
        let (server, tool) = find_tool(entries, "a__b__c").expect("should resolve via recomputation");
        assert_eq!(server, "a");
        assert_eq!(tool.name, "b__c");
    }

    #[test]
    fn find_tool_returns_none_for_an_id_nothing_produces() {
        let tools = [blank_tool("search")];
        let entries = [("weather", tools.as_slice())];
        assert!(find_tool(entries, "files__search").is_none());
    }

    #[test]
    fn extract_text_joins_only_text_blocks_and_ignores_the_rest() {
        let content = vec![
            json!({"type":"text","text":"first"}),
            json!({"type":"image","data":"...","mimeType":"image/png"}),
            json!({"type":"text","text":"second"}),
        ];
        assert_eq!(extract_text(&content), "first\nsecond");
    }

    #[test]
    fn extract_text_of_no_text_blocks_is_empty_not_a_panic() {
        let content = vec![json!({"type":"image","data":"...","mimeType":"image/png"})];
        assert_eq!(extract_text(&content), "");
    }

    // -- Config validation --------------------------------------------------

    #[test]
    fn server_names_accept_the_conservative_charset() {
        assert!(valid_server_name("weather-mcp"));
        assert!(valid_server_name("files_2"));
        assert!(!valid_server_name(""));
        assert!(!valid_server_name("has space"));
        assert!(!valid_server_name("has/slash"));
        assert!(!valid_server_name(&"x".repeat(MAX_SERVER_NAME_LEN + 1)));
    }

    // -- Idempotent auto-registration ---------------------------------------
    //
    // `register_server_if_absent` itself needs a real `AppHandle` (it reads
    // and writes the store), which nothing in this test module stands up —
    // see the file header's testing philosophy. `should_register` is the
    // entire decision pulled out specifically so it does not need one.

    fn stub_config(name: &str, enabled: bool) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            command: "/usr/bin/true".into(),
            args: Vec::new(),
            env: HashMap::new(),
            enabled,
        }
    }

    #[test]
    fn an_unconfigured_name_should_be_registered() {
        let existing: Vec<McpServerConfig> = Vec::new();
        assert!(should_register(&existing, &stub_config("cua", true)));
    }

    #[test]
    fn a_name_already_configured_is_never_registered_again() {
        let existing = vec![stub_config("cua", true)];
        assert!(!should_register(&existing, &stub_config("cua", true)));
    }

    #[test]
    fn a_disabled_existing_entry_blocks_registration_exactly_like_an_enabled_one() {
        // The critical case: a user who turned "cua" off must never have it
        // silently reappear as enabled because a background detector ran
        // again on the next launch.
        let existing = vec![stub_config("cua", false)];
        let candidate = stub_config("cua", true);
        assert!(!should_register(&existing, &candidate));
    }

    #[test]
    fn an_unrelated_existing_server_does_not_block_a_different_name() {
        let existing = vec![stub_config("weather", true)];
        assert!(should_register(&existing, &stub_config("cua", true)));
    }
}
