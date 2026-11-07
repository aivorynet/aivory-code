//! RFC 3339 time stamps for the `updated` key, as Unix milliseconds.

use std::time::{SystemTime, UNIX_EPOCH};

use toml::value::{Datetime, Offset};

/// Parse a full RFC 3339 date-time. The date, the time, and the offset are all required.
pub(crate) fn parse_ms(text: &str) -> Option<i64> {
    let dt: Datetime = text.trim().parse().ok()?;
    let (date, time, offset) = (dt.date?, dt.time?, dt.offset?);
    let offset_min = match offset {
        Offset::Z => 0,
        Offset::Custom { minutes } => i64::from(minutes),
    };
    let days = days_from_civil(
        i64::from(date.year),
        i64::from(date.month),
        i64::from(date.day),
    );
    let secs = days * 86_400
        + i64::from(time.hour) * 3600
        + i64::from(time.minute) * 60
        + i64::from(time.second.unwrap_or(0))
        - offset_min * 60;
    Some(secs * 1000 + i64::from(time.nanosecond.unwrap_or(0) / 1_000_000))
}

/// Format Unix milliseconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn format_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of `days_from_civil`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let d = day_of_year - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = year_of_era + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants() {
        assert_eq!(parse_ms("1970-01-01T00:00:00Z"), Some(0));
        // The design's demo date: 2026-10-21 14:12 UTC.
        assert_eq!(parse_ms("2026-10-21T14:12:00Z"), Some(1_792_591_920_000));
        assert_eq!(format_ms(1_792_591_920_000), "2026-10-21T14:12:00Z");
    }

    #[test]
    fn offsets_and_fractions() {
        assert_eq!(
            parse_ms("2026-10-21T16:12:00+02:00"),
            parse_ms("2026-10-21T14:12:00Z")
        );
        assert_eq!(parse_ms("1970-01-01T00:00:01.250Z"), Some(1250));
    }

    #[test]
    fn rejects_partial_stamps() {
        assert_eq!(parse_ms("2026-10-21"), None);
        assert_eq!(parse_ms("2026-10-21T14:12:00"), None);
        assert_eq!(parse_ms("yesterday"), None);
    }

    #[test]
    fn round_trip_over_many_days() {
        for day in (-700_000..2_900_000).step_by(997) {
            let ms = day * 86_400_000 + 3_723_000;
            assert_eq!(parse_ms(&format_ms(ms)), Some(ms), "day {day}");
        }
    }
}
