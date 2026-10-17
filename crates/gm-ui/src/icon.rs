//! The Phosphor icons the window draws (MIT, see `assets/icons/LICENSE-phosphor`),
//! painted in the text colour of their parent. Each name has a regular and a
//! fill weight.

use gpui::{Hsla, Pixels, Styled, Svg, px, svg};

/// An icon in a row or a button.
pub const SIZE: Pixels = px(16.);
/// An icon beside small counter text.
pub const SMALL: Pixels = px(14.);

/// Every bundled icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    CheckSquare,
    Square,
    CaretDown,
    Folder,
    GitBranch,
    ArrowUp,
    ArrowDown,
    MagnifyingGlass,
    Warning,
    CheckCircle,
    Minimize,
    Maximize,
    Restore,
    Close,
    Terminal,
}

impl Icon {
    /// Every bundled icon, for the test that checks each one loads.
    #[cfg(test)]
    pub const ALL: &[Icon] = &[
        Self::CheckSquare,
        Self::Square,
        Self::CaretDown,
        Self::Folder,
        Self::GitBranch,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::MagnifyingGlass,
        Self::Warning,
        Self::CheckCircle,
        Self::Minimize,
        Self::Maximize,
        Self::Restore,
        Self::Close,
        Self::Terminal,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::CheckSquare => "check-square",
            Self::Square | Self::Maximize => "square",
            Self::CaretDown => "caret-down",
            Self::Folder => "folder",
            Self::GitBranch => "git-branch",
            Self::ArrowUp => "arrow-up",
            Self::ArrowDown => "arrow-down",
            Self::MagnifyingGlass => "magnifying-glass",
            Self::Warning => "warning",
            Self::CheckCircle => "check-circle",
            Self::Minimize => "minus",
            Self::Restore => "copy",
            Self::Close => "x",
            Self::Terminal => "terminal",
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
