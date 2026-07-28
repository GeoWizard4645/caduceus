//! Moving and resizing *other* applications' windows.
//!
//! # Shape of this module
//!
//! The geometry and the Accessibility calls are deliberately separated:
//!
//! * [`Frame`], [`Screen`] and [`target_frame`] are pure arithmetic with no
//!   macOS types in sight, so every snapping rule is unit-tested on any machine;
//! * [`apply`] is the only function that talks to AX, and it does nothing except
//!   read the focused window, call [`target_frame`], and write the result back.
//!
//! # Coordinate space
//!
//! Everything here is in **AX coordinates**: origin at the top-left of the
//! primary display, Y increasing downwards, measured in points. That is what
//! `kAXPositionAttribute` speaks, so keeping one space throughout removes the
//! class of bug where a window lands correctly on the primary display and 900
//! points off on a second one above it.
//!
//! AppKit is the odd one out — `NSScreen` uses a bottom-left origin with Y up —
//! and [`screens`] is the single place that conversion happens.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::accessibility::{self as ax, AxElement};

/// A rectangle in AX coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    pub fn center_x(&self) -> f64 {
        self.x + self.width / 2.0
    }

    pub fn center_y(&self) -> f64 {
        self.y + self.height / 2.0
    }

    /// Area shared with `other`; zero when they do not overlap.
    pub fn intersection_area(&self, other: &Frame) -> f64 {
        let w = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let h = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if w <= 0.0 || h <= 0.0 {
            0.0
        } else {
            w * h
        }
    }

    /// Move (never resize) so the rectangle sits inside `bounds` where it can.
    pub fn clamped_into(mut self, bounds: &Frame) -> Frame {
        self.width = self.width.min(bounds.width);
        self.height = self.height.min(bounds.height);
        self.x = self.x.clamp(bounds.x, bounds.x + bounds.width - self.width);
        self.y = self.y.clamp(bounds.y, bounds.y + bounds.height - self.height);
        self
    }

    /// Round to whole points.
    ///
    /// Thirds of an odd-width display produce fractions, and a window left on a
    /// half-point boundary renders with a one-pixel seam against its neighbour.
    pub fn rounded(self) -> Frame {
        Frame {
            x: self.x.round(),
            y: self.y.round(),
            width: self.width.round(),
            height: self.height.round(),
        }
    }
}

/// One display: its full bounds and the part not covered by the menu bar or Dock.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    pub full: Frame,
    pub visible: Frame,
}

/// Every window arrangement Caduceus can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verb {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    CenterHalf,
    TopLeftQuarter,
    TopRightQuarter,
    BottomLeftQuarter,
    BottomRightQuarter,
    FirstThird,
    CenterThird,
    LastThird,
    TopThird,
    BottomThird,
    FirstTwoThirds,
    LastTwoThirds,
    TopTwoThirds,
    BottomTwoThirds,
    CenterTwoThirds,
    TopLeftSixth,
    TopCenterSixth,
    TopRightSixth,
    BottomLeftSixth,
    BottomCenterSixth,
    BottomRightSixth,
    FirstFourth,
    SecondFourth,
    ThirdFourth,
    LastFourth,
    FirstThreeFourths,
    LastThreeFourths,
    TopThreeFourths,
    BottomThreeFourths,
    CenterThreeFourths,
    MaximizeHeight,
    MaximizeWidth,
    Maximize,
    AlmostMaximize,
    ReasonableSize,
    Center,
    Larger,
    Smaller,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    Restore,
    NextDisplay,
    PreviousDisplay,
    ToggleFullScreen,
}

impl Verb {
    /// The label shown in the palette.
    pub fn label(self) -> &'static str {
        match self {
            Verb::LeftHalf => "Left half",
            Verb::RightHalf => "Right half",
            Verb::TopHalf => "Top half",
            Verb::BottomHalf => "Bottom half",
            Verb::CenterHalf => "Center half",
            Verb::TopLeftQuarter => "Top-left quarter",
            Verb::TopRightQuarter => "Top-right quarter",
            Verb::BottomLeftQuarter => "Bottom-left quarter",
            Verb::BottomRightQuarter => "Bottom-right quarter",
            Verb::FirstThird => "First third",
            Verb::CenterThird => "Middle third",
            Verb::LastThird => "Last third",
            Verb::TopThird => "Top third",
            Verb::BottomThird => "Bottom third",
            Verb::FirstTwoThirds => "First two-thirds",
            Verb::LastTwoThirds => "Last two-thirds",
            Verb::TopTwoThirds => "Top two-thirds",
            Verb::BottomTwoThirds => "Bottom two-thirds",
            Verb::CenterTwoThirds => "Center two-thirds",
            Verb::TopLeftSixth => "Top-left sixth",
            Verb::TopCenterSixth => "Top-center sixth",
            Verb::TopRightSixth => "Top-right sixth",
            Verb::BottomLeftSixth => "Bottom-left sixth",
            Verb::BottomCenterSixth => "Bottom-center sixth",
            Verb::BottomRightSixth => "Bottom-right sixth",
            Verb::FirstFourth => "First fourth",
            Verb::SecondFourth => "Second fourth",
            Verb::ThirdFourth => "Third fourth",
            Verb::LastFourth => "Last fourth",
            Verb::FirstThreeFourths => "First three-fourths",
            Verb::LastThreeFourths => "Last three-fourths",
            Verb::TopThreeFourths => "Top three-fourths",
            Verb::BottomThreeFourths => "Bottom three-fourths",
            Verb::CenterThreeFourths => "Center three-fourths",
            Verb::MaximizeHeight => "Maximize height",
            Verb::MaximizeWidth => "Maximize width",
            Verb::Maximize => "Maximize",
            Verb::AlmostMaximize => "Almost maximize",
            Verb::ReasonableSize => "Reasonable size",
            Verb::Center => "Center",
            Verb::Larger => "Make bigger",
            Verb::Smaller => "Make smaller",
            Verb::MoveUp => "Move up",
            Verb::MoveDown => "Move down",
            Verb::MoveLeft => "Move left",
            Verb::MoveRight => "Move right",
            Verb::Restore => "Restore",
            Verb::NextDisplay => "Move to next display",
            Verb::PreviousDisplay => "Move to previous display",
            Verb::ToggleFullScreen => "Toggle full screen",
        }
    }

    /// Whether this verb is a display move, which needs the neighbouring screen
    /// rather than the current one.
    fn moves_display(self) -> bool {
        matches!(self, Verb::NextDisplay | Verb::PreviousDisplay)
    }
}

/// How much of a display "almost maximize" leaves uncovered, per axis.
const ALMOST_MAXIMIZE: f64 = 0.92;
/// The fraction of a display a freshly tidied window occupies.
const REASONABLE: f64 = 0.68;
/// Growth/shrink step for [`Verb::Larger`] / [`Verb::Smaller`].
const RESIZE_STEP: f64 = 0.1;
/// Below this, a resized window stops being usable.
const MIN_SIDE: f64 = 120.0;
/// How far one [`Verb::MoveUp`]/[`Verb::MoveDown`]/[`Verb::MoveLeft`]/
/// [`Verb::MoveRight`] nudges the window, in points.
///
/// A fixed step rather than a fraction of the display: a nudge is for "one
/// pixel-perfect correction after a snap", and that correction is the same size
/// whether the monitor is 13" or a 4K panel — a percentage would make it too
/// small to notice on a big display and too coarse to use on a small one.
const NUDGE_STEP: f64 = 64.0;

/// Where a window should end up.
///
/// `current` and the returned frame are both in AX coordinates. `screens` is
/// only consulted for the display-move verbs; everything else is relative to
/// `screen`.
pub fn target_frame(
    verb: Verb,
    current: Frame,
    screen: &Screen,
    screens: &[Screen],
    screen_index: usize,
) -> Option<Frame> {
    let v = screen.visible;

    let frame = match verb {
        Verb::LeftHalf => Frame::new(v.x, v.y, v.width / 2.0, v.height),
        Verb::RightHalf => Frame::new(v.x + v.width / 2.0, v.y, v.width / 2.0, v.height),
        Verb::TopHalf => Frame::new(v.x, v.y, v.width, v.height / 2.0),
        Verb::BottomHalf => Frame::new(v.x, v.y + v.height / 2.0, v.width, v.height / 2.0),
        Verb::CenterHalf => centered(v, v.width / 2.0, v.height / 2.0),

        Verb::TopLeftQuarter => Frame::new(v.x, v.y, v.width / 2.0, v.height / 2.0),
        Verb::TopRightQuarter => {
            Frame::new(v.x + v.width / 2.0, v.y, v.width / 2.0, v.height / 2.0)
        }
        Verb::BottomLeftQuarter => {
            Frame::new(v.x, v.y + v.height / 2.0, v.width / 2.0, v.height / 2.0)
        }
        Verb::BottomRightQuarter => Frame::new(
            v.x + v.width / 2.0,
            v.y + v.height / 2.0,
            v.width / 2.0,
            v.height / 2.0,
        ),

        // --- thirds: columns (dividing width) --------------------------
        Verb::FirstThird => Frame::new(v.x, v.y, v.width / 3.0, v.height),
        Verb::CenterThird => Frame::new(v.x + v.width / 3.0, v.y, v.width / 3.0, v.height),
        Verb::LastThird => Frame::new(v.x + 2.0 * v.width / 3.0, v.y, v.width / 3.0, v.height),
        Verb::FirstTwoThirds => Frame::new(v.x, v.y, 2.0 * v.width / 3.0, v.height),
        Verb::LastTwoThirds => {
            Frame::new(v.x + v.width / 3.0, v.y, 2.0 * v.width / 3.0, v.height)
        }

        // --- thirds: rows (dividing height) -----------------------------
        Verb::TopThird => Frame::new(v.x, v.y, v.width, v.height / 3.0),
        Verb::BottomThird => Frame::new(v.x, v.y + 2.0 * v.height / 3.0, v.width, v.height / 3.0),
        Verb::TopTwoThirds => Frame::new(v.x, v.y, v.width, 2.0 * v.height / 3.0),
        Verb::BottomTwoThirds => {
            Frame::new(v.x, v.y + v.height / 3.0, v.width, 2.0 * v.height / 3.0)
        }
        Verb::CenterTwoThirds => centered(v, v.width, v.height * 2.0 / 3.0),

        // --- sixths: a 2-row, 3-column grid ------------------------------
        Verb::TopLeftSixth => Frame::new(v.x, v.y, v.width / 3.0, v.height / 2.0),
        Verb::TopCenterSixth => {
            Frame::new(v.x + v.width / 3.0, v.y, v.width / 3.0, v.height / 2.0)
        }
        Verb::TopRightSixth => {
            Frame::new(v.x + 2.0 * v.width / 3.0, v.y, v.width / 3.0, v.height / 2.0)
        }
        Verb::BottomLeftSixth => {
            Frame::new(v.x, v.y + v.height / 2.0, v.width / 3.0, v.height / 2.0)
        }
        Verb::BottomCenterSixth => Frame::new(
            v.x + v.width / 3.0,
            v.y + v.height / 2.0,
            v.width / 3.0,
            v.height / 2.0,
        ),
        Verb::BottomRightSixth => Frame::new(
            v.x + 2.0 * v.width / 3.0,
            v.y + v.height / 2.0,
            v.width / 3.0,
            v.height / 2.0,
        ),

        // --- fourths: columns (dividing width into four) -----------------
        Verb::FirstFourth => Frame::new(v.x, v.y, v.width / 4.0, v.height),
        Verb::SecondFourth => Frame::new(v.x + v.width / 4.0, v.y, v.width / 4.0, v.height),
        Verb::ThirdFourth => Frame::new(v.x + 2.0 * v.width / 4.0, v.y, v.width / 4.0, v.height),
        Verb::LastFourth => Frame::new(v.x + 3.0 * v.width / 4.0, v.y, v.width / 4.0, v.height),

        // --- three-fourths -----------------------------------------------
        Verb::FirstThreeFourths => Frame::new(v.x, v.y, v.width * 3.0 / 4.0, v.height),
        Verb::LastThreeFourths => {
            Frame::new(v.x + v.width / 4.0, v.y, v.width * 3.0 / 4.0, v.height)
        }
        Verb::TopThreeFourths => Frame::new(v.x, v.y, v.width, v.height * 3.0 / 4.0),
        Verb::BottomThreeFourths => {
            Frame::new(v.x, v.y + v.height / 4.0, v.width, v.height * 3.0 / 4.0)
        }
        Verb::CenterThreeFourths => centered(v, v.width * 3.0 / 4.0, v.height * 3.0 / 4.0),

        // --- single-axis maximize -----------------------------------------
        // Keeps the other axis exactly as it was, the way dragging just the
        // top or side edge to the screen border does.
        Verb::MaximizeHeight => Frame::new(current.x, v.y, current.width, v.height),
        Verb::MaximizeWidth => Frame::new(v.x, current.y, v.width, current.height),

        Verb::Maximize => v,

        Verb::AlmostMaximize => centered(v, v.width * ALMOST_MAXIMIZE, v.height * ALMOST_MAXIMIZE),

        Verb::ReasonableSize => centered(v, v.width * REASONABLE, v.height * REASONABLE),

        // Keeps the size, moves only the origin — "center" should never be a
        // resize, which is the mistake that makes it useless on a wide monitor.
        Verb::Center => centered(v, current.width, current.height),

        Verb::Larger => scaled_about_center(current, 1.0 + RESIZE_STEP, &v),
        Verb::Smaller => scaled_about_center(current, 1.0 - RESIZE_STEP, &v),

        // --- nudges: same size, a fixed step in one direction ------------
        Verb::MoveUp => Frame::new(current.x, current.y - NUDGE_STEP, current.width, current.height),
        Verb::MoveDown => {
            Frame::new(current.x, current.y + NUDGE_STEP, current.width, current.height)
        }
        Verb::MoveLeft => {
            Frame::new(current.x - NUDGE_STEP, current.y, current.width, current.height)
        }
        Verb::MoveRight => {
            Frame::new(current.x + NUDGE_STEP, current.y, current.width, current.height)
        }

        Verb::NextDisplay | Verb::PreviousDisplay => {
            if screens.len() < 2 {
                return None;
            }
            let step = if verb == Verb::NextDisplay { 1 } else { screens.len() - 1 };
            let target = &screens[(screen_index + step) % screens.len()];
            return Some(proportional_move(current, &v, &target.visible).rounded());
        }

        // Handled by AX directly, or by `apply` reading its own state; there is
        // no frame [`target_frame`] alone can compute.
        Verb::ToggleFullScreen | Verb::Restore => return None,
    };

    Some(frame.clamped_into(&v).rounded())
}

fn centered(bounds: Frame, width: f64, height: f64) -> Frame {
    let width = width.min(bounds.width);
    let height = height.min(bounds.height);
    Frame::new(
        bounds.x + (bounds.width - width) / 2.0,
        bounds.y + (bounds.height - height) / 2.0,
        width,
        height,
    )
}

/// Grow or shrink around the window's own centre, so a resize does not also
/// drift the window across the screen.
fn scaled_about_center(current: Frame, factor: f64, bounds: &Frame) -> Frame {
    let width = (current.width * factor).clamp(MIN_SIDE, bounds.width);
    let height = (current.height * factor).clamp(MIN_SIDE, bounds.height);
    Frame::new(
        current.center_x() - width / 2.0,
        current.center_y() - height / 2.0,
        width,
        height,
    )
}

/// Map a window onto another display, preserving where it sat proportionally.
///
/// Displays are routinely different sizes and scale factors; carrying absolute
/// coordinates across gives you a window half off the edge of a smaller screen.
fn proportional_move(current: Frame, from: &Frame, to: &Frame) -> Frame {
    let fx = if from.width > 0.0 { (current.x - from.x) / from.width } else { 0.0 };
    let fy = if from.height > 0.0 { (current.y - from.y) / from.height } else { 0.0 };
    let fw = if from.width > 0.0 { current.width / from.width } else { 1.0 };
    let fh = if from.height > 0.0 { current.height / from.height } else { 1.0 };

    Frame::new(
        to.x + fx * to.width,
        to.y + fy * to.height,
        (fw * to.width).min(to.width),
        (fh * to.height).min(to.height),
    )
    .clamped_into(to)
}

/// Index of the display a window is most on, by overlap area.
pub fn screen_for(frame: Frame, screens: &[Screen]) -> usize {
    let mut best = (0usize, -1.0f64);
    for (index, screen) in screens.iter().enumerate() {
        let area = frame.intersection_area(&screen.full);
        if area > best.1 {
            best = (index, area);
        }
    }
    // A window dragged fully off-screen overlaps nothing; fall back to whichever
    // display its centre is nearest rather than silently picking the primary.
    if best.1 <= 0.0 {
        let mut nearest = (0usize, f64::MAX);
        for (index, screen) in screens.iter().enumerate() {
            let dx = frame.center_x() - screen.full.center_x();
            let dy = frame.center_y() - screen.full.center_y();
            let distance = dx * dx + dy * dy;
            if distance < nearest.1 {
                nearest = (index, distance);
            }
        }
        return nearest.0;
    }
    best.0
}

// ---------------------------------------------------------------------------
// macOS plumbing
// ---------------------------------------------------------------------------

/// The result of a window command, shaped for a palette toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowOutcome {
    pub ok: bool,
    pub message: String,
    /// `true` when the failure was a missing Accessibility grant, so the UI can
    /// offer to open the right settings pane instead of just apologising.
    pub needs_permission: bool,
}

impl WindowOutcome {
    fn ok(message: impl Into<String>) -> Self {
        Self { ok: true, message: message.into(), needs_permission: false }
    }
    fn err(message: impl Into<String>) -> Self {
        Self { ok: false, message: message.into(), needs_permission: false }
    }
    fn needs_permission() -> Self {
        Self {
            ok: false,
            message: ax::describe_error(ax::kAXErrorAPIDisabled),
            needs_permission: true,
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{Frame, Screen};

    /// How long to wait for the main thread to report display geometry.
    const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(3);

    /// Every display, in AX coordinates.
    ///
    /// `NSScreen` is main-thread-only, so this hops across and converts while it
    /// is there. The conversion needs the primary display's height, because AppKit
    /// measures Y upwards from the bottom-left of *that* screen and AX measures it
    /// downwards from the top-left of the same one.
    pub fn screens<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<Screen> {
        let (tx, rx) = mpsc::sync_channel(1);

        let scheduled = app.run_on_main_thread(move || {
            let _ = tx.send(collect_on_main_thread());
        });
        if scheduled.is_err() {
            return Vec::new();
        }
        rx.recv_timeout(MAIN_THREAD_TIMEOUT).unwrap_or_default()
    }

    fn collect_on_main_thread() -> Vec<Screen> {
        use objc2_app_kit::NSScreen;

        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return Vec::new();
        };
        let list = NSScreen::screens(mtm);
        if list.is_empty() {
            return Vec::new();
        }

        // `screens[0]` is documented to be the screen carrying the menu bar, and
        // its frame origin is the AppKit global origin.
        let primary_height = list.objectAtIndex(0).frame().size.height;

        (0..list.count())
            .map(|index| {
                let screen = list.objectAtIndex(index);
                Screen {
                    full: to_ax(screen.frame(), primary_height),
                    visible: to_ax(screen.visibleFrame(), primary_height),
                }
            })
            .collect()
    }

    fn to_ax(rect: objc2_foundation::NSRect, primary_height: f64) -> Frame {
        Frame {
            x: rect.origin.x,
            y: primary_height - (rect.origin.y + rect.size.height),
            width: rect.size.width,
            height: rect.size.height,
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Screen;

    pub fn screens<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> Vec<Screen> {
        Vec::new()
    }
}

pub use platform::screens;

/// The frontmost window of the frontmost application.
///
/// Goes through the system-wide element's focused application rather than
/// `NSWorkspace.frontmostApplication`, because those disagree while a menu is
/// open — and a window command run from a palette that has just closed is
/// exactly that moment.
#[cfg(target_os = "macos")]
fn focused_window() -> Result<AxElement, WindowOutcome> {
    if !ax::is_trusted() {
        return Err(WindowOutcome::needs_permission());
    }
    let system = AxElement::system_wide().ok_or_else(|| {
        WindowOutcome::err("Could not reach the Accessibility system.")
    })?;
    let app = system
        .element_attribute("AXFocusedApplication")
        .ok_or_else(|| WindowOutcome::err("No application is focused."))?;

    app.element_attribute("AXFocusedWindow")
        .or_else(|| app.element_attribute("AXMainWindow"))
        .ok_or_else(|| {
            WindowOutcome::err("That application has no window Caduceus can move.")
        })
}

#[cfg(target_os = "macos")]
fn window_frame(window: &AxElement) -> Option<Frame> {
    let position = window.point_attribute("AXPosition")?;
    let size = window.size_attribute("AXSize")?;
    Some(Frame::new(position.x, position.y, size.width, size.height))
}

/// The frame a window had just before the last verb Caduceus applied to it.
///
/// One slot, not one per window: [`Verb::Restore`] undoes whatever Caduceus
/// itself just did, which in practice is always the window you were just
/// looking at. A map keyed by window identity would have to solve "what
/// identifies a window across an AX round-trip" for a feature that is used
/// once, right after a snap that went wrong.
static LAST_FRAME: Mutex<Option<Frame>> = Mutex::new(None);

/// Run a window verb against the focused window.
#[cfg(target_os = "macos")]
pub fn apply<R: tauri::Runtime>(app: &tauri::AppHandle<R>, verb: Verb) -> WindowOutcome {
    let window = match focused_window() {
        Ok(w) => w,
        Err(outcome) => return outcome,
    };

    if verb == Verb::ToggleFullScreen {
        let current = window.bool_attribute("AXFullScreen").unwrap_or(false);
        let err = window.set_bool("AXFullScreen", !current);
        return if err == ax::kAXErrorSuccess {
            WindowOutcome::ok(if current { "Left full screen." } else { "Entered full screen." })
        } else {
            WindowOutcome::err(ax::describe_error(err))
        };
    }

    // A full-screen window cannot be positioned; leaving it silently unchanged
    // looks like the shortcut is broken.
    if window.bool_attribute("AXFullScreen").unwrap_or(false) {
        let _ = window.set_bool("AXFullScreen", false);
    }

    let Some(current) = window_frame(&window) else {
        return WindowOutcome::err("Could not read that window's position.");
    };

    let target = if verb == Verb::Restore {
        let mut slot = LAST_FRAME.lock().unwrap_or_else(|e| e.into_inner());
        let Some(previous) = slot.replace(current) else {
            return WindowOutcome::err("Nothing to restore — Caduceus has not moved this window yet.");
        };
        previous
    } else {
        let screens = screens(app);
        if screens.is_empty() {
            return WindowOutcome::err("Could not read your display layout.");
        }
        let index = screen_for(current, &screens);

        let Some(target) = target_frame(verb, current, &screens[index], &screens, index) else {
            return if verb.moves_display() {
                WindowOutcome::err("There is only one display.")
            } else {
                WindowOutcome::err("Nothing to do.")
            };
        };

        *LAST_FRAME.lock().unwrap_or_else(|e| e.into_inner()) = Some(current);
        target
    };

    // Size, position, size. Apps with a minimum width clamp the first size
    // request against their *old* origin; moving in between and re-applying lets
    // them settle where they were actually asked to go. Repeating a no-op size
    // is free, so this costs nothing when an app behaves.
    let size = ax::CGSize { width: target.width, height: target.height };
    let point = ax::CGPoint { x: target.x, y: target.y };

    let _ = window.set_size("AXSize", size);
    let move_err = window.set_point("AXPosition", point);
    let size_err = window.set_size("AXSize", size);

    let err = if move_err != ax::kAXErrorSuccess { move_err } else { size_err };
    if err == ax::kAXErrorSuccess {
        WindowOutcome::ok(verb.label())
    } else if err == ax::kAXErrorAPIDisabled {
        WindowOutcome::needs_permission()
    } else {
        WindowOutcome::err(ax::describe_error(err))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply<R: tauri::Runtime>(_app: &tauri::AppHandle<R>, _verb: Verb) -> WindowOutcome {
    WindowOutcome::err("Window management is macOS-only.")
}

/// Whether window management is usable right now.
pub fn permission_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        ax::is_trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The text currently selected in the frontmost application, via AX.
///
/// Returns `None` rather than an error when nothing is selected or the app does
/// not expose a text element — both are ordinary, and callers fall back to the
/// clipboard.
#[cfg(target_os = "macos")]
pub fn selected_text() -> Option<String> {
    if !ax::is_trusted() {
        return None;
    }
    let system = AxElement::system_wide()?;
    let element = system.element_attribute("AXFocusedUIElement")?;
    let text = element.string_attribute("AXSelectedText")?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(not(target_os = "macos"))]
pub fn selected_text() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        // A 1512x982 visible area under a 1512x1050 display: roughly a 14"
        // MacBook Pro with the menu bar and Dock accounted for.
        Screen {
            full: Frame::new(0.0, 0.0, 1512.0, 1050.0),
            visible: Frame::new(0.0, 38.0, 1512.0, 944.0),
        }
    }

    fn window() -> Frame {
        Frame::new(400.0, 200.0, 800.0, 600.0)
    }

    fn place(verb: Verb) -> Frame {
        let screens = [screen()];
        target_frame(verb, window(), &screens[0], &screens, 0).expect("verb produces a frame")
    }

    #[test]
    fn halves_split_the_visible_area_not_the_display() {
        let left = place(Verb::LeftHalf);
        assert_eq!(left, Frame::new(0.0, 38.0, 756.0, 944.0));
        // Directly under the menu bar, not under the top of the screen.
        assert_eq!(left.y, screen().visible.y);
    }

    #[test]
    fn opposite_halves_tile_without_a_gap_or_an_overlap() {
        let left = place(Verb::LeftHalf);
        let right = place(Verb::RightHalf);
        assert_eq!(left.x + left.width, right.x);
        assert_eq!(left.width + right.width, screen().visible.width);
    }

    #[test]
    fn quarters_tile_the_visible_area_exactly() {
        let total: f64 = [
            Verb::TopLeftQuarter,
            Verb::TopRightQuarter,
            Verb::BottomLeftQuarter,
            Verb::BottomRightQuarter,
        ]
        .into_iter()
        .map(|v| {
            let f = place(v);
            f.width * f.height
        })
        .sum();
        let v = screen().visible;
        assert_eq!(total, v.width * v.height);
    }

    #[test]
    fn thirds_cover_the_width_with_no_rounding_drift() {
        let first = place(Verb::FirstThird);
        let middle = place(Verb::CenterThird);
        let last = place(Verb::LastThird);
        assert_eq!(first.x, 0.0);
        assert_eq!(first.x + first.width, middle.x);
        assert_eq!(middle.x + middle.width, last.x);
        assert_eq!(last.x + last.width, screen().visible.width);
    }

    #[test]
    fn two_thirds_variants_start_where_their_thirds_do() {
        assert_eq!(place(Verb::FirstTwoThirds).x, place(Verb::FirstThird).x);
        assert_eq!(place(Verb::LastTwoThirds).x, place(Verb::CenterThird).x);
    }

    #[test]
    fn maximize_fills_the_visible_area() {
        assert_eq!(place(Verb::Maximize), screen().visible);
    }

    #[test]
    fn row_thirds_divide_height_where_column_thirds_divide_width() {
        let top = place(Verb::TopThird);
        let bottom = place(Verb::BottomThird);
        let v = screen().visible;
        // Full width, a third of the height — the transpose of FirstThird.
        assert_eq!(top.width, v.width);
        assert_eq!(top.y, v.y);
        assert_eq!(bottom.y + bottom.height, v.y + v.height);
        assert_eq!(top.height + place(Verb::BottomTwoThirds).height, v.height);
    }

    #[test]
    fn six_cells_tile_the_visible_area_exactly() {
        let total: f64 = [
            Verb::TopLeftSixth,
            Verb::TopCenterSixth,
            Verb::TopRightSixth,
            Verb::BottomLeftSixth,
            Verb::BottomCenterSixth,
            Verb::BottomRightSixth,
        ]
        .into_iter()
        .map(|v| {
            let f = place(v);
            f.width * f.height
        })
        .sum();
        let v = screen().visible;
        assert_eq!(total, v.width * v.height);
    }

    #[test]
    fn the_sixths_grid_has_no_seams_between_neighbours() {
        let left = place(Verb::TopLeftSixth);
        let center = place(Verb::TopCenterSixth);
        let right = place(Verb::TopRightSixth);
        assert_eq!(left.x + left.width, center.x);
        assert_eq!(center.x + center.width, right.x);
        assert_eq!(right.x + right.width, screen().visible.width);
        // And the bottom row starts exactly where the top row ends.
        assert_eq!(left.y + left.height, place(Verb::BottomLeftSixth).y);
    }

    #[test]
    fn four_columns_tile_the_width_with_no_drift() {
        let columns = [Verb::FirstFourth, Verb::SecondFourth, Verb::ThirdFourth, Verb::LastFourth];
        let mut edge = screen().visible.x;
        for verb in columns {
            let f = place(verb);
            assert_eq!(f.x, edge, "{verb:?} does not start where the last one ended");
            edge = f.x + f.width;
        }
        assert_eq!(edge, screen().visible.x + screen().visible.width);
    }

    #[test]
    fn three_fourths_pairs_with_the_fourth_it_leaves_behind() {
        let wide = place(Verb::FirstThreeFourths);
        let narrow = place(Verb::LastFourth);
        assert_eq!(wide.x + wide.width, narrow.x);
        assert_eq!(wide.width + narrow.width, screen().visible.width);
    }

    /// The point of a single-axis maximize is that the *other* axis is untouched.
    #[test]
    fn maximizing_one_axis_leaves_the_other_alone() {
        let tall = place(Verb::MaximizeHeight);
        assert_eq!(tall.width, window().width);
        assert_eq!(tall.x, window().x);
        assert_eq!(tall.height, screen().visible.height);

        let wide = place(Verb::MaximizeWidth);
        assert_eq!(wide.height, window().height);
        assert_eq!(wide.y, window().y);
        assert_eq!(wide.width, screen().visible.width);
    }

    #[test]
    fn a_nudge_moves_without_resizing() {
        for verb in [Verb::MoveUp, Verb::MoveDown, Verb::MoveLeft, Verb::MoveRight] {
            let moved = place(verb);
            assert_eq!(moved.width, window().width, "{verb:?} resized");
            assert_eq!(moved.height, window().height, "{verb:?} resized");
        }
        assert_eq!(place(Verb::MoveLeft).x, window().x - NUDGE_STEP);
        assert_eq!(place(Verb::MoveRight).x, window().x + NUDGE_STEP);
        assert_eq!(place(Verb::MoveUp).y, window().y - NUDGE_STEP);
        assert_eq!(place(Verb::MoveDown).y, window().y + NUDGE_STEP);
    }

    /// Nudging repeatedly must stop at the edge rather than walk off-screen.
    #[test]
    fn nudging_stops_at_the_edge_of_the_display() {
        let screens = [screen()];
        let mut frame = window();
        for _ in 0..40 {
            frame = target_frame(Verb::MoveLeft, frame, &screens[0], &screens, 0).unwrap();
        }
        assert_eq!(frame.x, screens[0].visible.x);
        assert_eq!(frame.width, window().width, "clamping must not resize");
    }

    /// `Restore` is state held by `apply`, not arithmetic — it has no frame.
    #[test]
    fn restore_has_no_computed_frame() {
        let screens = [screen()];
        assert!(target_frame(Verb::Restore, window(), &screens[0], &screens, 0).is_none());
    }

    #[test]
    fn every_new_placement_also_stays_inside_the_visible_area() {
        let screens = [screen()];
        let verbs = [
            Verb::CenterHalf,
            Verb::TopThird, Verb::BottomThird,
            Verb::TopTwoThirds, Verb::BottomTwoThirds, Verb::CenterTwoThirds,
            Verb::TopLeftSixth, Verb::TopCenterSixth, Verb::TopRightSixth,
            Verb::BottomLeftSixth, Verb::BottomCenterSixth, Verb::BottomRightSixth,
            Verb::FirstFourth, Verb::SecondFourth, Verb::ThirdFourth, Verb::LastFourth,
            Verb::FirstThreeFourths, Verb::LastThreeFourths,
            Verb::TopThreeFourths, Verb::BottomThreeFourths, Verb::CenterThreeFourths,
            Verb::MaximizeHeight, Verb::MaximizeWidth,
            Verb::MoveUp, Verb::MoveDown, Verb::MoveLeft, Verb::MoveRight,
        ];
        let v = screens[0].visible;
        for verb in verbs {
            let f = target_frame(verb, window(), &screens[0], &screens, 0).unwrap();
            assert!(f.x >= v.x, "{verb:?} escaped left");
            assert!(f.y >= v.y, "{verb:?} escaped up");
            assert!(f.x + f.width <= v.x + v.width, "{verb:?} escaped right");
            assert!(f.y + f.height <= v.y + v.height, "{verb:?} escaped down");
        }
    }

    #[test]
    fn centering_moves_without_resizing() {
        let centered = place(Verb::Center);
        assert_eq!(centered.width, window().width);
        assert_eq!(centered.height, window().height);
        assert_eq!(centered.center_x(), screen().visible.center_x());
        assert_eq!(centered.center_y(), screen().visible.center_y());
    }

    #[test]
    fn growing_and_shrinking_keep_the_window_centred() {
        for verb in [Verb::Larger, Verb::Smaller] {
            let resized = place(verb);
            assert_eq!(resized.center_x(), window().center_x(), "{verb:?}");
            assert_eq!(resized.center_y(), window().center_y(), "{verb:?}");
        }
        assert!(place(Verb::Larger).width > window().width);
        assert!(place(Verb::Smaller).width < window().width);
    }

    #[test]
    fn growing_never_escapes_the_visible_area() {
        let screens = [screen()];
        let huge = Frame::new(0.0, 38.0, 1512.0, 944.0);
        let grown = target_frame(Verb::Larger, huge, &screens[0], &screens, 0).unwrap();
        assert!(grown.x >= screens[0].visible.x);
        assert!(grown.width <= screens[0].visible.width);
        assert!(grown.y + grown.height <= screens[0].visible.y + screens[0].visible.height);
    }

    #[test]
    fn shrinking_stops_before_the_window_becomes_unusable() {
        let screens = [screen()];
        let mut frame = Frame::new(600.0, 400.0, 130.0, 130.0);
        for _ in 0..20 {
            frame = target_frame(Verb::Smaller, frame, &screens[0], &screens, 0).unwrap();
        }
        assert!(frame.width >= MIN_SIDE);
        assert!(frame.height >= MIN_SIDE);
    }

    #[test]
    fn every_placement_stays_inside_the_visible_area() {
        let screens = [screen()];
        let verbs = [
            Verb::LeftHalf,
            Verb::RightHalf,
            Verb::TopHalf,
            Verb::BottomHalf,
            Verb::TopLeftQuarter,
            Verb::TopRightQuarter,
            Verb::BottomLeftQuarter,
            Verb::BottomRightQuarter,
            Verb::FirstThird,
            Verb::CenterThird,
            Verb::LastThird,
            Verb::FirstTwoThirds,
            Verb::LastTwoThirds,
            Verb::Maximize,
            Verb::AlmostMaximize,
            Verb::ReasonableSize,
            Verb::Center,
        ];
        let v = screens[0].visible;
        for verb in verbs {
            let f = target_frame(verb, window(), &screens[0], &screens, 0).unwrap();
            assert!(f.x >= v.x, "{verb:?} escaped left");
            assert!(f.y >= v.y, "{verb:?} escaped up");
            assert!(f.x + f.width <= v.x + v.width, "{verb:?} escaped right");
            assert!(f.y + f.height <= v.y + v.height, "{verb:?} escaped down");
        }
    }

    // --- multiple displays -------------------------------------------------

    fn two_screens() -> Vec<Screen> {
        vec![
            screen(),
            // A 4K display to the right, physically larger and differently shaped.
            Screen {
                full: Frame::new(1512.0, 0.0, 2560.0, 1440.0),
                visible: Frame::new(1512.0, 25.0, 2560.0, 1415.0),
            },
        ]
    }

    #[test]
    fn moving_to_the_next_display_keeps_the_relative_position() {
        let screens = two_screens();
        let source = screens[0].visible;
        // Bottom-right quadrant of the first display.
        let win = Frame::new(source.x + source.width * 0.5, source.y + source.height * 0.5, 400.0, 300.0);

        let moved = target_frame(Verb::NextDisplay, win, &screens[0], &screens, 0).unwrap();
        let target = screens[1].visible;

        assert!(moved.x >= target.x && moved.x + moved.width <= target.x + target.width);
        let fx = (moved.x - target.x) / target.width;
        assert!((fx - 0.5).abs() < 0.01, "expected to stay half-way across, got {fx}");
    }

    #[test]
    fn display_moves_wrap_around_in_both_directions() {
        let screens = two_screens();
        let win = Frame::new(100.0, 100.0, 400.0, 300.0);

        let next = target_frame(Verb::NextDisplay, win, &screens[0], &screens, 0).unwrap();
        assert!(next.x >= screens[1].visible.x);

        // From the second display, "next" wraps back to the first.
        let wrapped = target_frame(
            Verb::NextDisplay,
            Frame::new(1600.0, 100.0, 400.0, 300.0),
            &screens[1],
            &screens,
            1,
        )
        .unwrap();
        assert!(wrapped.x < screens[1].visible.x);

        let previous = target_frame(Verb::PreviousDisplay, win, &screens[0], &screens, 0).unwrap();
        assert!(previous.x >= screens[1].visible.x);
    }

    #[test]
    fn a_display_move_with_one_display_is_refused_rather_than_faked() {
        let screens = [screen()];
        assert!(target_frame(Verb::NextDisplay, window(), &screens[0], &screens, 0).is_none());
    }

    #[test]
    fn a_window_too_big_for_the_target_display_is_shrunk_to_fit() {
        let screens = two_screens();
        let huge = Frame::new(1512.0, 25.0, 2560.0, 1415.0);
        let moved = target_frame(Verb::PreviousDisplay, huge, &screens[1], &screens, 1).unwrap();
        let target = screens[0].visible;
        assert!(moved.width <= target.width);
        assert!(moved.height <= target.height);
    }

    // --- display selection -------------------------------------------------

    #[test]
    fn a_window_is_assigned_to_the_display_it_mostly_covers() {
        let screens = two_screens();
        assert_eq!(screen_for(Frame::new(100.0, 100.0, 400.0, 300.0), &screens), 0);
        assert_eq!(screen_for(Frame::new(2000.0, 100.0, 400.0, 300.0), &screens), 1);
        // Straddling the boundary: 300 points on the left, 100 on the right.
        assert_eq!(screen_for(Frame::new(1212.0, 100.0, 400.0, 300.0), &screens), 0);
    }

    #[test]
    fn an_offscreen_window_falls_back_to_the_nearest_display() {
        let screens = two_screens();
        // Far below every display, but horizontally over the second one.
        assert_eq!(screen_for(Frame::new(2600.0, 9000.0, 400.0, 300.0), &screens), 1);
    }

    #[test]
    fn full_screen_has_no_computed_frame() {
        let screens = [screen()];
        assert!(target_frame(Verb::ToggleFullScreen, window(), &screens[0], &screens, 0).is_none());
    }

    // --- the unsafe layer -------------------------------------------------
    //
    // These exercise the hand-written ApplicationServices FFI for real. They
    // assert on behaviour that holds whether or not this machine has granted
    // Accessibility, because CI and a fresh checkout will not have — what is
    // being checked is that the symbols link, the calls return, and the
    // Core Foundation ownership rules in `accessibility.rs` do not double-free.

    #[cfg(target_os = "macos")]
    #[test]
    fn the_trust_check_returns_without_prompting() {
        // Two calls: a prompting variant would block the second one.
        let first = ax::is_trusted();
        let second = ax::is_trusted();
        assert_eq!(first, second);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_system_wide_element_can_be_created_and_released() {
        // Creating and dropping repeatedly is what catches an over-release: the
        // second iteration would touch freed memory.
        for _ in 0..100 {
            let element = AxElement::system_wide();
            assert!(element.is_some(), "AXUIElementCreateSystemWide returned null");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reading_the_focused_window_either_works_or_says_why() {
        // Without the permission this must be the permission error, not a hang
        // and not a crash. With it, an element comes back.
        match focused_window() {
            Ok(window) => {
                // Reading attributes off a real window must not leak or fault.
                let _ = window_frame(&window);
                let _ = window.bool_attribute("AXFullScreen");
                let _ = window.string_attribute("AXTitle");
            }
            Err(outcome) => {
                assert!(!outcome.ok);
                assert!(!outcome.message.is_empty());
                if !ax::is_trusted() {
                    assert!(outcome.needs_permission, "{}", outcome.message);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn selected_text_never_panics_and_is_never_blank() {
        // Returns None on every machine without Accessibility, and on most with
        // it; the contract is that it is either absent or non-empty.
        if let Some(text) = selected_text() {
            assert!(!text.trim().is_empty());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ax_errors_all_translate_to_a_sentence() {
        for code in [0, -25211, -25204, -25205, -25200, -99999] {
            let message = ax::describe_error(code);
            assert!(!message.is_empty(), "no message for {code}");
            assert!(message.ends_with('.'), "not a sentence for {code}: {message}");
        }
    }

    #[test]
    fn every_verb_has_a_label() {
        let verbs = [
            Verb::LeftHalf, Verb::RightHalf, Verb::TopHalf, Verb::BottomHalf, Verb::CenterHalf,
            Verb::TopLeftQuarter, Verb::TopRightQuarter, Verb::BottomLeftQuarter,
            Verb::BottomRightQuarter, Verb::FirstThird, Verb::CenterThird, Verb::LastThird,
            Verb::TopThird, Verb::BottomThird,
            Verb::FirstTwoThirds, Verb::LastTwoThirds,
            Verb::TopTwoThirds, Verb::BottomTwoThirds, Verb::CenterTwoThirds,
            Verb::TopLeftSixth, Verb::TopCenterSixth, Verb::TopRightSixth,
            Verb::BottomLeftSixth, Verb::BottomCenterSixth, Verb::BottomRightSixth,
            Verb::FirstFourth, Verb::SecondFourth, Verb::ThirdFourth, Verb::LastFourth,
            Verb::FirstThreeFourths, Verb::LastThreeFourths,
            Verb::TopThreeFourths, Verb::BottomThreeFourths, Verb::CenterThreeFourths,
            Verb::MaximizeHeight, Verb::MaximizeWidth,
            Verb::Maximize, Verb::AlmostMaximize,
            Verb::ReasonableSize, Verb::Center, Verb::Larger, Verb::Smaller,
            Verb::MoveUp, Verb::MoveDown, Verb::MoveLeft, Verb::MoveRight, Verb::Restore,
            Verb::NextDisplay, Verb::PreviousDisplay, Verb::ToggleFullScreen,
        ];
        assert_eq!(verbs.len(), 50, "a verb was added without a label test");
        for verb in verbs {
            assert!(!verb.label().is_empty(), "{verb:?}");
        }
    }
}
