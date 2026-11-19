// A run file's save time as the menu shows it.
// See: context/lib/scripting.md §5 (Durable State Store)

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock UTC seconds since the Unix epoch; 0 if the clock reads before it.
pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `YYYY-MM-DD HH:MM UTC` for UTC seconds since the Unix epoch. UTC because
/// a local zone needs a timezone database the engine does not carry.
pub fn format_saved_at(unix_seconds: u64) -> String {
    let days = unix_seconds / 86_400;
    let seconds_of_day = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds_of_day / 3_600,
        seconds_of_day % 3_600 / 60
    )
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`, restricted to non-negative day counts).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::format_saved_at;

    #[test]
    fn saved_at_formats_utc_calendar_minutes() {
        assert_eq!(format_saved_at(0), "1970-01-01 00:00 UTC");
        // 2000-02-29 is a leap day in a century leap year.
        assert_eq!(format_saved_at(951_782_400 + 3_661), "2000-02-29 01:01 UTC");
        assert_eq!(format_saved_at(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(format_saved_at(4_102_444_799), "2099-12-31 23:59 UTC");
    }
}
