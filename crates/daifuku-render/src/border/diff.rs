//! What changed since the last border pass. Pure code, no Win32.
//!
//! The daemon calls [`crate::BorderManager::update`] on every pass, twenty
//! times a second while a waiting border pulses. It goes through a
//! [`BorderDiff`], as [`crate::BorderManager::follow_frame`] does, so a border
//! that has not changed is not repainted, not moved, and not even sent to the
//! border thread.

use std::collections::{BTreeMap, BTreeSet};

use crate::FrameUpdate;
use crate::WindowHandle;
use crate::border::BorderSpec;

/// The work one pass leaves for the border thread.
///
/// The three buckets are what the thread has to do, cheapest last:
/// [`BorderChanges::added`] needs a window, a paint and a position,
/// [`BorderChanges::repainted`] needs a paint because the colour changed, and
/// [`BorderChanges::moved`] only needs a `SetWindowPos`. A border that is in
/// none of them is left alone entirely.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BorderChanges {
    /// Windows that had no border before.
    pub added: Vec<BorderSpec>,
    /// The colour changed, the last draw failed or everything is being
    /// repainted: repaint, wherever it is now.
    pub repainted: Vec<BorderSpec>,
    /// Same colour, new rectangle: move it, do not touch its pixels.
    pub moved: Vec<BorderSpec>,
    /// Windows that no longer want a border.
    pub removed: Vec<WindowHandle>,
}

impl BorderChanges {
    /// `true` when nothing at all has to happen, which is the common case for
    /// an idle desktop.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.repainted.is_empty()
            && self.moved.is_empty()
            && self.removed.is_empty()
    }

    /// How many borders this pass touches.
    #[must_use]
    pub fn len(&self) -> usize {
        self.added.len() + self.repainted.len() + self.moved.len() + self.removed.len()
    }

    /// Every spec that has to be handed to a border window, in the order the
    /// thread applies them.
    pub fn specs(&self) -> impl Iterator<Item = &BorderSpec> {
        self.added
            .iter()
            .chain(self.repainted.iter())
            .chain(self.moved.iter())
    }
}

/// The last state the border thread was told about.
///
/// Held by the manager handle, not by the thread, so an unchanged pass costs
/// one hash lookup per window and no cross thread traffic at all.
#[derive(Debug, Clone, Default)]
pub struct BorderDiff {
    last: BTreeMap<isize, BorderSpec>,
    /// Borders the next pass hands over again even when nothing changed,
    /// because their last draw failed or everything has to be repainted.
    resend: BTreeSet<isize>,
}

impl BorderDiff {
    /// An empty diff: nothing is on screen.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last: BTreeMap::new(),
            resend: BTreeSet::new(),
        }
    }

    /// How many borders are on screen.
    #[must_use]
    pub fn len(&self) -> usize {
        self.last.len()
    }

    /// `true` when no border is on screen.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.last.is_empty()
    }

    /// The colour a window's border currently has, if it has one.
    #[must_use]
    pub fn colour(&self, handle: WindowHandle) -> Option<crate::Colour> {
        self.last.get(&handle.0).map(|spec| spec.colour)
    }

    /// Forgets everything, so that the next pass re-adds every border.
    ///
    /// Used when every border has come off the screen.
    pub fn clear(&mut self) {
        self.last.clear();
        self.resend.clear();
    }

    /// Marks every border for repainting, so that the next pass hands each
    /// one that is still wanted to its window again.
    ///
    /// Used when the configuration or the displays change, because then a
    /// border can need new pixels while its rectangle and colour stay the
    /// same. What is on screen is still known: a window that leaves the
    /// layout in that same pass still has its border taken down, which it
    /// would not if this forgot everything.
    pub fn invalidate(&mut self) {
        self.resend.extend(self.last.keys().copied());
    }

    /// Marks one border for the next pass to hand to its window again.
    ///
    /// The diff records a spec as applied the moment it is SENT, which is one
    /// thread hop before anything is drawn. That is right for the common case
    /// and wrong for the case that matters: when the draw fails, the diff
    /// still believes the border is on screen, the next pass sees nothing
    /// changed and sends nothing, and the border stays missing for as long as
    /// its window keeps the same rectangle and colour. One transient failure and
    /// the border is simply gone. The thread calls this when a draw fails, so
    /// the next pass tries again.
    ///
    /// The border stays in the record. Dropping it would lose track of a frame
    /// the thread may still hold: when its window then left the layout, the
    /// next pass would not name it as removed, and the frame would stay on
    /// screen for good. The same goes for a newer spec for the same window
    /// that was sent while the failed one was still on its way.
    pub fn retry(&mut self, target: WindowHandle) {
        if self.last.contains_key(&target.0) {
            self.resend.insert(target.0);
        }
    }

    /// Diffs a complete desired set against the last one.
    ///
    /// A window named twice keeps its last spec. A spec whose colour changed is
    /// repainted even when it also moved, because a repaint puts the frame in
    /// its new place anyway.
    pub fn diff(&mut self, specs: Vec<BorderSpec>) -> BorderChanges {
        let mut next: BTreeMap<isize, BorderSpec> = BTreeMap::new();
        let mut order: Vec<isize> = Vec::with_capacity(specs.len());
        for spec in specs {
            if next.insert(spec.target.0, spec).is_none() {
                order.push(spec.target.0);
            }
        }

        let mut changes = BorderChanges::default();
        for key in order {
            let spec = next[&key];
            match self.last.get(&key) {
                None => changes.added.push(spec),
                Some(_) if self.resend.contains(&key) => changes.repainted.push(spec),
                Some(previous)
                    if previous.colour != spec.colour || previous.width != spec.width =>
                {
                    changes.repainted.push(spec)
                }
                Some(previous) if previous.rect != spec.rect => changes.moved.push(spec),
                Some(_) => {}
            }
        }
        changes.removed = self
            .last
            .keys()
            .filter(|key| !next.contains_key(key))
            .map(|key| WindowHandle(*key))
            .collect();

        self.last = next;
        self.resend.clear();
        changes
    }

    /// Follows one animation frame: the borders of the windows that already
    /// have one move with them.
    ///
    /// Nothing is added and nothing is taken down, because an animation frame
    /// says where windows are, not which of them should have a border. A window
    /// without a border is skipped, and a frame that did not actually move a
    /// window costs nothing.
    pub fn follow(&mut self, frame: &[FrameUpdate]) -> BorderChanges {
        let mut changes = BorderChanges::default();
        for update in frame {
            let Some(spec) = self.last.get_mut(&update.handle.0) else {
                continue;
            };
            if spec.rect == update.rect {
                continue;
            }
            spec.rect = update.rect;
            changes.moved.push(*spec);
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use daifuku_core::Rect;

    use super::*;
    use crate::Colour;

    const BLUE: Colour = Colour::new(0x89, 0xb4, 0xfa);
    const GREEN: Colour = Colour::new(0xa6, 0xe3, 0xa1);
    const YELLOW: Colour = Colour::new(0xf9, 0xe2, 0xaf);

    const A: WindowHandle = WindowHandle(0x1111);
    const B: WindowHandle = WindowHandle(0x2222);

    fn spec(handle: WindowHandle, rect: Rect, colour: Colour) -> BorderSpec {
        BorderSpec::new(handle, rect, colour)
    }

    const LEFT: Rect = Rect::new(0, 0, 800, 600);
    const RIGHT: Rect = Rect::new(800, 0, 1600, 600);

    #[test]
    fn the_first_pass_adds_everything() {
        let mut diff = BorderDiff::new();
        assert!(diff.is_empty());

        let changes = diff.diff(vec![spec(A, LEFT, BLUE), spec(B, RIGHT, GREEN)]);
        assert_eq!(changes.added.len(), 2);
        assert_eq!(changes.added[0].target, A, "the input order is kept");
        assert!(changes.moved.is_empty());
        assert!(changes.repainted.is_empty());
        assert!(changes.removed.is_empty());
        assert_eq!(diff.len(), 2);
    }

    #[test]
    fn the_same_pass_twice_does_nothing_at_all() {
        let mut diff = BorderDiff::new();
        let specs = vec![spec(A, LEFT, BLUE), spec(B, RIGHT, GREEN)];
        let _ = diff.diff(specs.clone());

        let changes = diff.diff(specs);
        assert!(changes.is_empty(), "an idle desktop costs nothing");
        assert_eq!(changes.len(), 0);
    }

    #[test]
    fn a_window_that_only_moved_is_moved_not_repainted() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE), spec(B, RIGHT, GREEN)]);

        let changes = diff.diff(vec![
            spec(A, Rect::new(10, 10, 810, 610), BLUE),
            spec(B, RIGHT, GREEN),
        ]);
        assert_eq!(changes.moved.len(), 1);
        assert_eq!(changes.moved[0].target, A);
        assert!(changes.repainted.is_empty(), "the colour did not change");
        assert!(changes.added.is_empty());
        assert!(changes.removed.is_empty());
    }

    #[test]
    fn a_focus_change_repaints_both_windows_and_moves_neither() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE), spec(B, RIGHT, GREEN)]);

        let changes = diff.diff(vec![spec(A, LEFT, GREEN), spec(B, RIGHT, BLUE)]);
        assert_eq!(changes.repainted.len(), 2);
        assert!(changes.moved.is_empty());
        assert_eq!(changes.specs().count(), 2);
    }

    #[test]
    fn a_new_width_repaints_like_a_new_colour() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE)]);
        let changes = diff.diff(vec![spec(A, LEFT, BLUE).with_width(8)]);
        assert_eq!(changes.repainted.len(), 1);
        assert!(changes.moved.is_empty() && changes.added.is_empty());
    }

    #[test]
    fn a_window_that_changed_colour_and_moved_is_repainted_once() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE)]);

        let changes = diff.diff(vec![spec(A, RIGHT, YELLOW)]);
        assert_eq!(changes.repainted.len(), 1);
        assert_eq!(changes.repainted[0].rect, RIGHT);
        assert!(changes.moved.is_empty());
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn a_window_that_is_gone_is_taken_down() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE), spec(B, RIGHT, GREEN)]);

        let changes = diff.diff(vec![spec(A, LEFT, BLUE)]);
        assert_eq!(changes.removed, vec![B]);
        assert!(changes.added.is_empty());
        assert_eq!(diff.len(), 1);

        let changes = diff.diff(Vec::new());
        assert_eq!(changes.removed, vec![A]);
        assert!(diff.is_empty());
    }

    #[test]
    fn a_window_named_twice_keeps_its_last_spec() {
        let mut diff = BorderDiff::new();
        let changes = diff.diff(vec![spec(A, LEFT, GREEN), spec(A, RIGHT, BLUE)]);
        assert_eq!(changes.added.len(), 1);
        assert_eq!(changes.added[0].colour, BLUE);
        assert_eq!(changes.added[0].rect, RIGHT);
        assert_eq!(diff.colour(A), Some(BLUE));
    }

    #[test]
    fn a_border_that_failed_to_draw_is_handed_over_again() {
        // The diff records a spec as applied when it is SENT, one thread hop
        // before anything is drawn. When the draw fails, the border is not on
        // screen but the diff believes it is, so the next pass sees an
        // unchanged spec and sends nothing: one transient failure and the
        // border is gone for as long as its window keeps the same rectangle
        // and colour. The thread calls `retry` on a failed draw so the next
        // pass hands it over again.
        let mut diff = BorderDiff::new();
        let spec = spec(A, LEFT, BLUE);

        assert!(!diff.diff(vec![spec]).is_empty(), "the first pass adds it");
        assert!(
            diff.diff(vec![spec]).is_empty(),
            "an unchanged pass costs nothing, which is the whole problem"
        );

        diff.retry(A);
        let changes = diff.diff(vec![spec]);
        assert!(
            !changes.is_empty(),
            "the border that never got drawn was never tried again"
        );
        assert!(
            changes.specs().any(|s| s.target == A),
            "the retry has to name the window whose draw failed"
        );
        assert!(
            diff.diff(vec![spec]).is_empty(),
            "one retry per failure, not one per pass from then on"
        );
    }

    #[test]
    fn a_border_whose_draw_failed_is_still_taken_down() {
        // A failed draw can leave the old frame on screen. If the diff dropped
        // the window, a pass without it would not name it as removed, and the
        // thread would never take that frame down.
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE), spec(B, RIGHT, GREEN)]);

        diff.retry(A);
        let changes = diff.diff(vec![spec(B, RIGHT, GREEN)]);
        assert_eq!(
            changes.removed,
            vec![A],
            "the frame stays on screen for good"
        );
        assert!(changes.specs().next().is_none());
    }

    #[test]
    fn a_failed_draw_does_not_lose_a_newer_spec_still_on_its_way() {
        // Two passes in a row: blue, then yellow. The thread fails to draw the
        // blue one after the yellow one was already sent, and then draws the
        // yellow one. The yellow frame is on screen, so it must still be taken
        // down when its window goes.
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE)]);
        let _ = diff.diff(vec![spec(A, LEFT, YELLOW)]);

        diff.retry(A);
        assert_eq!(diff.colour(A), Some(YELLOW), "the newer spec is kept");
        let changes = diff.diff(Vec::new());
        assert_eq!(changes.removed, vec![A]);
    }

    #[test]
    fn a_retry_for_a_border_already_taken_down_is_ignored() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE)]);
        let _ = diff.diff(Vec::new());

        diff.retry(A);
        assert!(diff.is_empty());
        assert!(diff.diff(Vec::new()).is_empty());
    }

    #[test]
    fn clearing_re_adds_everything_next_time() {
        let mut diff = BorderDiff::new();
        let specs = vec![spec(A, LEFT, BLUE)];
        let _ = diff.diff(specs.clone());

        diff.clear();
        assert!(diff.is_empty());
        let changes = diff.diff(specs);
        assert_eq!(changes.added.len(), 1, "every border came off the screen");
        assert!(changes.removed.is_empty());
    }

    #[test]
    fn invalidating_repaints_everything_next_time() {
        let mut diff = BorderDiff::new();
        let specs = vec![spec(A, LEFT, BLUE), spec(B, RIGHT, GREEN)];
        let _ = diff.diff(specs.clone());

        diff.invalidate();
        assert_eq!(diff.len(), 2, "what is on screen is still known");
        let changes = diff.diff(specs.clone());
        assert_eq!(changes.repainted.len(), 2, "a new configuration repaints");
        assert!(changes.added.is_empty() && changes.removed.is_empty());
        assert!(diff.diff(specs).is_empty(), "and only once");
    }

    #[test]
    fn a_window_that_leaves_as_everything_repaints_is_taken_down() {
        // A reload or a monitor change invalidates the diff, and in that same
        // pass a window is minimised or closed. Its frame is on screen, so the
        // pass has to name it as removed, or nothing ever takes it down.
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE), spec(B, RIGHT, GREEN)]);

        diff.invalidate();
        let changes = diff.diff(vec![spec(B, RIGHT, GREEN)]);
        assert_eq!(changes.removed, vec![A], "A's frame stays on screen");
        assert_eq!(changes.repainted, vec![spec(B, RIGHT, GREEN)]);
    }

    #[test]
    fn a_frame_moves_the_borders_that_exist_and_adds_none() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE)]);

        let frame = [
            FrameUpdate {
                handle: A,
                rect: Rect::new(100, 0, 900, 600),
                finished: false,
            },
            FrameUpdate {
                handle: B,
                rect: RIGHT,
                finished: false,
            },
        ];
        let changes = diff.follow(&frame);
        assert_eq!(changes.moved.len(), 1, "B has no border to move");
        assert_eq!(changes.moved[0].target, A);
        assert_eq!(changes.moved[0].colour, BLUE, "the colour rides along");
        assert!(changes.added.is_empty());
        assert!(changes.removed.is_empty());
    }

    #[test]
    fn a_frame_that_moved_nothing_costs_nothing() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE)]);

        let frame = [FrameUpdate {
            handle: A,
            rect: LEFT,
            finished: true,
        }];
        assert!(diff.follow(&frame).is_empty());
    }

    #[test]
    fn the_layout_pass_after_an_animation_sees_no_change() {
        let mut diff = BorderDiff::new();
        let _ = diff.diff(vec![spec(A, LEFT, BLUE)]);

        // The animation walks the window to its target and the borders follow.
        let last = FrameUpdate {
            handle: A,
            rect: RIGHT,
            finished: true,
        };
        assert_eq!(diff.follow(&[last]).moved.len(), 1);

        // The daemon then declares the same end state it asked for.
        let changes = diff.diff(vec![spec(A, RIGHT, BLUE)]);
        assert!(changes.is_empty(), "the frame already put it there");
    }
}
