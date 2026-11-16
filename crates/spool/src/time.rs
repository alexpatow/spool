//! Timestamps as text. The store keeps instants; what leaves the engine is
//! RFC 3339 in UTC, which every client can parse.

/// Microseconds since the Unix epoch as `2026-01-01T00:00:00.000000Z`.
pub(crate) fn rfc3339(micros: i64) -> String {
    let seconds = micros.div_euclid(1_000_000);
    let fraction = micros.rem_euclid(1_000_000);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);

    // Days since 1970-01-01 to a civil date, after Howard Hinnant's
    // `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{fraction:06}Z",
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    #[test]
    fn instants_are_written_in_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000000Z");
        assert_eq!(
            rfc3339(1_767_225_600_000_000),
            "2026-01-01T00:00:00.000000Z"
        );
        assert_eq!(
            rfc3339(1_709_251_199_999_999),
            "2024-02-29T23:59:59.999999Z"
        );
        assert_eq!(rfc3339(951_782_400_000_000), "2000-02-29T00:00:00.000000Z");
        assert_eq!(rfc3339(-1), "1969-12-31T23:59:59.999999Z");
    }
}
