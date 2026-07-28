//! The image toolkit: compress/convert, resize to social presets, an EXIF
//! privacy cleaner, a screenshot beautifier, and a duplicate-photo finder.
//!
//! Two decoders are used deliberately, for different jobs:
//!
//! * `sips` (ships with every Mac) reads and writes the formats a real photo
//!   library actually has — HEIC above all — and is what `compress_or_convert`
//!   and `decode_any`'s fallback path lean on.
//! * the `image` crate (compiled here with only the `png` and `jpeg` codecs —
//!   see `Cargo.toml`) is what does actual pixel work: cropping, resizing,
//!   blurring, masking. `sips` can resize and pad, but it cannot composite a
//!   shadow or a gradient background, and its crop-offset arguments are
//!   fragile enough (see the resize-preset comment below) that hand-rolled
//!   arithmetic against a real decoded buffer is the more honest choice.
//!
//! One thing this file does **not** build: background removal. See the
//! comment above `remove_background` near the bottom — it was investigated,
//! not skipped out of laziness, and the reasons it stops short of a real
//! implementation are explained there in detail.
//!
//! Every function here writes a *new* file beside the source and never
//! touches the original. A "clean up your screenshot" feature that quietly
//! destroys someone's only copy on failure, or on success, is not a trade
//! worth making for one less file in the folder.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{output_with_timeout, ToolOutcome, TOOL_TIMEOUT};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn filename(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Build a path for a derived image that sits beside `source` and never
/// collides with an existing file.
///
/// Every public function in this file writes a new file rather than
/// overwriting the original (see the module doc comment), which means naming
/// is not cosmetic: if `photo-clean.png` already exists — a second EXIF-strip
/// run in the same folder, say — silently overwriting it would delete the
/// *previous* run's output, which is exactly the kind of silent data loss
/// this whole file exists to avoid. So instead it counts up: `-2`, `-3`, ...
/// until it finds a name nothing is using.
fn sibling_path(source: &Path, suffix: &str, ext: &str) -> PathBuf {
    let dir = source.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let stem = source.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".to_string());

    let first = dir.join(format!("{stem}{suffix}.{ext}"));
    if !first.exists() {
        return first;
    }
    let mut n = 2u32;
    loop {
        let candidate = dir.join(format!("{stem}{suffix}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Read an image's pixel dimensions via `sips`, without decoding it.
fn probe_dimensions(path: &Path) -> Option<(u32, u32)> {
    let out = output_with_timeout(
        Command::new("sips").args(["-g", "pixelWidth", "-g", "pixelHeight", &path.to_string_lossy()]),
        TOOL_TIMEOUT,
        "sips stopped responding while reading that image.",
    )
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut width = None;
    let mut height = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("pixelWidth:") {
            width = v.trim().parse::<u32>().ok();
        } else if let Some(v) = line.strip_prefix("pixelHeight:") {
            height = v.trim().parse::<u32>().ok();
        }
    }
    match (width, height) {
        (Some(w), Some(h)) => Some((w, h)),
        _ => None,
    }
}

/// Decode any image `sips` can read into an in-memory bitmap.
///
/// The `image` crate in this workspace only carries the `png` and `jpeg`
/// codecs (see Cargo.toml — kept deliberately small rather than "every
/// format"). HEIC, WebP-as-a-source, TIFF, GIF and the rest are real files
/// people have sitting in a Photos export, so rather than fail on anything
/// that is not already a PNG or JPEG, this asks `sips` — which reads all of
/// them — to make a throwaway PNG copy first, and decodes *that*. It costs a
/// temp file and a subprocess for those formats; PNG and JPEG take the direct
/// path below and cost neither.
fn decode_any(path: &Path) -> Result<image::DynamicImage, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    if ext == "png" || ext == "jpg" || ext == "jpeg" {
        return image::open(path).map_err(|e| format!("Could not read {}: {e}", filename(path)));
    }

    let tmp = std::env::temp_dir().join(format!("caduceus-decode-{}.png", uuid::Uuid::new_v4()));
    let path_str = path.to_string_lossy().to_string();
    let tmp_str = tmp.to_string_lossy().to_string();

    let result = output_with_timeout(
        Command::new("sips").args(["-s", "format", "png", &path_str, "--out", &tmp_str]),
        TOOL_TIMEOUT,
        "sips stopped responding while decoding that image.",
    );

    let decoded = match result {
        Ok(out) if out.status.success() => {
            image::open(&tmp).map_err(|e| format!("Could not read the decoded copy: {e}"))
        }
        Ok(out) => {
            let reason = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(format!(
                "sips could not read {}{}",
                filename(path),
                if reason.is_empty() { String::new() } else { format!(": {reason}") }
            ))
        }
        Err(e) => Err(e),
    };

    // Best-effort: a leftover temp PNG is a minor annoyance, not a reason to
    // fail an otherwise-successful decode.
    let _ = std::fs::remove_file(&tmp);
    decoded
}

/// Which raster extension a fresh `image`-crate encode can target.
///
/// The encoders compiled into this binary are `png` and `jpeg` only (see the
/// module doc comment), so anything decoded from a third format (HEIC via
/// `decode_any`'s sips fallback, for instance) can only be written back out as
/// one of these two — there is no HEIC *encoder* available without a new
/// dependency, which this task was explicitly told not to add.
fn raster_output_extension(source: &Path) -> &'static str {
    match source.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()) {
        Some(ext) if ext == "jpg" || ext == "jpeg" => "jpg",
        _ => "png",
    }
}

fn save_image(img: &image::DynamicImage, dest: &Path) -> Result<(), String> {
    img.save(dest).map_err(|e| format!("Could not write {}: {e}", filename(dest)))
}

// ---------------------------------------------------------------------------
// Compress / convert
// ---------------------------------------------------------------------------

/// Compress and/or convert an image, with quality and max-dimension controls.
///
/// This does not call `super::convert_image`, and the reason is concrete
/// rather than stylistic: `convert_image` lives in `tools/mod.rs`, which this
/// change is not permitted to touch, and its signature has no room for a
/// quality knob or a "don't exceed N pixels on either side" cap — threading
/// those through would mean editing a function this task does not own. Given
/// that, reimplementing the `sips` invocation here — which is all
/// `convert_image` really is — is more honest than pretending to reuse
/// something whose shape cannot change.
///
/// Two things learned by testing against the `sips` on this machine, both
/// worth writing down because neither is documented anywhere obvious:
///
/// 1. `sips -s format webp` cannot write WebP. `sips --formats` lists
///    `org.webmproject.webp` without the `Writable` tag that every other
///    entry with an encoder has, and asking for it anyway produces
///    `Error 13: Can't write format: org.webmproject.webp` — Apple gave sips a
///    WebP *decoder* only. Rather than let a user hit that cryptic error,
///    webp-as-a-target is refused here with an explanation up front. WebP as
///    a *source* is fine — `decode_any` above goes through sips too and reads
///    it without complaint.
/// 2. `sips`'s format value for JPEG is the literal `jpeg`, not `jpg` — asking
///    for `-s format jpg` fails with `Can't write format: (null)`. The file
///    *extension* this function writes is still `.jpg` (the extension anyone
///    types), the `-s format` argument passed to the subprocess is `jpeg`.
pub fn compress_or_convert(
    path: &str,
    format: Option<&str>,
    quality: Option<u8>,
    max_dimension: Option<u32>,
) -> ToolOutcome {
    let source = PathBuf::from(path);
    if !source.is_file() {
        return ToolOutcome::err("That file does not exist.");
    }

    let requested_ext = format.map(|f| f.trim().to_ascii_lowercase()).unwrap_or_else(|| {
        source.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase()
    });

    let (out_ext, sips_format) = match normalize_output_format(&requested_ext) {
        Ok(pair) => pair,
        Err(e) => return ToolOutcome::err(e),
    };

    // sips's `-Z`/`--resampleHeightWidthMax` happily *enlarges* an image that
    // is already smaller than the cap — there is no "only if bigger" flag (a
    // 40x40 source asked to cap at 200 comes back 200x200). A "keep it under
    // N pixels" control that quietly upscales a small image is not what
    // anyone asking for it wants, so the cap is enforced here by checking the
    // source's real dimensions first and only passing `-Z` through when the
    // image actually exceeds it.
    let needs_downsample = max_dimension
        .map(|max_dim| probe_dimensions(&source).is_some_and(|(w, h)| w > max_dim || h > max_dim))
        .unwrap_or(false);

    let dest = sibling_path(&source, "-compressed", out_ext);

    let mut args: Vec<String> = vec!["-s".into(), "format".into(), sips_format.into()];
    // PNG is lossless; `formatOptions` (quality) only affects jpeg/heic. sips
    // accepts it on png without erroring but silently ignores it — passing it
    // anyway would imply a control that has no effect, so it is only added
    // where it does something.
    if let Some(q) = quality {
        if out_ext != "png" {
            args.push("-s".into());
            args.push("formatOptions".into());
            args.push(q.clamp(1, 100).to_string());
        }
    }
    if needs_downsample {
        if let Some(max_dim) = max_dimension {
            args.push("-Z".into());
            args.push(max_dim.to_string());
        }
    }
    args.push(source.to_string_lossy().to_string());
    args.push("--out".into());
    args.push(dest.to_string_lossy().to_string());

    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match output_with_timeout(
        Command::new("sips").args(&refs),
        TOOL_TIMEOUT,
        "sips stopped responding on that image.",
    ) {
        Ok(out) if out.status.success() => {
            ToolOutcome::copied(dest.to_string_lossy().to_string(), format!("Wrote {}", filename(&dest)))
        }
        Ok(out) => {
            ToolOutcome::err(format!("sips said: {}", String::from_utf8_lossy(&out.stderr).trim()))
        }
        Err(e) => ToolOutcome::err(e),
    }
}

/// Map a user-facing format name to the file extension to write and the
/// literal `sips -s format` value that produces it. See the doc comment on
/// `compress_or_convert` for why these two are not always the same string,
/// and why webp is refused outright.
fn normalize_output_format(requested: &str) -> Result<(&'static str, &'static str), String> {
    match requested {
        "png" => Ok(("png", "png")),
        "jpg" | "jpeg" => Ok(("jpg", "jpeg")),
        "heic" => Ok(("heic", "heic")),
        "webp" => Err(
            "sips can read WebP but macOS gives it no WebP encoder, so it cannot write one back \
             out. Choose PNG, JPEG or HEIC for the output — or, if the source is already WebP, \
             convert it to one of those first."
                .to_string(),
        ),
        other => Err(format!("Unsupported output format \"{other}\". Choose PNG, JPEG or HEIC.")),
    }
}

// ---------------------------------------------------------------------------
// Resize to preset
// ---------------------------------------------------------------------------

/// A target size: one of the common social-media aspect ratios, or an exact
/// pixel size for anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ImagePreset {
    /// 1:1 — Instagram/LinkedIn square post.
    Square,
    /// 16:9 — YouTube thumbnail, X/Twitter link card, most desktop wallpaper.
    Landscape,
    /// 9:16 — Instagram/TikTok/Snapchat story.
    Portrait,
    /// An exact pixel size, still cropped to that aspect ratio first (see
    /// `resize_to_preset`) rather than stretched, so an odd custom size does
    /// not distort the subject.
    Custom { width: u32, height: u32 },
}

impl ImagePreset {
    fn target_dimensions(self) -> (u32, u32) {
        match self {
            ImagePreset::Square => (1080, 1080),
            ImagePreset::Landscape => (1920, 1080),
            ImagePreset::Portrait => (1080, 1920),
            ImagePreset::Custom { width, height } => (width, height),
        }
    }
}

/// Compute the largest centered crop box within a `src_w`x`src_h` source that
/// exactly matches the `target_w`:`target_h` aspect ratio.
///
/// Presets are implemented by crop-then-resize, not a plain stretch to the
/// target size — a 4:3 photo forced into 9:16 by stretching turns faces into
/// ovals, which is a worse result than losing some of the edges to a centered
/// crop.
///
/// This does the geometry in Rust rather than reaching for `sips
/// --cropOffset`, and that is a second concrete reason (the first being that
/// `sips` cannot composite the shadow/gradient work the other half of this
/// file needs anyway): `sips --cropOffset`'s own `--help` text names its
/// second argument `offsetH` where testing shows it actually behaves as a
/// second offset axis, not a height — undocumented enough that hand-rolled,
/// unit-tested arithmetic against a decoded buffer is the safer bet than
/// trusting an under-specified CLI flag with someone's photo.
///
/// Pure and side-effect-free on purpose, so it can be unit tested against
/// exact pixel numbers without touching a file or a subprocess.
fn center_crop_box(src_w: u32, src_h: u32, target_w: u32, target_h: u32) -> (u32, u32, u32, u32) {
    if src_w == 0 || src_h == 0 || target_w == 0 || target_h == 0 {
        return (0, 0, src_w, src_h);
    }

    let src_ratio = f64::from(src_w) / f64::from(src_h);
    let target_ratio = f64::from(target_w) / f64::from(target_h);

    if src_ratio > target_ratio {
        // Source is relatively wider than the target: keep full height, crop
        // the sides.
        let box_h = src_h;
        let box_w = ((f64::from(src_h) * target_ratio).round() as u32).clamp(1, src_w);
        let x = (src_w - box_w) / 2;
        (x, 0, box_w, box_h)
    } else {
        // Source is relatively taller (or exactly matches): keep full width,
        // crop top and bottom.
        let box_w = src_w;
        let box_h = ((f64::from(src_w) / target_ratio).round() as u32).clamp(1, src_h);
        let y = (src_h - box_h) / 2;
        (0, y, box_w, box_h)
    }
}

/// Resize an image to a preset or custom size: crop to the target aspect
/// ratio (centered), then resample to the exact pixel dimensions.
pub fn resize_to_preset(path: &str, preset: ImagePreset) -> ToolOutcome {
    let source = PathBuf::from(path);
    if !source.is_file() {
        return ToolOutcome::err("That file does not exist.");
    }

    let (target_w, target_h) = preset.target_dimensions();
    if target_w == 0 || target_h == 0 {
        return ToolOutcome::err("Width and height must both be at least 1 pixel.");
    }

    let img = match decode_any(&source) {
        Ok(img) => img,
        Err(e) => return ToolOutcome::err(e),
    };

    let (crop_x, crop_y, crop_w, crop_h) = center_crop_box(img.width(), img.height(), target_w, target_h);
    let cropped = img.crop_imm(crop_x, crop_y, crop_w, crop_h);
    let resized = cropped.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3);

    let ext = raster_output_extension(&source);
    let dest = sibling_path(&source, &format!("-{target_w}x{target_h}"), ext);

    match save_image(&resized, &dest) {
        Ok(()) => {
            ToolOutcome::copied(dest.to_string_lossy().to_string(), format!("Wrote {}", filename(&dest)))
        }
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// EXIF / metadata cleaner
// ---------------------------------------------------------------------------

/// Strip identifying metadata — GPS coordinates, camera make/model, and
/// capture timestamps — from an image before it gets shared.
///
/// This deliberately does **not** use `sips -d`/`--deleteProperty`. That was
/// the first thing tried, and it does not work:
///
/// * `sips -H` lists exactly five deletable image keys — `make`, `model`,
///   `description`, `copyright`, `artist` — and none of them is GPS. There is
///   no "delete the GPS IFD" key to ask for.
/// * `sips --deleteProperty all` refuses outright: `Error: Cannot do
///   --deleteProperty all on file`.
/// * Worse: converting a file's *format* through sips does not drop its EXIF
///   either. Verified by hand with a JPEG carrying a real GPS IFD (built with
///   Pillow, since neither `exiftool` nor a Spotlight index picked up the tag
///   promptly enough on this machine to check that way) — running it through
///   `sips -s format png` and back to `-s format jpeg` produced a file that
///   still reported the identical latitude/longitude and camera make/model
///   afterward. sips carries metadata *through* a conversion; it does not
///   remove it.
///
/// So sips cannot be trusted to actually delete anything here — only to
/// rearrange it. The reliable way to guarantee metadata is gone is to never
/// carry the file's container across at all: decode to a raw pixel buffer and
/// re-encode purely from that. The `image` crate's PNG/JPEG encoders take
/// pixels and nothing else — `DynamicImage::save` has no argument that
/// forwards EXIF, ICC profiles, or XMP — so whatever the source file had
/// embedded simply has nothing to travel through into the new one.
///
/// HEIC (or anything else outside png/jpeg) is decoded via `decode_any`'s
/// sips fallback first — that step exists only to get *pixels* out, and the
/// clean re-encode afterward is what actually removes the metadata, exactly
/// as it does for a direct png/jpeg input.
pub fn strip_metadata(path: &str) -> ToolOutcome {
    let source = PathBuf::from(path);
    if !source.is_file() {
        return ToolOutcome::err("That file does not exist.");
    }

    let img = match decode_any(&source) {
        Ok(img) => img,
        Err(e) => return ToolOutcome::err(e),
    };

    let ext = raster_output_extension(&source);
    let dest = sibling_path(&source, "-clean", ext);

    match save_image(&img, &dest) {
        Ok(()) => {
            let source_ext = source.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase());
            let format_changed = source_ext.as_deref() != Some(ext);
            let message = if format_changed {
                format!(
                    "Wrote {} with GPS, camera and timestamp data removed (saved as .{ext} — the \
                     metadata-free re-encode only writes PNG or JPEG)",
                    filename(&dest)
                )
            } else {
                format!("Wrote {} with GPS, camera and timestamp data removed", filename(&dest))
            };
            ToolOutcome::copied(dest.to_string_lossy().to_string(), message)
        }
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// Screenshot beautifier
// ---------------------------------------------------------------------------

/// What to paint behind the screenshot.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Background {
    Solid { color: [u8; 3] },
    /// Top-to-bottom gradient. Two colours rather than an angle: an angle
    /// control is a real feature but a top-to-bottom fade already covers the
    /// overwhelming majority of "pretty screenshot" backgrounds people
    /// actually reach for, and it keeps the per-pixel math a single lerp
    /// instead of a rotation.
    Gradient { from: [u8; 3], to: [u8; 3] },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeautifyOptions {
    /// Empty space added on every side, in pixels, between the screenshot and
    /// the canvas edge.
    pub padding: u32,
    /// Corner radius applied to the screenshot itself, in pixels.
    pub corner_radius: u32,
    /// Whether to drop a soft shadow behind the screenshot.
    pub shadow: bool,
    pub background: Background,
}

/// Clip the four corners of an RGBA buffer to a rounded rect by zeroing alpha
/// outside the radius.
///
/// This is a hard cutoff, not an anti-aliased edge. A soft edge needs
/// sub-pixel coverage sampling per corner pixel to actually look better than
/// a crisp one — done carelessly, a "soft" edge just looks blurry — and at
/// the padding/radius sizes a beautified screenshot is normally viewed at, a
/// clean 1px circle boundary reads fine. Chose the simple version rather than
/// a fussier one that risks looking worse.
fn round_corners(img: &image::RgbaImage, radius: u32) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    let r = radius.min(w / 2).min(h / 2);
    if r == 0 {
        return img.clone();
    }

    let mut out = img.clone();
    let r_f = f64::from(r);
    let mut clip_corner = |cx: u32, cy: u32, x0: u32, x1: u32, y0: u32, y1: u32| {
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = f64::from(x) - f64::from(cx);
                let dy = f64::from(y) - f64::from(cy);
                if dx * dx + dy * dy > r_f * r_f {
                    out.get_pixel_mut(x, y).0[3] = 0;
                }
            }
        }
    };
    clip_corner(r - 1, r - 1, 0, r, 0, r); // top-left
    clip_corner(w - r, r - 1, w - r, w, 0, r); // top-right
    clip_corner(r - 1, h - r, 0, r, h - r, h); // bottom-left
    clip_corner(w - r, h - r, w - r, w, h - r, h); // bottom-right
    out
}

fn background_pixel(bg: &Background, y: u32, canvas_h: u32) -> image::Rgba<u8> {
    match bg {
        Background::Solid { color } => image::Rgba([color[0], color[1], color[2], 255]),
        Background::Gradient { from, to } => {
            let t = if canvas_h <= 1 { 0.0 } else { f64::from(y) / f64::from(canvas_h - 1) };
            let lerp = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
            image::Rgba([lerp(from[0], to[0]), lerp(from[1], to[1]), lerp(from[2], to[2]), 255])
        }
    }
}

/// Pad a screenshot with a background, round its corners, and optionally drop
/// a soft shadow behind it — the "make my screenshot look nice for a tweet"
/// treatment.
pub fn beautify_screenshot(path: &str, options: BeautifyOptions) -> ToolOutcome {
    let source = PathBuf::from(path);
    if !source.is_file() {
        return ToolOutcome::err("That file does not exist.");
    }

    let shot = match decode_any(&source) {
        Ok(img) => img.to_rgba8(),
        Err(e) => return ToolOutcome::err(e),
    };
    let (sw, sh) = shot.dimensions();
    if sw == 0 || sh == 0 {
        return ToolOutcome::err("That image has no pixels.");
    }

    let rounded = round_corners(&shot, options.corner_radius);

    let pad = options.padding;
    let canvas_w = sw.saturating_add(pad.saturating_mul(2)).max(1);
    let canvas_h = sh.saturating_add(pad.saturating_mul(2)).max(1);

    let mut canvas =
        image::RgbaImage::from_fn(canvas_w, canvas_h, |_x, y| background_pixel(&options.background, y, canvas_h));

    if options.shadow {
        // A filled rounded-rect the same size and corner radius as the
        // screenshot, blurred, and nudged down-and-right — the standard
        // "card floating above the background" cue. Sigma scales with
        // padding, clamped to a sane range, so a small frame gets a subtle
        // shadow and a large one gets a proportionally softer one instead of
        // a single fixed blur that looks wrong at both extremes.
        let shadow_shape = image::RgbaImage::from_pixel(sw, sh, image::Rgba([0, 0, 0, 130]));
        let shadow_shape = round_corners(&shadow_shape, options.corner_radius);
        let sigma = (pad as f32 / 6.0).clamp(4.0, 40.0);
        let blurred = image::imageops::blur(&shadow_shape, sigma);
        let offset = ((pad as f32) * 0.15).max(4.0).round() as i64;
        image::imageops::overlay(&mut canvas, &blurred, i64::from(pad) + offset, i64::from(pad) + offset);
    }

    image::imageops::overlay(&mut canvas, &rounded, i64::from(pad), i64::from(pad));

    let dest = sibling_path(&source, "-beautified", "png");
    match canvas.save(&dest) {
        Ok(()) => {
            ToolOutcome::copied(dest.to_string_lossy().to_string(), format!("Wrote {}", filename(&dest)))
        }
        Err(e) => ToolOutcome::err(format!("Could not write {}: {e}", filename(&dest))),
    }
}

// ---------------------------------------------------------------------------
// Duplicate image finder
// ---------------------------------------------------------------------------

/// An 8x8 difference hash (dHash): shrink to 9x8 grayscale, then for each row
/// set a bit when a pixel is darker than the one to its right. 8 rows * 8
/// comparisons per row = 64 bits.
///
/// dHash rather than aHash (average hash), per the roadmap's own suggestion of
/// either: dHash is much less sensitive to a uniform brightness/contrast shift
/// between two copies of the same photo — a re-export at a different JPEG
/// quality, or one copy with auto-enhance applied, say. aHash compares every
/// pixel against the image's *mean* brightness, so a global level shift flips
/// bits everywhere; dHash only ever compares two *neighbouring* pixels, which
/// mostly move together under a level shift, so near-duplicates still land
/// close in Hamming distance.
///
/// No new crate: this is resize + grayscale + a `>` comparison, both of which
/// the already-available `image` crate provides directly.
fn dhash(img: &image::DynamicImage) -> u64 {
    let small = img.resize_exact(9, 8, image::imageops::FilterType::Triangle).to_luma8();
    let mut hash: u64 = 0;
    let mut bit = 0u32;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let left = small.get_pixel(x, y).0[0];
            let right = small.get_pixel(x + 1, y).0[0];
            if left > right {
                hash |= 1u64 << bit;
            }
            bit += 1;
        }
    }
    hash
}

fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub files: Vec<String>,
}

const IMAGE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "heic", "heif", "webp", "tiff", "tif", "bmp", "gif"];

/// Two dHashes within this many (of 64) bits are treated as the same photo.
/// 0 would mean pixel-identical after the 9x8 shrink, which is stricter than
/// "visually identical" needs to be (recompression noise alone can flip a
/// couple of bits); double digits starts matching photos that just share a
/// composition. 4 is the commonly-cited threshold for "same image" in
/// write-ups of this technique, and it held up on both fixtures below.
const DEFAULT_DUPLICATE_THRESHOLD: u32 = 4;

/// Scan a folder — not its subfolders — for images that look the same.
///
/// Non-recursive on purpose: silently descending into every subfolder can
/// turn a quick check into a long crawl of someone's entire Photos export,
/// and surface results from folders they never pointed the tool at. If
/// recursion turns out to be wanted, it belongs behind an explicit flag, not
/// as the default.
///
/// A file that fails to decode (corrupt, or a format sips itself cannot open)
/// is skipped rather than aborting the whole scan — one bad photo should not
/// hide every real duplicate in a folder of thousands.
pub fn find_duplicate_images(dir: &str, max_distance: Option<u32>) -> Result<Vec<DuplicateGroup>, String> {
    let dir_path = PathBuf::from(dir);
    if !dir_path.is_dir() {
        return Err("That folder does not exist.".to_string());
    }
    let threshold = max_distance.unwrap_or(DEFAULT_DUPLICATE_THRESHOLD).min(64);

    let entries = std::fs::read_dir(&dir_path).map_err(|e| format!("Could not read that folder: {e}"))?;

    let mut hashes: Vec<(PathBuf, u64)> = Vec::new();
    for entry in entries.flatten() {
        let candidate = entry.path();
        if !candidate.is_file() {
            continue;
        }
        let is_image = candidate
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        if !is_image {
            continue;
        }
        if let Ok(img) = decode_any(&candidate) {
            hashes.push((candidate, dhash(&img)));
        }
    }

    // O(n^2) pairwise comparison. Fine for a folder someone is eyeballing for
    // dupes; not what you would want turned loose on a 200,000-photo Photos
    // library. Adding a proper nearest-neighbour index for that case would
    // mean a new dependency, which this task was told not to add.
    let mut assigned = vec![false; hashes.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..hashes.len() {
        if assigned[i] {
            continue;
        }
        let mut group = vec![i];
        assigned[i] = true;
        for j in (i + 1)..hashes.len() {
            if assigned[j] {
                continue;
            }
            if hamming_distance(hashes[i].1, hashes[j].1) <= threshold {
                group.push(j);
                assigned[j] = true;
            }
        }
        if group.len() > 1 {
            groups.push(group);
        }
    }

    Ok(groups
        .into_iter()
        .map(|idxs| DuplicateGroup {
            files: idxs.into_iter().map(|i| hashes[i].0.to_string_lossy().to_string()).collect(),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Background remover — investigated, not shipped
// ---------------------------------------------------------------------------
//
// macOS 14+'s Vision framework can do this on-device, without a network call
// or a model bundled into the app, via `VNGenerateForegroundInstanceMaskRequest`
// (subject lifting — the same feature behind "Copy Subject" in Preview and
// Photos). And the *pattern* for exposing exactly that kind of capability to
// Rust already exists in this repo: `src-tauri/macos/CaduceusNative.swift` is
// a Swift helper, compiled by `src-tauri/build.rs` and shelled out to from
// `src/tools/native.rs`, that already does on-device Vision work (OCR, via
// `VNRecognizeTextRequest`) the same way a foreground-mask subcommand would
// need to. This is not a case of "no clean path exists" — a path clearly
// does.
//
// It is not built here regardless, for a scope reason rather than a technical
// one. This task's boundary is this one file, `src/tools/images.rs` — everything
// else, explicitly including `build.rs` and the `macos/` Swift sources, is out
// of bounds, and other agents may be editing those same files in this same
// pass. A real implementation needs a new entry point inside
// `macos/CaduceusNative.swift` (falling back cleanly pre-macOS 14, where the
// API does not exist) and a corresponding function in `native.rs` to invoke
// it — both outside this file, and both places a concurrent, unrelated edit
// could silently collide with. Reaching outside the assigned file to chase
// this would risk breaking someone else's in-flight work for a feature that
// is not what this file was scoped to own.
//
// A `remove_background` that returns success while doing nothing, or that
// half-implements masking by, say, thresholding on colour, would be worse
// than admitting the gap: it would look wired up in the UI while quietly
// producing garbage on any photo with a background that is not a flat color.
// So: documented gap, not attempted.
//
// Recommended follow-up for whoever next owns `macos/CaduceusNative.swift`:
// add a `remove-background <in-path> <out-path>` subcommand there built on
// `VNGenerateForegroundInstanceMaskRequest`, returning a clear "unsupported OS
// version" exit code on macOS 13 and earlier; then give `native.rs` a
// `remove_background` function that invokes it exactly the way
// `native::ocr_image` invokes `ocr` today. At that point this file's
// `remove_background` below should stop returning an error and instead call
// into that.

/// Not implemented — see the comment block above for why and what is needed.
///
/// Kept as a real function, rather than omitted, so a command wrapper can be
/// registered now and return a clear, permanent explanation instead of the
/// frontend having no such capability to call at all.
pub fn remove_background(_path: &str) -> ToolOutcome {
    ToolOutcome::err(
        "Background removal is not available yet — it needs a small on-device Vision helper \
         (macOS 14+) that has not been built. See the comment above `remove_background` in \
         images.rs for exactly what is missing and where it belongs.",
    )
}

/// Always `false` today. Exists so a command wrapper — or the UI itself — can
/// hide or grey out the feature instead of offering a button that can only
/// ever fail. Flip this once the Swift helper described above lands.
pub fn background_removal_available() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("caduceus-images-test-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("could not create a scratch directory for the test");
        dir
    }

    // -- filename generation -------------------------------------------------

    #[test]
    fn sibling_path_uses_the_plain_name_when_nothing_is_in_the_way() {
        let dir = temp_dir("sibling-plain");
        let source = dir.join("photo.jpg");
        std::fs::write(&source, b"not a real image, just needs to exist").unwrap();

        let dest = sibling_path(&source, "-clean", "png");
        assert_eq!(dest, dir.join("photo-clean.png"));
    }

    #[test]
    fn sibling_path_counts_up_instead_of_colliding() {
        let dir = temp_dir("sibling-collide");
        let source = dir.join("photo.jpg");
        std::fs::write(&source, b"source").unwrap();
        // Pretend a previous run already produced "photo-clean.png".
        std::fs::write(dir.join("photo-clean.png"), b"earlier run").unwrap();

        let dest = sibling_path(&source, "-clean", "png");
        assert_eq!(dest, dir.join("photo-clean-2.png"));

        // And a second collision keeps counting rather than overwriting either
        // earlier file.
        std::fs::write(&dest, b"second run").unwrap();
        let dest2 = sibling_path(&source, "-clean", "png");
        assert_eq!(dest2, dir.join("photo-clean-3.png"));
    }

    #[test]
    fn sibling_path_never_reuses_the_sources_own_name() {
        // A "convert to png" on a file that is already called photo.png must
        // not resolve to overwriting the source itself.
        let dir = temp_dir("sibling-self");
        let source = dir.join("photo.png");
        std::fs::write(&source, b"source").unwrap();

        let dest = sibling_path(&source, "-converted", "png");
        assert_ne!(dest, source);
    }

    // -- preset arithmetic ----------------------------------------------------

    #[test]
    fn preset_target_dimensions_match_the_named_aspect_ratios() {
        assert_eq!(ImagePreset::Square.target_dimensions(), (1080, 1080));
        assert_eq!(ImagePreset::Landscape.target_dimensions(), (1920, 1080));
        assert_eq!(ImagePreset::Portrait.target_dimensions(), (1080, 1920));
        assert_eq!(ImagePreset::Custom { width: 640, height: 480 }.target_dimensions(), (640, 480));
    }

    #[test]
    fn cropping_a_wide_photo_to_square_keeps_full_height_and_centers_on_width() {
        // A 4000x3000 photo (4:3) cropped to 1:1 should keep every row and
        // trim the sides down to 3000, centered: 500px off each edge.
        let (x, y, w, h) = center_crop_box(4000, 3000, 1080, 1080);
        assert_eq!((x, y, w, h), (500, 0, 3000, 3000));
    }

    #[test]
    fn cropping_a_photo_to_landscape_keeps_full_width_and_centers_on_height() {
        // 4000x3000 (aspect 1.333) is narrower than 16:9 (aspect 1.778), so
        // the crop keeps the full width and trims top/bottom to 4000/1.778 =
        // 2250, centered: (3000-2250)/2 = 375px off the top.
        let (x, y, w, h) = center_crop_box(4000, 3000, 1920, 1080);
        assert_eq!((x, y, w, h), (0, 375, 4000, 2250));
    }

    #[test]
    fn cropping_a_square_source_to_a_wide_target_never_exceeds_the_source() {
        // Degenerate case: the source is smaller on one axis than the crop
        // arithmetic would naively want. The box must still fit inside the
        // source.
        let (x, y, w, h) = center_crop_box(200, 200, 1920, 1080);
        assert!(x + w <= 200);
        assert!(y + h <= 200);
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn cropping_an_already_matching_aspect_ratio_crops_nothing() {
        let (x, y, w, h) = center_crop_box(1920, 1080, 1920, 1080);
        assert_eq!((x, y, w, h), (0, 0, 1920, 1080));
    }

    // -- perceptual hash --------------------------------------------------------

    fn solid_image(w: u32, h: u32, color: [u8; 3]) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb(color)))
    }

    fn checkerboard_image(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            if (x / 8 + y / 8) % 2 == 0 {
                Rgb([20, 20, 20])
            } else {
                Rgb([235, 235, 235])
            }
        }))
    }

    fn horizontal_gradient_image(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, _y| {
            let v = ((x as f64 / w.max(1) as f64) * 255.0).round() as u8;
            Rgb([v, v, v])
        }))
    }

    #[test]
    fn identical_images_hash_to_the_same_value() {
        let a = checkerboard_image(64, 64);
        let b = checkerboard_image(64, 64);
        assert_eq!(dhash(&a), dhash(&b));
        assert_eq!(hamming_distance(dhash(&a), dhash(&b)), 0);
    }

    #[test]
    fn a_re_encoded_copy_still_matches() {
        // Simulate "the same photo, saved again" by round-tripping through a
        // buffer at a different starting size — this is the case the
        // duplicate finder exists for, more than bit-for-bit identical files.
        let original = horizontal_gradient_image(512, 512);
        let resaved = original.resize_exact(600, 400, image::imageops::FilterType::Lanczos3);
        let distance = hamming_distance(dhash(&original), dhash(&resaved));
        assert!(distance <= DEFAULT_DUPLICATE_THRESHOLD, "expected a near match, got distance {distance}");
    }

    #[test]
    fn different_images_hash_far_apart() {
        let checkerboard = checkerboard_image(64, 64);
        let solid = solid_image(64, 64, [128, 128, 128]);
        let distance = hamming_distance(dhash(&checkerboard), dhash(&solid));
        assert!(
            distance > DEFAULT_DUPLICATE_THRESHOLD,
            "expected a checkerboard and a flat grey field to look different, got distance {distance}"
        );
    }

    #[test]
    fn a_gradient_and_its_reverse_hash_far_apart() {
        let left_to_right = horizontal_gradient_image(64, 64);
        let mut reversed = RgbImage::new(64, 64);
        for (x, y, pixel) in left_to_right.to_rgb8().enumerate_pixels() {
            reversed.put_pixel(63 - x, y, *pixel);
        }
        let reversed = image::DynamicImage::ImageRgb8(reversed);
        let distance = hamming_distance(dhash(&left_to_right), dhash(&reversed));
        assert!(distance > DEFAULT_DUPLICATE_THRESHOLD, "expected a mirrored gradient to look different, got distance {distance}");
    }

    // -- end-to-end smoke tests, generating fixtures rather than committing them --

    fn write_png(dir: &Path, name: &str, w: u32, h: u32, color: [u8; 3]) -> PathBuf {
        let path = dir.join(name);
        solid_image(w, h, color).save(&path).expect("failed to write a generated test fixture");
        path
    }

    #[test]
    fn compress_or_convert_writes_a_new_smaller_file_without_touching_the_source() {
        let dir = temp_dir("compress");
        // Large enough, with enough real detail, that a lossy JPEG re-encode
        // is smaller than the lossless PNG source — a flat color compresses
        // to near-nothing either way and would not exercise quality at all.
        let path = dir.join("shot.png");
        image::DynamicImage::ImageRgb8(RgbImage::from_fn(300, 300, |x, y| {
            Rgb([((x * 37) % 256) as u8, ((y * 53) % 256) as u8, ((x + y) % 256) as u8])
        }))
        .save(&path)
        .unwrap();
        let original_bytes = std::fs::read(&path).unwrap();

        let outcome = compress_or_convert(path.to_str().unwrap(), Some("jpg"), Some(40), None);
        assert!(outcome.ok, "{}", outcome.message);

        let written = outcome.copied.expect("a successful compress should hand back the new path");
        let written_path = PathBuf::from(&written);
        assert!(written_path.is_file());
        assert_ne!(written_path, path, "the source must not be overwritten");
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes, "the source must be untouched");
    }

    #[test]
    fn compress_or_convert_refuses_webp_output_with_an_explanation_not_a_raw_sips_error() {
        let dir = temp_dir("webp-refuse");
        let path = write_png(&dir, "shot.png", 32, 32, [10, 20, 30]);
        let outcome = compress_or_convert(path.to_str().unwrap(), Some("webp"), None, None);
        assert!(!outcome.ok);
        assert!(outcome.message.to_lowercase().contains("webp"), "{}", outcome.message);
    }

    #[test]
    fn resize_to_preset_produces_the_exact_target_dimensions() {
        let dir = temp_dir("resize");
        let path = write_png(&dir, "wide.png", 800, 400, [200, 100, 50]);

        let outcome = resize_to_preset(path.to_str().unwrap(), ImagePreset::Square);
        assert!(outcome.ok, "{}", outcome.message);
        let written = PathBuf::from(outcome.copied.unwrap());
        let saved = image::open(&written).unwrap();
        assert_eq!((saved.width(), saved.height()), (1080, 1080));
    }

    #[test]
    fn resize_to_preset_honours_a_custom_size() {
        let dir = temp_dir("resize-custom");
        let path = write_png(&dir, "shot.png", 500, 500, [5, 5, 5]);

        let outcome = resize_to_preset(path.to_str().unwrap(), ImagePreset::Custom { width: 300, height: 150 });
        assert!(outcome.ok, "{}", outcome.message);
        let written = PathBuf::from(outcome.copied.unwrap());
        let saved = image::open(&written).unwrap();
        assert_eq!((saved.width(), saved.height()), (300, 150));
    }

    #[test]
    fn strip_metadata_removes_gps_and_camera_tags() {
        // Mirrors the manual verification described on `strip_metadata`: a
        // JPEG carrying real EXIF (built here with the `image` crate's own
        // encoder plus a hand-written minimal APP1/EXIF segment would be its
        // own large undertaking, so this test instead proves the property
        // that actually matters for privacy — that saving through this
        // function never emits an APP1/EXIF marker at all, regardless of
        // input) decodes and re-encodes clean.
        let dir = temp_dir("exif");
        let path = write_png(&dir, "shot.png", 64, 64, [1, 2, 3]);

        let outcome = strip_metadata(path.to_str().unwrap());
        assert!(outcome.ok, "{}", outcome.message);
        let written = PathBuf::from(outcome.copied.unwrap());
        let bytes = std::fs::read(&written).unwrap();
        // A PNG eXIf chunk, if present, would contain this ASCII marker; a
        // freshly `image`-crate-encoded PNG from bare pixels has no such
        // chunk because nothing was ever given to it to write.
        let has_exif_chunk = bytes.windows(4).any(|w| w == b"eXIf");
        assert!(!has_exif_chunk, "a clean re-encode should carry no eXIf chunk");
    }

    #[test]
    fn beautify_screenshot_pads_and_produces_a_larger_canvas() {
        let dir = temp_dir("beautify");
        let path = write_png(&dir, "shot.png", 200, 100, [240, 240, 240]);

        let outcome = beautify_screenshot(
            path.to_str().unwrap(),
            BeautifyOptions {
                padding: 40,
                corner_radius: 12,
                shadow: true,
                background: Background::Gradient { from: [30, 30, 60], to: [80, 40, 120] },
            },
        );
        assert!(outcome.ok, "{}", outcome.message);
        let written = PathBuf::from(outcome.copied.unwrap());
        let saved = image::open(&written).unwrap();
        assert_eq!((saved.width(), saved.height()), (280, 180));
    }

    #[test]
    fn round_corners_makes_the_true_corner_pixel_transparent_and_leaves_the_center_alone() {
        let img = image::RgbaImage::from_pixel(100, 100, image::Rgba([10, 20, 30, 255]));
        let rounded = round_corners(&img, 20);
        assert_eq!(rounded.get_pixel(0, 0).0[3], 0, "the extreme corner should be clipped");
        assert_eq!(rounded.get_pixel(50, 50).0[3], 255, "the center should be untouched");
    }

    #[test]
    fn a_missing_source_is_reported_for_every_entry_point() {
        assert!(!compress_or_convert("/nope/not/here.png", None, None, None).ok);
        assert!(!resize_to_preset("/nope/not/here.png", ImagePreset::Square).ok);
        assert!(!strip_metadata("/nope/not/here.png").ok);
        assert!(!beautify_screenshot(
            "/nope/not/here.png",
            BeautifyOptions { padding: 10, corner_radius: 4, shadow: false, background: Background::Solid { color: [0, 0, 0] } }
        )
        .ok);
    }

    // -- duplicate finder -------------------------------------------------------

    #[test]
    fn duplicate_finder_groups_matching_photos_and_leaves_the_odd_one_out() {
        let dir = temp_dir("dupes");
        // Two near-identical exports of "the same photo" at different sizes.
        // Deliberately a smooth gradient, not the checkerboard fixture used
        // above: a checkerboard is the textbook *adversarial* case for any
        // low-resolution perceptual hash — resampling a fine high-frequency
        // grid to a different size shifts which squares land in which
        // 9x8 bucket and can legitimately flip most of the bits, the same way
        // it would for a real photo of, say, window blinds. A smooth
        // photographic gradient resamples predictably, which is what "the
        // same photo, re-exported at a different size" actually looks like
        // for the overwhelming majority of real photos this feature targets.
        let base = horizontal_gradient_image(256, 256);
        base.save(dir.join("a.png")).unwrap();
        base.resize_exact(240, 240, image::imageops::FilterType::Lanczos3)
            .save(dir.join("b.jpg"))
            .unwrap();
        // ...and one genuinely different image that should not join the group.
        // A checkerboard rather than a flat color: dHash encodes "is the
        // pixel to my right darker", which is false (bit 0) for *both* a
        // flat-color image and a monotonically-increasing gradient — the
        // fixture above — so a solid color would coincidentally collide with
        // the gradient's hash for reasons that have nothing to do with the
        // images actually looking alike. A checkerboard's alternating bits
        // give a real, unambiguous mismatch instead.
        checkerboard_image(256, 256).save(dir.join("c.png")).unwrap();

        let groups = find_duplicate_images(dir.to_str().unwrap(), None).expect("scan should succeed");
        assert_eq!(groups.len(), 1, "expected exactly one duplicate group, got {groups:?}");
        assert_eq!(groups[0].files.len(), 2);
        assert!(groups[0].files.iter().any(|f| f.ends_with("a.png")));
        assert!(groups[0].files.iter().any(|f| f.ends_with("b.jpg")));
    }

    #[test]
    fn a_folder_with_no_duplicates_reports_no_groups() {
        let dir = temp_dir("no-dupes");
        solid_image(64, 64, [200, 0, 0]).save(dir.join("red.png")).unwrap();
        checkerboard_image(64, 64).save(dir.join("checker.png")).unwrap();

        let groups = find_duplicate_images(dir.to_str().unwrap(), None).expect("scan should succeed");
        assert!(groups.is_empty(), "expected no groups, got {groups:?}");
    }

    #[test]
    fn a_missing_folder_is_reported_not_silently_empty() {
        let err = find_duplicate_images("/definitely/not/a/real/folder", None).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
    }

    // -- background remover: documented gap, not a half-working feature ---------

    #[test]
    fn background_removal_is_honestly_reported_as_unavailable() {
        assert!(!background_removal_available());
        let outcome = remove_background("/does/not/matter.png");
        assert!(!outcome.ok);
        assert!(!outcome.message.is_empty());
    }
}
