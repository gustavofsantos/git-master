//! The math of dragging a panel's edge: where the size ends up for a pointer
//! at some position, and the limits a size is kept within. Sizes are in
//! logical pixels along one axis of the window.
//!
//! Adapted from rvw's `crates/rvw-view/src/resize.rs` (MIT, same author).

/// The sidebar's width before the user drags it.
pub const SIDEBAR_DEFAULT: f32 = 240.;
/// The left column's width before the user drags it.
pub const LEFT_DEFAULT: f32 = 420.;
/// The commit message box's height before the user drags it: three rows of
/// the text area (20px each) plus its 8px padding top and bottom and its
/// 1px bottom border.
pub const COMMIT_DEFAULT: f32 = 77.;

/// The Code file tree's width before the user drags it.
pub const CODE_TREE_DEFAULT: f32 = 300.;
/// The Workflows list's width before the user drags it.
pub const WORKFLOW_LIST_DEFAULT: f32 = 260.;
/// The Threads list's width before the user drags it.
pub const THREAD_LIST_DEFAULT: f32 = 360.;

/// The narrowest the main area is kept while the columns beside it are
/// dragged wider. Below this the left column gives way first.
pub const MAIN_MIN: f32 = 320.;

/// The sidebar: at least 200px, at most 40% of the window's width.
pub const SIDEBAR: Limits = Limits {
    min: 200.,
    max_fraction: 0.4,
};

/// The left column: at least 280px, at most 60% of the window's width.
pub const LEFT: Limits = Limits {
    min: 280.,
    max_fraction: 0.6,
};
/// The list or tree beside a page's content: at least 180px, at most half the
/// window's width.
pub const PANE: Limits = Limits {
    min: 180.,
    max_fraction: 0.5,
};
/// The commit message box: at least 72px, at most 50% of the window's height.
pub const COMMIT: Limits = Limits {
    min: 72.,
    max_fraction: 0.5,
};

/// What a size may be: a floor, and a fraction of the window's extent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub min: f32,
    pub max_fraction: f32,
}

impl Limits {
    /// The largest size in a window `extent` long. Never below the floor, so
    /// a tiny window keeps the panel usable instead of inverting the range.
    #[must_use]
    pub fn max(self, extent: f32) -> f32 {
        (extent * self.max_fraction).max(self.min)
    }

    /// `size` held between the floor and the ceiling. A size that is not a
    /// number is the floor.
    #[must_use]
    pub fn clamp(self, size: f32, extent: f32) -> f32 {
        if size.is_nan() {
            return self.min;
        }
        size.clamp(self.min, self.max(extent))
    }
}

/// The left column's width once the sidebar is `sidebar` wide in a window
/// `extent` wide: its own limits, and no wider than leaves the main area
/// `MAIN_MIN` wide. Never below its floor, so a narrow window keeps the column
/// usable and the main area takes what is left.
#[must_use]
pub fn left_beside_sidebar(size: f32, sidebar: f32, extent: f32) -> f32 {
    let room = (extent - sidebar - MAIN_MIN).max(LEFT.min);
    LEFT.clamp(size, extent).min(room)
}

/// The size after the pointer moved from `origin` to `pointer`, along the
/// axis. `grows_with_pointer` is true when moving the pointer to larger
/// coordinates enlarges the panel (the left column's right edge, the commit
/// box's bottom edge), false when it shrinks it.
#[must_use]
pub fn dragged(
    start: f32,
    origin: f32,
    pointer: f32,
    grows_with_pointer: bool,
    limits: Limits,
    extent: f32,
) -> f32 {
    let delta = if grows_with_pointer {
        pointer - origin
    } else {
        origin - pointer
    };
    limits.clamp(start + delta, extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_left_column_is_held_between_280_and_60_percent() {
        assert_eq!(LEFT.clamp(100., 1000.), 280.);
        assert_eq!(LEFT.clamp(420., 1000.), 420.);
        assert_eq!(LEFT.clamp(900., 1000.), 600.);
        assert_eq!(LEFT.clamp(f32::NAN, 1000.), 280.);
    }

    #[test]
    fn the_commit_box_is_held_between_72_and_50_percent() {
        assert_eq!(COMMIT.clamp(10., 800.), 72.);
        assert_eq!(COMMIT.clamp(COMMIT_DEFAULT, 800.), COMMIT_DEFAULT);
        assert_eq!(COMMIT.clamp(900., 800.), 400.);
    }

    #[test]
    fn the_sidebar_is_held_between_200_and_40_percent() {
        assert_eq!(SIDEBAR.clamp(100., 1000.), 200.);
        assert_eq!(SIDEBAR.clamp(SIDEBAR_DEFAULT, 1000.), SIDEBAR_DEFAULT);
        assert_eq!(SIDEBAR.clamp(900., 1000.), 400.);
        assert_eq!(SIDEBAR.clamp(f32::NAN, 1000.), 200.);
    }

    #[test]
    fn the_sidebar_default_sits_inside_the_limits_of_a_1280_window() {
        assert_eq!(SIDEBAR.clamp(SIDEBAR_DEFAULT, 1280.), SIDEBAR_DEFAULT);
    }

    #[test]
    fn dragging_right_grows_the_sidebar() {
        assert_eq!(dragged(240., 240., 300., true, SIDEBAR, 1280.), 300.);
        assert_eq!(dragged(240., 240., 100., true, SIDEBAR, 1280.), 200.);
        assert_eq!(dragged(240., 240., 900., true, SIDEBAR, 1280.), 512.);
    }

    #[test]
    fn the_left_column_yields_to_a_wide_sidebar_and_keeps_the_main_area() {
        // 1280 - 240 - 320 leaves room for 720px, below the column's own 768px.
        assert_eq!(left_beside_sidebar(900., 240., 1280.), 720.);
        // The column's own size wins when the room is larger.
        assert_eq!(left_beside_sidebar(420., 240., 1280.), 420.);
        // A widened sidebar takes room from the column.
        assert_eq!(left_beside_sidebar(768., 512., 1280.), 448.);
        assert_eq!(left_beside_sidebar(420., 512., 1280.), 420.);
    }

    #[test]
    fn a_narrow_window_keeps_the_column_at_its_floor() {
        // The room would be 140px; the column keeps its 280px floor.
        assert_eq!(left_beside_sidebar(420., 240., 700.), 280.);
        assert_eq!(left_beside_sidebar(f32::NAN, 240., 700.), 280.);
    }

    #[test]
    fn the_main_area_stays_positive_and_at_its_minimum_when_it_can() {
        // From 500px: at 480 the two floors already fill the window.
        let mut extent = 500.;
        while extent <= 3000. {
            for sidebar in [200., 240., 512., 900.] {
                let sidebar = SIDEBAR.clamp(sidebar, extent);
                for size in [0., LEFT_DEFAULT, 900., extent] {
                    let left = left_beside_sidebar(size, sidebar, extent);
                    let main = extent - sidebar - left;
                    assert!(
                        main > 0.,
                        "main {main} at {extent} with {sidebar} and {size}"
                    );
                    if extent - sidebar - MAIN_MIN >= LEFT.min {
                        assert!(
                            main >= MAIN_MIN,
                            "main {main} at {extent}, {sidebar}, {size}"
                        );
                    }
                }
            }
            extent += 20.;
        }
    }

    #[test]
    fn a_small_window_keeps_the_floor() {
        assert_eq!(LEFT.max(300.), 280.);
        assert_eq!(LEFT.clamp(420., 300.), 280.);
        assert_eq!(COMMIT.max(100.), 72.);
    }

    #[test]
    fn the_defaults_sit_inside_the_limits_of_a_900_by_600_window() {
        assert_eq!(LEFT.clamp(LEFT_DEFAULT, 900.), LEFT_DEFAULT);
        assert_eq!(COMMIT.clamp(COMMIT_DEFAULT, 600.), COMMIT_DEFAULT);
    }

    #[test]
    fn dragging_right_grows_the_left_column() {
        assert_eq!(dragged(420., 500., 560., true, LEFT, 1280.), 480.);
        assert_eq!(dragged(420., 500., 440., true, LEFT, 1280.), 360.);
    }

    #[test]
    fn dragging_down_grows_the_commit_box() {
        // The bottom edge sits on the far side of the box, so a pointer moving
        // down enlarges it, and a pointer moving up shrinks it.
        assert_eq!(dragged(77., 300., 340., true, COMMIT, 800.), 117.);
        assert_eq!(dragged(77., 300., 260., true, COMMIT, 800.), 72.);
    }

    #[test]
    fn dragging_is_clamped_at_both_ends() {
        assert_eq!(dragged(420., 500., 0., true, LEFT, 1280.), 280.);
        assert_eq!(dragged(420., 500., 1200., true, LEFT, 1280.), 768.);
        assert_eq!(dragged(77., 300., 0., true, COMMIT, 800.), 72.);
        assert_eq!(dragged(77., 300., 790., true, COMMIT, 800.), 400.);
    }

    #[test]
    fn a_pointer_moving_against_the_growth_shrinks_the_panel() {
        assert_eq!(dragged(340., 900., 860., false, LEFT, 1280.), 380.);
    }

    #[test]
    fn no_movement_keeps_the_size() {
        assert_eq!(dragged(420., 42., 42., true, LEFT, 1280.), 420.);
    }

    #[test]
    fn a_nan_size_falls_back_to_the_floor_not_a_panic() {
        assert_eq!(COMMIT.clamp(f32::NAN, 800.), 72.);
    }
}
