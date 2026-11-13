//! Keys and key sequences in one notation, for the code and for a person's key map (D241):
//! `j`, `G`, `<space>`, `<tab>`, `<cr>`, `<esc>`, `<c-s>`, `<s-down>`, `<pagedown>`, and
//! sequences separated by spaces, such as `<space> s` or `g g`. The notation is vim's.

use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One key with its modifiers. A capital letter is its own key (`G`), as in vim, so Shift is
/// kept only for keys that have no capital (`<s-down>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub code: Code,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// The keys the notation names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Code {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Delete,
    F(u8),
}

impl Key {
    pub const fn plain(code: Code) -> Self {
        Self {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    pub const fn char(c: char) -> Self {
        Self::plain(Code::Char(c))
    }

    pub const fn ctrl(c: char) -> Self {
        Self {
            ctrl: true,
            ..Self::char(c)
        }
    }

    /// The key a terminal reported, or `None` for one the notation does not name.
    pub fn from_event(event: KeyEvent) -> Option<Self> {
        let code = match event.code {
            KeyCode::Char(c) => Code::Char(c),
            KeyCode::Enter => Code::Enter,
            KeyCode::Esc => Code::Esc,
            KeyCode::Tab => Code::Tab,
            KeyCode::BackTab => Code::BackTab,
            KeyCode::Backspace => Code::Backspace,
            KeyCode::Up => Code::Up,
            KeyCode::Down => Code::Down,
            KeyCode::Left => Code::Left,
            KeyCode::Right => Code::Right,
            KeyCode::PageUp => Code::PageUp,
            KeyCode::PageDown => Code::PageDown,
            KeyCode::Home => Code::Home,
            KeyCode::End => Code::End,
            KeyCode::Delete => Code::Delete,
            KeyCode::F(n) => Code::F(n),
            _ => return None,
        };
        let mods = event.modifiers;
        // A character carries its own case; Shift on it says nothing more.
        let shift = mods.contains(KeyModifiers::SHIFT) && !matches!(code, Code::Char(_));
        Some(Self {
            code,
            ctrl: mods.contains(KeyModifiers::CONTROL),
            alt: mods.contains(KeyModifiers::ALT),
            shift,
        })
    }

    /// Parses one key: `j`, `<space>`, `<c-s>`, `<s-down>`, `<a-cr>`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c != '<' && c != ' ' => return Ok(Self::char(c)),
            (Some('<'), Some(_)) if text.ends_with('>') => {}
            _ => return Err(format!("not a key: {text:?}")),
        }
        let inner = text[1..text.len() - 1].to_ascii_lowercase();
        let mut key = Self::char(' ');
        let mut rest = inner.as_str();
        while let Some((modifier, tail)) = rest
            .split_once('-')
            .filter(|(m, t)| m.len() == 1 && !t.is_empty())
        {
            match modifier {
                "c" => key.ctrl = true,
                "a" | "m" => key.alt = true,
                "s" => key.shift = true,
                _ => return Err(format!("unknown modifier {modifier:?} in {text:?}")),
            }
            rest = tail;
        }
        key.code = match rest {
            "space" => Code::Char(' '),
            "cr" | "enter" | "return" => Code::Enter,
            "esc" => Code::Esc,
            "tab" => Code::Tab,
            "bs" | "backspace" => Code::Backspace,
            "up" => Code::Up,
            "down" => Code::Down,
            "left" => Code::Left,
            "right" => Code::Right,
            "pageup" => Code::PageUp,
            "pagedown" => Code::PageDown,
            "home" => Code::Home,
            "end" => Code::End,
            "del" => Code::Delete,
            "lt" => Code::Char('<'),
            f if f.starts_with('f') && f.len() > 1 => {
                Code::F(f[1..].parse().map_err(|_| format!("not a key: {text:?}"))?)
            }
            c if c.chars().count() == 1 => {
                // `<c-S>` keeps the case of the original.
                let original = &text[text.len() - 1 - c.len()..text.len() - 1];
                Code::Char(original.chars().next().unwrap_or(' '))
            }
            _ => return Err(format!("not a key: {text:?}")),
        };
        if key.shift && matches!(key.code, Code::Char(_)) {
            return Err(format!(
                "write a capital letter instead of shift in {text:?}"
            ));
        }
        Ok(key)
    }
}

impl fmt::Display for Key {
    /// The key as the menu and the keys screen show it: `j`, `Space`, `Ctrl+S`, `Shift+↓`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        match self.code {
            Code::Char(' ') => f.write_str("Space"),
            Code::Char(c) if self.ctrl => write!(f, "{}", c.to_ascii_uppercase()),
            Code::Char(c) => write!(f, "{c}"),
            Code::Enter => f.write_str("Enter"),
            Code::Esc => f.write_str("Esc"),
            Code::Tab => f.write_str("Tab"),
            Code::BackTab => f.write_str("Shift+Tab"),
            Code::Backspace => f.write_str("Backspace"),
            Code::Up => f.write_str("↑"),
            Code::Down => f.write_str("↓"),
            Code::Left => f.write_str("←"),
            Code::Right => f.write_str("→"),
            Code::PageUp => f.write_str("PgUp"),
            Code::PageDown => f.write_str("PgDn"),
            Code::Home => f.write_str("Home"),
            Code::End => f.write_str("End"),
            Code::Delete => f.write_str("Del"),
            Code::F(n) => write!(f, "F{n}"),
        }
    }
}

/// Parses a sequence: keys separated by spaces, such as `<space> s` or `g g`.
pub fn parse_sequence(text: &str) -> Result<Vec<Key>, String> {
    let keys: Vec<Key> = text
        .split_whitespace()
        .map(Key::parse)
        .collect::<Result<_, _>>()?;
    if keys.is_empty() {
        return Err("an empty key sequence".to_owned());
    }
    Ok(keys)
}

/// A sequence as the menu shows it: `Space s`, `g g`.
pub fn show_sequence(keys: &[Key]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    #[test]
    fn the_notation_names_keys_as_vim_does() {
        assert_eq!(Key::parse("j"), Ok(Key::char('j')));
        assert_eq!(Key::parse("G"), Ok(Key::char('G')));
        assert_eq!(Key::parse("<space>"), Ok(Key::char(' ')));
        assert_eq!(Key::parse("<c-s>"), Ok(Key::ctrl('s')));
        assert_eq!(
            Key::parse("<s-down>"),
            Ok(Key {
                shift: true,
                ..Key::plain(Code::Down)
            })
        );
        assert_eq!(Key::parse("<CR>"), Ok(Key::plain(Code::Enter)));
        assert_eq!(Key::parse("<f1>"), Ok(Key::plain(Code::F(1))));
        assert!(Key::parse("<s-a>").is_err());
        assert!(Key::parse("<nope>").is_err());
        assert!(Key::parse("ab").is_err());
        assert_eq!(
            parse_sequence("<space> s"),
            Ok(vec![Key::char(' '), Key::char('s')])
        );
        assert_eq!(
            parse_sequence("g g"),
            Ok(vec![Key::char('g'), Key::char('g')])
        );
        assert!(parse_sequence("  ").is_err());
    }

    #[test]
    fn keys_show_as_people_read_them() {
        assert_eq!(
            show_sequence(&parse_sequence("<space> s").unwrap()),
            "Space s"
        );
        assert_eq!(Key::ctrl('s').to_string(), "Ctrl+S");
        assert_eq!(Key::parse("<s-down>").unwrap().to_string(), "Shift+↓");
    }

    #[test]
    fn a_terminal_key_becomes_the_same_key() {
        let event = |code, modifiers| KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert_eq!(
            Key::from_event(event(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Key::char('G'))
        );
        assert_eq!(
            Key::from_event(event(KeyCode::Down, KeyModifiers::SHIFT)),
            Key::parse("<s-down>").ok()
        );
        assert_eq!(
            Key::from_event(event(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            Some(Key::ctrl('s'))
        );
    }
}
