//! The Phosphor icons the window draws (MIT, see `assets/icons/LICENSE-phosphor`),
//! painted in the text colour of their parent. Each name has a regular and a
//! fill weight.

use gpui::{Div, Hsla, IntoElement, ParentElement, Pixels, Styled, Svg, div, px, svg};

/// An icon in a row, a button or a tree.
pub const SIZE: Pixels = px(16.);
/// An icon beside small counter text.
pub const SMALL: Pixels = px(14.);

/// Every bundled icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    CheckSquare,
    Square,
    MinusSquare,
    CaretRight,
    CaretDown,
    File,
    FilePlus,
    FileMinus,
    Folder,
    FolderOpen,
    GitBranch,
    ArrowUp,
    ArrowDown,
    ArrowClockwise,
    MagnifyingGlass,
    Warning,
    CheckCircle,
}

impl Icon {
    /// Every bundled icon, for the test that checks each one loads.
    #[cfg(test)]
    pub const ALL: &[Icon] = &[
        Self::CheckSquare,
        Self::Square,
        Self::MinusSquare,
        Self::CaretRight,
        Self::CaretDown,
        Self::File,
        Self::FilePlus,
        Self::FileMinus,
        Self::Folder,
        Self::FolderOpen,
        Self::GitBranch,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::ArrowClockwise,
        Self::MagnifyingGlass,
        Self::Warning,
        Self::CheckCircle,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::CheckSquare => "check-square",
            Self::Square => "square",
            Self::MinusSquare => "minus-square",
            Self::CaretRight => "caret-right",
            Self::CaretDown => "caret-down",
            Self::File => "file",
            Self::FilePlus => "file-plus",
            Self::FileMinus => "file-minus",
            Self::Folder => "folder",
            Self::FolderOpen => "folder-open",
            Self::GitBranch => "git-branch",
            Self::ArrowUp => "arrow-up",
            Self::ArrowDown => "arrow-down",
            Self::ArrowClockwise => "arrow-clockwise",
            Self::MagnifyingGlass => "magnifying-glass",
            Self::Warning => "warning",
            Self::CheckCircle => "check-circle",
        }
    }

    /// Asset path of the regular weight.
    pub fn path(self) -> String {
        format!("icons/{}.svg", self.name())
    }

    /// Asset path of the fill weight, for active or checked states.
    pub fn fill_path(self) -> String {
        format!("icons/{}-fill.svg", self.name())
    }
}

/// The icon as an element of `size`, painted in `colour`.
pub fn view(icon: Icon, size: Pixels, colour: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .size(size)
        .flex_none()
        .text_color(colour)
}

/// The fill weight of the icon.
pub fn filled(icon: Icon, size: Pixels, colour: Hsla) -> Svg {
    svg()
        .path(icon.fill_path())
        .size(size)
        .flex_none()
        .text_color(colour)
}

/// An icon followed by a label, centred on one line.
pub fn labelled(icon: Icon, size: Pixels, colour: Hsla, label: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .text_color(colour)
        .child(view(icon, size, colour))
        .child(label)
}
