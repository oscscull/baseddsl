//! Timestamp and date values at the bind boundary: parsed once, then carried in one
//! canonical form. The canonical timestamp is UTC with six fractional digits
//! (`2026-10-21T08:00:00.000000Z`) and the canonical date is `YYYY-MM-DD`, so values
//! stored as text (SQLite) compare and sort in time order.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

/// The canonical timestamp layout: fixed-width, UTC, lexically ordered.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6fZ";

/// The canonical date layout.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// Parse a wire timestamp into UTC. Accepts RFC 3339, the Postgres text form
/// (`2024-01-02 12:30:45.500000+00`), and a naive datetime (space or `T` separated,
/// taken as UTC). Years outside `0000..=9999` are rejected: they have no fixed-width
/// text form.
pub(crate) fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    let parsed = parse_timestamp_any(s.trim())?;
    in_four_digit_years(parsed.date_naive()).then_some(parsed)
}

fn parse_timestamp_any(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%dT%H:%M:%S%.f%#z"] {
        if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
            return Some(dt.with_timezone(&Utc));
        }
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(n.and_utc());
        }
    }
    None
}

/// A wire timestamp in its canonical form, or `None` when it does not parse.
pub(crate) fn canonical_timestamp(s: &str) -> Option<String> {
    parse_timestamp(s).map(|dt| dt.format(TIMESTAMP_FORMAT).to_string())
}

/// A timestamp as a zone-less UTC datetime (`2026-10-21 08:00:00.000000`), the form a
/// MariaDB `DATETIME` takes. An unparseable value passes through for the server to judge.
#[cfg(feature = "mariadb")]
pub(crate) fn naive_utc_timestamp(s: &str) -> String {
    parse_timestamp(s).map_or_else(
        || s.to_string(),
        |dt| dt.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
    )
}

/// A wire date (`YYYY-MM-DD`) in its canonical zero-padded form, or `None` when it
/// does not parse.
pub(crate) fn canonical_date(s: &str) -> Option<String> {
    let d = NaiveDate::parse_from_str(s.trim(), DATE_FORMAT).ok()?;
    in_four_digit_years(d).then(|| d.format(DATE_FORMAT).to_string())
}

fn in_four_digit_years(d: NaiveDate) -> bool {
    use chrono::Datelike;
    (0..=9999).contains(&d.year())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_timestamp_accepts_wire_forms() {
        for s in [
            "2024-01-02 12:30:45.500000+00",
            "2024-01-02 12:30:45+00",
            "2024-01-02T12:30:45.5Z",
            "2024-01-02T12:30:45+00:00",
            "2024-01-02 12:30:45",
            "2024-01-02T12:30:45.500000",
        ] {
            assert!(parse_timestamp(s).is_some(), "should parse: {s}");
        }
        assert!(parse_timestamp("not a time").is_none());
        assert!(parse_timestamp("2024-13-02T00:00:00Z").is_none());
    }

    #[test]
    fn canonical_timestamp_is_utc_with_fixed_micros() {
        assert_eq!(
            canonical_timestamp("2026-10-21T08:00:00Z").as_deref(),
            Some("2026-10-21T08:00:00.000000Z")
        );
        assert_eq!(
            canonical_timestamp("2026-10-21 08:30:00").as_deref(),
            Some("2026-10-21T08:30:00.000000Z")
        );
        assert_eq!(
            canonical_timestamp("2026-10-21T10:30:00.5+02:00").as_deref(),
            Some("2026-10-21T08:30:00.500000Z")
        );
        assert_eq!(
            canonical_timestamp("2024-01-02 12:30:45.500000+00").as_deref(),
            Some("2024-01-02T12:30:45.500000Z")
        );
    }

    /// Mixed input forms sort in time order once canonical (`' ' < 'T'` no longer matters).
    #[test]
    fn canonical_timestamps_sort_in_time_order() {
        let a = canonical_timestamp("2026-10-21T08:00:00Z").unwrap();
        let b = canonical_timestamp("2026-10-21 08:30:00").unwrap();
        let cut = canonical_timestamp("2026-10-21T08:15:00Z").unwrap();
        assert!(a < cut && cut < b);
    }

    #[test]
    fn canonical_timestamp_rejects_out_of_range_years() {
        assert!(canonical_timestamp("+10000-01-01T00:00:00Z").is_none());
    }

    #[cfg(feature = "mariadb")]
    #[test]
    fn naive_utc_timestamp_drops_the_zone() {
        assert_eq!(
            naive_utc_timestamp("2026-10-21T10:00:00.000000+02:00"),
            "2026-10-21 08:00:00.000000"
        );
    }

    #[test]
    fn canonical_date_validates_and_pads() {
        assert_eq!(canonical_date("2026-10-21").as_deref(), Some("2026-10-21"));
        assert_eq!(canonical_date("2026-1-5").as_deref(), Some("2026-01-05"));
        assert!(canonical_date("2026-02-30").is_none());
        assert!(canonical_date("21/10/2026").is_none());
        assert!(canonical_date("2026-10-21T00:00:00Z").is_none());
    }
}
