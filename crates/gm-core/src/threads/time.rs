//! Calendar arithmetic shared by the thread adapters and the search filters: RFC 3339
//! timestamps and proleptic Gregorian dates, as milliseconds since the Unix epoch.
//! Pure: no I/O, no clock.

/// `YYYY-MM-DD` then `T` (or `t` or a space), `hh:mm:ss`, an optional fraction of which the
/// first three digits count, then `Z` or `+hh:mm` or `-hh:mm`. No zone is not accepted.
pub(crate) fn parse_rfc3339(text: &str) -> Option<i64> {
    let year = num(text.get(0..4)?)?;
    separator(text, 4, "-")?;
    let month = num(text.get(5..7)?)?;
    separator(text, 7, "-")?;
    let day = num(text.get(8..10)?)?;
    if !matches!(text.get(10..11)?, "T" | "t" | " ") {
        return None;
    }
    let hour = num(text.get(11..13)?)?;
    separator(text, 13, ":")?;
    let minute = num(text.get(14..16)?)?;
    separator(text, 16, ":")?;
    let second = num(text.get(17..19)?)?;
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let mut rest = text.get(19..)?;
    let mut millis = 0_i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let mut scale = 100_i64;
        for digit in fraction.bytes().take(digits.min(3)) {
            millis += i64::from(digit - b'0') * scale;
            scale /= 10;
        }
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => offset_seconds(rest)?,
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second - offset;
    Some(seconds * 1_000 + millis)
}

fn offset_seconds(text: &str) -> Option<i64> {
    if text.len() != 6 {
        return None;
    }
    let sign = match text.get(0..1)? {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let hours = num(text.get(1..3)?)?;
    separator(text, 3, ":")?;
    let minutes = num(text.get(4..6)?)?;
    Some(sign * (hours * 3_600 + minutes * 60))
}

fn separator(text: &str, at: usize, want: &str) -> Option<()> {
    (text.get(at..at + want.len())? == want).then_some(())
}

fn num(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

pub(crate) fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
