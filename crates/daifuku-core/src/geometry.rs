//! Rectangles in physical pixels, the unit every Win32 call Daifuku makes
//! speaks once the process is per-monitor DPI aware.

use serde::{Deserialize, Serialize};

/// A rectangle with Win32's convention: `right` and `bottom` are exclusive,
/// so a rectangle from 0 to 100 is 100 pixels wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge, inclusive.
    pub left: i32,
    /// Top edge, inclusive.
    pub top: i32,
    /// Right edge, exclusive.
    pub right: i32,
    /// Bottom edge, exclusive.
    pub bottom: i32,
}

impl Rect {
    /// A rectangle from its four edges.
    #[must_use]
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// Width in pixels, never negative.
    #[must_use]
    pub const fn width(&self) -> i32 {
        let w = self.right - self.left;
        if w < 0 { 0 } else { w }
    }

    /// Height in pixels, never negative.
    #[must_use]
    pub const fn height(&self) -> i32 {
        let h = self.bottom - self.top;
        if h < 0 { 0 } else { h }
    }

    /// Whether the rectangle covers no pixel at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width() == 0 || self.height() == 0
    }

    /// Taller than wide, which is how a monitor turned on its side reports.
    #[must_use]
    pub const fn is_portrait(&self) -> bool {
        self.height() > self.width()
    }

    /// The rectangle pulled in by `by` pixels on every side. A margin larger
    /// than half the rectangle collapses it onto its centre instead of turning
    /// it inside out.
    #[must_use]
    pub const fn shrink(&self, by: i32) -> Self {
        let by_x = if by * 2 > self.width() {
            self.width() / 2
        } else {
            by
        };
        let by_y = if by * 2 > self.height() {
            self.height() / 2
        } else {
            by
        };
        Self::new(
            self.left + by_x,
            self.top + by_y,
            self.right - by_x,
            self.bottom - by_y,
        )
    }

    /// Whether the point lies inside.
    #[must_use]
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// The centre point, rounded towards the top left.
    #[must_use]
    pub const fn centre(&self) -> (i32, i32) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_is_not_portrait() {
        assert!(!Rect::new(0, 0, 100, 100).is_portrait());
        assert!(Rect::new(0, 0, 99, 100).is_portrait());
    }

    #[test]
    fn shrinking_moves_every_edge_by_the_margin() {
        assert_eq!(
            Rect::new(10, 20, 110, 220).shrink(5),
            Rect::new(15, 25, 105, 215)
        );
    }

    #[test]
    fn an_oversized_margin_collapses_onto_the_centre_line() {
        // 3 on each side of a 5-wide rectangle does not fit: half of 5 is 2.
        assert_eq!(Rect::new(0, 0, 5, 7).shrink(3), Rect::new(2, 3, 3, 4));
    }

    #[test]
    fn the_centre_rounds_towards_the_top_left() {
        assert_eq!(Rect::new(10, 20, 20, 40).centre(), (15, 30));
        assert_eq!(Rect::new(0, 0, 5, 5).centre(), (2, 2));
        assert_eq!(Rect::new(-10, -10, 10, 10).centre(), (0, 0));
    }

    #[test]
    fn size_follows_the_exclusive_convention() {
        let r = Rect::new(10, 20, 110, 70);
        assert_eq!((r.width(), r.height()), (100, 50));
        assert!(!r.is_empty());
        assert!(!r.is_portrait());
    }

    #[test]
    fn an_inverted_rect_has_no_size() {
        let r = Rect::new(100, 100, 50, 50);
        assert_eq!((r.width(), r.height()), (0, 0));
        assert!(r.is_empty());
    }

    #[test]
    fn a_portrait_monitor_is_portrait() {
        assert!(Rect::new(3840, 120, 4920, 1992).is_portrait());
    }

    #[test]
    fn shrinking_never_turns_a_rect_inside_out() {
        let r = Rect::new(0, 0, 10, 100).shrink(20);
        assert_eq!(r, Rect::new(5, 20, 5, 80));
        assert!(r.is_empty());
    }

    #[test]
    fn contains_excludes_the_far_edges() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains(0, 0));
        assert!(r.contains(9, 9));
        assert!(!r.contains(10, 5));
        assert!(!r.contains(5, 10));
    }
}
