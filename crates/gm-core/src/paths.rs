//! The app's directories, the same on every OS (D-paths, docs/reference/configuration.md): the
//! config directory is `$XDG_CONFIG_HOME/gitmaster`, or `~/.config/gitmaster` when that is unset;
//! the data directory is `$XDG_DATA_HOME/gitmaster`, or `~/.local/share/gitmaster`. A relative or
//! empty XDG value is ignored, and without a home directory there is no directory. Nothing is
//! written into a repository (D17, D64).

use std::ffi::OsString;
use std::path::PathBuf;

/// The directory under `xdg`, or under `home` joined with `fallback` when `xdg` is not usable.
fn base_under(xdg: Option<OsString>, home: Option<OsString>, fallback: &str) -> Option<PathBuf> {
    let absolute = |value: Option<OsString>| {
        value
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
    };
    let base = match absolute(xdg) {
        Some(base) => base,
        None => absolute(home)?.join(fallback),
    };
    Some(base.join("gitmaster"))
}

/// The config directory under the given environment values.
#[must_use]
pub fn config_dir_under(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    base_under(xdg_config_home, home, ".config")
}

/// The data directory under the given environment values.
#[must_use]
pub fn data_dir_under(xdg_data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    base_under(xdg_data_home, home, ".local/share")
}

/// The config directory of this process, from the real environment.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    config_dir_under(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// The data directory of this process, from the real environment.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    data_dir_under(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_dir_follows_xdg_then_home() {
        assert_eq!(
            data_dir_under(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x/gitmaster"))
        );
        assert_eq!(
            data_dir_under(None, Some("/h".into())),
            Some(PathBuf::from("/h/.local/share/gitmaster"))
        );
    }

    #[test]
    fn the_config_dir_follows_xdg_then_home() {
        assert_eq!(
            config_dir_under(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x/gitmaster"))
        );
        assert_eq!(
            config_dir_under(None, Some("/h".into())),
            Some(PathBuf::from("/h/.config/gitmaster"))
        );
    }

    #[test]
    fn the_same_layout_on_a_linux_and_a_mac_home() {
        // There is no per-OS branch: the home directory alone decides the base.
        assert_eq!(
            config_dir_under(None, Some("/Users/u".into())),
            Some(PathBuf::from("/Users/u/.config/gitmaster"))
        );
        assert_eq!(
            data_dir_under(None, Some("/Users/u".into())),
            Some(PathBuf::from("/Users/u/.local/share/gitmaster"))
        );
        assert_eq!(
            config_dir_under(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.config/gitmaster"))
        );
        assert_eq!(
            data_dir_under(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/share/gitmaster"))
        );
    }

    #[test]
    fn the_xdg_override_is_honored_on_a_mac_home() {
        assert_eq!(
            config_dir_under(Some("/cfg".into()), Some("/Users/u".into())),
            Some(PathBuf::from("/cfg/gitmaster"))
        );
        assert_eq!(
            data_dir_under(Some("/data".into()), Some("/Users/u".into())),
            Some(PathBuf::from("/data/gitmaster"))
        );
    }

    #[test]
    fn a_relative_or_empty_xdg_value_is_ignored() {
        for bad in ["relative/dir", ""] {
            assert_eq!(
                config_dir_under(Some(bad.into()), Some("/Users/u".into())),
                Some(PathBuf::from("/Users/u/.config/gitmaster"))
            );
            assert_eq!(
                data_dir_under(Some(bad.into()), Some("/Users/u".into())),
                Some(PathBuf::from("/Users/u/.local/share/gitmaster"))
            );
        }
    }

    #[test]
    fn no_home_means_no_directory() {
        assert_eq!(config_dir_under(None, None), None);
        assert_eq!(data_dir_under(None, None), None);
        assert_eq!(config_dir_under(Some("relative".into()), None), None);
        assert_eq!(data_dir_under(Some("".into()), None), None);
    }
}
