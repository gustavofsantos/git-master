//! The bundled assets: the Phosphor icons, embedded in the binary so the
//! window never reads a file from disk at runtime.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Embeds `assets/icons/<name>.svg` as `("icons/<name>.svg", bytes)`.
macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((
            concat!("icons/", $name, ".svg"),
            include_bytes!(concat!("../assets/icons/", $name, ".svg")) as &[u8],
        )),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "check-square",
    "check-square-fill",
    "square",
    "square-fill",
    "minus-square",
    "minus-square-fill",
    "caret-right",
    "caret-right-fill",
    "caret-down",
    "caret-down-fill",
    "file",
    "file-fill",
    "file-plus",
    "file-plus-fill",
    "file-minus",
    "file-minus-fill",
    "folder",
    "folder-fill",
    "folder-open",
    "folder-open-fill",
    "git-branch",
    "git-branch-fill",
    "arrow-up",
    "arrow-up-fill",
    "arrow-down",
    "arrow-down-fill",
    "arrow-clockwise",
    "arrow-clockwise-fill",
    "magnifying-glass",
    "magnifying-glass-fill",
    "warning",
    "warning-fill",
    "check-circle",
    "check-circle-fill",
    "minus",
    "minus-fill",
    "copy",
    "copy-fill",
    "x",
    "x-fill",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon::Icon;

    #[test]
    fn every_icon_loads_in_both_weights_and_is_svg() {
        for icon in Icon::ALL {
            for path in [icon.path(), icon.fill_path()] {
                let bytes = Assets
                    .load(&path)
                    .unwrap()
                    .unwrap_or_else(|| panic!("{path} missing"));
                assert!(bytes.starts_with(b"<svg"), "{path} is not an svg");
            }
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }
}
