//! Screen capture and input simulation — the "hands and eyes" of computer use.
//!
//! # Coordinate spaces
//!
//! Three different pixel grids are in play, and mixing them up is the classic
//! way computer-use implementations end up clicking 40px off on a Retina
//! display:
//!
//! | space          | where it comes from                    | example        |
//! |----------------|----------------------------------------|----------------|
//! | **capture**    | `xcap` `capture_image()` — real pixels  | 3024 × 1964    |
//! | **input**      | `xcap` `width()/height()` — what `enigo` addresses | 1512 × 982 |
//! | **model**      | the downscaled screenshot we send       | 1280 × 831     |
//!
//! [`Screenshot`] carries the factors needed to convert, and
//! [`ComputerController::to_input_space`] is the only place the conversion is
//! done.
//!
//! # Threading
//!
//! `enigo::Enigo` owns platform event sources that are not safe to share across
//! threads, so exactly one thread owns it and receives work over a channel.
//! Everything else talks to that thread through [`ComputerController`], which is
//! cheap to clone and `Send + Sync`.

use std::sync::mpsc;
use std::time::Duration;

use base64::Engine as _;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use image::ImageEncoder;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ComputerError {
    #[error("no monitor at index {0}")]
    NoSuchMonitor(usize),
    #[error("screen capture failed: {0}. On macOS, grant Orbit Screen Recording access in System Settings \u{2192} Privacy & Security.")]
    Capture(String),
    #[error("input simulation failed: {0}. On macOS, grant Orbit Accessibility access in System Settings \u{2192} Privacy & Security.")]
    Input(String),
    #[error("the input thread is not running")]
    ThreadGone,
    #[error("unsupported action: {0}")]
    Unsupported(String),
    #[error("{0}")]
    BadArguments(String),
}

pub type ComputerResult<T> = Result<T, ComputerError>;

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// The action set Orbit implements, mirroring Anthropic's computer-use tool.
///
/// Deserialised straight from the model's `tool_use` input, which is why the
/// field names match the API rather than Rust convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ComputerAction {
    Screenshot,
    /// Inspect a region at full resolution. `region` is `[x1, y1, x2, y2]` in
    /// model space. Only offered when `enable_zoom` is set.
    Zoom {
        region: [i32; 4],
    },
    MouseMove {
        coordinate: [i32; 2],
    },
    LeftClick {
        coordinate: Option<[i32; 2]>,
        /// Modifier keys held during the click, e.g. `"shift"` or `"ctrl+alt"`.
        text: Option<String>,
    },
    RightClick {
        coordinate: Option<[i32; 2]>,
        text: Option<String>,
    },
    MiddleClick {
        coordinate: Option<[i32; 2]>,
        text: Option<String>,
    },
    DoubleClick {
        coordinate: Option<[i32; 2]>,
        text: Option<String>,
    },
    TripleClick {
        coordinate: Option<[i32; 2]>,
        text: Option<String>,
    },
    LeftMouseDown {
        coordinate: Option<[i32; 2]>,
    },
    LeftMouseUp {
        coordinate: Option<[i32; 2]>,
    },
    LeftClickDrag {
        /// Where the drag ends.
        coordinate: [i32; 2],
        /// Where it starts; defaults to the current pointer position.
        start_coordinate: Option<[i32; 2]>,
    },
    Type {
        text: String,
    },
    Key {
        /// xdotool-style combo, e.g. `"ctrl+s"`, `"Return"`, `"super+shift+4"`.
        text: String,
    },
    HoldKey {
        text: String,
        duration: f64,
    },
    Scroll {
        coordinate: Option<[i32; 2]>,
        scroll_direction: String,
        scroll_amount: i32,
        text: Option<String>,
    },
    Wait {
        duration: f64,
    },
    CursorPosition,
}

impl ComputerAction {
    /// One-line description for the step feed the user watches.
    pub fn describe(&self) -> String {
        match self {
            Self::Screenshot => "Looked at the screen".into(),
            Self::Zoom { region } => format!("Zoomed into {region:?}"),
            Self::MouseMove { coordinate } => format!("Moved to {},{}", coordinate[0], coordinate[1]),
            Self::LeftClick { coordinate, .. } => format!("Clicked {}", fmt_coord(coordinate)),
            Self::RightClick { coordinate, .. } => format!("Right-clicked {}", fmt_coord(coordinate)),
            Self::MiddleClick { coordinate, .. } => format!("Middle-clicked {}", fmt_coord(coordinate)),
            Self::DoubleClick { coordinate, .. } => format!("Double-clicked {}", fmt_coord(coordinate)),
            Self::TripleClick { coordinate, .. } => format!("Triple-clicked {}", fmt_coord(coordinate)),
            Self::LeftMouseDown { .. } => "Pressed the mouse button".into(),
            Self::LeftMouseUp { .. } => "Released the mouse button".into(),
            Self::LeftClickDrag { coordinate, .. } => {
                format!("Dragged to {},{}", coordinate[0], coordinate[1])
            }
            Self::Type { text } => format!("Typed \u{201c}{}\u{201d}", truncate(text, 48)),
            Self::Key { text } => format!("Pressed {text}"),
            Self::HoldKey { text, duration } => format!("Held {text} for {duration}s"),
            Self::Scroll {
                scroll_direction,
                scroll_amount,
                ..
            } => format!("Scrolled {scroll_direction} \u{d7}{scroll_amount}"),
            Self::Wait { duration } => format!("Waited {duration}s"),
            Self::CursorPosition => "Checked the pointer position".into(),
        }
    }

    /// Whether this action changes anything on screen. Read-only actions skip
    /// the settle delay.
    pub fn is_mutating(&self) -> bool {
        !matches!(
            self,
            Self::Screenshot | Self::CursorPosition | Self::Zoom { .. } | Self::MouseMove { .. }
        )
    }
}

fn fmt_coord(c: &Option<[i32; 2]>) -> String {
    match c {
        Some([x, y]) => format!("at {x},{y}"),
        None => "at the pointer".into(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}\u{2026}", s.chars().take(max).collect::<String>())
    }
}

/// What an executed action produced.
#[derive(Debug, Clone, Default)]
pub struct ActionResult {
    pub text: Option<String>,
    /// Base64 PNG, present for screenshot/zoom actions.
    pub image_base64: Option<String>,
}

// ---------------------------------------------------------------------------
// Screenshots
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Screenshot {
    pub png_base64: String,
    /// Dimensions of the image handed to the model.
    pub model_width: u32,
    pub model_height: u32,
    /// Dimensions `enigo` addresses.
    pub input_width: u32,
    pub input_height: u32,
    /// Monitor origin in input space, for multi-monitor setups.
    pub origin_x: i32,
    pub origin_y: i32,
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

type Job = (ComputerAction, Screenshot, mpsc::Sender<ComputerResult<ActionResult>>);

/// Handle to screen capture and input simulation.
#[derive(Clone)]
pub struct ComputerController {
    tx: mpsc::Sender<Job>,
    monitor_index: usize,
    max_dimension: u32,
}

impl ComputerController {
    /// Spawn the input thread and return a handle.
    ///
    /// Note that `Enigo` is constructed lazily on the first action, not here:
    /// on macOS constructing it can trigger the Accessibility permission
    /// prompt, and Orbit promises not to ask until an agent actually runs.
    pub fn start(monitor_index: usize, max_dimension: u32) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();

        std::thread::Builder::new()
            .name("orbit-input".into())
            .spawn(move || {
                let mut enigo: Option<Enigo> = None;

                while let Ok((action, shot, reply)) = rx.recv() {
                    // Actions that never touch input devices are served without
                    // constructing Enigo at all.
                    if let Some(result) = handle_without_input(&action) {
                        let _ = reply.send(result);
                        continue;
                    }

                    if enigo.is_none() {
                        match Enigo::new(&Settings::default()) {
                            Ok(e) => enigo = Some(e),
                            Err(e) => {
                                let _ = reply.send(Err(ComputerError::Input(e.to_string())));
                                continue;
                            }
                        }
                    }

                    let e = enigo.as_mut().expect("enigo constructed above");
                    let _ = reply.send(perform(e, &action, &shot));
                }
                log::debug!("input thread exiting");
            })
            .expect("failed to spawn input thread");

        Self {
            tx,
            monitor_index,
            max_dimension,
        }
    }

    pub fn monitor_index(&self) -> usize {
        self.monitor_index
    }

    /// Capture the configured monitor and downscale it for the model.
    ///
    /// Blocking; call from `spawn_blocking`.
    pub fn capture(&self) -> ComputerResult<Screenshot> {
        let monitors =
            xcap::Monitor::all().map_err(|e| ComputerError::Capture(e.to_string()))?;
        let monitor = monitors
            .get(self.monitor_index)
            .or_else(|| monitors.first())
            .ok_or(ComputerError::NoSuchMonitor(self.monitor_index))?;

        let input_width = monitor.width().map_err(|e| ComputerError::Capture(e.to_string()))?;
        let input_height = monitor.height().map_err(|e| ComputerError::Capture(e.to_string()))?;
        let origin_x = monitor.x().unwrap_or(0);
        let origin_y = monitor.y().unwrap_or(0);

        let image = monitor
            .capture_image()
            .map_err(|e| ComputerError::Capture(e.to_string()))?;

        let (cap_w, cap_h) = (image.width(), image.height());
        let longest = cap_w.max(cap_h);
        let scaled = if longest > self.max_dimension && longest > 0 {
            let ratio = self.max_dimension as f32 / longest as f32;
            let w = ((cap_w as f32 * ratio).round() as u32).max(1);
            let h = ((cap_h as f32 * ratio).round() as u32).max(1);
            image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle)
        } else {
            image
        };

        Ok(Screenshot {
            model_width: scaled.width(),
            model_height: scaled.height(),
            png_base64: encode_png_base64(&scaled)?,
            input_width,
            input_height,
            origin_x,
            origin_y,
        })
    }

    /// Capture a region of the screen at full resolution (the `zoom` action).
    ///
    /// `region` is `[x1, y1, x2, y2]` in model space.
    pub fn capture_region(&self, shot: &Screenshot, region: [i32; 4]) -> ComputerResult<String> {
        let monitors =
            xcap::Monitor::all().map_err(|e| ComputerError::Capture(e.to_string()))?;
        let monitor = monitors
            .get(self.monitor_index)
            .or_else(|| monitors.first())
            .ok_or(ComputerError::NoSuchMonitor(self.monitor_index))?;

        let full = monitor
            .capture_image()
            .map_err(|e| ComputerError::Capture(e.to_string()))?;

        // Model space → capture space.
        let sx = full.width() as f32 / shot.model_width.max(1) as f32;
        let sy = full.height() as f32 / shot.model_height.max(1) as f32;

        let x1 = ((region[0].min(region[2]) as f32 * sx).round() as i64).clamp(0, full.width() as i64 - 1) as u32;
        let y1 = ((region[1].min(region[3]) as f32 * sy).round() as i64).clamp(0, full.height() as i64 - 1) as u32;
        let x2 = ((region[2].max(region[0]) as f32 * sx).round() as i64).clamp(0, full.width() as i64) as u32;
        let y2 = ((region[3].max(region[1]) as f32 * sy).round() as i64).clamp(0, full.height() as i64) as u32;

        let w = x2.saturating_sub(x1).max(1);
        let h = y2.saturating_sub(y1).max(1);
        if w < 2 || h < 2 {
            return Err(ComputerError::BadArguments(
                "zoom region is too small to be useful".into(),
            ));
        }

        let crop = image::imageops::crop_imm(&full, x1, y1, w, h).to_image();
        encode_png_base64(&crop)
    }

    /// Send an action to the input thread and wait for it.
    ///
    /// Blocking; call from `spawn_blocking`.
    pub fn execute(&self, action: ComputerAction, shot: &Screenshot) -> ComputerResult<ActionResult> {
        if let ComputerAction::Zoom { region } = action {
            return Ok(ActionResult {
                image_base64: Some(self.capture_region(shot, region)?),
                text: None,
            });
        }
        if matches!(action, ComputerAction::Screenshot) {
            let fresh = self.capture()?;
            return Ok(ActionResult {
                image_base64: Some(fresh.png_base64),
                text: None,
            });
        }

        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send((action, shot.clone(), reply_tx))
            .map_err(|_| ComputerError::ThreadGone)?;
        reply_rx.recv().map_err(|_| ComputerError::ThreadGone)?
    }

    /// Convert model-space coordinates into the space `enigo` addresses.
    pub fn to_input_space(shot: &Screenshot, coordinate: [i32; 2]) -> (i32, i32) {
        let sx = shot.input_width as f32 / shot.model_width.max(1) as f32;
        let sy = shot.input_height as f32 / shot.model_height.max(1) as f32;
        let x = shot.origin_x + (coordinate[0] as f32 * sx).round() as i32;
        let y = shot.origin_y + (coordinate[1] as f32 * sy).round() as i32;
        (
            x.clamp(shot.origin_x, shot.origin_x + shot.input_width as i32 - 1),
            y.clamp(shot.origin_y, shot.origin_y + shot.input_height as i32 - 1),
        )
    }
}

fn encode_png_base64(img: &image::RgbaImage) -> ComputerResult<String> {
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| ComputerError::Capture(format!("PNG encode failed: {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf))
}

/// Actions that need neither Enigo nor the screen.
fn handle_without_input(action: &ComputerAction) -> Option<ComputerResult<ActionResult>> {
    match action {
        ComputerAction::Wait { duration } => {
            // Capped so a hallucinated `duration: 3600` cannot wedge a session.
            let secs = duration.clamp(0.0, 10.0);
            std::thread::sleep(Duration::from_secs_f64(secs));
            Some(Ok(ActionResult {
                text: Some(format!("Waited {secs}s")),
                ..Default::default()
            }))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Action execution
// ---------------------------------------------------------------------------

fn perform(
    enigo: &mut Enigo,
    action: &ComputerAction,
    shot: &Screenshot,
) -> ComputerResult<ActionResult> {
    let map = |c: [i32; 2]| ComputerController::to_input_space(shot, c);
    let err = |e: enigo::InputError| ComputerError::Input(e.to_string());

    match action {
        // Handled by the controller before reaching this thread.
        ComputerAction::Screenshot | ComputerAction::Zoom { .. } | ComputerAction::Wait { .. } => {
            Err(ComputerError::Unsupported(format!("{action:?}")))
        }

        ComputerAction::CursorPosition => {
            let (x, y) = enigo.location().map_err(err)?;
            // Report back in model space so the model's arithmetic lines up.
            let sx = shot.model_width as f32 / shot.input_width.max(1) as f32;
            let sy = shot.model_height as f32 / shot.input_height.max(1) as f32;
            Ok(ActionResult {
                text: Some(format!(
                    "X={},Y={}",
                    ((x - shot.origin_x) as f32 * sx).round() as i32,
                    ((y - shot.origin_y) as f32 * sy).round() as i32
                )),
                ..Default::default()
            })
        }

        ComputerAction::MouseMove { coordinate } => {
            let (x, y) = map(*coordinate);
            enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
            Ok(ActionResult::default())
        }

        ComputerAction::LeftClick { coordinate, text } => {
            click(enigo, Button::Left, 1, coordinate.map(map), text.as_deref())
        }
        ComputerAction::RightClick { coordinate, text } => {
            click(enigo, Button::Right, 1, coordinate.map(map), text.as_deref())
        }
        ComputerAction::MiddleClick { coordinate, text } => {
            click(enigo, Button::Middle, 1, coordinate.map(map), text.as_deref())
        }
        ComputerAction::DoubleClick { coordinate, text } => {
            click(enigo, Button::Left, 2, coordinate.map(map), text.as_deref())
        }
        ComputerAction::TripleClick { coordinate, text } => {
            click(enigo, Button::Left, 3, coordinate.map(map), text.as_deref())
        }

        ComputerAction::LeftMouseDown { coordinate } => {
            if let Some(c) = coordinate {
                let (x, y) = map(*c);
                enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
            }
            enigo.button(Button::Left, Direction::Press).map_err(err)?;
            Ok(ActionResult::default())
        }
        ComputerAction::LeftMouseUp { coordinate } => {
            if let Some(c) = coordinate {
                let (x, y) = map(*c);
                enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
            }
            enigo.button(Button::Left, Direction::Release).map_err(err)?;
            Ok(ActionResult::default())
        }

        ComputerAction::LeftClickDrag {
            coordinate,
            start_coordinate,
        } => {
            if let Some(start) = start_coordinate {
                let (x, y) = map(*start);
                enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
            }
            enigo.button(Button::Left, Direction::Press).map_err(err)?;
            // Interpolate: a single jump reads as a teleport to most drag
            // handlers and they simply do not follow it.
            let (from_x, from_y) = enigo.location().map_err(err)?;
            let (to_x, to_y) = map(*coordinate);
            const STEPS: i32 = 24;
            for i in 1..=STEPS {
                let t = i as f32 / STEPS as f32;
                let x = from_x + ((to_x - from_x) as f32 * t).round() as i32;
                let y = from_y + ((to_y - from_y) as f32 * t).round() as i32;
                enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
                std::thread::sleep(Duration::from_millis(8));
            }
            enigo.button(Button::Left, Direction::Release).map_err(err)?;
            Ok(ActionResult::default())
        }

        ComputerAction::Type { text } => {
            // Chunked so a very long string does not overrun the target app's
            // input queue and drop characters.
            for chunk in chunk_str(text, 200) {
                enigo.text(chunk).map_err(err)?;
                std::thread::sleep(Duration::from_millis(12));
            }
            Ok(ActionResult::default())
        }

        ComputerAction::Key { text } => {
            press_combo(enigo, text)?;
            Ok(ActionResult::default())
        }

        ComputerAction::HoldKey { text, duration } => {
            let keys = parse_combo(text)?;
            for k in &keys {
                enigo.key(*k, Direction::Press).map_err(err)?;
            }
            std::thread::sleep(Duration::from_secs_f64(duration.clamp(0.0, 10.0)));
            for k in keys.iter().rev() {
                enigo.key(*k, Direction::Release).map_err(err)?;
            }
            Ok(ActionResult::default())
        }

        ComputerAction::Scroll {
            coordinate,
            scroll_direction,
            scroll_amount,
            text,
        } => {
            if let Some(c) = coordinate {
                let (x, y) = map(*c);
                enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
            }
            let modifiers = text.as_deref().map(parse_combo).transpose()?.unwrap_or_default();
            for k in &modifiers {
                enigo.key(*k, Direction::Press).map_err(err)?;
            }

            let amount = (*scroll_amount).clamp(-100, 100);
            let (axis, length) = match scroll_direction.to_ascii_lowercase().as_str() {
                "up" => (Axis::Vertical, -amount),
                "down" => (Axis::Vertical, amount),
                "left" => (Axis::Horizontal, -amount),
                "right" => (Axis::Horizontal, amount),
                other => {
                    for k in modifiers.iter().rev() {
                        let _ = enigo.key(*k, Direction::Release);
                    }
                    return Err(ComputerError::BadArguments(format!(
                        "unknown scroll direction \u{201c}{other}\u{201d}"
                    )));
                }
            };
            let result = enigo.scroll(length, axis).map_err(err);

            for k in modifiers.iter().rev() {
                let _ = enigo.key(*k, Direction::Release);
            }
            result?;
            Ok(ActionResult::default())
        }
    }
}

fn click(
    enigo: &mut Enigo,
    button: Button,
    times: u32,
    at: Option<(i32, i32)>,
    modifiers: Option<&str>,
) -> ComputerResult<ActionResult> {
    let err = |e: enigo::InputError| ComputerError::Input(e.to_string());

    if let Some((x, y)) = at {
        enigo.move_mouse(x, y, Coordinate::Abs).map_err(err)?;
        // Some apps ignore a click that lands in the same event as the move.
        std::thread::sleep(Duration::from_millis(24));
    }

    let mods = modifiers
        .filter(|s| !s.trim().is_empty())
        .map(parse_combo)
        .transpose()?
        .unwrap_or_default();
    for k in &mods {
        enigo.key(*k, Direction::Press).map_err(err)?;
    }

    let mut result = Ok(());
    for i in 0..times {
        if i > 0 {
            // Inside the OS double-click threshold.
            std::thread::sleep(Duration::from_millis(60));
        }
        result = enigo.button(button, Direction::Click).map_err(err);
        if result.is_err() {
            break;
        }
    }

    for k in mods.iter().rev() {
        let _ = enigo.key(*k, Direction::Release);
    }
    result?;
    Ok(ActionResult::default())
}

fn press_combo(enigo: &mut Enigo, combo: &str) -> ComputerResult<()> {
    let err = |e: enigo::InputError| ComputerError::Input(e.to_string());
    let keys = parse_combo(combo)?;
    let Some((last, mods)) = keys.split_last() else {
        return Ok(());
    };
    for k in mods {
        enigo.key(*k, Direction::Press).map_err(err)?;
    }
    let result = enigo.key(*last, Direction::Click).map_err(err);
    for k in mods.iter().rev() {
        let _ = enigo.key(*k, Direction::Release);
    }
    result
}

/// Parse an xdotool-style combo such as `ctrl+shift+t` into enigo keys.
///
/// Models emit a mix of X11 keysym names (`Return`, `BackSpace`, `Page_Down`),
/// macOS-flavoured names (`cmd`, `option`) and plain characters, so all three
/// are accepted.
pub fn parse_combo(combo: &str) -> ComputerResult<Vec<Key>> {
    let combo = combo.trim();
    if combo.is_empty() {
        return Err(ComputerError::BadArguments("empty key combination".into()));
    }
    // A lone "+" is the plus key, not a separator.
    let parts: Vec<&str> = if combo == "+" {
        vec!["+"]
    } else {
        combo.split('+').map(str::trim).filter(|p| !p.is_empty()).collect()
    };

    parts
        .iter()
        .map(|p| parse_key(p))
        .collect::<ComputerResult<Vec<_>>>()
}

fn parse_key(name: &str) -> ComputerResult<Key> {
    let lower = name.to_ascii_lowercase();

    // Function keys: F1–F20.
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return match n {
            1 => Ok(Key::F1),
            2 => Ok(Key::F2),
            3 => Ok(Key::F3),
            4 => Ok(Key::F4),
            5 => Ok(Key::F5),
            6 => Ok(Key::F6),
            7 => Ok(Key::F7),
            8 => Ok(Key::F8),
            9 => Ok(Key::F9),
            10 => Ok(Key::F10),
            11 => Ok(Key::F11),
            12 => Ok(Key::F12),
            13 => Ok(Key::F13),
            14 => Ok(Key::F14),
            15 => Ok(Key::F15),
            16 => Ok(Key::F16),
            17 => Ok(Key::F17),
            18 => Ok(Key::F18),
            19 => Ok(Key::F19),
            20 => Ok(Key::F20),
            other => Err(ComputerError::BadArguments(format!("no F{other} key"))),
        };
    }

    Ok(match lower.as_str() {
        "ctrl" | "control" | "ctl" => Key::Control,
        "shift" => Key::Shift,
        "alt" | "option" | "opt" => Key::Alt,
        "cmd" | "command" | "super" | "meta" | "win" | "windows" => Key::Meta,

        "return" | "enter" | "kp_enter" | "\n" => Key::Return,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "space" | " " => Key::Space,
        "backspace" | "bksp" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "home" => Key::Home,
        "end" => Key::End,
        "page_up" | "pageup" | "prior" | "pgup" => Key::PageUp,
        "page_down" | "pagedown" | "next" | "pgdn" => Key::PageDown,
        "up" | "uparrow" | "arrowup" => Key::UpArrow,
        "down" | "downarrow" | "arrowdown" => Key::DownArrow,
        "left" | "leftarrow" | "arrowleft" => Key::LeftArrow,
        "right" | "rightarrow" | "arrowright" => Key::RightArrow,
        "capslock" | "caps_lock" => Key::CapsLock,

        // X11 keysym names for punctuation that models reach for.
        "minus" => Key::Unicode('-'),
        "plus" => Key::Unicode('+'),
        "equal" => Key::Unicode('='),
        "slash" => Key::Unicode('/'),
        "backslash" => Key::Unicode('\\'),
        "period" | "dot" => Key::Unicode('.'),
        "comma" => Key::Unicode(','),
        "semicolon" => Key::Unicode(';'),
        "apostrophe" | "quote" => Key::Unicode('\''),
        "grave" => Key::Unicode('`'),
        "bracketleft" => Key::Unicode('['),
        "bracketright" => Key::Unicode(']'),

        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::Unicode(c),
                _ => {
                    return Err(ComputerError::BadArguments(format!(
                        "unrecognised key \u{201c}{name}\u{201d}"
                    )))
                }
            }
        }
    })
}

/// Split on character boundaries, never mid-codepoint.
fn chunk_str(s: &str, chars_per_chunk: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (i, _) in s.char_indices() {
        if count == chars_per_chunk {
            out.push(&s[start..i]);
            start = i;
            count = 0;
        }
        count += 1;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    if out.is_empty() && !s.is_empty() {
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(model: (u32, u32), input: (u32, u32), origin: (i32, i32)) -> Screenshot {
        Screenshot {
            png_base64: String::new(),
            model_width: model.0,
            model_height: model.1,
            input_width: input.0,
            input_height: input.1,
            origin_x: origin.0,
            origin_y: origin.1,
        }
    }

    #[test]
    fn maps_model_coordinates_onto_a_retina_display() {
        // 1280-wide screenshot of a 1512x982-point display.
        let s = shot((1280, 831), (1512, 982), (0, 0));
        assert_eq!(ComputerController::to_input_space(&s, [0, 0]), (0, 0));
        assert_eq!(ComputerController::to_input_space(&s, [640, 415]), (756, 490));
        // Bottom-right corner stays inside the display.
        let (x, y) = ComputerController::to_input_space(&s, [1279, 830]);
        assert!(x < 1512 && y < 982);
    }

    #[test]
    fn respects_a_secondary_monitor_origin() {
        let s = shot((1280, 800), (1280, 800), (1512, 0));
        assert_eq!(ComputerController::to_input_space(&s, [10, 10]), (1522, 10));
    }

    #[test]
    fn clamps_out_of_range_coordinates() {
        let s = shot((1000, 1000), (500, 500), (0, 0));
        assert_eq!(ComputerController::to_input_space(&s, [99999, 99999]), (499, 499));
        assert_eq!(ComputerController::to_input_space(&s, [-50, -50]), (0, 0));
    }

    #[test]
    fn parses_modifier_combinations() {
        assert_eq!(parse_combo("ctrl+s").unwrap(), vec![Key::Control, Key::Unicode('s')]);
        assert_eq!(parse_combo("cmd+shift+t").unwrap(), vec![Key::Meta, Key::Shift, Key::Unicode('t')]);
        assert_eq!(parse_combo("Return").unwrap(), vec![Key::Return]);
        assert_eq!(parse_combo("super").unwrap(), vec![Key::Meta]);
        assert_eq!(parse_combo("Page_Down").unwrap(), vec![Key::PageDown]);
        assert_eq!(parse_combo("F5").unwrap(), vec![Key::F5]);
    }

    #[test]
    fn treats_a_lone_plus_as_a_key() {
        assert_eq!(parse_combo("+").unwrap(), vec![Key::Unicode('+')]);
    }

    #[test]
    fn rejects_nonsense_keys() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("nonexistent_key").is_err());
        assert!(parse_combo("F99").is_err());
    }

    #[test]
    fn chunking_never_splits_a_codepoint() {
        let s = "\u{1f30d}\u{1f30e}\u{1f30f}abc";
        let chunks = chunk_str(s, 2);
        assert_eq!(chunks.concat(), s);
        assert!(chunks.iter().all(|c| std::str::from_utf8(c.as_bytes()).is_ok()));
    }

    #[test]
    fn read_only_actions_skip_the_settle_delay() {
        assert!(!ComputerAction::Screenshot.is_mutating());
        assert!(!ComputerAction::CursorPosition.is_mutating());
        assert!(ComputerAction::Type { text: "x".into() }.is_mutating());
        assert!(ComputerAction::Key { text: "a".into() }.is_mutating());
    }

    #[test]
    fn actions_deserialize_from_the_api_shape() {
        let a: ComputerAction =
            serde_json::from_value(serde_json::json!({"action": "left_click", "coordinate": [12, 34]}))
                .unwrap();
        assert!(matches!(a, ComputerAction::LeftClick { coordinate: Some([12, 34]), .. }));

        let a: ComputerAction = serde_json::from_value(serde_json::json!({
            "action": "scroll", "coordinate": [1, 2], "scroll_direction": "down", "scroll_amount": 3
        }))
        .unwrap();
        assert!(matches!(a, ComputerAction::Scroll { scroll_amount: 3, .. }));

        let a: ComputerAction =
            serde_json::from_value(serde_json::json!({"action": "screenshot"})).unwrap();
        assert!(matches!(a, ComputerAction::Screenshot));
    }
}
