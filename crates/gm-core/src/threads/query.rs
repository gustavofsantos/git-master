//! Turns the user's search text into an FTS5 `MATCH` expression (D128). The text
//! is never passed through: FTS operators typed by accident become ordinary
//! words, and only the rules below put FTS syntax into the query.

/// Builds an FTS5 `MATCH` expression from plain user text, or `None` when the
/// text holds no word.
///
/// Rules: a word token is a maximal run of Unicode alphanumeric characters and
/// `_`; every other character only separates tokens. Text inside a balanced pair
/// of double quotes is a phrase (`"a b"`). Each other token is a prefix term
/// (`"tok"*`). Terms are joined with ` AND `. `AND`, `OR`, `NOT` and `NEAR` are
/// ordinary words because they are quoted like any other token.
#[must_use]
pub fn build_fts_query(user_text: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut word = String::new();
    // `Some` while inside an open quote, holding the phrase's tokens so far.
    let mut phrase: Option<Vec<String>> = None;

    for ch in user_text.chars() {
        if ch == '"' {
            flush_word(&mut word, &mut phrase, &mut terms);
            match phrase.take() {
                Some(tokens) => {
                    if !tokens.is_empty() {
                        terms.push(phrase_term(&tokens));
                    }
                }
                None => phrase = Some(Vec::new()),
            }
        } else if is_word_char(ch) {
            word.push(ch);
        } else {
            flush_word(&mut word, &mut phrase, &mut terms);
        }
    }
    flush_word(&mut word, &mut phrase, &mut terms);

    // An unbalanced quote: the words after it are ordinary words, not a phrase.
    if let Some(tokens) = phrase {
        terms.extend(tokens.iter().map(|token| prefix_term(token)));
    }

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Moves the pending word into the open phrase, or into `terms` as a prefix term.
fn flush_word(word: &mut String, phrase: &mut Option<Vec<String>>, terms: &mut Vec<String>) {
    if word.is_empty() {
        return;
    }
    let token = std::mem::take(word);
    match phrase {
        Some(tokens) => tokens.push(token),
        None => terms.push(prefix_term(&token)),
    }
}

fn prefix_term(token: &str) -> String {
    format!("\"{}\"*", escape(token))
}

fn phrase_term(tokens: &[String]) -> String {
    format!("\"{}\"", escape(&tokens.join(" ")))
}

/// Doubles embedded double quotes, the FTS5 string escape. Tokens never contain
/// one, so this only guards the helper's contract.
fn escape(text: &str) -> String {
    text.replace('"', "\"\"")
}

#[cfg(test)]
mod tests {
    use super::build_fts_query;
    use proptest::prelude::*;

    #[test]
    fn plain_words_become_prefix_terms_joined_by_and() {
        assert_eq!(
            build_fts_query("retry back").as_deref(),
            Some("\"retry\"* AND \"back\"*")
        );
    }

    #[test]
    fn quoted_text_is_a_phrase_and_mixes_with_words() {
        assert_eq!(
            build_fts_query("\"back off\"").as_deref(),
            Some("\"back off\"")
        );
        assert_eq!(
            build_fts_query("retry \"back off\"").as_deref(),
            Some("\"retry\"* AND \"back off\"")
        );
    }

    #[test]
    fn unbalanced_quote_falls_back_to_words() {
        assert_eq!(
            build_fts_query("\"back off").as_deref(),
            Some("\"back\"* AND \"off\"*")
        );
    }

    #[test]
    fn operators_typed_by_accident_are_plain_words() {
        assert_eq!(
            build_fts_query("foo AND (bar OR").as_deref(),
            Some("\"foo\"* AND \"AND\"* AND \"bar\"* AND \"OR\"*")
        );
        assert_eq!(
            build_fts_query("foo* NEAR:x ^y -z +w").as_deref(),
            Some("\"foo\"* AND \"NEAR\"* AND \"x\"* AND \"y\"* AND \"z\"* AND \"w\"*")
        );
    }

    #[test]
    fn unicode_words_are_kept_whole() {
        assert_eq!(
            build_fts_query("café naïve 日本").as_deref(),
            Some("\"café\"* AND \"naïve\"* AND \"日本\"*")
        );
    }

    #[test]
    fn underscore_identifiers_stay_one_token() {
        assert_eq!(
            build_fts_query("file_index").as_deref(),
            Some("\"file_index\"*")
        );
    }

    #[test]
    fn empty_or_symbol_only_text_has_no_query() {
        assert_eq!(build_fts_query(""), None);
        assert_eq!(build_fts_query("   \t\n"), None);
        assert_eq!(build_fts_query("*:()^-+"), None);
        assert_eq!(build_fts_query("\"\""), None);
        assert_eq!(build_fts_query("\"  \""), None);
    }

    #[test]
    fn fts5_accepts_the_query_and_finds_prefixes_and_phrases() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(x);
             INSERT INTO t(x) VALUES
               ('retry backoff'), ('back off'), ('backing off later'), ('retry the job');",
        )
        .expect("fts5 is available in the bundled SQLite");

        let matches = |text: &str| -> Vec<String> {
            let query = build_fts_query(text).expect("query");
            let mut stmt = conn
                .prepare("SELECT x FROM t WHERE t MATCH ?1 ORDER BY rowid")
                .expect("prepare");
            stmt.query_map([query.as_str()], |row| row.get::<_, String>(0))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };

        assert_eq!(matches("retry back"), vec!["retry backoff"]);
        assert_eq!(matches("\"back off\""), vec!["back off"]);
    }

    /// Checks the output contract: outside double quotes only words, spaces and
    /// `AND` appear, and a `*` appears only right after a closing quote.
    fn assert_fts_safe(query: &str) {
        let mut in_quote = false;
        let mut prev = ' ';
        for ch in query.chars() {
            if ch == '"' {
                in_quote = !in_quote;
            } else if !in_quote {
                assert!(
                    !matches!(ch, ':' | '(' | ')' | '^'),
                    "bare {ch:?} in {query:?}"
                );
                if ch == '*' {
                    assert_eq!(prev, '"', "bare * in {query:?}");
                }
            }
            prev = ch;
        }
        assert!(!in_quote, "unbalanced quote in {query:?}");
    }

    proptest! {
        #[test]
        fn output_never_contains_bare_fts_operators(input in "\\PC{0,80}") {
            if let Some(query) = build_fts_query(&input) {
                assert_fts_safe(&query);
            }
        }

        #[test]
        fn output_is_safe_for_operator_heavy_input(input in "[a-zA-Z \"*:()^_-]{0,60}") {
            if let Some(query) = build_fts_query(&input) {
                assert_fts_safe(&query);
            }
        }
    }
}
