use chrono::{DateTime, Duration, Months, NaiveDate, Timelike, Utc};
use regex::Regex;
use std::sync::LazyLock;

pub(super) fn parse_listing_date(text: &str, observed_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    // Match yt-dlp's flat-playlist approximate_date labels, including WEB's "15y ago".
    static RELATIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?i)\b(today|yesterday|now)\b|\b(\d+)\s*",
            r"(second|sec|minute|min|hour|hr|day|week|wk|month|mo|year|yr|s|h|d|w|y)s?\s*ago\b",
        ))
        .unwrap()
    });
    static ABSOLUTE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b([a-z]+\s+\d{1,2},?\s+\d{4}|\d{4}-\d{2}-\d{2})\b").unwrap()
    });
    let now = observed_at;
    if let Some(parts) = RELATIVE.captures(text) {
        if let Some(word) = parts.get(1) {
            return if word.as_str().eq_ignore_ascii_case("yesterday") {
                now.checked_sub_signed(Duration::days(1))
            } else {
                Some(now)
            }
            .and_then(|date| date.with_nanosecond(0));
        }
        let count: u32 = parts[2].parse().ok()?;
        let (date, precision) = match parts[3].to_ascii_lowercase().as_str() {
            "second" | "sec" | "s" => (now.checked_sub_signed(Duration::seconds(count.into()))?, 1),
            "minute" | "min" => (now.checked_sub_signed(Duration::minutes(count.into()))?, 60),
            "hour" | "hr" | "h" => (now.checked_sub_signed(Duration::hours(count.into()))?, 3600),
            "day" | "d" => (now.checked_sub_signed(Duration::days(count.into()))?, 86400),
            "week" | "wk" | "w" => (
                now.checked_sub_signed(Duration::weeks(count.into()))?,
                86400,
            ),
            "month" | "mo" => (now.checked_sub_months(Months::new(count))?, 86400),
            "year" | "yr" | "y" => (
                now.checked_sub_months(Months::new(count.checked_mul(12)?))?,
                86400,
            ),
            _ => return None,
        };
        // yt-dlp rounds to the nearest unit, including rounding afternoons to the next day.
        let rounded = if precision == 1 {
            date.timestamp() + i64::from(date.timestamp_subsec_nanos() >= 500_000_000)
        } else {
            (date.timestamp() + precision / 2).div_euclid(precision) * precision
        };
        return DateTime::from_timestamp(rounded, 0);
    }
    if let Some(date) = super::parse_publication_date(text) {
        return Some(date);
    }
    let label = ABSOLUTE.find(text)?.as_str().replace(',', "");
    ["%B %d %Y", "%b %d %Y", "%Y-%m-%d"]
        .iter()
        .find_map(|format| {
            NaiveDate::parse_from_str(&label, format)
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_playlist_labels_match_yt_dlp_precision() {
        let now = super::super::parse_publication_date("2026-09-24T16:35:42Z").unwrap();
        for (text, expected) in [
            ("15y ago", "2011-09-25T00:00:00+00:00"),
            ("50K views • 10 years ago", "2016-09-25T00:00:00+00:00"),
            ("Streamed 2 months ago", "2026-07-25T00:00:00+00:00"),
            ("1mo ago", "2026-08-25T00:00:00+00:00"),
            ("2wk ago", "2026-09-11T00:00:00+00:00"),
            ("Premiered 6 days ago", "2026-09-19T00:00:00+00:00"),
            ("3hrs ago", "2026-09-24T14:00:00+00:00"),
            ("20min ago", "2026-09-24T16:16:00+00:00"),
            ("5 seconds ago (edited)", "2026-09-24T16:35:37+00:00"),
            ("Yesterday", "2026-09-23T16:35:42+00:00"),
            ("today", "2026-09-24T16:35:42+00:00"),
            ("just now", "2026-09-24T16:35:42+00:00"),
            ("Premiered on Feb 22, 2011", "2011-02-22T00:00:00+00:00"),
            (
                "Streamed live on January 11, 2011",
                "2011-01-11T00:00:00+00:00",
            ),
            ("2011-01-11T05:41:25-08:00", "2011-01-11T13:41:25+00:00"),
            ("2011-01-11", "2011-01-11T00:00:00+00:00"),
        ] {
            assert_eq!(
                parse_listing_date(text, now).unwrap().to_rfc3339(),
                expected,
                "{text}"
            );
        }
        for text in [
            "",
            "N/A",
            "50K views",
            "unknown",
            "in 2 days",
            "15y",
            "2011-02-30",
            "9999999999999 years ago",
            "4294967295 years ago",
        ] {
            assert!(parse_listing_date(text, now).is_none(), "{text}");
        }
    }

    #[test]
    fn relative_rounding_uses_unit_midpoints_and_today_truncates_subseconds() {
        for (now, text, expected) in [
            (
                "2026-09-24T11:59:59Z",
                "1 day ago",
                "2026-09-23T00:00:00+00:00",
            ),
            (
                "2026-09-24T12:00:00Z",
                "1 day ago",
                "2026-09-24T00:00:00+00:00",
            ),
            (
                "2026-09-24T16:35:42.5Z",
                "1 second ago",
                "2026-09-24T16:35:42+00:00",
            ),
            (
                "2026-09-24T16:35:42.5Z",
                "today",
                "2026-09-24T16:35:42+00:00",
            ),
        ] {
            let now = super::super::parse_publication_date(now).unwrap();
            assert_eq!(
                parse_listing_date(text, now).unwrap().to_rfc3339(),
                expected
            );
        }
    }

    #[test]
    fn month_and_year_estimates_clamp_to_calendar_month_end() {
        for (now, text, expected) in [
            ("2024-03-31", "1 month ago", "2024-02-29"),
            ("2024-02-29", "1 year ago", "2023-02-28"),
        ] {
            let now = super::super::parse_publication_date(now).unwrap();
            assert_eq!(
                parse_listing_date(text, now)
                    .unwrap()
                    .date_naive()
                    .to_string(),
                expected
            );
        }
    }
}
