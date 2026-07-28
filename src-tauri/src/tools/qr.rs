//! QR codes, and finding the URL to put in one.
//!
//! The encoder is [`qrcodegen`] — Nayuki's, the reference implementation most
//! other libraries are ports of. It is a dependency rather than vendored source
//! because it is MIT, published by the same author, and has no transitive
//! dependencies of its own.
//!
//! Output is **SVG**, not a raster. A QR code is line art: at any size it is a
//! grid of squares, so a vector is both smaller and correct at every zoom level,
//! and it drops straight into the webview without a round-trip through base64
//! and an image decoder.

use qrcodegen::{QrCode, QrCodeEcc};

/// The largest input worth trying.
///
/// Version 40 at the lowest error correction holds ~2,953 bytes, so anything
/// past that cannot be encoded at all. Refusing early gives a sentence instead
/// of the library's own failure.
const MAX_INPUT: usize = 2_900;

/// How much of the code can be lost and still scanned.
fn ecc_from(name: &str) -> QrCodeEcc {
    match name {
        "low" => QrCodeEcc::Low,
        "quartile" => QrCodeEcc::Quartile,
        "high" => QrCodeEcc::High,
        // Medium is the default every phone camera is tuned against, and the
        // level that survives being printed and photographed at an angle.
        _ => QrCodeEcc::Medium,
    }
}

/// Encode `text` as an SVG QR code.
///
/// `border` is in modules (QR's own unit), not pixels: the spec's "quiet zone"
/// is four modules, and scanners genuinely fail without it — a QR with no
/// margin is the single most common reason one will not read.
pub fn svg(text: &str, ecc: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("There is nothing to encode yet.".into());
    }
    if text.len() > MAX_INPUT {
        return Err(format!(
            "That is {} characters. A QR code holds about {MAX_INPUT}, and one that big needs a \
             camera closer than it is useful to hold.",
            text.len()
        ));
    }

    let code = QrCode::encode_text(text, ecc_from(ecc))
        .map_err(|e| format!("Could not encode that: {e}"))?;

    Ok(to_svg(&code, 4))
}

/// Render a code as SVG, one `<path>` for every dark module.
///
/// A single path with many subpaths rather than one `<rect>` per module: a
/// version-40 code is 177×177, and 31,000 elements is a document the webview
/// has to lay out rather than a shape it can rasterise once.
fn to_svg(code: &QrCode, border: i32) -> String {
    let size = code.size();
    let dimension = size + border * 2;

    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if code.get_module(x, y) {
                if !path.is_empty() {
                    path.push(' ');
                }
                // `h1v1h-1z` is one module: right, down, left, close.
                path.push_str(&format!("M{},{}h1v1h-1z", x + border, y + border));
            }
        }
    }

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dimension} {dimension}\" \
         stroke=\"none\" shape-rendering=\"crispEdges\">\
         <rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\
         <path d=\"{path}\" fill=\"#000000\"/>\
         </svg>"
    )
}

/// The URL of the frontmost browser's active tab, if a browser is frontmost.
///
/// Every Chromium browser answers `URL of active tab of front window`; Safari
/// spells the same thing `URL of front document`. Nothing here *launches* a
/// browser — each branch is guarded on the app already running, so asking for a
/// QR code of "this tab" never starts Chrome to answer the question.
pub fn front_tab_url() -> Option<String> {
    const CHROMIUM: &[&str] = &[
        "Google Chrome",
        "Arc",
        "Brave Browser",
        "Microsoft Edge",
        "Vivaldi",
        "Chromium",
        "Dia",
        "Comet",
    ];

    let frontmost = super::apple::run_script(
        "tell application \"System Events\" to get name of first application process \
         whose frontmost is true",
    )
    .ok()?;
    let frontmost = frontmost.trim();

    let script = if frontmost == "Safari" {
        "tell application \"Safari\" to if it is running then return URL of front document"
            .to_string()
    } else if CHROMIUM.contains(&frontmost) {
        format!(
            "tell application \"{}\" to if it is running then return URL of active tab of front window",
            crate::shortcuts::escape_applescript(frontmost)
        )
    } else {
        return None;
    };

    let url = super::apple::run_script(&script).ok()?;
    let url = url.trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_encodes_to_a_well_formed_svg() {
        let out = svg("https://caduceus.vivaanshahani.com", "medium").unwrap();
        assert!(out.starts_with("<svg"));
        assert!(out.ends_with("</svg>"));
        assert!(out.contains("viewBox="));
        assert!(out.contains("<path"));
    }

    /// The quiet zone is what makes a code scannable; losing it is silent.
    #[test]
    fn the_quiet_zone_is_included_on_every_side() {
        let code = QrCode::encode_text("hello", QrCodeEcc::Medium).unwrap();
        let out = to_svg(&code, 4);
        let expected = code.size() + 8;
        assert!(out.contains(&format!("viewBox=\"0 0 {expected} {expected}\"")));
    }

    #[test]
    fn stronger_error_correction_produces_a_denser_code() {
        let text = "https://caduceus.vivaanshahani.com/docs";
        let low = QrCode::encode_text(text, QrCodeEcc::Low).unwrap().size();
        let high = QrCode::encode_text(text, QrCodeEcc::High).unwrap().size();
        assert!(high >= low, "high ECC should need at least as many modules");
    }

    #[test]
    fn nothing_to_encode_says_so_rather_than_producing_an_empty_code() {
        assert!(svg("   ", "medium").unwrap_err().contains("nothing"));
    }

    #[test]
    fn an_input_too_large_to_encode_is_refused_with_the_size() {
        let err = svg(&"x".repeat(MAX_INPUT + 1), "medium").unwrap_err();
        assert!(err.contains(&(MAX_INPUT + 1).to_string()));
    }

    /// Unicode must go through as UTF-8 rather than being truncated by byte.
    #[test]
    fn non_ascii_text_encodes() {
        assert!(svg("café — 日本語", "medium").is_ok());
    }
}
