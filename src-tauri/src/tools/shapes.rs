//! Arranging the Desktop's icons into a shape.
//!
//! # Two halves, deliberately separated
//!
//! [`points`] and everything under it is arithmetic: how many icons, what
//! rectangle, where does each one go. No Finder, no AppleScript, no screen — so
//! every curve is unit-tested on any machine. [`apply`] is the only part that
//! talks to Finder, and all it does is read the current layout, ask [`points`]
//! where things should be, and write that back.
//!
//! # Coordinates
//!
//! Finder's `desktop position` is the **centre of the icon**, in points, from
//! the top-left of the main display with Y increasing downwards. That is the
//! same space as [`Frame`], which is why the usable area comes straight from
//! `NSScreen.visibleFrame` with no conversion: the menu bar and the Dock are
//! already excluded, and nothing about the screen size is hardcoded.
//!
//! # Undo
//!
//! [`apply`] reads every icon's position *before* it moves anything and returns
//! that list, exactly as `sorter::apply` returns its moves. [`revert`] writes it
//! back. The list is returned even when the arranging itself half-failed —
//! a partly rearranged Desktop is precisely when undo matters.

use std::f64::consts::{PI, TAU};

use serde::{Deserialize, Serialize};

use super::apple;
use crate::window::manage::Frame;

/// The shapes the icons can be laid out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    Circle,
    Grid,
    Heart,
    Line,
    Spiral,
}

/// Where one icon is, or should be: its centre, in Finder's desktop space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spot {
    pub name: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapePlan {
    pub shape: Shape,
    /// The rectangle the shape is drawn in, for the preview to scale itself to.
    pub area: Frame,
    /// Where each icon would end up.
    pub spots: Vec<Spot>,
    /// Where each icon is now.
    pub current: Vec<Spot>,
    /// What Finder is keeping the Desktop sorted by, and what that costs.
    pub arrangement: Arrangement,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeResult {
    pub ok: bool,
    pub message: String,
    /// Every icon's position *before* this ran. Feed it to [`revert`].
    pub previous: Vec<Spot>,
}

/// Finder's "Sort By" setting for the Desktop.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Arrangement {
    /// The setting in the words Finder's own menu uses.
    pub label: String,
    /// Finder throws explicit positions away in this mode, so arranging is
    /// pointless until the user turns it off.
    pub blocks: bool,
    /// Positions are honoured but pulled onto the icon grid, which rounds the
    /// corners off a circle. Worth saying; not worth refusing.
    pub snaps: bool,
    /// The menu the user has to visit to turn it off. Empty when there is
    /// nothing to turn off.
    ///
    /// It lives here rather than in the page because turning Stacks off and
    /// turning Sort By off are two different menus, and the code that knows
    /// which one is on should be the code that names it.
    pub fix: String,
}

/// Finder's default Desktop grid step for a 64pt icon: 88 across, 112 down.
///
/// The point Finder stores is the icon's *centre*, so half a cell is the margin
/// that keeps an icon — and the name underneath it — inside the usable area
/// rather than half-way under the Dock.
const CELL_W: f64 = 88.0;
const CELL_H: f64 = 112.0;

/// How many turns the spiral makes from the middle to the edge.
const SPIRAL_TURNS: f64 = 3.0;

/// How finely a curve is sampled before it is spread evenly along its length.
const SAMPLES: usize = 720;

// ---------------------------------------------------------------------------
// The arithmetic
// ---------------------------------------------------------------------------

/// The rectangle icon centres may occupy inside the visible screen area.
pub fn usable(visible: Frame) -> Frame {
    let width = (visible.width - CELL_W).max(1.0);
    let height = (visible.height - CELL_H).max(1.0);
    Frame::new(visible.x + CELL_W / 2.0, visible.y + CELL_H / 2.0, width, height)
}

/// `count` points tracing `shape`, every one of them inside `area`.
pub fn points(shape: Shape, count: usize, area: Frame) -> Vec<(f64, f64)> {
    if count == 0 || area.width <= 0.0 || area.height <= 0.0 {
        return Vec::new();
    }
    match shape {
        Shape::Circle => circle(count, area),
        Shape::Grid => grid(count, area),
        Shape::Line => line(count, area),
        Shape::Heart => fit(&spread(&sample(heart, true), count, true), area),
        Shape::Spiral => fit(&spread(&sample(spiral, false), count, false), area),
    }
}

fn circle(count: usize, area: Frame) -> Vec<(f64, f64)> {
    let radius = area.width.min(area.height) / 2.0;
    let (cx, cy) = (area.center_x(), area.center_y());
    (0..count)
        .map(|i| {
            // From the top, going clockwise, so the first icon lands where a
            // clock's twelve is rather than at three o'clock.
            let angle = -PI / 2.0 + TAU * i as f64 / count as f64;
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect()
}

/// An even grid, with as square a cell as the area allows.
fn grid(count: usize, area: Frame) -> Vec<(f64, f64)> {
    let columns = ((count as f64 * area.width / area.height).sqrt().round() as usize)
        .clamp(1, count);
    let rows = count.div_ceil(columns);
    (0..count)
        .map(|i| {
            let (row, column) = (i / columns, i % columns);
            (
                area.x + (column as f64 + 0.5) * area.width / columns as f64,
                area.y + (row as f64 + 0.5) * area.height / rows as f64,
            )
        })
        .collect()
}

fn line(count: usize, area: Frame) -> Vec<(f64, f64)> {
    let y = area.center_y();
    if count == 1 {
        return vec![(area.center_x(), y)];
    }
    (0..count)
        .map(|i| (area.x + area.width * i as f64 / (count - 1) as f64, y))
        .collect()
}

/// The usual parametric heart, flipped because screen Y grows downwards.
fn heart(t: f64) -> (f64, f64) {
    let x = 16.0 * t.sin().powi(3);
    let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
    (x, -y)
}

/// An Archimedean spiral from the centre outwards.
fn spiral(u: f64) -> (f64, f64) {
    let angle = TAU * SPIRAL_TURNS * u;
    (u * angle.cos(), u * angle.sin())
}

/// Sample a curve densely. `closed` curves are parameterised over `[0, 2π)`,
/// open ones over `[0, 1]`.
fn sample(curve: fn(f64) -> (f64, f64), closed: bool) -> Vec<(f64, f64)> {
    (0..SAMPLES)
        .map(|i| {
            let u = i as f64 / (SAMPLES - 1) as f64;
            curve(if closed { u * TAU } else { u })
        })
        .collect()
}

/// Pick `count` points spread evenly *along the curve*, not evenly in `t`.
///
/// Uniform `t` bunches icons up wherever the curve is drawn slowly — the whole
/// top of the heart, the middle of the spiral. Walking the arc length instead
/// is the difference between a shape and a smudge.
fn spread(dense: &[(f64, f64)], count: usize, closed: bool) -> Vec<(f64, f64)> {
    if dense.is_empty() || count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![dense[0]];
    }

    let mut lengths = Vec::with_capacity(dense.len() + 1);
    let mut total = 0.0;
    lengths.push(0.0);
    for pair in dense.windows(2) {
        total += distance(pair[0], pair[1]);
        lengths.push(total);
    }
    if closed {
        total += distance(dense[dense.len() - 1], dense[0]);
        lengths.push(total);
    }
    if total <= f64::EPSILON {
        return vec![dense[0]; count];
    }

    // A closed curve gets `count` gaps around the loop; an open one gets
    // `count - 1` between its two ends, so both start and end are used.
    let step = total / if closed { count as f64 } else { (count - 1) as f64 };
    let at = |target: f64| -> (f64, f64) {
        let index = match lengths.binary_search_by(|l| l.partial_cmp(&target).unwrap()) {
            Ok(i) => i.min(lengths.len() - 2),
            Err(i) => (i - 1).min(lengths.len() - 2),
        };
        let span = lengths[index + 1] - lengths[index];
        let fraction = if span <= f64::EPSILON { 0.0 } else { (target - lengths[index]) / span };
        let from = dense[index];
        let to = dense[(index + 1) % dense.len()];
        (from.0 + (to.0 - from.0) * fraction, from.1 + (to.1 - from.1) * fraction)
    };

    (0..count).map(|i| at((i as f64 * step).min(total))).collect()
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

/// Scale a curve into `area` without distorting it, and centre it there.
fn fit(curve: &[(f64, f64)], area: Frame) -> Vec<(f64, f64)> {
    if curve.is_empty() {
        return Vec::new();
    }
    let (mut min_x, mut max_x) = (f64::MAX, f64::MIN);
    let (mut min_y, mut max_y) = (f64::MAX, f64::MIN);
    for &(x, y) in curve {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }

    let (span_x, span_y) = (max_x - min_x, max_y - min_y);
    let scale = match (span_x > f64::EPSILON, span_y > f64::EPSILON) {
        (true, true) => (area.width / span_x).min(area.height / span_y),
        (true, false) => area.width / span_x,
        (false, true) => area.height / span_y,
        (false, false) => 0.0,
    };

    let (cx, cy) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    curve
        .iter()
        .map(|&(x, y)| (area.center_x() + (x - cx) * scale, area.center_y() + (y - cy) * scale))
        .collect()
}

/// Hand the shape's points out to the icons.
///
/// Sorted by name first, so the same Desktop always produces the same picture
/// and running it twice does not shuffle everything a second time.
fn assign(names: &[String], shape: Shape, area: Frame) -> Vec<Spot> {
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort_by_key(|name| name.to_lowercase());

    points(shape, sorted.len(), area)
        .into_iter()
        .zip(sorted)
        .map(|((x, y), name)| Spot { name: name.clone(), x: x.round(), y: y.round() })
        .collect()
}

// ---------------------------------------------------------------------------
// Finder
// ---------------------------------------------------------------------------

/// The rectangle icons may be placed in on the main display.
///
/// Only the main display: `desktop position` is measured from its top-left, and
/// a shape spanning two screens with a gap down the middle is not a shape.
pub fn desktop_area<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<Frame, String> {
    let screens = crate::window::manage::screens(app);
    let primary = screens.first().ok_or("Could not read your display layout.")?;
    Ok(usable(primary.visible))
}

/// What is on the Desktop and where it sits.
///
/// Reads `desktop position` rather than `position`: the latter answers `-1, -1`
/// for every icon Finder placed itself, which is most of them on most Macs.
/// The name goes last on each line so that a name containing a tab still parses.
pub fn read_desktop() -> Result<Vec<Spot>, String> {
    let script = r#"tell application "Finder"
        set out to ""
        repeat with theItem in (get every item of desktop)
            set p to desktop position of theItem
            set out to out & (item 1 of p) & tab & (item 2 of p) & tab & (name of theItem) & linefeed
        end repeat
        return out
    end tell"#;

    Ok(apple::run_script(script)?.lines().filter_map(parse_line).collect())
}

fn parse_line(line: &str) -> Option<Spot> {
    let mut fields = line.splitn(3, '\t');
    let x: f64 = fields.next()?.trim().parse().ok()?;
    let y: f64 = fields.next()?.trim().parse().ok()?;
    let name = fields.next()?.to_string();
    (!name.is_empty()).then_some(Spot { name, x, y })
}

/// Whether Finder is keeping the Desktop sorted, and by what.
///
/// Read out of Finder's own preferences rather than over AppleScript, because
/// there is no AppleScript for it: `icon view options` is a property of a Finder
/// *window*, and the Desktop is not one — asking for it fails with -1728. The
/// `defaults` domain is the user's own and needs no permission, and going
/// through `defaults` rather than the plist file means the answer is the live
/// one rather than whatever was last flushed to disk.
pub fn arrangement() -> Arrangement {
    let settings = std::process::Command::new("defaults")
        .args(["read", "com.apple.finder", "DesktopViewSettings"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();

    const SORT_BY: &str = "Click the Desktop, then choose View → Sort By → None.";

    // Stacks group the icons into piles and take the positions with them.
    if let Some(group) = setting(&settings, "GroupBy") {
        if !group.eq_ignore_ascii_case("none") {
            return Arrangement {
                label: format!("Stacks (grouped by {})", human(&group)),
                blocks: true,
                snaps: false,
                fix: "Click the Desktop, then choose View → Use Stacks to turn them off.".into(),
            };
        }
    }

    match setting(&settings, "arrangeBy").as_deref() {
        None | Some("none") => {
            Arrangement { label: "None".into(), blocks: false, snaps: false, fix: String::new() }
        }
        Some("grid") => Arrangement {
            label: "Snap to Grid".into(),
            blocks: false,
            snaps: true,
            fix: SORT_BY.into(),
        },
        Some(other) => {
            Arrangement { label: human(other), blocks: true, snaps: false, fix: SORT_BY.into() }
        }
    }
}

/// Pull `key = value;` out of the block `defaults` prints.
fn setting(settings: &str, key: &str) -> Option<String> {
    settings.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim().trim_matches('"') == key)
            .then(|| value.trim().trim_end_matches(';').trim().trim_matches('"').to_string())
    })
}

/// `dateModified` → `Date Modified`, to match the words in Finder's menu.
fn human(key: &str) -> String {
    let mut out = String::new();
    for (index, character) in key.chars().enumerate() {
        if character.is_uppercase() && index > 0 {
            out.push(' ');
        }
        if index == 0 {
            out.extend(character.to_uppercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// What the icons would do, without touching one of them.
pub fn plan(shape: Shape, area: Frame) -> Result<ShapePlan, String> {
    let current = read_desktop()?;
    let names: Vec<String> = current.iter().map(|spot| spot.name.clone()).collect();
    Ok(ShapePlan {
        shape,
        area,
        spots: assign(&names, shape, area),
        current,
        arrangement: arrangement(),
    })
}

/// Arrange the Desktop, and hand back what it looked like first.
pub fn apply(shape: Shape, area: Frame) -> Result<ShapeResult, String> {
    let arrangement = arrangement();
    if arrangement.blocks {
        return Err(format!(
            "Finder is keeping your Desktop arranged by {}, and it throws away any position it is \
             given while that is on. {}",
            arrangement.label, arrangement.fix
        ));
    }

    // Read the layout again here rather than trusting the one the preview was
    // built from: it may have been on screen for a while, and undo has to
    // restore where things are *now*.
    let previous = read_desktop()?;
    if previous.is_empty() {
        return Err("There is nothing on your Desktop to arrange.".into());
    }

    let names: Vec<String> = previous.iter().map(|spot| spot.name.clone()).collect();
    let spots = assign(&names, shape, area);
    let placed = set_positions(&spots);

    let total = spots.len();
    let (ok, message) = match placed {
        Ok(done) if done == total && arrangement.snaps => (
            true,
            format!(
                "Arranged {total} icons — but Finder's Snap to Grid is on, so it has pulled each \
                 one onto the nearest grid position."
            ),
        ),
        Ok(done) if done == total => (true, format!("Arranged {total} icons.")),
        Ok(done) => (false, format!("Arranged {done} of {total} icons.")),
        Err(reason) => (false, reason),
    };

    Ok(ShapeResult { ok, message, previous })
}

/// Put every icon back where it was.
pub fn revert(previous: &[Spot]) -> ShapeResult {
    if previous.is_empty() {
        return ShapeResult { ok: true, message: "Nothing to put back.".into(), previous: Vec::new() };
    }
    match set_positions(previous) {
        Ok(done) if done == previous.len() => ShapeResult {
            ok: true,
            message: format!("Put {done} icons back."),
            previous: Vec::new(),
        },
        // The ones that did not move are still where the shape left them, so the
        // list is handed back rather than dropped — a second Undo can try again.
        Ok(done) => ShapeResult {
            ok: false,
            message: format!("Put {done} of {} icons back.", previous.len()),
            previous: previous.to_vec(),
        },
        Err(reason) => ShapeResult { ok: false, message: reason, previous: previous.to_vec() },
    }
}

/// How many icons one script moves.
///
/// Finder answers each `set` as its own Apple event, and `apple::run_script`
/// gives a script ten seconds before assuming the app has wedged. A Desktop with
/// two hundred icons on it would run out of time; a batch of twenty-five never
/// does, and a batch that fails still leaves the ones before it moved and
/// undoable.
const BATCH: usize = 25;

/// Move icons, reporting how many actually went.
///
/// Every `set` is wrapped in its own `try` so that one icon Finder cannot find —
/// renamed, deleted or unmounted since the layout was read — does not abandon
/// the rest of the shape.
fn set_positions(spots: &[Spot]) -> Result<usize, String> {
    let mut placed = 0usize;
    for batch in spots.chunks(BATCH) {
        let mut script = String::from("tell application \"Finder\"\nset n to 0\n");
        for spot in batch {
            script.push_str(&format!(
                "try\nset desktop position of item \"{}\" of desktop to {{{}, {}}}\nset n to n + 1\nend try\n",
                escape(&spot.name),
                spot.x.round() as i64,
                spot.y.round() as i64,
            ));
        }
        script.push_str("return n\nend tell");

        let answer = apple::run_script(&script)?;
        placed += answer.trim().parse::<usize>().unwrap_or(0);
    }
    Ok(placed)
}

/// Make a filename safe to sit inside an AppleScript string literal.
fn escape(name: &str) -> String {
    name.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1512×982 display with a 33pt menu bar and a 55pt Dock — the numbers a
    /// 14" MacBook Pro actually reports.
    fn screen() -> Frame {
        Frame::new(0.0, 33.0, 1512.0, 894.0)
    }

    fn area() -> Frame {
        usable(screen())
    }

    fn inside(area: Frame, points: &[(f64, f64)]) -> bool {
        const SLACK: f64 = 1e-6;
        points.iter().all(|&(x, y)| {
            x >= area.x - SLACK
                && x <= area.x + area.width + SLACK
                && y >= area.y - SLACK
                && y <= area.y + area.height + SLACK
        })
    }

    const EVERY: [Shape; 5] =
        [Shape::Circle, Shape::Grid, Shape::Heart, Shape::Line, Shape::Spiral];

    #[test]
    fn the_usable_area_keeps_whole_icons_off_the_menu_bar_and_the_dock() {
        let usable = area();
        // The point is an icon's centre, so half a cell has to be left on each
        // side of the visible frame.
        assert_eq!(usable.x, CELL_W / 2.0);
        assert_eq!(usable.y, 33.0 + CELL_H / 2.0);
        assert_eq!(usable.x + usable.width, 1512.0 - CELL_W / 2.0);
        assert_eq!(usable.y + usable.height, 33.0 + 894.0 - CELL_H / 2.0);
    }

    #[test]
    fn nothing_is_ever_placed_outside_the_usable_area() {
        for shape in EVERY {
            for count in [1, 2, 3, 7, 21, 60, 200] {
                let placed = points(shape, count, area());
                assert_eq!(placed.len(), count, "{shape:?} with {count}");
                assert!(inside(area(), &placed), "{shape:?} with {count} escaped the screen");
            }
        }
    }

    #[test]
    fn every_shape_is_finite_for_every_count() {
        for shape in EVERY {
            for count in 1..40 {
                for (x, y) in points(shape, count, area()) {
                    assert!(x.is_finite() && y.is_finite(), "{shape:?} with {count}");
                }
            }
        }
    }

    #[test]
    fn no_icons_means_no_points_rather_than_a_division_by_zero() {
        for shape in EVERY {
            assert!(points(shape, 0, area()).is_empty());
        }
    }

    #[test]
    fn a_circle_is_round() {
        let area = area();
        let placed = points(Shape::Circle, 32, area);
        let radius = area.width.min(area.height) / 2.0;
        for (x, y) in placed {
            let from_centre =
                ((x - area.center_x()).powi(2) + (y - area.center_y()).powi(2)).sqrt();
            assert!((from_centre - radius).abs() < 1e-9, "{from_centre} should be {radius}");
        }
    }

    #[test]
    fn a_circle_starts_at_the_top_and_goes_clockwise() {
        let area = area();
        let placed = points(Shape::Circle, 4, area);
        assert!((placed[0].0 - area.center_x()).abs() < 1e-9);
        assert!(placed[0].1 < area.center_y(), "the first icon belongs at twelve o'clock");
        assert!(placed[1].0 > area.center_x(), "the second at three o'clock");
        assert!(placed[2].1 > area.center_y(), "the third at six");
    }

    #[test]
    fn a_line_is_flat_and_spans_the_whole_width() {
        let area = area();
        let placed = points(Shape::Line, 9, area);
        assert!(placed.iter().all(|&(_, y)| (y - area.center_y()).abs() < 1e-9));
        assert!((placed[0].0 - area.x).abs() < 1e-9);
        assert!((placed[8].0 - (area.x + area.width)).abs() < 1e-9);
        // And it is in order, left to right.
        assert!(placed.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn a_single_icon_is_a_sensible_point_rather_than_a_crash() {
        for shape in EVERY {
            let placed = points(shape, 1, area());
            assert_eq!(placed.len(), 1);
            assert!(placed[0].0.is_finite() && placed[0].1.is_finite());
        }
        // One icon in a line is the middle of it, not the left-hand edge.
        assert_eq!(points(Shape::Line, 1, area())[0].0, area().center_x());
    }

    #[test]
    fn a_grid_has_as_many_rows_as_it_needs_and_no_more() {
        for count in [4, 7, 12, 21, 40] {
            let placed = points(Shape::Grid, count, area());
            let rows: std::collections::BTreeSet<i64> =
                placed.iter().map(|&(_, y)| y.round() as i64).collect();
            let columns: std::collections::BTreeSet<i64> =
                placed.iter().map(|&(x, _)| x.round() as i64).collect();
            let (rows, columns) = (rows.len(), columns.len());

            assert!(rows * columns >= count, "{count} icons do not fit in {rows}×{columns}");
            assert!(
                (rows - 1) * columns < count,
                "{count} icons in {rows}×{columns} leaves an empty row"
            );
            // The area is wider than it is tall, so the grid should be too.
            assert!(columns >= rows, "{count} icons went taller than wide on a wide screen");
        }
    }

    #[test]
    fn a_heart_is_wider_at_the_top_than_at_the_bottom() {
        let placed = points(Shape::Heart, 200, area());
        let middle = area().center_y();
        let width_of = |half: &[&(f64, f64)]| {
            let xs: Vec<f64> = half.iter().map(|p| p.0).collect();
            xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min)
        };
        let top: Vec<&(f64, f64)> = placed.iter().filter(|p| p.1 < middle).collect();
        let bottom: Vec<&(f64, f64)> = placed.iter().filter(|p| p.1 > middle).collect();
        assert!(width_of(&top) > width_of(&bottom), "a heart is lobes on top, a point below");
    }

    #[test]
    fn a_heart_fills_the_area_it_is_given_in_one_direction() {
        let area = area();
        let placed = points(Shape::Heart, 300, area);
        let xs: Vec<f64> = placed.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = placed.iter().map(|p| p.1).collect();
        let span_x = xs.iter().cloned().fold(f64::MIN, f64::max)
            - xs.iter().cloned().fold(f64::MAX, f64::min);
        let span_y = ys.iter().cloned().fold(f64::MIN, f64::max)
            - ys.iter().cloned().fold(f64::MAX, f64::min);
        // Scaled to touch the shorter side, and not stretched on the other.
        assert!((span_y - area.height).abs() < 1.0, "{span_y} should fill {}", area.height);
        assert!(span_x < area.width);
    }

    #[test]
    fn icons_on_a_curve_are_evenly_spaced_rather_than_bunched_at_the_slow_parts() {
        // Uniform `t` on a heart puts a third of the icons in the cleft. Even
        // spacing is the whole reason `spread` exists, so hold it to it.
        for shape in [Shape::Heart, Shape::Spiral] {
            let placed = points(shape, 40, area());
            let gaps: Vec<f64> =
                placed.windows(2).map(|pair| distance(pair[0], pair[1])).collect();
            let longest = gaps.iter().cloned().fold(f64::MIN, f64::max);
            let shortest = gaps.iter().cloned().fold(f64::MAX, f64::min);
            assert!(longest / shortest < 2.5, "{shape:?} spacing ranged {shortest}–{longest}");
        }
    }

    #[test]
    fn a_spiral_winds_outwards_from_the_middle() {
        // Measured before the curve is fitted to the screen: fitting centres it
        // on its bounding box, and a spiral is not symmetric about its own
        // centre, so the fitted radii wobble by a turn's width either way.
        let curve = spread(&sample(spiral, false), 60, false);
        let radius = |&(x, y): &(f64, f64)| (x * x + y * y).sqrt();
        assert!(radius(&curve[0]) < 0.01, "it starts in the middle");
        assert!(
            curve.windows(2).all(|pair| radius(&pair[1]) > radius(&pair[0])),
            "every icon must sit further out than the one before it",
        );

        let area = area();
        let placed = points(Shape::Spiral, 60, area);
        let from_centre = |&(x, y): &(f64, f64)| {
            ((x - area.center_x()).powi(2) + (y - area.center_y()).powi(2)).sqrt()
        };
        assert!(from_centre(&placed[0]) * 3.0 < from_centre(placed.last().unwrap()));
    }

    #[test]
    fn a_shape_in_a_tall_area_is_still_the_same_shape() {
        // A vertical display, to prove nothing assumes a landscape screen.
        let tall = usable(Frame::new(0.0, 33.0, 900.0, 1500.0));
        for shape in EVERY {
            let placed = points(shape, 24, tall);
            assert!(inside(tall, &placed), "{shape:?} escaped a portrait screen");
        }
    }

    #[test]
    fn icons_are_given_their_places_in_a_stable_order() {
        let names: Vec<String> =
            ["zebra.png", "Apple.txt", "mango"].iter().map(|s| s.to_string()).collect();
        let first = assign(&names, Shape::Circle, area());
        let shuffled: Vec<String> =
            ["mango", "zebra.png", "Apple.txt"].iter().map(|s| s.to_string()).collect();
        let second = assign(&shuffled, Shape::Circle, area());
        assert_eq!(first, second, "the same Desktop must produce the same picture");
        assert_eq!(first[0].name, "Apple.txt", "sorted by name, ignoring case");
    }

    #[test]
    fn positions_are_whole_numbers_because_finder_stores_integers() {
        let names: Vec<String> = (0..9).map(|i| format!("file {i}")).collect();
        for spot in assign(&names, Shape::Heart, area()) {
            assert_eq!(spot.x, spot.x.round());
            assert_eq!(spot.y, spot.y.round());
        }
    }

    #[test]
    fn a_finder_line_is_read_back_into_a_position() {
        let spot = parse_line("1288\t70\tScreenshot 2026-04-26.png").unwrap();
        assert_eq!(spot.name, "Screenshot 2026-04-26.png");
        assert_eq!((spot.x, spot.y), (1288.0, 70.0));
        // A name with a tab in it survives, because the name is the last field.
        assert_eq!(parse_line("0\t0\ta\tb").unwrap().name, "a\tb");
        assert!(parse_line("").is_none());
        assert!(parse_line("not a position at all").is_none());
    }

    #[test]
    fn a_quote_in_a_filename_cannot_break_out_of_the_script() {
        assert_eq!(escape(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(escape(r"back\slash"), r"back\\slash");
    }

    #[test]
    fn finders_sort_setting_is_read_out_of_the_block_defaults_prints() {
        let settings = "{\n    GroupBy = None;\n    IconViewSettings =     {\n        \
                        arrangeBy = name;\n        iconSize = 64;\n    };\n}";
        assert_eq!(setting(settings, "arrangeBy").as_deref(), Some("name"));
        assert_eq!(setting(settings, "GroupBy").as_deref(), Some("None"));
        assert_eq!(setting(settings, "nothing"), None);
    }

    #[test]
    fn the_sort_setting_is_named_the_way_finders_menu_names_it() {
        assert_eq!(human("dateModified"), "Date Modified");
        assert_eq!(human("kind"), "Kind");
        assert_eq!(human(""), "");
    }

    #[test]
    fn reverting_nothing_is_not_an_error() {
        let result = revert(&[]);
        assert!(result.ok);
        assert!(result.previous.is_empty());
    }
}
