//! The scroll bar at the right edge of a screen (D265): where its thumb sits for a scroll
//! position, and which scroll position a thumb dragged to a row gives. Pure, in rows of the
//! screen's list (a wrapped line of code is one row), so it needs no height of every row.

/// A scroll bar `height` cells tall over a list that starts at `scroll` of `0..=max`, with
/// `shown` of its `total` rows on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub height: usize,
    pub total: usize,
    pub shown: usize,
    pub scroll: usize,
    pub max: usize,
}

impl Bar {
    /// Whether there is anything to scroll: a bar is drawn only then.
    pub fn needed(&self) -> bool {
        self.max > 0 && self.height > 0
    }

    /// The thumb's length in cells: the share of the list on screen, at least one cell.
    pub fn thumb_len(&self) -> usize {
        if self.total == 0 {
            return self.height;
        }
        (self.height * self.shown.min(self.total))
            .div_ceil(self.total)
            .clamp(1, self.height.max(1))
    }

    /// The thumb's first cell: at the top for the first row, at the bottom for `max`.
    pub fn thumb_top(&self) -> usize {
        let room = self.height - self.thumb_len();
        if self.max == 0 {
            return 0;
        }
        (room * self.scroll.min(self.max) + self.max / 2) / self.max
    }

    /// The scroll position that puts the thumb's first cell at `top` (any value; it is
    /// clamped to the track).
    pub fn scroll_for(&self, top: isize) -> usize {
        let room = self.height - self.thumb_len();
        if room == 0 {
            return 0;
        }
        let top = usize::try_from(top.max(0)).unwrap_or(0).min(room);
        (top * self.max + room / 2) / room
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(scroll: usize) -> Bar {
        Bar {
            height: 10,
            total: 100,
            shown: 10,
            scroll,
            max: 90,
        }
    }

    #[test]
    fn the_thumb_is_the_share_on_screen_and_runs_from_top_to_bottom() {
        assert_eq!(bar(0).thumb_len(), 1);
        assert_eq!(bar(0).thumb_top(), 0);
        assert_eq!(bar(45).thumb_top(), 5);
        assert_eq!(bar(90).thumb_top(), 9);
        let half = Bar {
            total: 20,
            max: 10,
            ..bar(0)
        };
        assert_eq!(half.thumb_len(), 5);
        assert!(half.needed());
        assert!(!Bar { max: 0, ..bar(0) }.needed());
    }

    #[test]
    fn dragging_the_thumb_to_a_cell_scrolls_there_and_stops_at_the_ends() {
        assert_eq!(bar(0).scroll_for(0), 0);
        assert_eq!(bar(0).scroll_for(9), 90);
        assert_eq!(bar(0).scroll_for(50), 90);
        assert_eq!(bar(0).scroll_for(-3), 0);
        assert_eq!(bar(0).scroll_for(5), 50);
    }

    proptest::proptest! {
        #[test]
        fn the_thumb_stays_on_the_track_and_dragging_it_back_lands_on_it(
            height in 1usize..60,
            total in 1usize..5000,
            shown in 1usize..80,
            scroll in 0usize..5000,
        ) {
            let shown = shown.min(total);
            let max = total - shown;
            let bar = Bar { height, total, shown, scroll: scroll.min(max), max };
            proptest::prop_assert!(bar.thumb_len() >= 1);
            proptest::prop_assert!(bar.thumb_top() + bar.thumb_len() <= height);
            let top = isize::try_from(bar.thumb_top()).unwrap();
            let back = Bar { scroll: bar.scroll_for(top), ..bar };
            proptest::prop_assert_eq!(back.thumb_top(), bar.thumb_top());
        }
    }
}
