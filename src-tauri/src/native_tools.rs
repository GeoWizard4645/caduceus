//! A registry for **built-in** agent tools — compiled into Caduceus itself,
//! as opposed to [`crate::mcp`]'s tools, which live in a separate process
//! reached over stdio. `skills_list` / `skill_view` / `skill_manage` (see
//! [`crate::skills`]) are the first tenant; anything else the agent should be
//! able to call without the ceremony of an MCP server — a `memory` tool being
//! the next obvious candidate — registers here the same way.
//!
//! # Why this exists as its own module rather than living inside `skills`
//!
//! A tool-calling loop (see `agent::toolloop::run_tool_loop`) should be able
//! to hand a model *one* combined tool list without caring which tools came
//! from a spawned MCP server and which came from Rust functions in this
//! binary. That merge point needs a home neither `mcp` (a specific transport)
//! nor `skills` (a specific tool) should own, or the next built-in tool
//! module would have to choose between duplicating this plumbing or reaching
//! into `skills` for something that has nothing to do with skills. Putting it
//! here means every future built-in tool module — `skills`, and whatever
//! follows it — depends on this, and this depends on nothing.
//!
//! # Why a process-global registry rather than `app.manage()`-ed state
//!
//! Every other shared runtime in this codebase (`ClipboardStore`,
//! `UsageStore`, `AgentRuntime`, ...) is Tauri-managed state, reached via
//! `AppHandle::state()` inside a `#[tauri::command]`. This registry
//! deliberately is not, for one concrete reason: registration is static
//! (every entry is known at compile time, wired once in `setup()`) and
//! lookup has nothing to do with any particular window or webview — it is
//! consulted from the *agent tool loop*, which already juggles enough
//! generic-over-`Runtime` plumbing reaching into `mcp` and `openai`. A tool's
//! handler here is a plain closure over whatever state it needs (typically
//! just a resolved directory path — see [`crate::skills::register_native_tools`]
//! for the pattern), so nothing here needs an `AppHandle<R>` at all, and a
//! unit test can register a tool and call it with no Tauri app instance in
//! sight. [`std::sync::LazyLock`] (stable since Rust 1.80; this workspace
//! targets 1.82+) gives the same "exists for the life of the process"
//! guarantee `app.manage()` would, without the generic parameter.
//!
//! If a future tool genuinely needs per-window state (not just "a directory
//! under app data"), that is a sign it belongs behind its own
//! `#[tauri::command]` instead of in this registry — this module is
//! deliberately narrow: name, description, JSON Schema, synchronous handler.
//!
//! # Wiring into the tool loop (not done by this module)
//!
//! [`list`] returns the same `(name, description, input_schema)` shape
//! `agent::toolloop::build_tool_table` already extracts from
//! [`crate::mcp::McpTool`], so a future integration is a small merge, not a
//! redesign: fetch MCP tools as today, extend with `native_tools::list()`,
//! and on a tool call, try [`call`] before falling back to
//! `mcp::mcp_call_tool`. [`call`] is synchronous (every handler registered so
//! far is bounded filesystem I/O against a small directory) — a caller on an
//! async runtime should run it via `tokio::task::spawn_blocking` rather than
//! calling it directly from an async fn, the same way any other blocking
//! filesystem work would be scheduled.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use serde_json::Value;

/// A single built-in tool: what a model sees when deciding whether to call
/// it, plus the function that actually runs it.
pub struct NativeTool {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's arguments — same shape MCP tools and
    /// `openai::ToolSpec` already expect, so no translation is needed at the
    /// merge point described in the module doc.
    pub input_schema: Value,
    /// Synchronous by design — see the module doc's "Wiring into the tool
    /// loop" section for how a caller on an async runtime should invoke it.
    handler: Box<dyn Fn(Value) -> Result<Value, String> + Send + Sync>,
}

impl NativeTool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        handler: impl Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Self {
        Self { name: name.into(), description: description.into(), input_schema, handler: Box::new(handler) }
    }
}

/// The `list()`-facing view of a tool — everything except the handler, which
/// has no business leaving this module (it closes over paths and, unlike a
/// name/description/schema, is not something a caller should be able to hold
/// onto and call directly, bypassing whatever [`call`] decides to do around
/// dispatch in the future — logging, timing, a rename).
#[derive(Debug, Clone, serde::Serialize)]
pub struct NativeToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

static REGISTRY: LazyLock<RwLock<HashMap<String, NativeTool>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a tool, replacing anything already registered under the same
/// name.
///
/// A collision here is a programming error, not a runtime one — tool names
/// are literal strings chosen by the module that owns them, not user input —
/// so this logs loudly rather than returning a `Result` nobody would check.
/// `setup()` calling a registration function twice (e.g. a hot-reload path
/// added later) is the only realistic way to hit this.
pub fn register(tool: NativeTool) {
    let name = tool.name.clone();
    let mut registry = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    if registry.contains_key(&name) {
        log::warn!("native_tools: '{name}' registered twice; the newest registration wins");
    }
    registry.insert(name, tool);
}

/// Every registered tool's model-facing spec, in the shape a tool-calling
/// loop hands to a model. Order is not significant to callers today, but is
/// kept stable (sorted by name) so a rendered tool list — or a test
/// assertion against one — does not reshuffle between runs for no reason.
pub fn list() -> Vec<NativeToolSpec> {
    let registry = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
    let mut specs: Vec<NativeToolSpec> = registry
        .values()
        .map(|t| NativeToolSpec {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect();
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
}

/// Whether a tool by this name is registered — lets a future tool-loop
/// integration decide "mine or MCP's?" without paying for a failed [`call`].
pub fn is_registered(name: &str) -> bool {
    REGISTRY.read().unwrap_or_else(|e| e.into_inner()).contains_key(name)
}

/// Run a registered tool's handler with `args`.
///
/// `Err` covers both "no such tool" and the tool's own reported failure —
/// deliberately, unlike `mcp`'s split between a protocol error and a tool's
/// `isError` result (see `mcp`'s module doc). There is no protocol here to
/// fail independently of the tool: every handler is a plain Rust function
/// running in this process, so there is nothing an "unknown tool" error and
/// a handler's own `Err` need to be told apart *for* — both just become the
/// text a model reads back as its tool result.
pub fn call(name: &str, args: Value) -> Result<Value, String> {
    let registry = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
    let tool = registry.get(name).ok_or_else(|| format!("no native tool named '{name}'"))?;
    (tool.handler)(args)
}

/// Test-only helpers for working with the process-global registry safely.
///
/// `cargo test` runs every test in the crate in one process with no
/// ordering guarantee, and this registry is a `static` — so any two tests
/// that register a tool under the same name (or that need "nothing else is
/// registered right now") would otherwise race. That is not hypothetical:
/// `crate::skills::native`'s tests register the crate's *real*
/// `skills_list`/`skill_view`/`skill_manage` tools into this exact registry
/// to verify the wiring end-to-end, so this module's own tests below and
/// `skills::native`'s tests both need to serialize against each other, not
/// just against themselves. [`test_support::locked`] is one mutex shared
/// crate-wide for that; [`test_support::clear`] resets to a clean slate
/// while holding it.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Hold for the duration of any test that registers, lists, or calls
    /// tools — including via a module (like `skills::native`) that
    /// registers into this same registry as a side effect of what it is
    /// actually testing.
    pub(crate) fn locked() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drop every registration. Call only while holding [`locked`].
    pub(crate) fn clear() {
        super::REGISTRY.write().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{clear, locked};
    use super::*;
    use serde_json::json;

    #[test]
    fn a_registered_tool_is_listed_and_callable() {
        let _g = locked();
        clear();
        register(NativeTool::new("echo", "echoes its input", json!({"type": "object"}), |args| {
            Ok(args)
        }));

        let specs = list();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "echo");
        assert_eq!(specs[0].description, "echoes its input");
        assert!(is_registered("echo"));

        let result = call("echo", json!({"x": 1})).unwrap();
        assert_eq!(result, json!({"x": 1}));
    }

    #[test]
    fn calling_an_unknown_tool_is_an_error_not_a_panic() {
        let _g = locked();
        clear();
        let err = call("does-not-exist", json!({})).unwrap_err();
        assert!(err.contains("does-not-exist"));
    }

    #[test]
    fn a_handlers_own_error_propagates_as_is() {
        let _g = locked();
        clear();
        register(NativeTool::new("always_fails", "", json!({}), |_args| {
            Err("deliberate failure".to_string())
        }));
        let err = call("always_fails", json!(null)).unwrap_err();
        assert_eq!(err, "deliberate failure");
    }

    #[test]
    fn re_registering_a_name_replaces_the_handler() {
        let _g = locked();
        clear();
        register(NativeTool::new("v", "first", json!({}), |_| Ok(json!(1))));
        register(NativeTool::new("v", "second", json!({}), |_| Ok(json!(2))));

        let specs = list();
        assert_eq!(specs.len(), 1, "the second registration replaces the first, it does not add a second entry");
        assert_eq!(specs[0].description, "second");
        assert_eq!(call("v", json!(null)).unwrap(), json!(2));
    }

    #[test]
    fn list_is_sorted_by_name_regardless_of_registration_order() {
        let _g = locked();
        clear();
        register(NativeTool::new("zeta", "", json!({}), |_| Ok(json!(null))));
        register(NativeTool::new("alpha", "", json!({}), |_| Ok(json!(null))));
        register(NativeTool::new("mid", "", json!({}), |_| Ok(json!(null))));

        let specs = list();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mid", "zeta"]);
    }
}
