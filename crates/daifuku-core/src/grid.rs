//! The grid a fleet is laid out in.
//!
//! Given how many terminals there are and the work area of the monitor they go
//! on, [`Grid::plan`] picks a shape and returns one rectangle per terminal.
//! Every rectangle is the visible frame the window should end up with, which
//! is not the rectangle Win32 wants: the daemon adds the invisible resize
//! borders itself.
//!
//! Two promises, both tested below:
//!
//! - **The rectangles tile the area exactly.** Gaps are the configured width
//!   everywhere, the outer margin is the same on all four sides, and pixels a
//!   division cannot share out evenly are spread one per cell instead of piling
//!   up in the last column. No two cells differ by more than one pixel.
//! - **An incomplete last row is still symmetric.** Five terminals in a three
//!   by two grid get three on top and two below, and the two share the whole
//!   width between them rather than leaving a hole in the corner.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// The shape a terminal looks best at: a little wider than tall, which is what
/// eighty columns by thirty rows of a typical monospace font come out as.
pub const TARGET_ASPECT: f64 = 1.3;

/// What a grid costs per empty cell when [`Grid::shape_for`] compares two
/// shapes, in the same unit as the aspect penalty (the distance of the log of
/// the aspect ratio from the target). At 0.1, four terminals on a landscape
/// monitor came out three on top and one stretched below, which looks broken;
/// at 0.25 they are two by two, and five still get three and two.
const EMPTY_CELL_COST: f64 = 0.25;

/// Spacing around and between terminals, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Gaps {
    /// Margin between the outermost terminals and the edge of the work area.
    pub outer: i32,
    /// Space between two neighbouring terminals.
    pub inner: i32,
}

impl Default for Gaps {
    /// The spacing a common tiling setup ends up with at padding 14 around
    /// the workspace and 10 around each window: 24 at the edge, 20 between.
    fn default() -> Self {
        Self {
            outer: 24,
            inner: 20,
        }
    }
}

/// A grid shape: how many columns and rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shape {
    /// Columns, at least one.
    pub columns: u32,
    /// Rows, at least one.
    pub rows: u32,
}

/// Plans a fleet's rectangles.
pub struct Grid;

impl Grid {
    /// The shape that gives `count` terminals in `area` cells closest to
    /// [`TARGET_ASPECT`], preferring shapes without empty cells.
    ///
    /// Six terminals on a monitor turned on its side come out two by three,
    /// on a landscape one three by two.
    #[must_use]
    pub fn shape_for(count: u32, area: Rect, gaps: Gaps) -> Shape {
        if count <= 1 {
            return Shape {
                columns: 1,
                rows: 1,
            };
        }
        let inner = f64::from(area.shrink(gaps.outer).width().max(1));
        let inner_h = f64::from(area.shrink(gaps.outer).height().max(1));
        let mut best = Shape {
            columns: 1,
            rows: count,
        };
        let mut best_cost = f64::INFINITY;
        for columns in 1..=count {
            let rows = count.div_ceil(columns);
            let cell_w =
                (inner - f64::from(columns - 1) * f64::from(gaps.inner)) / f64::from(columns);
            let cell_h = (inner_h - f64::from(rows - 1) * f64::from(gaps.inner)) / f64::from(rows);
            if cell_w <= 0.0 || cell_h <= 0.0 {
                continue;
            }
            let empty = f64::from(columns * rows - count);
            let cost = (cell_w / cell_h / TARGET_ASPECT).ln().abs() + empty * EMPTY_CELL_COST;
            if cost < best_cost {
                best_cost = cost;
                best = Shape { columns, rows };
            }
        }
        best
    }

    /// One rectangle per terminal, row by row from the top left.
    ///
    /// `shape` overrides the automatic choice; a shape too small for `count`
    /// grows extra rows rather than dropping terminals.
    #[must_use]
    pub fn plan(count: u32, area: Rect, gaps: Gaps, shape: Option<Shape>) -> Vec<Rect> {
        if count == 0 {
            return Vec::new();
        }
        let shape = match shape {
            Some(s) if s.columns > 0 => Shape {
                columns: s.columns,
                rows: s.rows.max(count.div_ceil(s.columns)),
            },
            _ => Self::shape_for(count, area, gaps),
        };
        let body = area.shrink(gaps.outer);
        let rows = count.div_ceil(shape.columns).min(shape.rows);
        let row_spans = spans(body.top, body.bottom, rows, gaps.inner);
        let mut cells = Vec::with_capacity(count as usize);
        let mut left_to_place = count;
        for (top, bottom) in row_spans {
            let in_row = left_to_place.min(shape.columns);
            for (left, right) in spans(body.left, body.right, in_row, gaps.inner) {
                cells.push(Rect::new(left, top, right, bottom));
            }
            left_to_place -= in_row;
        }
        cells
    }
}

/// Splits `start..end` into `parts` spans separated by `gap`, sharing the
/// pixels that do not divide evenly one per span from the front.
fn spans(start: i32, end: i32, parts: u32, gap: i32) -> Vec<(i32, i32)> {
    if parts == 0 {
        return Vec::new();
    }
    let parts_i = i64::from(parts);
    let usable = (i64::from(end) - i64::from(start) - (parts_i - 1) * i64::from(gap)).max(0);
    (0..parts_i)
        .map(|i| {
            let from = i64::from(start) + i * i64::from(gap) + i * usable / parts_i;
            let to = i64::from(start) + i * i64::from(gap) + (i + 1) * usable / parts_i;
            // Both fit: they lie between start and end, which are i32.
            (
                i32::try_from(from).unwrap_or(i32::MAX),
                i32::try_from(to).unwrap_or(i32::MAX),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The portrait monitor's work area on the machine Daifuku was built on.
    const PORTRAIT: Rect = Rect::new(3840, 120, 4920, 1992);
    /// A 4K landscape monitor with a taskbar.
    const LANDSCAPE: Rect = Rect::new(0, 0, 3840, 2088);

    fn shape(columns: u32, rows: u32) -> Shape {
        Shape { columns, rows }
    }

    #[test]
    fn six_on_a_portrait_monitor_is_two_by_three() {
        assert_eq!(Grid::shape_for(6, PORTRAIT, Gaps::default()), shape(2, 3));
    }

    #[test]
    fn six_on_a_landscape_monitor_is_three_by_two() {
        assert_eq!(Grid::shape_for(6, LANDSCAPE, Gaps::default()), shape(3, 2));
    }

    #[test]
    fn four_on_a_landscape_monitor_is_a_square_grid() {
        assert_eq!(Grid::shape_for(4, LANDSCAPE, Gaps::default()), shape(2, 2));
    }

    #[test]
    fn five_on_a_landscape_monitor_is_three_over_two() {
        assert_eq!(Grid::shape_for(5, LANDSCAPE, Gaps::default()), shape(3, 2));
    }

    #[test]
    fn small_fleets_on_a_portrait_monitor_stack_vertically() {
        assert_eq!(Grid::shape_for(2, PORTRAIT, Gaps::default()), shape(1, 2));
        assert_eq!(Grid::shape_for(3, PORTRAIT, Gaps::default()), shape(1, 3));
    }

    #[test]
    fn one_terminal_fills_the_area_inside_the_margin() {
        let cells = Grid::plan(1, LANDSCAPE, Gaps::default(), None);
        assert_eq!(cells, vec![LANDSCAPE.shrink(24)]);
    }

    #[test]
    fn zero_terminals_plan_nothing() {
        assert!(Grid::plan(0, LANDSCAPE, Gaps::default(), None).is_empty());
    }

    #[test]
    fn the_portrait_plan_fills_the_monitor_to_the_last_pixel() {
        // The first spike placed six real terminals on this monitor with plain
        // integer division and left two rows of pixels unused at the bottom
        // (its last cell ended at 1966). The planner shares those two pixels
        // out, one to each of the lower rows, and ends exactly on the margin.
        let cells = Grid::plan(6, PORTRAIT, Gaps::default(), None);
        assert_eq!(
            cells,
            vec![
                Rect::new(3864, 144, 4370, 738),
                Rect::new(4390, 144, 4896, 738),
                Rect::new(3864, 758, 4370, 1353),
                Rect::new(4390, 758, 4896, 1353),
                Rect::new(3864, 1373, 4370, 1968),
                Rect::new(4390, 1373, 4896, 1968),
            ]
        );
    }

    #[test]
    fn gaps_are_exact_everywhere_for_every_count() {
        let gaps = Gaps::default();
        for area in [PORTRAIT, LANDSCAPE, Rect::new(0, 0, 1001, 997)] {
            for count in 1..=16 {
                let cells = Grid::plan(count, area, gaps, None);
                assert_eq!(cells.len(), count as usize);
                let body = area.shrink(gaps.outer);
                // Outer margin: the extremes touch the body exactly.
                assert_eq!(cells.iter().map(|c| c.left).min(), Some(body.left));
                assert_eq!(cells.iter().map(|c| c.top).min(), Some(body.top));
                assert_eq!(cells.iter().map(|c| c.right).max(), Some(body.right));
                assert_eq!(cells.iter().map(|c| c.bottom).max(), Some(body.bottom));
                // Every row ends flush with the body: no hole in a corner.
                for c in &cells {
                    let row: Vec<_> = cells.iter().filter(|o| o.top == c.top).collect();
                    assert_eq!(
                        row.iter().map(|o| o.right).max(),
                        Some(body.right),
                        "{count} in {area:?}"
                    );
                    assert_eq!(
                        row.iter().map(|o| o.left).min(),
                        Some(body.left),
                        "{count} in {area:?}"
                    );
                }
                // Neighbours sit exactly one gap apart.
                for a in &cells {
                    for b in &cells {
                        if a.top == b.top && b.left > a.right {
                            let between = cells
                                .iter()
                                .any(|m| m.top == a.top && m.left > a.left && m.left < b.left);
                            if !between {
                                assert_eq!(b.left - a.right, gaps.inner);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn cells_in_a_row_differ_by_at_most_one_pixel() {
        let cells = Grid::plan(
            3,
            Rect::new(0, 0, 1000, 500),
            Gaps { outer: 0, inner: 0 },
            Some(shape(3, 1)),
        );
        let widths: Vec<_> = cells.iter().map(Rect::width).collect();
        assert_eq!(widths.iter().sum::<i32>(), 1000);
        assert!(widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1);
    }

    #[test]
    fn an_incomplete_last_row_shares_the_whole_width() {
        let cells = Grid::plan(5, LANDSCAPE, Gaps::default(), Some(shape(3, 2)));
        assert_eq!(cells.len(), 5);
        let bottom: Vec<_> = cells.iter().filter(|c| c.top == cells[4].top).collect();
        assert_eq!(bottom.len(), 2);
        assert_eq!(bottom[0].width(), bottom[1].width());
    }

    #[test]
    fn a_shape_too_small_grows_rows_instead_of_dropping_terminals() {
        let cells = Grid::plan(7, LANDSCAPE, Gaps::default(), Some(shape(2, 2)));
        assert_eq!(cells.len(), 7);
    }

    #[test]
    fn a_shape_with_zero_columns_falls_back_to_automatic() {
        let cells = Grid::plan(6, PORTRAIT, Gaps::default(), Some(shape(0, 9)));
        assert_eq!(cells, Grid::plan(6, PORTRAIT, Gaps::default(), None));
    }

    #[test]
    fn an_absurd_count_on_a_tiny_area_does_not_panic() {
        let cells = Grid::plan(64, Rect::new(0, 0, 50, 50), Gaps::default(), None);
        assert_eq!(cells.len(), 64);
    }
}
