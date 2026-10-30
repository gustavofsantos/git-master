//! Turns the user's search text into FTS5 `MATCH` expressions (D128, D131). The text
//! is never passed through: FTS operators typed by accident become ordinary
//! words, and only the rules below put FTS syntax into the query.

/// Fewest characters a token needs to be searched in the trigram index.
pub const MIN_SUBSTRING_CHARS: usize = 3;
/// Fewest characters a word needs to be searched by similarity (D131).
pub const MIN_FUZZY_CHARS: usize = 4;

/// One piece of the user's text: a word, or a quoted phrase of words.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Term {
    Word(String),
    /// Non-empty; the words of a phrase in order.
    Phrase(Vec<String>),
}

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
    let terms: Vec<String> = parse_terms(user_text)
        .iter()
        .map(|term| match term {
            Term::Word(word) => prefix_term(word),
            Term::Phrase(tokens) => phrase_term(tokens),
        })
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

/// The trigram queries of a search (D131). Both are FTS5 `MATCH` expressions over the
/// trigram tables, built from the same tokens as `build_fts_query`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrigramQuery {
    /// Every token as a substring, joined by AND: `"index" AND "retry"`.
    pub substring: String,
    /// For each single word of at least `MIN_FUZZY_CHARS` characters, the OR of its
    /// trigrams, the groups joined by AND. Empty when no word qualifies. Phrases are not
    /// in it: a phrase asks for an exact sequence, so it never falls back to similarity.
    pub fuzzy: String,
    /// Lowercased tokens of at least `MIN_SUBSTRING_CHARS` characters, in order. A phrase
    /// keeps its inner spaces. Similarity is verified against these in Rust.
    pub tokens: Vec<String>,
}

/// Builds the trigram queries of plain user text, or `None` when no token has enough
/// characters for the trigram index (tokens shorter than three characters are dropped).
#[must_use]
pub fn build_trigram_queries(user_text: &str) -> Option<TrigramQuery> {
    let mut substrings = Vec::new();
    let mut groups = Vec::new();
    let mut tokens = Vec::new();
    for term in parse_terms(user_text) {
        let (token, is_word) = match term {
            Term::Word(word) => (word, true),
            Term::Phrase(words) => (words.join(" "), false),
        };
        let token = token.to_lowercase();
        let chars = token.chars().count();
        if chars < MIN_SUBSTRING_CHARS {
            continue;
        }
        substrings.push(quoted(&token));
        if is_word && chars >= MIN_FUZZY_CHARS {
            let mut grams: Vec<String> = Vec::new();
            for gram in trigrams(&token) {
                let quoted = quoted(&gram);
                if !grams.contains(&quoted) {
                    grams.push(quoted);
                }
            }
            groups.push(format!("({})", grams.join(" OR ")));
        }
        tokens.push(token);
    }
    if substrings.is_empty() {
        return None;
    }
    Some(TrigramQuery {
        substring: substrings.join(" AND "),
        fuzzy: groups.join(" AND "),
        tokens,
    })
}

/// Dice coefficient of the trigram sets of two texts, in `0.0..=1.0`. Each word is
/// padded with two spaces before and one after, so the first and last letters count:
/// `bakcoff` and `backoff` share 4 of 8 trigrams each and score exactly `0.5`. Case is
/// ignored. An empty text scores `0.0`.
#[must_use]
pub fn trigram_similarity(a: &str, b: &str) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    dice(&trigram_set(a), &trigram_set(b))
}

/// The padded trigram set of a text, sorted and without repeats, for `dice`.
#[must_use]
pub fn trigram_set(text: &str) -> Vec<[char; 3]> {
    let mut chars = vec![' ', ' '];
    chars.extend(text.chars().flat_map(char::to_lowercase));
    chars.push(' ');
    let mut grams: Vec<[char; 3]> = chars.windows(3).map(|w| [w[0], w[1], w[2]]).collect();
    grams.sort_unstable();
    grams.dedup();
    grams
}

/// Dice coefficient of two sorted trigram sets.
#[must_use]
pub fn dice(a: &[[char; 3]], b: &[[char; 3]]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut common) = (0, 0, 0_usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    // Saturating conversions keep the arithmetic in `f32::from`, which is exact for u16.
    let common = f32::from(u16::try_from(common).unwrap_or(u16::MAX));
    let total = f32::from(u16::try_from(a.len()).unwrap_or(u16::MAX))
        + f32::from(u16::try_from(b.len()).unwrap_or(u16::MAX));
    2.0 * common / total
}

/// True for the characters of a word token: alphanumeric or `_`.
pub(super) fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Splits the user's text into terms, the same way for both queries.
fn parse_terms(user_text: &str) -> Vec<Term> {
    let mut terms: Vec<Term> = Vec::new();
    let mut word = String::new();
    // `Some` while inside an open quote, holding the phrase's words so far.
    let mut phrase: Option<Vec<String>> = None;

    for ch in user_text.chars() {
        if ch == '"' {
            flush_word(&mut word, &mut phrase, &mut terms);
            match phrase.take() {
                Some(tokens) => {
                    if !tokens.is_empty() {
                        terms.push(Term::Phrase(tokens));
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
        terms.extend(tokens.into_iter().map(Term::Word));
    }
    terms
}

/// Moves the pending word into the open phrase, or into `terms` as a word.
fn flush_word(word: &mut String, phrase: &mut Option<Vec<String>>, terms: &mut Vec<Term>) {
    if word.is_empty() {
        return;
    }
    let token = std::mem::take(word);
    match phrase {
        Some(tokens) => tokens.push(token),
        None => terms.push(Term::Word(token)),
    }
}

/// The trigrams of a token, in order, repeats included.
fn trigrams(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    chars.windows(3).map(|w| w.iter().collect()).collect()
}

fn prefix_term(token: &str) -> String {
    format!("{}*", quoted(token))
}

fn phrase_term(tokens: &[String]) -> String {
    quoted(&tokens.join(" "))
}

/// A FTS5 string: the text in double quotes, with embedded quotes doubled.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::{build_fts_query, build_trigram_queries, trigram_similarity};
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
    fn trigram_queries_quote_every_token_as_a_substring() {
        let query = build_trigram_queries("index retry").expect("query");
        assert_eq!(query.substring, "\"index\" AND \"retry\"");
        assert_eq!(query.tokens, vec!["index", "retry"]);
    }

    #[test]
    fn trigram_queries_group_the_trigrams_of_each_word_for_fuzzy_matching() {
        let query = build_trigram_queries("bakcoff").expect("query");
        assert_eq!(
            query.fuzzy,
            "(\"bak\" OR \"akc\" OR \"kco\" OR \"cof\" OR \"off\")"
        );
        let two = build_trigram_queries("retry backoff").expect("query");
        assert_eq!(
            two.fuzzy,
            "(\"ret\" OR \"etr\" OR \"try\") AND (\"bac\" OR \"ack\" OR \"cko\" OR \"kof\" OR \"off\")"
        );
    }

    #[test]
    fn short_words_leave_the_trigram_queries_and_the_fuzzy_one_needs_four_characters() {
        let query = build_trigram_queries("ab index fidx").expect("query");
        assert_eq!(query.substring, "\"index\" AND \"fidx\"");
        assert_eq!(query.tokens, vec!["index", "fidx"]);
        assert!(query.fuzzy.contains("\"fid\""));
        let three = build_trigram_queries("abc").expect("query");
        assert_eq!(three.substring, "\"abc\"");
        assert!(three.fuzzy.is_empty());
    }

    #[test]
    fn no_token_of_three_characters_means_no_trigram_query() {
        assert_eq!(build_trigram_queries("ab cd"), None);
        assert_eq!(build_trigram_queries("\"ab\""), None);
        assert_eq!(build_trigram_queries("*:()"), None);
        assert_eq!(build_trigram_queries(""), None);
    }

    #[test]
    fn a_phrase_is_a_substring_token_but_never_fuzzy() {
        let query = build_trigram_queries("\"Back Off\" retry").expect("query");
        assert_eq!(query.substring, "\"back off\" AND \"retry\"");
        assert_eq!(query.tokens, vec!["back off", "retry"]);
        assert!(!query.fuzzy.contains("back"));
        assert!(query.fuzzy.contains("\"ret\""));
    }

    #[test]
    fn trigram_queries_are_lowercase_and_case_insensitive_in_the_tokens() {
        let query = build_trigram_queries("HistoryStream").expect("query");
        assert_eq!(query.substring, "\"historystream\"");
        assert_eq!(query.tokens, vec!["historystream"]);
    }

    #[test]
    fn similarity_of_a_word_with_itself_is_one_and_of_unrelated_words_is_low() {
        assert!((trigram_similarity("backoff", "backoff") - 1.0).abs() < f32::EPSILON);
        assert!((trigram_similarity("BackOff", "backoff") - 1.0).abs() < f32::EPSILON);
        assert!(trigram_similarity("backoff", "parser") < 0.2);
        assert!(trigram_similarity("", "backoff").abs() < f32::EPSILON);
    }

    #[test]
    fn a_transposed_typo_scores_exactly_the_threshold() {
        assert!((trigram_similarity("bakcoff", "backoff") - 0.5).abs() < f32::EPSILON);
        assert!(trigram_similarity("backof", "backoff") >= 0.5);
        assert!(trigram_similarity("fidx", "file_index") < 0.5);
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
    /// `AND`, `OR`, parentheses appear, and a `*` appears only right after a closing quote.
    fn assert_fts_safe(query: &str) {
        let mut in_quote = false;
        let mut prev = ' ';
        for ch in query.chars() {
            if ch == '"' {
                in_quote = !in_quote;
            } else if !in_quote {
                assert!(
                    !matches!(ch, ':' | '^' | '-' | '+' | '/' | '\\'),
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

    /// Checks the trigram output: outside quotes only `AND`, `OR`, parentheses and
    /// spaces appear, parentheses balance, and every quoted term is non-empty.
    fn assert_trigram_safe(query: &str) {
        let mut in_quote = false;
        let mut depth: i32 = 0;
        let mut outside = String::new();
        let mut quote_len = 0;
        for ch in query.chars() {
            if ch == '"' {
                if in_quote {
                    assert!(quote_len > 0, "empty term in {query:?}");
                }
                in_quote = !in_quote;
                quote_len = 0;
                outside.push(' ');
            } else if in_quote {
                quote_len += 1;
            } else {
                match ch {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ' ' | 'A' | 'N' | 'D' | 'O' | 'R' => {}
                    other => panic!("bare {other:?} in {query:?}"),
                }
                assert!(depth >= 0, "unbalanced parentheses in {query:?}");
                outside.push(ch);
            }
        }
        assert!(!in_quote, "unbalanced quote in {query:?}");
        assert_eq!(depth, 0, "unbalanced parentheses in {query:?}");
        for word in outside.split_whitespace() {
            assert!(
                matches!(word, "AND" | "OR" | "(" | ")")
                    || word.chars().all(|c| matches!(c, '(' | ')')),
                "bare word {word:?} in {query:?}"
            );
        }
    }

    #[test]
    fn trigram_queries_stay_inside_quotes_for_hostile_input() {
        for input in [
            "\"",
            "\"\"\"",
            "a\"b\"c",
            "OR AND NOT ( ) \" * :",
            "xxxx) OR (yyyy",
            "\"unterminated phrase here",
        ] {
            if let Some(query) = build_trigram_queries(input) {
                assert_trigram_safe(&query.substring);
                assert_trigram_safe(&query.fuzzy);
            }
        }
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

        #[test]
        fn trigram_output_is_only_quoted_terms_and_connectives(input in "\\PC{0,80}") {
            if let Some(query) = build_trigram_queries(&input) {
                assert_trigram_safe(&query.substring);
                assert_trigram_safe(&query.fuzzy);
                prop_assert!(!query.substring.is_empty());
            }
        }

        #[test]
        fn trigram_output_for_operator_heavy_input_is_safe(input in "[a-zA-Z \"*:()^_-]{0,60}") {
            if let Some(query) = build_trigram_queries(&input) {
                assert_trigram_safe(&query.substring);
                assert_trigram_safe(&query.fuzzy);
            }
        }

        #[test]
        fn similarity_is_symmetric_and_bounded(a in "[a-z_]{0,12}", b in "[a-z_]{0,12}") {
            let forward = trigram_similarity(&a, &b);
            prop_assert!((0.0..=1.0).contains(&forward));
            prop_assert!((forward - trigram_similarity(&b, &a)).abs() < 1e-6);
        }
    }
}
