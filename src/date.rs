//! ISO 8601 timestamps with the exact semantics of `fs.Stats.mtime.toISOString()` in Node.js.

use std::fs::Metadata;

/// JavaScript's `Math.round`: nearest integer, ties towards positive infinity.
pub fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// Node computes `mtimeMs` as `sec * 1e3 + nsec / 1e6` in doubles, then rounds it into a `Date`.
pub fn js_millis(seconds: i64, nanos: i64) -> i64 {
    js_round(seconds as f64 * 1e3 + nanos as f64 / 1e6) as i64
}

/// The modification time of `metadata` in milliseconds since the epoch, as Node sees it.
pub fn mtime_millis(metadata: &Metadata) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        js_millis(metadata.mtime(), metadata.mtime_nsec())
    }
    #[cfg(not(unix))]
    {
        use std::time::UNIX_EPOCH;
        let Ok(modified) = metadata.modified() else { return 0 };
        let total_nanos: i128 = match modified.duration_since(UNIX_EPOCH) {
            Ok(after) => after.as_nanos() as i128,
            Err(before) => -(before.duration().as_nanos() as i128),
        };
        let seconds = total_nanos.div_euclid(1_000_000_000) as i64;
        let nanos = total_nanos.rem_euclid(1_000_000_000) as i64;
        js_millis(seconds, nanos)
    }
}

/// Converts days since 1970-01-01 to a (year, month, day) civil date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `Date.prototype.toISOString()` for a millisecond timestamp.
pub fn iso_from_millis(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let millis_of_day = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hours = millis_of_day / 3_600_000;
    let minutes = millis_of_day / 60_000 % 60;
    let seconds = millis_of_day / 1000 % 60;
    let millis_part = millis_of_day % 1000;
    let year_text = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:06}", -year)
    } else {
        format!("+{year:06}")
    };
    format!("{year_text}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis_part:03}Z")
}

/// The ISO 8601 UTC modification time sent as `fileCreatedAt` and `fileModifiedAt`.
pub fn mtime_iso(metadata: &Metadata) -> String {
    iso_from_millis(mtime_millis(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_and_known_dates() {
        assert_eq!(iso_from_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_millis(1_704_067_200_123), "2024-01-01T00:00:00.123Z");
        assert_eq!(iso_from_millis(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso_from_millis(1_735_689_599_999), "2024-12-31T23:59:59.999Z");
    }

    #[test]
    fn formats_dates_before_epoch_and_extended_years() {
        assert_eq!(iso_from_millis(-876), "1969-12-31T23:59:59.124Z");
        assert_eq!(iso_from_millis(253_402_300_800_000), "+010000-01-01T00:00:00.000Z");
        assert_eq!(iso_from_millis(-62_198_755_200_000), "-000001-01-01T00:00:00.000Z");
    }

    #[test]
    fn rounds_sub_millisecond_times_like_node() {
        // Values observed from Node 22 for files touched with these nanosecond timestamps.
        assert_eq!(js_millis(1_704_067_200, 123_400_000), 1_704_067_200_123);
        assert_eq!(js_millis(1_704_067_200, 123_500_000), 1_704_067_200_124);
        assert_eq!(js_millis(1_704_067_200, 999_999_999), 1_704_067_201_000);
        assert_eq!(js_millis(-1, 123_600_000), -876);
    }

    #[test]
    fn js_round_ties_go_up() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(-2.6), -3.0);
    }
}
