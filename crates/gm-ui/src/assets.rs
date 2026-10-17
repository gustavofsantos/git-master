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
    "caret-down",
    "caret-down-fill",
    "folder",
    "folder-fill",
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
    "terminal",
    "terminal-fill",
];

/// The app icon in both themes, as the PNG sizes the welcome screen draws.
const BRAND: &[(&str, &[u8])] = &[
    (
        "brand/gitmaster-dark-256.png",
        include_bytes!("../../../assets/icons/png/dark/gitmaster-256.png") as &[u8],
    ),
    (
        "brand/gitmaster-light-256.png",
        include_bytes!("../../../assets/icons/png/light/gitmaster-256.png") as &[u8],
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .chain(BRAND)
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

    #[test]
    fn both_app_icons_load_as_png() {
        for path in [
            "brand/gitmaster-dark-256.png",
            "brand/gitmaster-light-256.png",
        ] {
            let bytes = Assets
                .load(path)
                .unwrap()
                .unwrap_or_else(|| panic!("{path} missing"));
            assert!(bytes.starts_with(b"\x89PNG"), "{path} is not a png");
        }
    }
}
