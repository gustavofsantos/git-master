// Copy of crates/gm-ui/src/fuzzy.rs (the matcher the palette uses today), verbatim.
// The baseline for the path matcher.
#![allow(dead_code)]
//! Fuzzy matching for the command palette (slice 1c, item 4). Pure: no GPUI,
//! so the ranking and the highlight ranges are unit-tested on their own.
//!
//! A query matches a text when its characters appear in the text in order,
//! ignoring case. A run of the query found whole ranks above letters scattered
//! through the text, and a run that starts a word ranks above one in the middle
//! of a word.

use std::ops::Range;

/// How well a query matches a text, and which parts of the text to highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Higher ranks first. Only comparable for one query.
    pub score: i64,
    /// Byte ranges of the text that matched, in order. Adjacent characters
    /// share one range.
    pub ranges: Vec<Range<usize>>,
}

/// The score of a whole run of the query. Scattered matches stay below it.
const WHOLE_RUN: i64 = 10_000;
/// The most a scattered match can score, so it never ranks above a whole run.
const SCATTERED_CAP: i64 = 5_000;

/// One character of the text: its byte range, its lowercase form and whether it
/// starts a word.
struct Character {
    start: usize,
    end: usize,
    lower: char,
    word_start: bool,
}

/// How well `query` matches `text`, or `None` when it does not. An empty query
/// matches everything, with no highlight.
#[must_use]
pub fn fuzzy_match(query: &str, text: &str) -> Option<Match> {
    let needle: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Some(Match {
            score: 0,
            ranges: Vec::new(),
        });
    }
    let characters = characters(text);
    let whole = characters.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(&needle)
            .all(|(character, wanted)| character.lower == *wanted)
    });
    if let Some(at) = whole {
        let first = &characters[at];
        let last = &characters[at + needle.len() - 1];
        let bonus = if at == 0 {
            200
        } else if first.word_start {
            100
        } else {
            0
        };
        let position = i64::try_from(at).unwrap_or(i64::MAX);
        return Some(Match {
            score: WHOLE_RUN + bonus - position,
            ranges: std::iter::once(first.start..last.end).collect(),
        });
    }
    scattered(&characters, &needle)
}

/// Each letter of the query at its first place after the one before. Letters
/// next to each other, at a word start and close to the start score more.
fn scattered(characters: &[Character], needle: &[char]) -> Option<Match> {
    let mut picked = Vec::with_capacity(needle.len());
    let mut from = 0;
    for &wanted in needle {
        let at = (from..characters.len()).find(|&i| characters[i].lower == wanted)?;
        picked.push(at);
        from = at + 1;
    }

    let mut score: i64 = 0;
    let mut previous: Option<usize> = None;
    for &at in &picked {
        score += 10;
        let skipped = at - previous.map_or(0, |before| before + 1);
        if skipped == 0 && previous.is_some() {
            score += 15;
        } else {
            score = score.saturating_sub(i64::try_from(skipped).unwrap_or(i64::MAX));
        }
        if characters[at].word_start {
            score += 20;
        }
        previous = Some(at);
    }

    let mut ranges: Vec<Range<usize>> = Vec::new();
    for &at in &picked {
        let character = &characters[at];
        match ranges.last_mut() {
            Some(last) if last.end == character.start => last.end = character.end,
            _ => ranges.push(character.start..character.end),
        }
    }
    Some(Match {
        score: score.min(SCATTERED_CAP),
        ranges,
    })
}

fn characters(text: &str) -> Vec<Character> {
    let mut previous: Option<char> = None;
    text.char_indices()
        .map(|(start, character)| {
            let word_start = match previous {
                None => true,
                Some(before) => {
                    !before.is_alphanumeric() || (before.is_lowercase() && character.is_uppercase())
                }
            };
            previous = Some(character);
            Character {
                start,
                end: start + character.len_utf8(),
                lower: character.to_lowercase().next().unwrap_or(character),
                word_start,
            }
        })
        .collect()
}

