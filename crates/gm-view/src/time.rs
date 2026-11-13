//! How long ago a Unix time was, as the lists show it in English.

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// How long ago `then` was, as the list shows it in English. A time in the
/// future (clock skew) reads as "just now".
#[must_use]
pub fn relative_time(now: i64, then: i64) -> String {
    let age = (now - then).max(0);
    if age < MINUTE {
        "just now".to_owned()
    } else if age < HOUR {
        format!("{} min ago", age / MINUTE)
    } else if age < DAY {
        format!("{} h ago", age / HOUR)
    } else if age < 2 * DAY {
        "yesterday".to_owned()
    } else if age < WEEK {
        ago(age / DAY, "day")
    } else if age < MONTH {
        ago(age / WEEK, "week")
    } else if age < YEAR {
        ago(age / MONTH, "month")
    } else {
        ago(age / YEAR, "year")
    }
}

fn ago(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_times_are_in_minutes_and_hours() {
        let now = 1_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now - 59), "just now");
        assert_eq!(relative_time(now, now - 5 * MINUTE), "5 min ago");
        assert_eq!(relative_time(now, now - 3 * HOUR), "3 h ago");
    }

    #[test]
    fn a_day_is_yesterday_and_then_counted() {
        let now = 1_000_000;
        assert_eq!(relative_time(now, now - DAY - HOUR), "yesterday");
        assert_eq!(relative_time(now, now - 2 * DAY), "2 days ago");
        assert_eq!(relative_time(now, now - 6 * DAY), "6 days ago");
    }

    #[test]
    fn weeks_months_and_years_are_singular_at_one() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now, now - WEEK), "1 week ago");
        assert_eq!(relative_time(now, now - 3 * WEEK), "3 weeks ago");
        assert_eq!(relative_time(now, now - MONTH), "1 month ago");
        assert_eq!(relative_time(now, now - 2 * YEAR), "2 years ago");
    }

    #[test]
    fn a_time_in_the_future_reads_as_just_now() {
        assert_eq!(relative_time(100, 500), "just now");
    }
}
