//! The Hermes Agent backend — Caduceus's default AI.
//!
//! [Hermes Agent](https://github.com/NousResearch/hermes-agent) is Nous
//! Research's open-source agent. It already solves the hard parts Caduceus
//! would otherwise have to reimplement: model routing across providers, tool
//! calling, memory across sessions, and screen control via its `computer_use`
//! toolset. Caduceus drives it rather than duplicating it — which is why there
//! is no screen-capture or input-simulation code in this repo any more.
//!
//! # Integration surface
//!
//! Hermes exposes no local HTTP API, so Caduceus shells out to the `hermes`
//! binary:
//!
//! | what              | command                                      |
//! |-------------------|----------------------------------------------|
//! | one-shot chat     | `hermes -z "<prompt>"`                       |
//! | screen control    | `hermes -z --yolo -t computer_use "<task>"`  |
//! | health check      | `hermes status`                              |
//!
//! `-z` is Hermes' scripting mode: one prompt in, the final reply out on
//! stdout, nothing else. That makes it trivially safe to parse.
//!
//! # Finding the binary
//!
//! A GUI app launched from Finder inherits almost nothing from your shell — in
//! particular not a `PATH` containing `~/.local/bin`, which is where the Hermes
//! installer puts things. Every candidate location is therefore probed
//! explicitly; see [`find_hermes`].

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::backend::{AgentBackend, AgentLoopContext};
use super::types::{
    AgentError, AgentOutcome, AgentResponse, AgentResult, AgentStep, Message, Role, StopReason,
};
use crate::settings::BackendConfig;

pub struct HermesBackend;

const PROVIDER: &str = "Hermes Agent";

/// The toolset that grants screen control, per `hermes computer-use --help`.
const COMPUTER_USE_TOOLSET: &str = "computer_use";

/// Where the Hermes installer is known to put the binary.
///
/// Ordered by likelihood. `$PATH` is consulted first for anyone who installed
/// it somewhere unusual, but is not relied upon.
const CANDIDATE_PATHS: &[&str] = &[
    ".local/bin/hermes",
    ".hermes/bin/hermes",
    ".local/share/hermes/bin/hermes",
];

const SYSTEM_PATHS: &[&str] = &[
    "/usr/local/bin/hermes",
    "/opt/homebrew/bin/hermes",
    "/usr/bin/hermes",
];

/// Locate the `hermes` binary, or `None` if it is not installed.
pub fn find_hermes() -> Option<PathBuf> {
    // 1. An explicit override always wins.
    if let Some(custom) = std::env::var_os("CADUCEUS_HERMES_BIN") {
        let path = PathBuf::from(custom);
        if path.is_file() {
            return Some(path);
        }
    }

    // 2. Anything already on PATH (true when Caduceus is run from a terminal).
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("hermes");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // 3. The standard install locations, since a Finder-launched app has a
    //    minimal PATH that contains none of them.
    if let Some(home) = dirs::home_dir() {
        for relative in CANDIDATE_PATHS {
            let candidate = home.join(relative);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for absolute in SYSTEM_PATHS {
        let candidate = PathBuf::from(absolute);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

/// What Settings shows about the Hermes installation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesStatus {
    pub installed: bool,
    /// Absolute path to the binary, when found.
    pub path: Option<String>,
    /// First line of `hermes --version`.
    pub version: Option<String>,
    /// The model Hermes is configured to use, parsed from `hermes status`.
    pub model: Option<String>,
    pub provider: Option<String>,
    /// True when a model is configured. Hermes can be installed but unset up.
    pub configured: bool,
    /// One-line, actionable summary for the UI.
    pub detail: String,
}

/// Probe the local Hermes installation. Never fails — an absent Hermes is a
/// normal state that the UI explains rather than an error.
pub async fn status() -> HermesStatus {
    let Some(path) = find_hermes() else {
        return HermesStatus {
            installed: false,
            path: None,
            version: None,
            model: None,
            provider: None,
            configured: false,
            detail: "Hermes Agent is not installed. Caduceus can install it for you, or run \
                     `curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash`."
                .into(),
        };
    };

    let version = run_capture(&path, &["--version"], 20)
        .await
        .ok()
        .and_then(|out| out.lines().next().map(str::to_string));

    // `hermes status` prints a boxed report; the two lines that matter are
    // "Model:" and "Provider:".
    let report = run_capture(&path, &["status"], 45).await.unwrap_or_default();
    let model = extract_field(&report, "Model:");
    let provider = extract_field(&report, "Provider:");
    let configured = model.is_some();

    let detail = if configured {
        format!(
            "Ready — using {}{}.",
            model.clone().unwrap_or_default(),
            provider
                .clone()
                .map(|p| format!(" via {p}"))
                .unwrap_or_default()
        )
    } else {
        "Hermes is installed but has no model configured. Run `hermes setup --portal` in a \
         terminal to connect one."
            .to_string()
    };

    HermesStatus {
        installed: true,
        path: Some(path.display().to_string()),
        version,
        model,
        provider,
        configured,
        detail,
    }
}

/// Pull `Label: value` out of Hermes' boxed status output, ignoring the box
/// drawing characters it pads lines with.
fn extract_field(report: &str, label: &str) -> Option<String> {
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .map(|value| {
            value
                .trim()
                .trim_end_matches(['│', '|', ' '])
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

#[async_trait]
impl AgentBackend for HermesBackend {
    fn id(&self) -> &str {
        "hermes"
    }

    fn display_name(&self) -> &str {
        "Hermes Agent"
    }

    fn supports_computer_use(&self) -> bool {
        true
    }

    async fn chat(&self, messages: Vec<Message>, config: &BackendConfig) -> AgentResult<AgentResponse> {
        let path = require_hermes()?;

        // `-z` takes a single prompt, so a multi-turn conversation is flattened
        // into one. Caduceus' palette is one-shot anyway; Hermes keeps its own
        // session memory across invocations, which is the better place for it.
        let prompt = flatten(&messages, &config.system_prompt);
        let mut args: Vec<String> = Vec::new();
        push_model_args(&mut args, config);
        args.push("-z".into());
        args.push(prompt);

        let text = run_capture_owned(&path, &args, config.timeout_secs).await?;
        let text = text.trim().to_string();

        // Hermes prints provider failures to stdout and still exits 0, so a
        // reply that is only an error line has to be surfaced as one.
        if let Some(err) = detect_provider_error(&text) {
            return Err(AgentError::Other(err));
        }
        if text.is_empty() {
            return Err(AgentError::Protocol {
                provider: PROVIDER.into(),
                detail: "Hermes returned nothing.".into(),
            });
        }

        Ok(AgentResponse {
            text,
            model: config.model.clone(),
            usage: None,
        })
    }

    async fn run_agent_loop(
        &self,
        task: &str,
        config: &BackendConfig,
        ctx: AgentLoopContext,
    ) -> AgentResult<AgentOutcome> {
        let path = require_hermes()?;
        let emit = &ctx.on_step;

        emit(AgentStep::Started {
            session_id: ctx.session_id.clone(),
            task: task.to_string(),
            backend: PROVIDER.into(),
            model: config.model.clone(),
        });

        // Nothing touches the machine until the user says so. Hermes' own
        // confirmations are bypassed with --yolo because Caduceus is running it
        // non-interactively, which makes this gate the only one there is.
        if !ctx
            .approval
            .request(&ctx.session_id, "control your mouse, keyboard and screen")
            .await
        {
            return Ok(finish(&ctx, 0, String::new(), StopReason::Declined));
        }

        let mut args: Vec<String> = Vec::new();
        push_model_args(&mut args, config);
        args.push("-t".into());
        args.push(COMPUTER_USE_TOOLSET.into());
        // Non-interactive: Hermes must not stop to ask for confirmation, since
        // there is no terminal attached to answer it.
        args.push("--yolo".into());
        args.push("-z".into());
        args.push(task.to_string());

        let mut child = tokio::process::Command::new(&path)
            .args(&args)
            .envs(child_env(&path))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AgentError::Other(format!("could not start Hermes: {e}")))?;

        let stdout = child.stdout.take().expect("stdout piped above");
        let mut lines = BufReader::new(stdout).lines();

        let mut steps: u32 = 0;
        let mut transcript = String::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(config.timeout_secs.max(60));

        loop {
            if ctx.cancel.is_cancelled() {
                let _ = child.kill().await;
                return Ok(finish(&ctx, steps, transcript, StopReason::UserStopped));
            }

            let next = tokio::time::timeout_at(deadline, lines.next_line()).await;
            match next {
                // Timed out overall.
                Err(_) => {
                    let _ = child.kill().await;
                    emit(AgentStep::Error {
                        message: format!("Hermes did not finish within {}s.", config.timeout_secs),
                    });
                    return Ok(finish(&ctx, steps, transcript, StopReason::MaxSteps));
                }
                Ok(Err(e)) => {
                    let _ = child.kill().await;
                    return Err(AgentError::Other(format!("could not read Hermes output: {e}")));
                }
                Ok(Ok(None)) => break, // stdout closed: the run is over
                Ok(Ok(Some(line))) => {
                    let line = line.trim_end().to_string();
                    if line.trim().is_empty() {
                        continue;
                    }
                    steps += 1;
                    emit(AgentStep::Thinking { text: line.clone() });
                    if !transcript.is_empty() {
                        transcript.push('\n');
                    }
                    transcript.push_str(&line);
                }
            }
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| AgentError::Other(format!("Hermes exited badly: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            emit(AgentStep::Error {
                message: if stderr.is_empty() {
                    format!("Hermes exited with status {}.", output.status)
                } else {
                    stderr.clone()
                },
            });
            return Ok(finish(&ctx, steps, transcript, StopReason::Error));
        }

        if let Some(err) = detect_provider_error(&transcript) {
            emit(AgentStep::Error { message: err });
            return Ok(finish(&ctx, steps, transcript, StopReason::Error));
        }

        Ok(finish(&ctx, steps, transcript, StopReason::Completed))
    }

    async fn test_connection(&self, _config: &BackendConfig) -> AgentResult<String> {
        let probe = status().await;
        if !probe.installed {
            return Err(AgentError::NotConfigured(probe.detail));
        }
        if !probe.configured {
            return Err(AgentError::NotConfigured(probe.detail));
        }
        Ok(probe.detail)
    }
}

fn finish(
    ctx: &AgentLoopContext,
    steps: u32,
    final_message: String,
    stop_reason: StopReason,
) -> AgentOutcome {
    let outcome = AgentOutcome {
        session_id: ctx.session_id.clone(),
        completed: stop_reason == StopReason::Completed,
        steps,
        final_message,
        stop_reason,
        usage: None,
    };
    (ctx.on_step)(AgentStep::Finished {
        outcome: outcome.clone(),
    });
    outcome
}

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

fn require_hermes() -> AgentResult<PathBuf> {
    find_hermes().ok_or_else(|| {
        AgentError::NotConfigured(
            "Hermes Agent is not installed.\n\nOpen Settings \u{2192} AI and press Install, or run:\n\
             curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash"
                .into(),
        )
    })
}

/// Environment for the child process.
///
/// The Hermes binary shells out to its own helpers (python, node, ripgrep), so
/// its directory and the usual user bin directories are prepended to whatever
/// `PATH` the app inherited — which, launched from Finder, is nearly empty.
fn child_env(hermes: &std::path::Path) -> Vec<(String, String)> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(dir) = hermes.parent() {
        parts.push(dir.display().to_string());
    }
    if let Some(home) = dirs::home_dir() {
        parts.push(home.join(".local/bin").display().to_string());
    }
    parts.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
            .iter()
            .map(|s| s.to_string()),
    );
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    vec![("PATH".to_string(), parts.join(":"))]
}

fn push_model_args(args: &mut Vec<String>, config: &BackendConfig) {
    // Blank means "whatever Hermes is already configured to use", which is the
    // right default: the user set it up once with `hermes setup`.
    if !config.model.trim().is_empty() {
        args.push("-m".into());
        args.push(config.model.trim().to_string());
    }
}

async fn run_capture(path: &std::path::Path, args: &[&str], timeout_secs: u64) -> AgentResult<String> {
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    run_capture_owned(path, &owned, timeout_secs).await
}

async fn run_capture_owned(
    path: &std::path::Path,
    args: &[String],
    timeout_secs: u64,
) -> AgentResult<String> {
    let fut = tokio::process::Command::new(path)
        .args(args)
        .envs(child_env(path))
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();

    let output = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs.clamp(5, 900)), fut)
        .await
        .map_err(|_| AgentError::Other(format!("Hermes timed out after {timeout_secs}s.")))?
        .map_err(|e| AgentError::Other(format!("could not run Hermes: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() && stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AgentError::Other(if stderr.is_empty() {
            format!("Hermes exited with status {}.", output.status)
        } else {
            stderr
        }));
    }
    Ok(stdout)
}

/// Flatten a conversation into one prompt for `-z`.
fn flatten(messages: &[Message], system_prompt: &str) -> String {
    let mut out = String::new();
    if !system_prompt.trim().is_empty() {
        out.push_str(system_prompt.trim());
        out.push_str("\n\n");
    }
    for m in messages {
        match m.role {
            Role::System => {
                out.push_str(m.content.trim());
                out.push_str("\n\n");
            }
            Role::User => {
                out.push_str(m.content.trim());
                out.push('\n');
            }
            Role::Assistant => {
                out.push_str("(previously answered: ");
                out.push_str(m.content.trim());
                out.push_str(")\n");
            }
        }
    }
    out.trim().to_string()
}

/// Hermes reports provider failures on stdout and still exits 0, so a reply
/// consisting only of an error has to be recognised rather than shown as an
/// answer.
fn detect_provider_error(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.lines().count() > 3 {
        return None;
    }
    const MARKERS: &[&str] = &[
        "API call failed",
        "Connection error",
        "No model configured",
        "authentication failed",
        "rate limit",
    ];
    let lower = trimmed.to_lowercase();
    MARKERS
        .iter()
        .any(|m| lower.contains(&m.to_lowercase()))
        .then(|| {
            format!(
                "{trimmed}\n\nThis came from Hermes, not Caduceus. Check its model with \
                 `hermes status`, or reconnect one with `hermes setup --portal`."
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_boxed_status_report() {
        let report = "\
┌───────────────────────────┐
│    Hermes Agent Status    │
└───────────────────────────┘

◆ Environment
  Project:      /Users/x/.hermes
  Model:        qwen3.5:9b
  Provider:     custom:local-(localhost:11434)
";
        assert_eq!(extract_field(report, "Model:").as_deref(), Some("qwen3.5:9b"));
        assert_eq!(
            extract_field(report, "Provider:").as_deref(),
            Some("custom:local-(localhost:11434)")
        );
        assert!(extract_field(report, "Nonexistent:").is_none());
    }

    #[test]
    fn status_fields_ignore_trailing_box_characters() {
        assert_eq!(
            extract_field("  Model:  gpt-4  │", "Model:").as_deref(),
            Some("gpt-4")
        );
    }

    #[test]
    fn empty_status_values_are_treated_as_absent() {
        assert!(extract_field("  Model:   \n", "Model:").is_none());
    }

    #[test]
    fn provider_failures_are_recognised_not_shown_as_answers() {
        let err = detect_provider_error("API call failed after 3 retries: Connection error.");
        assert!(err.is_some());
        assert!(err.unwrap().contains("hermes status"));
    }

    #[test]
    fn a_real_answer_mentioning_an_error_is_not_misread() {
        // Long prose that happens to contain the word is an answer, not a failure.
        let answer = "Here is what a connection error means:\n\
            line two\nline three\nline four\nline five";
        assert!(detect_provider_error(answer).is_none());
    }

    #[test]
    fn model_argument_is_omitted_when_blank() {
        let mut args = Vec::new();
        push_model_args(&mut args, &BackendConfig::default());
        assert!(args.is_empty(), "blank model must defer to Hermes' own config");

        let mut args = Vec::new();
        push_model_args(
            &mut args,
            &BackendConfig {
                model: "qwen3.5:9b".into(),
                ..Default::default()
            },
        );
        assert_eq!(args, vec!["-m".to_string(), "qwen3.5:9b".to_string()]);
    }

    #[test]
    fn flattening_puts_the_system_prompt_first() {
        let prompt = flatten(&[Message::user("hello")], "be terse");
        assert!(prompt.starts_with("be terse"));
        assert!(prompt.ends_with("hello"));
    }

    #[test]
    fn child_path_includes_the_binary_directory() {
        let path = child_env(std::path::Path::new("/Users/x/.local/bin/hermes"));
        let (key, value) = &path[0];
        assert_eq!(key, "PATH");
        assert!(value.starts_with("/Users/x/.local/bin"));
    }
}
