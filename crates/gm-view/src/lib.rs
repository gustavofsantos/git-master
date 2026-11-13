//! GPUI-free view models shared by the front ends (D235): the diff rows and line
//! selection, the review comment model and the relative time of a list. Nothing
//! here depends on GPUI or ratatui, so every rule is tested without a window or a
//! terminal.

pub mod compose;
pub mod diff_rows;
pub mod review;
pub mod time;
