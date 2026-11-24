//! NEXRAD date/time → UTC civil time and Unix seconds (S4-W6 §9.1, plan
//! §2.5). A few dozen lines of arithmetic, no dependency: `paths.rs` declined
//! `dirs`, `event.rs` declined a logging crate, and a calendar conversion this
//! small does not justify `chrono`/`time` on the < 2 s first-render path.
//!
//! Library-side (not `render/`, which is binary-side by ADR-0022) because the
//! data-age readout and the live wrap regression test (plan W6) both need
//! NEXRAD-time → Unix-seconds from the library crate.
//!
//! NEXRAD carries a **modified Julian date**: whole days since 1970-01-01
//! with day 1 = 1970-01-01 (so the Unix epoch is day 0 here, i.e.
//! `julian_date - 1` days after the epoch), plus milliseconds past midnight
//! UTC. See `nexrad_decoder::VolumeScan::{julian_date, scan_time_ms}`.

/// Broken-down UTC civil time: `(year, month, day, hour, minute, second)`.
/// `year` is a full year (2026, not 126 or 26); `month` and `day` are
/// 1-based.
pub type CivilTime = (i32, u32, u32, u32, u32, u32);

/// Days since 1970-01-01 for a NEXRAD julian date (day 1 = the epoch). One
/// definition of the epoch offset, shared by every conversion below.
fn days_since_epoch(julian_date: u16) -> i64 {
    julian_date as i64 - 1
}

/// Milliseconds past midnight → whole seconds of the day, clamped into
/// `[0, 86_399]`. A well-formed value is `< 86_400_000`; clamp defensively
/// rather than letting a malformed volume roll the clock past midnight
/// (Stability as Ethics — the decoder's bytes are attacker-influenceable).
/// The **one** place the clamp lives; both `utc_from_nexrad` and
/// `unix_secs_from_nexrad` reach it through here.
fn secs_of_day(scan_time_ms: u32) -> u32 {
    (scan_time_ms / 1000).min(86_399)
}

/// Convert a NEXRAD `(julian_date, scan_time_ms)` pair to UTC civil time.
///
/// Days-to-civil is Howard Hinnant's `civil_from_days` (public domain),
/// exact for the full proleptic Gregorian range.
pub fn utc_from_nexrad(julian_date: u16, scan_time_ms: u32) -> CivilTime {
    let (year, month, day) = civil_from_days(days_since_epoch(julian_date));

    let s = secs_of_day(scan_time_ms);
    (year, month, day, s / 3600, (s % 3600) / 60, s % 60)
}

/// Seconds since the Unix epoch for a NEXRAD `(julian_date, scan_time_ms)`.
/// Shares the epoch offset and the time-of-day clamp with `utc_from_nexrad`
/// by going through it, so the two can never disagree.
pub fn unix_secs_from_nexrad(julian_date: u16, scan_time_ms: u32) -> i64 {
    unix_secs_from_civil(utc_from_nexrad(julian_date, scan_time_ms))
}

/// Seconds since the Unix epoch for a broken-down UTC civil time.
pub fn unix_secs_from_civil(t: CivilTime) -> i64 {
    let (y, mo, d, h, mi, s) = t;
    days_from_civil(y, mo, d) * 86_400 + h as i64 * 3600 + mi as i64 * 60 + s as i64
}

/// Hinnant's `civil_from_days`: days since 1970-01-01 → `(year, month,
/// day)`, month and day 1-based. Valid for any `i64` day count.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

/// Hinnant's `days_from_civil` — the exact inverse of `civil_from_days`:
/// `(year, month, day)` → days since 1970-01-01. Public domain.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let m = m as i64;
    let d = d as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DD HH:MM:SSZ` — the status bar's scan-time format (§9.1).
pub fn format_utc(t: CivilTime) -> String {
    let (y, mo, d, h, mi, s) = t;
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_day_one_is_1970_01_01() {
        assert_eq!(utc_from_nexrad(1, 0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn milliseconds_past_midnight_become_hms() {
        // 12:34:56 = 45_296 s = 45_296_000 ms.
        assert_eq!(utc_from_nexrad(1, 45_296_789), (1970, 1, 1, 12, 34, 56));
    }

    #[test]
    fn hand_computed_2026_06_29() {
        // Days from 1970-01-01 to 2026-06-29: 20_633 (verified against an
        // independent date calculator). NEXRAD julian_date = days + 1.
        assert_eq!(utc_from_nexrad(20_634, 0), (2026, 6, 29, 0, 0, 0));
    }

    #[test]
    fn kdox_fixture_scan_date_round_trips_through_the_formatter() {
        // CLAUDE.md's confirmed KDOX fixture: VCP 35, June 29 2026.
        let t = utc_from_nexrad(20_634, 18 * 3_600_000 + 11 * 60_000);
        assert_eq!(format_utc(t), "2026-06-29 18:11:00Z");
    }

    #[test]
    fn leap_day_2024_02_29() {
        // 2024-02-29 is day 19_782 since the epoch.
        assert_eq!(utc_from_nexrad(19_783, 0), (2024, 2, 29, 0, 0, 0));
    }

    #[test]
    fn malformed_scan_time_ms_clamps_within_the_day() {
        let (_, _, _, h, m, s) = utc_from_nexrad(1, u32::MAX);
        assert_eq!((h, m, s), (23, 59, 59), "a malformed ms value must not roll past midnight");
    }

    #[test]
    fn days_from_civil_inverts_civil_from_days() {
        // ~-27 y to ~+82 y around the epoch — well past any date the chunk
        // retention window or an archive read can produce.
        for n in -10_000..30_000 {
            let (y, m, d) = civil_from_days(n);
            assert_eq!(days_from_civil(y, m, d), n, "round-trip failed at day {n}");
        }
    }

    #[test]
    fn unix_secs_from_nexrad_matches_the_kdox_fixture_scan_time() {
        // 2026-06-29 is day 20_633 since the epoch; 18:11:00 = 65_460 s.
        assert_eq!(unix_secs_from_nexrad(20_634, 0), 20_633 * 86_400);
        assert_eq!(
            unix_secs_from_nexrad(20_634, 18 * 3_600_000 + 11 * 60_000),
            20_633 * 86_400 + 65_460
        );
    }

    #[test]
    fn unix_secs_from_nexrad_clamps_a_malformed_time_of_day() {
        // days_since_epoch(1) == 0, so only the clamped time-of-day remains.
        assert_eq!(unix_secs_from_nexrad(1, u32::MAX), 86_399);
    }

    #[test]
    fn unix_secs_from_civil_agrees_with_unix_secs_from_nexrad() {
        let t = utc_from_nexrad(20_634, 12 * 3_600_000);
        assert_eq!(unix_secs_from_civil(t), unix_secs_from_nexrad(20_634, 12 * 3_600_000));
    }
}
