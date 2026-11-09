//! Glob patterns for gates and reviewers (D214 (1)): `when.branches` matches a branch name, and
//! `when.paths` matches the repository-relative paths a change touches. Pure: no I/O and no
//! regex, so a pattern cannot make matching run away.
//!
//! `*` matches any run of characters except `/`, `**` matches any run including `/`, and `**/`
//! also matches no directory at all (so `a/**/b` matches `a/b`). `?` matches one character
//! except `/`. Everything else is literal and case-sensitive, and the whole text must match.
//!
//! Matching is a set simulation over the pattern's tokens, one text character at a time: the
//! work is the text length times the pattern length, and a pattern is at most 200 characters.

/// The longest pattern, in characters.
pub const PATTERN_LIMIT: usize = 200;

/// One step of a pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// A character that must be this one.
    Literal(char),
    /// `?`: one character except `/`.
    One,
    /// `*`: any run of characters except `/`.
    Star,
    /// `**`: any run of characters.
    AnyStar,
    /// `**/`: a run of characters ending in `/`, or nothing at all.
    AnyDirs,
}

/// Whether `text` matches `pattern` (see the module for the rules).
#[must_use]
pub fn matches(pattern: &str, text: &str) -> bool {
    let tokens = tokenize(pattern);
    let last = tokens.len();
    // `active[j]` says whether the pattern can have consumed `j` tokens so far. `entered[j]`
    // says the state was reached by moving on, not by staying in a loop: a `**/` may be
    // skipped only when entered, since it ends in a `/` it has just read.
    let mut active = vec![false; last + 1];
    let mut entered = vec![false; last + 1];
    let mut next = vec![false; last + 1];
    let mut next_entered = vec![false; last + 1];
    active[0] = true;
    entered[0] = true;
    close(&tokens, &mut active, &mut entered);
    for character in text.chars() {
        next.fill(false);
        next_entered.fill(false);
        for (index, token) in tokens.iter().enumerate() {
            if !active[index] {
                continue;
            }
            match *token {
                Token::Literal(literal) => {
                    if literal == character {
                        next[index + 1] = true;
                        next_entered[index + 1] = true;
                    }
                }
                Token::One => {
                    if character != '/' {
                        next[index + 1] = true;
                        next_entered[index + 1] = true;
                    }
                }
                Token::Star => {
                    if character != '/' {
                        next[index] = true;
                    }
                }
                Token::AnyStar => next[index] = true,
                Token::AnyDirs => {
                    next[index] = true;
                    if character == '/' {
                        next[index + 1] = true;
                        next_entered[index + 1] = true;
                    }
                }
            }
        }
        close(&tokens, &mut next, &mut next_entered);
        std::mem::swap(&mut active, &mut next);
        std::mem::swap(&mut entered, &mut next_entered);
        if !active.iter().any(|state| *state) {
            return false;
        }
    }
    active[last]
}

/// Checks a pattern of a branch or of any text: not empty, at most 200 characters and no NUL.
pub fn check_pattern(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("a pattern is not empty".to_owned());
    }
    if pattern.chars().count() > PATTERN_LIMIT {
        return Err(format!(
            "a pattern is at most {PATTERN_LIMIT} characters, not {}",
            pattern.chars().count()
        ));
    }
    if pattern.contains('\0') {
        return Err("a pattern cannot contain a NUL character".to_owned());
    }
    Ok(())
}

/// Checks a pattern over repository paths: [`check_pattern`], and not absolute and without a
/// `..` segment, so it cannot name a file outside the repository.
pub fn check_path_pattern(pattern: &str) -> Result<(), String> {
    check_pattern(pattern)?;
    if pattern.starts_with('/') {
        return Err(format!(
            "`{pattern}` is absolute: a path is relative to the repository"
        ));
    }
    if pattern.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "`{pattern}` has a `..` segment: a path stays in the repository"
        ));
    }
    Ok(())
}

/// Splits a pattern into tokens. A run of two or more `*` is `**`, and `**` followed by `/` is
/// `**/`, which also takes that slash.
fn tokenize(pattern: &str) -> Vec<Token> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '*' => {
                let run = chars[index..].iter().take_while(|c| **c == '*').count();
                index += run;
                if run == 1 {
                    tokens.push(Token::Star);
                } else if chars.get(index) == Some(&'/') {
                    tokens.push(Token::AnyDirs);
                    index += 1;
                } else {
                    tokens.push(Token::AnyStar);
                }
            }
            '?' => {
                tokens.push(Token::One);
                index += 1;
            }
            literal => {
                tokens.push(Token::Literal(literal));
                index += 1;
            }
        }
    }
    tokens
}

/// Adds the states a token can skip without consuming a character: a star may match nothing
/// at any point, and `**/` may match nothing only where it was entered (see `matches`). States
/// are visited in order, so a chain of skips is followed in one pass.
fn close(tokens: &[Token], states: &mut [bool], entered: &mut [bool]) {
    for (index, token) in tokens.iter().enumerate() {
        let skippable = match token {
            Token::Star | Token::AnyStar => states[index],
            Token::AnyDirs => entered[index],
            _ => false,
        };
        if skippable {
            states[index + 1] = true;
            entered[index + 1] = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_literal_matches_only_itself() {
        assert!(matches("feat/x", "feat/x"));
        assert!(!matches("feat/x", "feat/xy"));
        assert!(!matches("feat/x", "feat/"));
        assert!(!matches("feat/x", "Feat/x"));
    }

    #[test]
    fn a_star_stays_inside_one_segment() {
        assert!(matches("feat/*", "feat/a"));
        assert!(matches("feat/*", "feat/"));
        assert!(!matches("feat/*", "feat/a/b"));
        assert!(matches("*.rs", "lib.rs"));
        assert!(!matches("*", "a/b"));
        assert!(matches("a*b*c", "axxbyyc"));
    }

    #[test]
    fn a_double_star_crosses_segments() {
        assert!(matches("services/**", "services/billing/api/main.rs"));
        assert!(matches("services/**", "services/"));
        assert!(!matches("services/**", "services"));
        assert!(matches("**", "a/b/c"));
        assert!(matches("**", ""));
        assert!(matches("a**b", "a/x/b"));
    }

    #[test]
    fn a_double_star_and_slash_also_matches_no_directory() {
        assert!(matches("a/**/b", "a/b"));
        assert!(matches("a/**/b", "a/x/b"));
        assert!(matches("a/**/b", "a/x/y/b"));
        assert!(!matches("a/**/b", "a/xb"));
        assert!(!matches("a/**/b", "ab"));
        assert!(matches("**/*.md", "README.md"));
        assert!(matches("**/*.md", "docs/a/b.md"));
    }

    #[test]
    fn a_run_of_stars_is_a_double_star() {
        assert!(matches("a/***", "a/b/c"));
        assert!(matches("a/***/b", "a/x/b"));
    }

    #[test]
    fn a_question_mark_is_one_character_but_not_a_slash() {
        assert!(matches("v?", "v1"));
        assert!(!matches("v?", "v"));
        assert!(!matches("v?", "v12"));
        assert!(!matches("a?b", "a/b"));
        assert!(matches("a?b", "axb"));
    }

    #[test]
    fn the_whole_text_must_match() {
        assert!(!matches("feat", "feat/x"));
        assert!(!matches("feat/*", "xfeat/a"));
        assert!(matches("", ""));
        assert!(!matches("", "a"));
    }

    #[test]
    fn a_pathological_pattern_on_a_long_text_finishes_at_once() {
        let text = "a".repeat(10_000);
        assert!(!matches("*a*a*a*a*b", &text));
        assert!(matches("*a*a*a*a*a", &text));
        assert!(matches("**a**a**a**a**a", &text));
        assert!(!matches("**/**/**/**/x", &text));
        let deep = "a/".repeat(5_000);
        assert!(matches("**/a", &format!("{deep}a")));
    }

    #[test]
    fn a_pattern_is_checked_for_length_emptiness_and_nul() {
        assert!(check_pattern("feat/*").is_ok());
        assert!(check_pattern("").is_err());
        assert!(check_pattern(&"a".repeat(200)).is_ok());
        assert!(
            check_pattern(&"a".repeat(201))
                .unwrap_err()
                .contains("at most 200")
        );
        assert!(check_pattern("a\0b").unwrap_err().contains("NUL"));
    }

    #[test]
    fn a_path_pattern_is_relative_and_stays_in_the_repository() {
        assert!(check_path_pattern("services/billing/**").is_ok());
        assert!(check_path_pattern("").is_err());
        assert!(
            check_path_pattern("/etc/*")
                .unwrap_err()
                .contains("absolute")
        );
        assert!(check_path_pattern("a/../b").unwrap_err().contains("`..`"));
        assert!(check_path_pattern("..").unwrap_err().contains("`..`"));
        assert!(check_path_pattern("a..b/*").is_ok());
        assert!(check_path_pattern("a\0").is_err());
    }

    proptest! {
        #[test]
        fn a_literal_pattern_matches_only_its_own_text(
            pattern in "[a-z/]{0,20}",
            text in "[a-z/]{0,20}",
        ) {
            prop_assert_eq!(matches(&pattern, &text), pattern == text);
        }

        #[test]
        fn a_double_star_matches_every_text(text in "[a-zA-Z0-9/._-]{0,40}") {
            prop_assert!(matches("**", &text));
        }

        #[test]
        fn a_pattern_with_one_character_replaced_by_a_question_mark_matches_it(
            text in "[a-z/]{1,20}",
            position in 0usize..20,
        ) {
            let chars: Vec<char> = text.chars().collect();
            let index = position % chars.len();
            prop_assume!(chars[index] != '/');
            let mut pattern = chars.clone();
            pattern[index] = '?';
            let pattern: String = pattern.into_iter().collect();
            prop_assert!(matches(&pattern, &text));
        }
    }
}
