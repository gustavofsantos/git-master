//! The companion's icons (D238): Nerd Font glyphs only, with no fallback; the terminal must
//! use a Nerd Font. Each is named by its Nerd Fonts class, so a wrong one is easy to check.
//! Box-drawing lines (`│`, `┌─┐`) are not icons and stay as they are.

/// `nf-dev-git_branch`
pub const BRANCH: &str = "\u{e725}";
/// `nf-fa-arrow_up`
pub const AHEAD: &str = "\u{f062}";
/// `nf-fa-arrow_down`
pub const BEHIND: &str = "\u{f063}";
/// `nf-fa-folder`
pub const FOLDER: &str = "\u{f07b}";
/// `nf-fa-comments`: the threads on a file.
pub const THREADS: &str = "\u{f086}";
/// `nf-fa-caret_right`: a line of the selection.
pub const SELECTED: &str = "\u{f0da}";
/// `nf-fa-circle_o`: an open thread.
pub const OPEN: &str = "\u{f10c}";
/// `nf-fa-check`: a thread the agent handled.
pub const HANDLED: &str = "\u{f00c}";
/// `nf-fa-check_circle`: a resolved thread.
pub const RESOLVED: &str = "\u{f058}";
/// `nf-fa-exclamation_triangle`: an outdated thread.
pub const OUTDATED: &str = "\u{f071}";
/// `nf-md-robot`: what an agent wrote.
pub const AGENT: &str = "\u{f06a9}";
/// `nf-fa-check`: a tool call that ended well.
pub const CALL_DONE: &str = "\u{f00c}";
/// `nf-fa-times`: a tool call that failed.
pub const CALL_FAILED: &str = "\u{f00d}";
/// `nf-fa-clock_o`: a tool call with no result recorded.
pub const CALL_PENDING: &str = "\u{f017}";
/// `nf-fa-chevron_right`: something folded that a click unfolds.
pub const FOLDED: &str = "\u{f054}";
/// `nf-fa-chevron_down`: something unfolded.
pub const UNFOLDED: &str = "\u{f078}";

/// The icon of a file, by its name: a language's own where Nerd Fonts has one, else a page.
pub fn file(name: &str) -> &'static str {
    if name.ends_with('/') {
        // An untracked folder, as Git lists it.
        return FOLDER;
    }
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension {
        "rs" => "\u{e7a8}",                                     // nf-dev-rust
        "py" => "\u{e73c}",                                     // nf-dev-python
        "go" => "\u{e627}",                                     // nf-seti-go
        "ts" | "tsx" => "\u{e628}",                             // nf-seti-typescript
        "js" | "jsx" | "mjs" | "cjs" => "\u{e74e}",             // nf-dev-javascript
        "java" => "\u{e738}",                                   // nf-dev-java
        "c" | "h" => "\u{e61e}",                                // nf-custom-c
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "\u{e61d}",      // nf-custom-cpp
        "cs" => "\u{f031b}",                                    // nf-md-language_csharp
        "rb" => "\u{e739}",                                     // nf-dev-ruby
        "lua" => "\u{e620}",                                    // nf-seti-lua
        "sh" | "bash" | "zsh" => "\u{e795}",                    // nf-dev-terminal
        "md" => "\u{e73e}",                                     // nf-dev-markdown
        "json" => "\u{e60b}",                                   // nf-seti-json
        "toml" | "yaml" | "yml" | "ini" | "lock" => "\u{e615}", // nf-seti-config
        "html" | "htm" => "\u{e736}",                           // nf-dev-html5
        "css" | "scss" => "\u{e749}",                           // nf-dev-css3
        _ => "\u{f016}",                                        // nf-fa-file_o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_in_a_private_use_area() {
        let private = |text: &str| {
            text.chars().all(|c| {
                ('\u{e000}'..='\u{f8ff}').contains(&c) || ('\u{f0000}'..='\u{fffff}').contains(&c)
            })
        };
        for icon in [
            BRANCH,
            AHEAD,
            BEHIND,
            FOLDER,
            THREADS,
            SELECTED,
            OPEN,
            HANDLED,
            RESOLVED,
            OUTDATED,
            AGENT,
            CALL_DONE,
            CALL_FAILED,
            CALL_PENDING,
            FOLDED,
            UNFOLDED,
        ] {
            assert!(private(icon), "{icon:?}");
        }
        for name in ["a.rs", "b.py", "x.unknown", "Makefile", "c.toml", "d.md"] {
            assert!(private(file(name)), "{name}");
        }
    }
}
