//! Finding AI runtimes that are already on this machine.
//!
//! Every local runtime worth supporting speaks the OpenAI `/v1` dialect, so
//! detection is one shape repeated: `GET {base}/models` on the port that
//! runtime is known for. A response means it is installed *and* serving, which
//! is the only state Caduceus can actually connect to — an installed-but-stopped
//! server is indistinguishable from an absent one over HTTP, and telling someone
//! "found LM Studio" when nothing will answer is worse than saying nothing.
//!
//! Probes run concurrently with a short timeout because this sits behind a
//! button someone is waiting on: the whole scan should cost about as long as
//! the slowest single probe, not the sum of them.

use std::time::Duration;

use futures::future::join_all;
use serde::Serialize;

/// How long a local server gets to answer before we treat it as absent.
/// Generous for a loopback request, short enough that five dead ports do not
/// make the button feel broken.
const PROBE_TIMEOUT_SECS: u64 = 3;

struct Candidate {
    id: &'static str,
    display_name: &'static str,
    /// OpenAI-compatible base, including the `/v1`.
    base_url: &'static str,
    /// What to do when it is not running. Shown verbatim in Settings.
    hint: &'static str,
}

/// Ports are the documented defaults for each project. A runtime moved to a
/// custom port will not be found here — that is what the manual "add a backend"
/// form is for, and pretending otherwise would mean port-scanning the machine.
const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "ollama",
        display_name: "Ollama",
        base_url: "http://localhost:11434/v1",
        hint: "Install from ollama.com, then run `ollama serve`.",
    },
    Candidate {
        id: "lmstudio",
        display_name: "LM Studio",
        base_url: "http://localhost:1234/v1",
        hint: "In LM Studio: Developer → Start Server.",
    },
    Candidate {
        id: "llamacpp",
        display_name: "llama.cpp",
        base_url: "http://localhost:8080/v1",
        hint: "Run `llama-server --port 8080`.",
    },
    Candidate {
        id: "jan",
        display_name: "Jan",
        base_url: "http://localhost:1337/v1",
        hint: "In Jan: Settings → Local API Server → Start.",
    },
    Candidate {
        id: "vllm",
        display_name: "vLLM",
        base_url: "http://localhost:8000/v1",
        hint: "Run `vllm serve <model>`.",
    },
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedProvider {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    /// True when the server answered. False entries are kept so the UI can say
    /// "not running" with a hint rather than silently omitting them.
    pub running: bool,
    pub models: Vec<String>,
    pub detail: String,
}

/// Everything the "Configure AI" scan turns up.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAiScan {
    pub providers: Vec<DetectedProvider>,
    pub hermes: super::hermes::HermesStatus,
}

/// Probe every known runtime plus Hermes, concurrently.
pub async fn scan() -> LocalAiScan {
    let (providers, hermes) = futures::join!(
        join_all(CANDIDATES.iter().map(probe)),
        super::hermes::status()
    );

    LocalAiScan { providers, hermes }
}

async fn probe(candidate: &'static Candidate) -> DetectedProvider {
    let models = list_models(candidate.base_url).await;

    let (running, detail) = match &models {
        Some(found) if found.is_empty() => (
            true,
            format!(
                "Running, but no models are loaded. Pull one first (e.g. `ollama pull qwen3:1.7b` for {}).",
                candidate.display_name
            ),
        ),
        Some(found) => (
            true,
            format!(
                "Running with {} model{}.",
                found.len(),
                if found.len() == 1 { "" } else { "s" }
            ),
        ),
        None => (false, format!("Not running. {}", candidate.hint)),
    };

    DetectedProvider {
        id: candidate.id.to_string(),
        display_name: candidate.display_name.to_string(),
        base_url: candidate.base_url.to_string(),
        running,
        models: models.unwrap_or_default(),
        detail,
    }
}

/// `GET {base}/models`, returning `None` when nothing answered.
///
/// `Some(vec![])` and `None` mean different things here — served-but-empty
/// versus not served at all — so this deliberately does not collapse to a
/// plain `Vec`.
async fn list_models(base_url: &str) -> Option<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;

    let response = client.get(format!("{base_url}/models")).send().await.ok()?;
    if !response.status().is_success() {
        // Something is listening and it is not what we expected. Report it as
        // absent rather than offering a connection that will fail on first use.
        return None;
    }

    let json: serde_json::Value = response.json().await.ok()?;
    let mut models: Vec<String> = json
        .get("data")
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    models.sort();
    models.dedup();
    Some(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_candidate_has_a_versioned_base_and_a_hint() {
        for c in CANDIDATES {
            assert!(
                c.base_url.ends_with("/v1"),
                "{} must point at an OpenAI-compatible /v1 base",
                c.id
            );
            assert!(!c.hint.is_empty(), "{} needs a not-running hint", c.id);
        }
    }

    #[test]
    fn candidate_ids_and_ports_are_unique() {
        let mut ids: Vec<_> = CANDIDATES.iter().map(|c| c.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate candidate id");

        let mut urls: Vec<_> = CANDIDATES.iter().map(|c| c.base_url).collect();
        urls.sort_unstable();
        urls.dedup();
        assert_eq!(urls.len(), count, "two runtimes claim the same port");
    }
}
