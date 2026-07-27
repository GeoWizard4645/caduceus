//! `ctx.fetch` — the only way out of the sandbox and onto the network.
//!
//! The webview cannot make this request itself. Its CSP allows `connect-src
//! 'self'` and nothing else, which is what stops an extension reaching the
//! network by finding some primitive the sandbox forgot to remove. So the
//! request comes here, where it is checked against the extension's header
//! first, and the bounds below apply to everything that gets through.
//!
//! What is *not* bounded is which host. `network` means the network; an
//! allow-list of domains an extension declares up front would be a nicer story
//! than it is a control, since the useful ones all end up asking for a domain
//! you have no way to audit anyway. The honest version is the one shown at
//! install time: this extension can talk to the internet, and the file is
//! sitting there for you to read.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Long enough for a slow API, short enough that a wedged host does not become
/// a spinner with no end.
const TIMEOUT: Duration = Duration::from_secs(90);

/// Responses past this are refused rather than truncated: half a JSON document
/// is worse than an error that says what happened.
const MAX_BODY: usize = 25 * 1024 * 1024;

/// Methods with ordinary request semantics. `CONNECT` and `TRACE` are not here
/// because nothing an extension does needs them.
const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    /// Pairs rather than a map, so a header may legitimately repeat.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResponse {
    pub ok: bool,
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Perform a request on an extension's behalf.
///
/// The caller has already checked that the extension asked for `network`.
pub async fn fetch(request: FetchRequest) -> Result<FetchResponse, String> {
    let url = reqwest::Url::parse(request.url.trim())
        .map_err(|_| format!("“{}” is not a URL ctx.fetch can use.", request.url.trim()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "ctx.fetch only speaks http and https. “{}” is not one of them.",
            url.scheme()
        ));
    }

    let method = request.method.unwrap_or_else(|| "GET".into()).to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return Err(format!("ctx.fetch does not send {method} requests."));
    }
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| "That is not an HTTP method.".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Could not start the request: {e}"))?;

    let mut req = client.request(method, url);
    for (name, value) in &request.headers {
        // A header this crate cannot represent is skipped rather than failing
        // the whole request: it is almost always a typo in one line of a
        // header object, and the request usually works without it.
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            req = req.header(name, value);
        }
    }
    if let Some(body) = request.body {
        req = req.body(body);
    }

    let response = req.send().await.map_err(describe)?;

    let status = response.status();
    let final_url = response.url().to_string();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (name.as_str().to_string(), value.to_str().unwrap_or_default().to_string())
        })
        .collect();

    // `content-length` is a claim, not a guarantee, so the check below is on
    // what actually arrived. It is worth reading first anyway: a server that
    // admits to sending 400 MB should not be given 400 MB of memory to prove it.
    if let Some(len) = response.content_length() {
        if len as usize > MAX_BODY {
            return Err(too_big());
        }
    }

    let bytes = response.bytes().await.map_err(describe)?;
    if bytes.len() > MAX_BODY {
        return Err(too_big());
    }

    Ok(FetchResponse {
        ok: status.is_success(),
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or_default().to_string(),
        url: final_url,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

fn too_big() -> String {
    format!(
        "That response is over {} MB. ctx.fetch is for API calls, not downloads.",
        MAX_BODY / (1024 * 1024)
    )
}

/// Turn a transport failure into something worth reading.
fn describe(error: reqwest::Error) -> String {
    if error.is_timeout() {
        return "That request timed out.".into();
    }
    if error.is_connect() {
        return "Could not reach that host. Check the address, and that you are online.".into();
    }
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str) -> FetchRequest {
        FetchRequest { url: url.into(), method: None, headers: Vec::new(), body: None }
    }

    async fn refuse(request: FetchRequest) -> String {
        fetch(request).await.unwrap_err()
    }

    /// `file://` would turn the network permission into a filesystem.
    #[tokio::test]
    async fn only_http_and_https_are_spoken() {
        assert!(refuse(request("file:///etc/passwd")).await.contains("http"));
        assert!(refuse(request("data:text/plain,hi")).await.contains("http"));
    }

    #[tokio::test]
    async fn a_non_url_says_so() {
        assert!(refuse(request("not a url")).await.contains("not a URL"));
    }

    #[tokio::test]
    async fn an_unusual_method_is_refused() {
        let mut req = request("https://example.com");
        req.method = Some("CONNECT".into());
        assert!(refuse(req).await.contains("CONNECT"));
    }
}
