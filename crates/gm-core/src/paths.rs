//! The app's data directory (D17, D64): the trust store and the workflow trash live in it, never in
//! a repository. `$XDG_DATA_HOME/gitmaster`, or `~/.local/share/gitmaster`, on Linux and
//! `~/Library/Application Support/gitmaster` on macOS. A relative `XDG_DATA_HOME` is ignored, and
//! without a home directory there is none.

use std::ffi::OsString;
use std::path::PathBuf;

/// The data directory under the given environment values.
#[must_use]
pub fn data_dir_under(
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
    macos: bool,
) -> Option<PathBuf> {
    let absolute = |value: Option<OsString>| {
        value
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
    };
    let base = if macos {
        absolute(home)?.join("Library/Application Support")
    } else {
        match absolute(xdg_data_home) {
            Some(base) => base,
            None => absolute(home)?.join(".local/share"),
        }
    };
    Some(base.join("gitmaster"))
}

/// The data directory of this process, from the real environment.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    data_dir_under(
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
        cfg!(target_os = "macos"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_dir_follows_xdg_then_home() {
        assert_eq!(
            data_dir_under(Some("/x".into()), Some("/h".into()), false),
            Some(PathBuf::from("/x/gitmaster"))
        );
        assert_eq!(
            data_dir_under(Some("rel".into()), Some("/h".into()), false),
            Some(PathBuf::from("/h/.local/share/gitmaster"))
        );
        assert_eq!(
            data_dir_under(None, Some("/h".into()), true),
            Some(PathBuf::from("/h/Library/Application Support/gitmaster"))
        );
        assert_eq!(data_dir_under(None, None, false), None);
    }
}
