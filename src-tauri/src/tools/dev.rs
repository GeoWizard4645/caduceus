//! The developer toolbox: encoders, generators, formatters and text surgery.
//!
//! # One command, not sixty
//!
//! Every tool here is reached through [`run`] with a [`ToolId`], rather than
//! sixty `#[tauri::command]` functions. The IPC surface stays one entry wide and
//! the closed enum means a webview cannot ask for a tool that does not exist —
//! the same "resolve by id, never by string" rule the shortcut runner follows.
//!
//! # Everything here is pure
//!
//! No tool in this file touches the filesystem, the network or a subprocess,
//! with the single exception of [`ToolId::Md5`] and friends, which shell out to
//! the hashing binaries macOS already ships rather than linking three crypto
//! crates into the binary for a feature most people use twice a year.

use serde::{Deserialize, Serialize};

/// Every tool the developer toolbox can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolId {
    // --- identifiers ---
    Uuid,
    UuidBatch,
    Ulid,
    NanoId,
    Password,

    // --- encoding ---
    Base64Encode,
    Base64Decode,
    Base64UrlEncode,
    Base64UrlDecode,
    HexEncode,
    HexDecode,
    UrlEncode,
    UrlDecode,
    HtmlEncode,
    HtmlDecode,

    // --- inspection ---
    JwtDecode,
    JsonFormat,
    JsonMinify,
    JsonEscape,

    // --- time ---
    TimestampNow,
    TimestampConvert,

    // --- text ---
    Lorem,
    Slugify,
    TextStats,
    SortLines,
    SortLinesDescending,
    DedupeLines,
    ReverseLines,
    ShuffleLines,
    NumberLines,
    JoinLines,
    TrimLines,
    CountOccurrences,

    // --- numbers and colour ---
    ColorConvert,
    NumberBase,
    RandomNumber,

    // --- hashes ---
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

/// What a tool produced.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub ok: bool,
    /// Heading for the output panel.
    pub title: String,
    /// The result itself. Rendered monospaced, and what gets copied.
    pub output: String,
    /// One line of context, or the reason it failed.
    pub message: String,
    /// Whether the palette should put `output` on the clipboard straight away.
    pub auto_copy: bool,
}

impl ToolResult {
    fn ok(title: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            ok: true,
            title: title.into(),
            output: output.into(),
            message: String::new(),
            auto_copy: false,
        }
    }

    fn copied(title: impl Into<String>, output: impl Into<String>) -> Self {
        Self { auto_copy: true, ..Self::ok(title, output) }
    }

    fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            title: String::new(),
            output: String::new(),
            message: message.into(),
            auto_copy: false,
        }
    }
}

/// Whether a tool needs the user to type something after the command name.
pub fn needs_input(id: ToolId) -> bool {
    !matches!(
        id,
        ToolId::Uuid
            | ToolId::UuidBatch
            | ToolId::Ulid
            | ToolId::NanoId
            | ToolId::Password
            | ToolId::TimestampNow
            | ToolId::Lorem
    )
}

/// Run a tool.
pub fn run(id: ToolId, input: &str) -> ToolResult {
    if needs_input(id) && input.trim().is_empty() {
        return ToolResult::err("Type something after the command first.");
    }

    match id {
        ToolId::Uuid => ToolResult::copied("UUID v4", uuid::Uuid::new_v4().to_string()),
        ToolId::UuidBatch => ToolResult::ok(
            "10 UUIDs",
            (0..10).map(|_| uuid::Uuid::new_v4().to_string()).collect::<Vec<_>>().join("\n"),
        ),
        ToolId::Ulid => ToolResult::copied("ULID", ulid()),
        ToolId::NanoId => ToolResult::copied("Nano ID", nano_id(21)),
        ToolId::Password => {
            let value = password(24);
            ToolResult::copied("Password", value)
                .with_message("24 characters, drawn from the system CSPRNG")
        }

        ToolId::Base64Encode => ToolResult::copied("Base64", b64_encode(input.as_bytes(), false)),
        ToolId::Base64Decode => match b64_decode(input.trim(), false) {
            Ok(text) => ToolResult::copied("Decoded", text),
            Err(e) => ToolResult::err(e),
        },
        ToolId::Base64UrlEncode => {
            ToolResult::copied("Base64 (URL-safe)", b64_encode(input.as_bytes(), true))
        }
        ToolId::Base64UrlDecode => match b64_decode(input.trim(), true) {
            Ok(text) => ToolResult::copied("Decoded", text),
            Err(e) => ToolResult::err(e),
        },

        ToolId::HexEncode => ToolResult::copied("Hex", hex_encode(input.as_bytes())),
        ToolId::HexDecode => match hex_decode(input.trim()) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => ToolResult::copied("Decoded", text),
                Err(_) => ToolResult::err("Those bytes are not valid UTF-8 text."),
            },
            Err(e) => ToolResult::err(e),
        },

        ToolId::UrlEncode => ToolResult::copied("URL-encoded", url_encode(input)),
        ToolId::UrlDecode => match url_decode(input.trim()) {
            Ok(text) => ToolResult::copied("URL-decoded", text),
            Err(e) => ToolResult::err(e),
        },

        ToolId::HtmlEncode => ToolResult::copied("HTML-escaped", html_encode(input)),
        ToolId::HtmlDecode => ToolResult::copied("HTML-unescaped", html_decode(input)),

        ToolId::JwtDecode => jwt_decode(input.trim()),
        ToolId::JsonFormat => match serde_json::from_str::<serde_json::Value>(input) {
            Ok(value) => ToolResult::ok(
                "Formatted JSON",
                serde_json::to_string_pretty(&value).unwrap_or_default(),
            ),
            Err(e) => ToolResult::err(format!("That is not valid JSON: {e}")),
        },
        ToolId::JsonMinify => match serde_json::from_str::<serde_json::Value>(input) {
            Ok(value) => {
                let minified = serde_json::to_string(&value).unwrap_or_default();
                let saved = input.len().saturating_sub(minified.len());
                ToolResult::copied("Minified JSON", minified)
                    .with_message(format!("{saved} bytes smaller"))
            }
            Err(e) => ToolResult::err(format!("That is not valid JSON: {e}")),
        },
        ToolId::JsonEscape => ToolResult::copied(
            "Escaped string",
            serde_json::to_string(input).unwrap_or_default(),
        ),

        ToolId::TimestampNow => {
            let now = chrono::Utc::now();
            ToolResult::copied("Now", now.timestamp().to_string()).with_message(format!(
                "{} · {} ms",
                now.to_rfc3339(),
                now.timestamp_millis()
            ))
        }
        ToolId::TimestampConvert => timestamp_convert(input.trim()),

        ToolId::Lorem => ToolResult::copied("Lorem ipsum", lorem(3)),
        ToolId::Slugify => ToolResult::copied("Slug", slugify(input)),
        ToolId::TextStats => text_stats(input),

        ToolId::SortLines => ToolResult::copied("Sorted", transform_lines(input, |lines| {
            lines.sort_by_key(|l| l.to_lowercase());
        })),
        ToolId::SortLinesDescending => {
            ToolResult::copied("Sorted (Z→A)", transform_lines(input, |lines| {
                lines.sort_by_key(|l| std::cmp::Reverse(l.to_lowercase()));
            }))
        }
        ToolId::DedupeLines => {
            let before = input.lines().count();
            let output = transform_lines(input, |lines| {
                let mut seen = std::collections::HashSet::new();
                lines.retain(|line| seen.insert(line.to_string()));
            });
            let after = output.lines().count();
            ToolResult::copied("Deduplicated", output)
                .with_message(format!("{} duplicate line(s) removed", before - after))
        }
        ToolId::ReverseLines => {
            ToolResult::copied("Reversed", transform_lines(input, |lines| lines.reverse()))
        }
        ToolId::ShuffleLines => {
            ToolResult::copied("Shuffled", transform_lines(input, |lines| shuffle(lines)))
        }
        ToolId::NumberLines => ToolResult::copied(
            "Numbered",
            input
                .lines()
                .enumerate()
                .map(|(i, line)| format!("{}. {line}", i + 1))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        ToolId::JoinLines => ToolResult::copied(
            "Joined",
            input.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(", "),
        ),
        ToolId::TrimLines => ToolResult::copied(
            "Trimmed",
            input.lines().map(str::trim).collect::<Vec<_>>().join("\n"),
        ),
        ToolId::CountOccurrences => count_occurrences(input),

        ToolId::ColorConvert => color_convert(input.trim()),
        ToolId::NumberBase => number_base(input.trim()),
        ToolId::RandomNumber => random_number(input.trim()),

        ToolId::Md5 => hash_with("md5", &["-q", "-s"], input, "MD5"),
        ToolId::Sha1 => hash_with("shasum", &["-a", "1"], input, "SHA-1"),
        ToolId::Sha256 => hash_with("shasum", &["-a", "256"], input, "SHA-256"),
        ToolId::Sha512 => hash_with("shasum", &["-a", "512"], input, "SHA-512"),
    }
}

// ---------------------------------------------------------------------------
// Randomness
// ---------------------------------------------------------------------------

/// Fill a buffer from the OS CSPRNG.
///
/// Panicking is not an option and neither is a weak fallback, so a failure
/// returns `false` and every caller treats that as "cannot generate".
fn random_bytes(buffer: &mut [u8]) -> bool {
    getrandom::fill(buffer).is_ok()
}

/// A uniformly distributed index into `len` items, without modulo bias.
fn random_index(len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    // Rejection sampling: values in the final, short bucket of the u32 range
    // would otherwise make the first `2^32 % len` items very slightly likelier.
    let limit = u32::MAX - (u32::MAX % len as u32) - 1;
    loop {
        let mut raw = [0u8; 4];
        if !random_bytes(&mut raw) {
            return 0;
        }
        let value = u32::from_le_bytes(raw);
        if value <= limit {
            return (value % len as u32) as usize;
        }
    }
}

fn shuffle<T>(items: &mut Vec<T>) {
    // Fisher-Yates, back to front.
    for i in (1..items.len()).rev() {
        items.swap(i, random_index(i + 1));
    }
}

/// Crockford base32, as ULID specifies (no I, L, O or U).
const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A ULID: 48 bits of millisecond timestamp then 80 bits of randomness.
///
/// Sorts lexicographically by creation time, which is the whole reason to
/// prefer one over a UUID v4 for database keys.
fn ulid() -> String {
    let now = chrono::Utc::now().timestamp_millis().max(0) as u128;
    let mut random = [0u8; 10];
    random_bytes(&mut random);

    let mut value: u128 = now << 80;
    for (i, byte) in random.iter().enumerate() {
        value |= (*byte as u128) << (8 * (9 - i));
    }

    // 128 bits is 26 base32 characters, most significant first.
    let mut out = [0u8; 26];
    for i in (0..26).rev() {
        out[i] = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const NANO_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";

fn nano_id(length: usize) -> String {
    (0..length).map(|_| NANO_ALPHABET[random_index(NANO_ALPHABET.len())] as char).collect()
}

/// A password with at least one character from each class.
///
/// Ambiguous glyphs are left out on purpose: a password that cannot be read off
/// a screen and typed on a phone gets written down somewhere worse.
fn password(length: usize) -> String {
    const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const DIGITS: &[u8] = b"23456789";
    const SYMBOLS: &[u8] = b"!@#$%^&*-_=+?";

    let classes = [LOWER, UPPER, DIGITS, SYMBOLS];
    let mut chars: Vec<char> =
        classes.iter().map(|set| set[random_index(set.len())] as char).collect();

    let all: Vec<u8> = classes.concat();
    while chars.len() < length {
        chars.push(all[random_index(all.len())] as char);
    }
    shuffle(&mut chars);
    chars.into_iter().collect()
}

/// Pull the integers out of a range expression.
///
/// `-` is genuinely ambiguous here: in `5-10` it separates two numbers, in
/// `-5 10` it is a sign. The rule is the one a reader applies without thinking —
/// a `-` immediately after a digit is a separator, anywhere else it is a sign.
fn parse_numbers(input: &str) -> Vec<i64> {
    let chars: Vec<char> = input.chars().collect();
    let mut numbers = Vec::new();
    let mut current = String::new();
    let mut previous_was_digit = false;

    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_digit() {
            current.push(*c);
            previous_was_digit = true;
            continue;
        }
        if *c == '-' && !previous_was_digit && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
            if let Ok(value) = current.parse::<i64>() {
                numbers.push(value);
            }
            current = String::from("-");
            continue;
        }
        if let Ok(value) = current.parse::<i64>() {
            numbers.push(value);
        }
        current.clear();
        previous_was_digit = false;
    }
    if let Ok(value) = current.parse::<i64>() {
        numbers.push(value);
    }
    numbers
}

fn random_number(input: &str) -> ToolResult {
    // Accepts "1-100", "1 100", "1..100" or a bare "100" meaning 1 to 100.
    let parts = parse_numbers(input);

    let (low, high) = match parts.as_slice() {
        [] => (1, 100),
        [single] => (1, *single),
        [a, b, ..] => (*a, *b),
    };
    let (low, high) = if low <= high { (low, high) } else { (high, low) };

    let span = (high - low + 1).max(1) as usize;
    let value = low + random_index(span) as i64;
    ToolResult::copied("Random number", value.to_string())
        .with_message(format!("between {low} and {high}"))
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

const B64_STANDARD: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64_URLSAFE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64_encode(bytes: &[u8], url_safe: bool) -> String {
    let table = if url_safe { B64_URLSAFE } else { B64_STANDARD };
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(table[(triple >> 18) as usize & 0x3f] as char);
        out.push(table[(triple >> 12) as usize & 0x3f] as char);
        // URL-safe base64 is conventionally unpadded, which is what every JWT
        // and every `data:` URL in the wild expects.
        if chunk.len() > 1 {
            out.push(table[(triple >> 6) as usize & 0x3f] as char);
        } else if !url_safe {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(table[triple as usize & 0x3f] as char);
        } else if !url_safe {
            out.push('=');
        }
    }
    out
}

fn b64_value(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some((byte - b'A') as u32),
        b'a'..=b'z' => Some((byte - b'a') as u32 + 26),
        b'0'..=b'9' => Some((byte - b'0') as u32 + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

/// Decode base64, accepting both alphabets and tolerating missing padding.
///
/// `url_safe` only affects the error message: the value table already accepts
/// `-`/`_` alongside `+`/`/`, because pasted tokens mix them constantly and
/// refusing one would be pedantry with no upside.
fn b64_decode_bytes(input: &str, url_safe: bool) -> Result<Vec<u8>, String> {
    let cleaned: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();

    let mut out = Vec::with_capacity(cleaned.len() * 3 / 4);
    for chunk in cleaned.chunks(4) {
        if chunk.len() == 1 {
            return Err("That is not valid base64 — it ends mid-character.".into());
        }
        let mut triple: u32 = 0;
        for (i, byte) in chunk.iter().enumerate() {
            let value = b64_value(*byte).ok_or_else(|| {
                let alphabet = if url_safe { "URL-safe base64" } else { "base64" };
                format!("'{}' is not a {alphabet} character.", *byte as char)
            })?;
            triple |= value << (18 - 6 * i);
        }
        out.push((triple >> 16) as u8);
        if chunk.len() > 2 {
            out.push((triple >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

fn b64_decode(input: &str, url_safe: bool) -> Result<String, String> {
    let bytes = b64_decode_bytes(input, url_safe)?;
    String::from_utf8(bytes).map_err(|_| "That decodes to bytes, not to text.".to_string())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(input: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':' && *c != '-')
        .collect();
    let cleaned = cleaned.strip_prefix("0x").unwrap_or(&cleaned);

    if cleaned.len() % 2 != 0 {
        return Err("Hex needs an even number of digits.".into());
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .map_err(|_| format!("'{}' is not a hex byte.", &cleaned[i..i + 2]))
        })
        .collect()
}

fn url_encode(input: &str) -> String {
    input
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn url_decode(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err("A '%' escape is cut short.".into());
                }
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                let value = u8::from_str_radix(hex, 16)
                    .map_err(|_| format!("'%{hex}' is not a valid escape."))?;
                out.push(value);
                i += 3;
            }
            // Only meaningful in query strings, but that is where these come
            // from, and "+" surviving a decode is never what anyone wanted.
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "That decodes to bytes, not to text.".to_string())
}

fn html_encode(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&#39;".into(),
            other => other.to_string(),
        })
        .collect()
}

fn html_decode(input: &str) -> String {
    // Ampersand last, so "&amp;lt;" round-trips to "&lt;" rather than "<".
    let named = [
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&nbsp;", " "),
        ("&amp;", "&"),
    ];
    let mut out = input.to_string();
    for (entity, literal) in named {
        out = out.replace(entity, literal);
    }
    out
}

// ---------------------------------------------------------------------------
// JWT
// ---------------------------------------------------------------------------

/// Decode a JWT's header and payload.
///
/// Explicitly does **not** verify the signature: doing so needs the key, and a
/// tool that showed a green tick without one would be worse than useless. The
/// output says so, so nobody mistakes "decoded" for "trusted".
fn jwt_decode(token: &str) -> ToolResult {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return ToolResult::err("A JWT has at least two dot-separated parts.");
    }

    let mut sections = Vec::new();
    for (label, part) in [("Header", parts[0]), ("Payload", parts[1])] {
        let decoded = match b64_decode(part, true) {
            Ok(text) => text,
            Err(e) => return ToolResult::err(format!("{label} is not valid base64url: {e}")),
        };
        let pretty = serde_json::from_str::<serde_json::Value>(&decoded)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or(decoded);
        sections.push(format!("{label}\n{pretty}"));
    }

    // Expiry is the one claim worth reading out, because "is this token dead"
    // is the question that sends people to a JWT decoder in the first place.
    let expiry = serde_json::from_str::<serde_json::Value>(
        &b64_decode(parts[1], true).unwrap_or_default(),
    )
    .ok()
    .and_then(|v| v.get("exp").and_then(serde_json::Value::as_i64))
    .map(|exp| {
        let when = chrono::DateTime::from_timestamp(exp, 0);
        match when {
            Some(when) if when < chrono::Utc::now() => {
                format!("Expired {}", when.format("%Y-%m-%d %H:%M UTC"))
            }
            Some(when) => format!("Expires {}", when.format("%Y-%m-%d %H:%M UTC")),
            None => String::new(),
        }
    })
    .unwrap_or_default();

    let message = if expiry.is_empty() {
        "Decoded locally. The signature is not verified.".to_string()
    } else {
        format!("{expiry} · signature not verified")
    };

    ToolResult::ok("JWT", sections.join("\n\n")).with_message(message)
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Convert between epoch numbers and readable dates, guessing which was meant.
fn timestamp_convert(input: &str) -> ToolResult {
    use chrono::TimeZone;

    let digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();

    // A bare number is an epoch. Ten digits is seconds, thirteen milliseconds —
    // the boundary between them stays unambiguous until the year 2286.
    if !digits.is_empty() && digits.len() == input.trim().len() {
        if let Ok(value) = digits.parse::<i64>() {
            let (seconds, unit) = if digits.len() >= 13 {
                (value / 1000, "milliseconds")
            } else {
                (value, "seconds")
            };
            let Some(utc) = chrono::DateTime::from_timestamp(seconds, 0) else {
                return ToolResult::err("That number is outside the range of a date.");
            };
            let local = chrono::Local.from_utc_datetime(&utc.naive_utc());
            return ToolResult::ok(
                "Timestamp",
                format!(
                    "UTC    {}\nLocal  {}\nISO    {}\nEpoch  {} s\nEpoch  {} ms",
                    utc.format("%Y-%m-%d %H:%M:%S"),
                    local.format("%Y-%m-%d %H:%M:%S %Z"),
                    utc.to_rfc3339(),
                    seconds,
                    seconds * 1000,
                ),
            )
            .with_message(format!("read as {unit}"));
        }
    }

    // Otherwise treat it as a date and go the other way.
    let parsed = chrono::DateTime::parse_from_rfc3339(input)
        .map(|d| d.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(input, "%Y-%m-%d %H:%M:%S")
                .map(|d| chrono::Utc.from_utc_datetime(&d))
        })
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d").map(|d| {
                chrono::Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default())
            })
        });

    match parsed {
        Ok(when) => ToolResult::ok(
            "Timestamp",
            format!(
                "Epoch  {} s\nEpoch  {} ms\nUTC    {}\nISO    {}",
                when.timestamp(),
                when.timestamp_millis(),
                when.format("%Y-%m-%d %H:%M:%S"),
                when.to_rfc3339(),
            ),
        ),
        Err(_) => ToolResult::err(
            "Type an epoch number, or a date like 2026-07-26 or 2026-07-26 14:30:00.",
        ),
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

const LOREM_SENTENCES: &[&str] = &[
    "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.",
    "Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.",
    "Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.",
    "Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.",
    "Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium.",
    "Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit, sed quia consequuntur magni dolores.",
];

fn lorem(sentences: usize) -> String {
    (0..sentences)
        .map(|i| LOREM_SENTENCES[i % LOREM_SENTENCES.len()])
        .collect::<Vec<_>>()
        .join(" ")
}

fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_dash = false;
    for c in input.chars() {
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(c.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out
}

fn transform_lines(input: &str, apply: impl FnOnce(&mut Vec<&str>)) -> String {
    let mut lines: Vec<&str> = input.lines().collect();
    apply(&mut lines);
    lines.join("\n")
}

fn text_stats(input: &str) -> ToolResult {
    let words = input.split_whitespace().count();
    let chars = input.chars().count();
    let no_spaces = input.chars().filter(|c| !c.is_whitespace()).count();
    let lines = input.lines().count();
    let paragraphs = input.split("\n\n").filter(|p| !p.trim().is_empty()).count();
    let sentences = input.matches(['.', '!', '?']).count();
    // 200 wpm is the usual figure for silent reading of prose.
    let minutes = (words as f64 / 200.0).ceil().max(1.0) as usize;

    ToolResult::ok(
        "Text statistics",
        format!(
            "Words        {words}\n\
             Characters   {chars}\n\
             Without ws   {no_spaces}\n\
             Lines        {lines}\n\
             Paragraphs   {paragraphs}\n\
             Sentences    {sentences}\n\
             Reading time {minutes} min"
        ),
    )
}

/// Count how often each line appears, most frequent first.
fn count_occurrences(input: &str) -> ToolResult {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for line in input.lines().map(str::trim).filter(|l| !l.is_empty()) {
        *counts.entry(line).or_insert(0) += 1;
    }
    if counts.is_empty() {
        return ToolResult::err("There are no lines to count.");
    }

    let mut ordered: Vec<(&str, usize)> = counts.into_iter().collect();
    // Count descending, then alphabetically, so the output is stable between runs.
    ordered.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    ToolResult::ok(
        "Line counts",
        ordered
            .iter()
            .map(|(line, count)| format!("{count:>6}  {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .with_message(format!("{} distinct line(s)", ordered.len()))
}

// ---------------------------------------------------------------------------
// Colour and numbers
// ---------------------------------------------------------------------------

/// Parse `#rgb`, `#rrggbb`, `rgb(r,g,b)` or three bare numbers.
fn parse_color(input: &str) -> Option<(u8, u8, u8)> {
    let text = input.trim().trim_start_matches('#');

    if text.len() == 3 && text.chars().all(|c| c.is_ascii_hexdigit()) {
        let digit = |c: char| u8::from_str_radix(&c.to_string(), 16).ok();
        let mut chars = text.chars();
        let r = digit(chars.next()?)?;
        let g = digit(chars.next()?)?;
        let b = digit(chars.next()?)?;
        // #abc means #aabbcc, not #0a0b0c.
        return Some((r * 17, g * 17, b * 17));
    }
    if text.len() == 6 && text.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some((
            u8::from_str_radix(&text[0..2], 16).ok()?,
            u8::from_str_radix(&text[2..4], 16).ok()?,
            u8::from_str_radix(&text[4..6], 16).ok()?,
        ));
    }

    let numbers: Vec<u32> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if numbers.len() >= 3 && numbers[..3].iter().all(|n| *n <= 255) {
        return Some((numbers[0] as u8, numbers[1] as u8, numbers[2] as u8));
    }
    None
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;

    if delta == 0.0 {
        return (0.0, 0.0, lightness * 100.0);
    }

    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    (hue, saturation * 100.0, lightness * 100.0)
}

/// Relative luminance, per WCAG 2.1.
fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    let channel = |value: u8| {
        let v = value as f64 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

fn color_convert(input: &str) -> ToolResult {
    let Some((r, g, b)) = parse_color(input) else {
        return ToolResult::err("Type a colour like #3b82f6, 3b82f6 or rgb(59, 130, 246).");
    };
    let (h, s, l) = rgb_to_hsl(r, g, b);
    let luminance = relative_luminance(r, g, b);
    // Contrast against white and black, the two things text is usually on.
    let on_white = 1.05 / (luminance + 0.05);
    let on_black = (luminance + 0.05) / 0.05;
    let readable = if on_white >= 4.5 { "white" } else if on_black >= 4.5 { "black" } else { "neither" };

    ToolResult::copied("Colour", format!("#{r:02x}{g:02x}{b:02x}")).with_message(format!(
        "rgb({r}, {g}, {b}) · hsl({:.0}, {:.0}%, {:.0}%) · readable on {readable} \
         (contrast {:.1} on white, {:.1} on black)",
        h, s, l, on_white, on_black
    ))
}

/// Show a number in binary, octal, decimal and hex at once.
fn number_base(input: &str) -> ToolResult {
    let text = input.trim();
    let (digits, radix) = if let Some(rest) = text.strip_prefix("0x").or(text.strip_prefix("0X")) {
        (rest, 16)
    } else if let Some(rest) = text.strip_prefix("0b").or(text.strip_prefix("0B")) {
        (rest, 2)
    } else if let Some(rest) = text.strip_prefix("0o").or(text.strip_prefix("0O")) {
        (rest, 8)
    } else {
        (text, 10)
    };

    let cleaned: String = digits.chars().filter(|c| *c != '_' && *c != ' ').collect();
    let Ok(value) = i128::from_str_radix(&cleaned, radix) else {
        return ToolResult::err(
            "Type a number, optionally prefixed with 0x, 0b or 0o.",
        );
    };

    let magnitude = value.unsigned_abs();
    let sign = if value < 0 { "-" } else { "" };
    ToolResult::ok(
        "Number bases",
        format!(
            "Decimal  {value}\n\
             Hex      {sign}0x{magnitude:X}\n\
             Octal    {sign}0o{magnitude:o}\n\
             Binary   {sign}0b{magnitude:b}"
        ),
    )
    .with_message(format!("read as base {radix}"))
}

// ---------------------------------------------------------------------------
// Hashes
// ---------------------------------------------------------------------------

/// Hash text using the binaries macOS already ships.
///
/// `md5 -s` takes the string as an argument; `shasum` reads stdin. Both are in
/// the base system, which is why neither `sha2` nor `md-5` is a dependency —
/// three crypto crates is a poor trade for a tool used twice a year.
fn hash_with(program: &str, args: &[&str], input: &str, label: &str) -> ToolResult {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let output = if program == "md5" {
        Command::new(program).args(args).arg(input).output()
    } else {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => return ToolResult::err(format!("Could not run {program}: {e}")),
        };
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(input.as_bytes());
        }
        // Dropping stdin closes the pipe, without which `shasum` waits forever.
        drop(child.stdin.take());
        child.wait_with_output()
    };

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            // `shasum` prints "<digest>  -"; `md5 -q -s` prints just the digest.
            let digest = text.split_whitespace().next().unwrap_or("").to_string();
            if digest.is_empty() {
                ToolResult::err(format!("{label} produced no output."))
            } else {
                ToolResult::copied(label, digest)
            }
        }
        Ok(out) => ToolResult::err(format!(
            "{label} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => ToolResult::err(format!("Could not run {program}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(id: ToolId, input: &str) -> String {
        let result = run(id, input);
        assert!(result.ok, "{id:?} failed: {}", result.message);
        result.output
    }

    // --- input guards ------------------------------------------------------

    #[test]
    fn tools_that_need_input_refuse_an_empty_one() {
        for id in [ToolId::Base64Encode, ToolId::JwtDecode, ToolId::Slugify] {
            assert!(!run(id, "   ").ok, "{id:?} accepted empty input");
        }
    }

    #[test]
    fn generators_need_no_input() {
        for id in [ToolId::Uuid, ToolId::Ulid, ToolId::NanoId, ToolId::Password, ToolId::Lorem] {
            assert!(!needs_input(id));
            assert!(run(id, "").ok, "{id:?} refused to run without input");
        }
    }

    // --- identifiers -------------------------------------------------------

    #[test]
    fn uuids_are_well_formed_and_distinct() {
        let a = output(ToolId::Uuid, "");
        let b = output(ToolId::Uuid, "");
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }

    #[test]
    fn ulids_are_26_crockford_characters() {
        let value = output(ToolId::Ulid, "");
        assert_eq!(value.len(), 26);
        assert!(value.bytes().all(|b| CROCKFORD.contains(&b)), "{value}");
    }

    #[test]
    fn ulids_sort_in_creation_order() {
        // The timestamp occupies the leading characters, so a later ULID is
        // lexicographically greater. Same-millisecond ties are allowed.
        let first = ulid();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let second = ulid();
        assert!(second > first, "{second} should sort after {first}");
    }

    #[test]
    fn nano_ids_use_only_the_url_safe_alphabet() {
        let value = output(ToolId::NanoId, "");
        assert_eq!(value.len(), 21);
        assert!(value.bytes().all(|b| NANO_ALPHABET.contains(&b)));
    }

    #[test]
    fn passwords_contain_every_character_class() {
        for _ in 0..40 {
            let value = password(24);
            assert_eq!(value.chars().count(), 24);
            assert!(value.chars().any(|c| c.is_lowercase()), "{value}");
            assert!(value.chars().any(|c| c.is_uppercase()), "{value}");
            assert!(value.chars().any(|c| c.is_ascii_digit()), "{value}");
            assert!(value.chars().any(|c| "!@#$%^&*-_=+?".contains(c)), "{value}");
        }
    }

    #[test]
    fn passwords_leave_out_glyphs_that_cannot_be_told_apart() {
        for _ in 0..40 {
            let value = password(32);
            for ambiguous in ['l', 'I', 'O', '0', '1'] {
                assert!(!value.contains(ambiguous), "{value} contains {ambiguous}");
            }
        }
    }

    // --- base64 ------------------------------------------------------------

    #[test]
    fn base64_round_trips_including_every_padding_length() {
        for text in ["", "a", "ab", "abc", "abcd", "hello world", "Caduceus 2.0!"] {
            let encoded = b64_encode(text.as_bytes(), false);
            assert_eq!(b64_decode(&encoded, false).unwrap(), text, "failed on {text:?}");
        }
    }

    #[test]
    fn base64_matches_the_canonical_encoding() {
        assert_eq!(b64_encode(b"", false), "");
        assert_eq!(b64_encode(b"f", false), "Zg==");
        assert_eq!(b64_encode(b"fo", false), "Zm8=");
        assert_eq!(b64_encode(b"foo", false), "Zm9v");
        assert_eq!(b64_encode(b"foob", false), "Zm9vYg==");
        assert_eq!(b64_encode(b"fooba", false), "Zm9vYmE=");
        assert_eq!(b64_encode(b"foobar", false), "Zm9vYmFy");
    }

    #[test]
    fn url_safe_base64_is_unpadded_and_uses_the_other_two_characters() {
        let bytes = [0xfbu8, 0xff, 0xbf];
        let standard = b64_encode(&bytes, false);
        let url_safe = b64_encode(&bytes, true);
        assert!(standard.contains('+') || standard.contains('/'));
        assert!(!url_safe.contains('+') && !url_safe.contains('/'));
        assert!(!url_safe.contains('='));
        assert_eq!(b64_decode_bytes(&url_safe, true).unwrap(), bytes);
    }

    #[test]
    fn base64_decoding_tolerates_missing_padding_and_whitespace() {
        assert_eq!(b64_decode("Zm9vYmFy", false).unwrap(), "foobar");
        // Padding stripped entirely, and interior whitespace of both kinds.
        assert_eq!(b64_decode("Zg", false).unwrap(), "f");
        assert_eq!(b64_decode("Zm8", false).unwrap(), "fo");
        assert_eq!(b64_decode("Zm9v YmFy", false).unwrap(), "foobar");
        assert_eq!(b64_decode("Zm9v\nYmFy", false).unwrap(), "foobar");
    }

    #[test]
    fn base64_rejects_a_length_no_encoder_could_have_produced() {
        // A trailing single character carries only six bits — not a whole byte.
        let result = b64_decode("Zm9vYmFyZ", false);
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn base64_rejects_characters_that_are_not_base64() {
        let result = run(ToolId::Base64Decode, "not valid!!");
        assert!(!result.ok);
        assert!(result.message.contains('!'), "{}", result.message);
    }

    // --- hex, url, html ----------------------------------------------------

    #[test]
    fn hex_round_trips_and_accepts_common_separators() {
        assert_eq!(hex_encode(b"hi"), "6869");
        assert_eq!(hex_decode("6869").unwrap(), b"hi");
        assert_eq!(hex_decode("68:69").unwrap(), b"hi");
        assert_eq!(hex_decode("0x6869").unwrap(), b"hi");
        assert!(hex_decode("689").is_err());
    }

    #[test]
    fn url_encoding_round_trips_and_leaves_unreserved_characters_alone() {
        let text = "a b&c=d/e?f#g~h-i_j.k";
        assert_eq!(url_decode(&url_encode(text)).unwrap(), text);
        assert_eq!(url_encode("safe-chars_only.here~"), "safe-chars_only.here~");
        assert_eq!(url_encode("a b"), "a%20b");
    }

    #[test]
    fn url_decoding_treats_plus_as_a_space() {
        assert_eq!(url_decode("hello+world").unwrap(), "hello world");
    }

    #[test]
    fn url_decoding_reports_a_truncated_escape() {
        assert!(url_decode("abc%2").is_err());
        assert!(url_decode("abc%zz").is_err());
    }

    #[test]
    fn html_escaping_round_trips_without_double_unescaping() {
        let text = r#"<a href="x">Tom & Jerry's</a>"#;
        assert_eq!(html_decode(&html_encode(text)), text);
        // The classic ordering bug: this must stay "&lt;", not become "<".
        assert_eq!(html_decode("&amp;lt;"), "&lt;");
    }

    // --- JWT ---------------------------------------------------------------

    #[test]
    fn a_jwt_decodes_to_readable_header_and_payload() {
        // {"alg":"HS256","typ":"JWT"} / {"sub":"1234","name":"Ada"}
        let token = format!(
            "{}.{}.signature",
            b64_encode(br#"{"alg":"HS256","typ":"JWT"}"#, true),
            b64_encode(br#"{"sub":"1234","name":"Ada"}"#, true),
        );
        let result = run(ToolId::JwtDecode, &token);
        assert!(result.ok, "{}", result.message);
        assert!(result.output.contains("HS256"));
        assert!(result.output.contains("Ada"));
        assert!(result.message.contains("not verified"), "{}", result.message);
    }

    #[test]
    fn an_expired_jwt_says_so() {
        let payload = format!(r#"{{"exp":{}}}"#, 1_000_000_000);
        let token = format!(
            "{}.{}.sig",
            b64_encode(br#"{"alg":"none"}"#, true),
            b64_encode(payload.as_bytes(), true),
        );
        assert!(run(ToolId::JwtDecode, &token).message.contains("Expired"));
    }

    #[test]
    fn a_string_with_no_dots_is_not_a_jwt() {
        assert!(!run(ToolId::JwtDecode, "just-some-text").ok);
    }

    // --- JSON --------------------------------------------------------------

    #[test]
    fn json_formatting_and_minifying_are_inverses() {
        let compact = r#"{"b":2,"a":[1,2,3]}"#;
        let pretty = output(ToolId::JsonFormat, compact);
        assert!(pretty.contains("\n  "));
        assert_eq!(output(ToolId::JsonMinify, &pretty).len(), compact.len());
    }

    #[test]
    fn invalid_json_is_reported_with_the_parser_message() {
        let result = run(ToolId::JsonFormat, "{nope}");
        assert!(!result.ok);
        assert!(result.message.starts_with("That is not valid JSON"));
    }

    #[test]
    fn escaping_produces_a_quoted_json_string() {
        assert_eq!(output(ToolId::JsonEscape, "a\"b\nc"), r#""a\"b\nc""#);
    }

    // --- time --------------------------------------------------------------

    #[test]
    fn ten_digit_epochs_read_as_seconds_and_thirteen_as_milliseconds() {
        let seconds = run(ToolId::TimestampConvert, "1000000000");
        assert!(seconds.message.contains("seconds"));
        assert!(seconds.output.contains("2001-09-09"), "{}", seconds.output);

        let millis = run(ToolId::TimestampConvert, "1000000000000");
        assert!(millis.message.contains("milliseconds"));
        assert!(millis.output.contains("2001-09-09"), "{}", millis.output);
    }

    #[test]
    fn dates_convert_back_to_epoch_numbers() {
        let result = run(ToolId::TimestampConvert, "2001-09-09");
        assert!(result.ok, "{}", result.message);
        assert!(result.output.contains("999993600"), "{}", result.output);
    }

    #[test]
    fn nonsense_is_refused_with_an_example() {
        let result = run(ToolId::TimestampConvert, "next tuesday");
        assert!(!result.ok);
        assert!(result.message.contains("2026-07-26"));
    }

    // --- text --------------------------------------------------------------

    #[test]
    fn slugs_are_lowercase_dashed_and_have_no_leading_or_trailing_dash() {
        assert_eq!(slugify("  Hello, World! 2.0  "), "hello-world-2-0");
        assert_eq!(slugify("already-a-slug"), "already-a-slug");
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify("Ünïcodé Wörks"), "ünïcodé-wörks");
    }

    #[test]
    fn line_tools_do_what_their_names_say() {
        assert_eq!(output(ToolId::SortLines, "c\na\nb"), "a\nb\nc");
        assert_eq!(output(ToolId::SortLinesDescending, "a\nc\nb"), "c\nb\na");
        assert_eq!(output(ToolId::ReverseLines, "a\nb\nc"), "c\nb\na");
        assert_eq!(output(ToolId::DedupeLines, "a\nb\na\nb"), "a\nb");
        assert_eq!(output(ToolId::TrimLines, "  a  \n  b"), "a\nb");
        assert_eq!(output(ToolId::NumberLines, "a\nb"), "1. a\n2. b");
        assert_eq!(output(ToolId::JoinLines, "a\n\nb\nc"), "a, b, c");
    }

    #[test]
    fn deduplication_reports_how_many_lines_went() {
        let result = run(ToolId::DedupeLines, "a\na\na\nb");
        assert!(result.message.contains('2'), "{}", result.message);
    }

    #[test]
    fn shuffling_keeps_every_line_exactly_once() {
        let input = (0..50).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let shuffled = output(ToolId::ShuffleLines, &input);
        let mut before: Vec<&str> = input.lines().collect();
        let mut after: Vec<&str> = shuffled.lines().collect();
        before.sort_unstable();
        after.sort_unstable();
        assert_eq!(before, after);
    }

    #[test]
    fn text_statistics_count_what_they_claim_to() {
        let result = run(ToolId::TextStats, "one two three.\n\nfour five!");
        assert!(result.output.contains("Words        5"), "{}", result.output);
        assert!(result.output.contains("Lines        3"), "{}", result.output);
        assert!(result.output.contains("Sentences    2"), "{}", result.output);
    }

    #[test]
    fn occurrence_counting_orders_by_frequency_then_alphabetically() {
        let result = run(ToolId::CountOccurrences, "b\na\nb\nc\nb\na");
        let lines: Vec<&str> = result.output.lines().collect();
        assert!(lines[0].contains('b') && lines[0].contains('3'), "{:?}", lines);
        assert!(lines[1].contains('a'), "{:?}", lines);
        assert!(lines[2].contains('c'), "{:?}", lines);
    }

    // --- colour ------------------------------------------------------------

    #[test]
    fn colours_parse_from_every_notation_people_paste() {
        for input in ["#3b82f6", "3b82f6", "rgb(59, 130, 246)", "59 130 246"] {
            assert_eq!(parse_color(input), Some((59, 130, 246)), "failed on {input}");
        }
        // Three-digit hex expands by repeating each digit.
        assert_eq!(parse_color("#abc"), Some((0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn colour_conversion_reports_hex_rgb_and_hsl() {
        let result = run(ToolId::ColorConvert, "#3b82f6");
        assert_eq!(result.output, "#3b82f6");
        assert!(result.message.contains("rgb(59, 130, 246)"), "{}", result.message);
        assert!(result.message.contains("hsl(217"), "{}", result.message);
    }

    #[test]
    fn contrast_advice_matches_the_wcag_threshold() {
        // Near-black: white text passes, black text does not.
        assert!(run(ToolId::ColorConvert, "#111111").message.contains("readable on white"));
        // Near-white: the other way round.
        assert!(run(ToolId::ColorConvert, "#fefefe").message.contains("readable on black"));
    }

    #[test]
    fn greyscale_has_no_hue_or_saturation() {
        let (h, s, _) = rgb_to_hsl(128, 128, 128);
        assert_eq!(h, 0.0);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn a_colour_that_is_not_a_colour_is_refused() {
        assert!(!run(ToolId::ColorConvert, "chartreuse").ok);
    }

    // --- number bases ------------------------------------------------------

    #[test]
    fn number_bases_are_read_from_the_prefix_and_shown_in_all_four() {
        let decimal = run(ToolId::NumberBase, "255");
        assert!(decimal.output.contains("0xFF"), "{}", decimal.output);
        assert!(decimal.output.contains("0b11111111"), "{}", decimal.output);

        assert_eq!(
            run(ToolId::NumberBase, "0xff").output,
            run(ToolId::NumberBase, "255").output
        );
        assert_eq!(
            run(ToolId::NumberBase, "0b1010").output,
            run(ToolId::NumberBase, "10").output
        );
    }

    #[test]
    fn underscores_in_numbers_are_ignored_the_way_source_code_writes_them() {
        assert_eq!(
            run(ToolId::NumberBase, "1_000").output,
            run(ToolId::NumberBase, "1000").output
        );
    }

    #[test]
    fn negative_numbers_keep_their_sign_in_every_base() {
        let result = run(ToolId::NumberBase, "-255");
        assert!(result.output.contains("-0xFF"), "{}", result.output);
    }

    // --- random ------------------------------------------------------------

    #[test]
    fn random_numbers_stay_inside_the_requested_range() {
        for _ in 0..200 {
            let value: i64 = run(ToolId::RandomNumber, "5-10").output.parse().unwrap();
            assert!((5..=10).contains(&value), "{value} out of range");
        }
    }

    #[test]
    fn a_single_number_means_one_to_that_number() {
        let result = run(ToolId::RandomNumber, "6");
        assert!(result.message.contains("between 1 and 6"), "{}", result.message);
    }

    #[test]
    fn a_reversed_range_is_read_the_way_it_was_meant() {
        let result = run(ToolId::RandomNumber, "10-5");
        assert!(result.message.contains("between 5 and 10"), "{}", result.message);
    }

    #[test]
    fn ranges_are_recognised_however_they_are_written() {
        for input in ["5-10", "5 10", "5..10", "5 to 10"] {
            assert_eq!(parse_numbers(input), vec![5, 10], "failed on {input:?}");
        }
    }

    #[test]
    fn a_minus_after_a_digit_separates_but_a_leading_one_signs() {
        // The whole reason this needs its own parser.
        assert_eq!(parse_numbers("5-10"), vec![5, 10]);
        assert_eq!(parse_numbers("-5 10"), vec![-5, 10]);
        assert_eq!(parse_numbers("-20 -10"), vec![-20, -10]);
    }

    #[test]
    fn a_negative_range_still_produces_values_inside_it() {
        for _ in 0..200 {
            let value: i64 = run(ToolId::RandomNumber, "-20 -10").output.parse().unwrap();
            assert!((-20..=-10).contains(&value), "{value} out of range");
        }
    }

    #[test]
    fn random_indices_cover_their_whole_range() {
        let mut seen = [false; 6];
        for _ in 0..600 {
            seen[random_index(6)] = true;
        }
        assert!(seen.iter().all(|s| *s), "some values never came up: {seen:?}");
    }

    // --- hashes ------------------------------------------------------------

    #[test]
    fn hashes_match_their_published_test_vectors() {
        // These shell out to macOS's own binaries, so this also proves the
        // stdin plumbing does not hang or truncate.
        assert_eq!(
            run(ToolId::Sha256, "abc").output,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(run(ToolId::Sha1, "abc").output, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(run(ToolId::Md5, "abc").output, "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn hashing_a_long_input_does_not_deadlock_on_the_pipe() {
        let long = "x".repeat(200_000);
        let result = run(ToolId::Sha256, &long);
        assert!(result.ok, "{}", result.message);
        assert_eq!(result.output.len(), 64);
    }
}
