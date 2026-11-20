//! Timestamps as text. The store keeps instants; what leaves the engine is
//! RFC 3339 in UTC, which every client can parse.

use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};

/// Microseconds since the Unix epoch as `2026-01-01T00:00:00.000000Z`.
///
/// An instant outside the years the `time` crate writes, -9999 to 9999, is
/// written as the nearest it can. The store's instants come from a clock, so
/// none is.
pub(crate) fn format_rfc3339(micros: i64) -> String {
    const FORM: &[BorrowedFormatItem<'_>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");
    let instant = OffsetDateTime::from_unix_timestamp_nanos(i128::from(micros) * 1_000).unwrap_or(
        if micros < 0 {
            PrimitiveDateTime::MIN.assume_utc()
        } else {
            PrimitiveDateTime::MAX.assume_utc()
        },
    );
    instant
        .format(FORM)
        .expect("an instant in UTC has every part of the form")
}

#[cfg(test)]
mod tests {
    use super::format_rfc3339;

    #[test]
    fn instants_are_written_in_utc() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00.000000Z");
        assert_eq!(
            format_rfc3339(1_767_225_600_000_000),
            "2026-01-01T00:00:00.000000Z"
        );
        assert_eq!(
            format_rfc3339(1_709_251_199_999_999),
            "2024-02-29T23:59:59.999999Z"
        );
        assert_eq!(
            format_rfc3339(951_782_400_000_000),
            "2000-02-29T00:00:00.000000Z"
        );
        assert_eq!(format_rfc3339(-1), "1969-12-31T23:59:59.999999Z");
    }

    #[test]
    fn an_instant_too_far_to_write_is_written_as_the_nearest() {
        assert_eq!(format_rfc3339(i64::MAX), "9999-12-31T23:59:59.999999Z");
        assert_eq!(format_rfc3339(i64::MIN), "-9999-01-01T00:00:00.000000Z");
    }
}
