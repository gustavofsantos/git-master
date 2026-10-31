//! The time forms of the thread search filters `since` and `until` (D147). `parse_when`
//! reads one text and gives milliseconds since the Unix epoch. It is pure: the caller passes
//! `now`, so the answer does not depend on the clock.

use super::time::{days_from_civil, days_in_month, parse_rfc3339};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;

/// The forms `parse_when` accepts, listed in every error message.
const FORMS: &str = "an ISO date (2026-10-31, UTC midnight), an ISO timestamp with an offset \
    or Z (2026-10-31T09:30:00Z or 2026-10-31T06:30:00-03:00), a relative span before now \
    (90m, 36h, 7d or 2w), or epoch milliseconds (1793404800000)";

/// Parses a time from a filter. Accepted forms: an ISO date `YYYY-MM-DD` (UTC midnight), an
/// ISO timestamp with `Z` or an offset, a relative span `<digits><m|h|d|w>` meaning `now`
/// minus the span, or an integer of epoch milliseconds (a leading `-` is allowed). Surrounding
/// spaces are ignored. Any other text is an error whose message lists the forms.
pub fn parse_when(text: &str, now_ms: i64) -> Result<i64, String> {
    let trimmed = text.trim();
    epoch_ms(trimmed)
        .or_else(|| relative(trimmed, now_ms))
        .or_else(|| date(trimmed))
        .or_else(|| parse_rfc3339(trimmed))
        .ok_or_else(|| format!("{text:?} is not a time. Use {FORMS}."))
}

/// An integer of epoch milliseconds, with an optional minus sign and nothing else.
fn epoch_ms(text: &str) -> Option<i64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !is_digits(digits) {
        return None;
    }
    text.parse().ok()
}

/// `<digits>` then one unit: `m` minutes, `h` hours, `d` days or `w` weeks, before `now`.
fn relative(text: &str, now_ms: i64) -> Option<i64> {
    let unit = match text.bytes().last()? {
        b'm' => MINUTE,
        b'h' => HOUR,
        b'd' => DAY,
        b'w' => WEEK,
        _ => return None,
    };
    let count = &text[..text.len() - 1];
    if !is_digits(count) {
        return None;
    }
    let span = count.parse::<i64>().ok()?.checked_mul(unit)?;
    now_ms.checked_sub(span)
}

/// `YYYY-MM-DD`, a real calendar day, at UTC midnight.
fn date(text: &str) -> Option<i64> {
    let mut parts = text.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let year = number(year)?;
    let month = number(month)?;
    let day = number(day)?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some(days_from_civil(year, month, day) * DAY)
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn number(text: &str) -> Option<i64> {
    is_digits(text).then(|| text.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// 2026-10-31T00:00:00Z, the ISO date and the epoch value the brief's examples use.
    const OCT_31: i64 = 1_793_404_800_000;
    const NOW: i64 = 1_800_000_000_000;

    fn ok(text: &str) -> i64 {
        parse_when(text, NOW).unwrap_or_else(|error| panic!("{text}: {error}"))
    }

    fn rejected(text: &str) -> String {
        match parse_when(text, NOW) {
            Ok(value) => panic!("{text:?} should be refused, got {value}"),
            Err(error) => error,
        }
    }

    #[test]
    fn an_iso_date_is_utc_midnight() {
        assert_eq!(ok("2026-10-31"), OCT_31);
        assert_eq!(ok(" 2026-10-31 "), OCT_31);
        assert_eq!(ok("1970-01-01"), 0);
        assert_eq!(ok("1969-12-31"), -DAY);
    }

    #[test]
    fn an_iso_timestamp_with_z_or_an_offset_is_the_same_instant() {
        assert_eq!(ok("2026-10-31T00:00:00Z"), OCT_31);
        assert_eq!(ok("2026-10-31T00:00:00z"), OCT_31);
        assert_eq!(ok("2026-10-31T02:00:00+02:00"), OCT_31);
        assert_eq!(ok("2026-10-30T22:00:00-02:00"), OCT_31);
        assert_eq!(ok("2026-10-31T00:00:00.250Z"), OCT_31 + 250);
        assert_eq!(ok("2026-10-31T00:00:00.5Z"), OCT_31 + 500);
    }

    #[test]
    fn a_timestamp_without_a_zone_is_refused() {
        rejected("2026-10-31T09:30:00");
        rejected("2026-10-31T09:30");
    }

    #[test]
    fn relative_spans_count_back_from_now() {
        assert_eq!(ok("90m"), NOW - 90 * MINUTE);
        assert_eq!(ok("36h"), NOW - 36 * HOUR);
        assert_eq!(ok("7d"), NOW - 7 * DAY);
        assert_eq!(ok("2w"), NOW - 14 * DAY);
        assert_eq!(ok("0d"), NOW);
    }

    #[test]
    fn epoch_milliseconds_are_taken_as_they_are() {
        assert_eq!(ok("1793404800000"), OCT_31);
        assert_eq!(ok("0"), 0);
        assert_eq!(ok("-1"), -1);
    }

    #[test]
    fn leap_days_follow_the_calendar() {
        assert_eq!(ok("2024-02-29"), days_from_civil(2024, 2, 29) * DAY);
        assert_eq!(ok("2000-02-29"), days_from_civil(2000, 2, 29) * DAY);
        rejected("2025-02-29");
        rejected("2026-02-29");
        rejected("1900-02-29");
    }

    #[test]
    fn impossible_dates_are_refused() {
        for text in [
            "2026-13-01",
            "2026-00-10",
            "2026-04-31",
            "2026-10-00",
            "2026-10-32",
            "2026-1-1",
            "26-10-31",
            "2026/10/31",
            "2026-10-31-",
            "-2026-10-31",
        ] {
            rejected(text);
        }
    }

    #[test]
    fn bad_relative_spans_and_other_text_are_refused() {
        for text in [
            "",
            " ",
            "7D",
            "-7d",
            "7x",
            "d",
            "m",
            "7 d",
            "+5",
            "1.5d",
            "abc",
            "7d ago",
            "99999999999999999999d",
            "9223372036854775807w",
        ] {
            rejected(text);
        }
    }

    #[test]
    fn a_bare_integer_is_epoch_milliseconds_not_a_span() {
        assert_eq!(ok("7"), 7);
    }

    #[test]
    fn the_error_lists_the_accepted_forms() {
        let error = rejected("last tuesday");
        for form in [
            "ISO date",
            "ISO timestamp",
            "90m",
            "7d",
            "epoch milliseconds",
        ] {
            assert!(error.contains(form), "{error}");
        }
        assert!(error.contains("last tuesday"), "{error}");
    }

    #[test]
    fn consecutive_days_are_one_day_apart() {
        let mut previous = ok("2023-12-31");
        let (mut year, mut month, mut dom) = (2024, 1, 1);
        for _ in 0..(366 + 365 + 365 + 365) {
            let text = format!("{year:04}-{month:02}-{dom:02}");
            let value = ok(&text);
            assert_eq!(value, previous + DAY, "{text}");
            previous = value;
            if dom < days_in_month(year, month) {
                dom += 1;
            } else {
                dom = 1;
                if month == 12 {
                    month = 1;
                    year += 1;
                } else {
                    month += 1;
                }
            }
        }
        assert_eq!(previous, days_from_civil(2027, 12, 31) * DAY);
    }

    proptest! {
        #[test]
        fn relative_spans_subtract_exactly(count in 0_i64..1_000_000, unit in 0_usize..4, now in -1_000_000_000_000_000_i64..1_000_000_000_000_000) {
            let (letter, span) = [('m', MINUTE), ('h', HOUR), ('d', DAY), ('w', WEEK)][unit];
            let text = format!("{count}{letter}");
            prop_assert_eq!(parse_when(&text, now), Ok(now - count * span));
        }

        #[test]
        fn epoch_integers_round_trip(value in any::<i64>()) {
            prop_assert_eq!(parse_when(&value.to_string(), NOW), Ok(value));
        }

        #[test]
        fn any_text_gives_a_value_or_an_error_and_never_panics(text in ".{0,40}") {
            let _ = parse_when(&text, NOW);
        }

        #[test]
        fn utc_and_offset_forms_agree(hours in 0_i64..24, minutes in 0_i64..60, sign in any::<bool>(), oh in 0_i64..24, om in 0_i64..60) {
            let zone = format!("{}{oh:02}:{om:02}", if sign { '+' } else { '-' });
            let local = format!("2026-10-31T{hours:02}:{minutes:02}:00{zone}");
            let utc = format!("2026-10-31T{hours:02}:{minutes:02}:00Z");
            let offset = (oh * 60 + om) * MINUTE * if sign { 1 } else { -1 };
            prop_assert_eq!(ok(&local), ok(&utc) - offset);
        }
    }
}
