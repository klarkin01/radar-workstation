//! Bundled NEXRAD site list (S2-W3, FR-MU-3, FR-SS-1). A generated `const`
//! table rather than a bundled JSON file parsed at startup — see the
//! ADR-0006 erratum. Zero dependencies, zero startup parse failure mode,
//! validated by the compiler at build time.

pub use crate::sites_generated::SITES;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Site {
    /// ICAO site identifier, e.g. "KDOX". Always 4 uppercase alphanumeric
    /// characters — see `utility/nexrad-sites/generate.py`'s assertion.
    pub id: &'static str,
    pub name: &'static str,
    /// Two-letter US state/territory abbreviation. Empty for the handful of
    /// overseas DoD-operated sites the source export leaves blank (e.g.
    /// Kadena, Kunsan) — see `utility/README.md`.
    pub state: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub elevation_m: i32,
}

/// All bundled operational WSR-88D sites, sorted by `id`. `by_id` depends on
/// this ordering for its binary search — see the debug assertion there and
/// `tests::table_is_sorted_with_no_duplicate_ids`.
pub fn all() -> &'static [Site] {
    SITES
}

/// Look up a site by its ICAO identifier, case-insensitively. `O(log n)` —
/// relies on `SITES` being sorted by `id`, ASCII-uppercase, which
/// `generate.py` guarantees and `tests::table_is_sorted_with_no_duplicate_ids`
/// checks on every test run.
pub fn by_id(id: &str) -> Option<&'static Site> {
    debug_assert!(is_sorted_no_duplicates(SITES), "SITES must stay sorted by id with no duplicates");
    let upper = id.to_ascii_uppercase();
    SITES.binary_search_by(|site| site.id.cmp(upper.as_str())).ok().map(|idx| &SITES[idx])
}

fn is_sorted_no_duplicates(sites: &[Site]) -> bool {
    sites.windows(2).all(|w| w[0].id < w[1].id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_with_no_duplicate_ids() {
        assert!(is_sorted_no_duplicates(SITES), "SITES must be sorted by id with no duplicate ids");
    }

    #[test]
    fn table_is_non_empty_and_plausible() {
        assert!(SITES.len() > 100, "expected on the order of 160 WSR-88D sites, got {}", SITES.len());
        for site in SITES {
            assert_eq!(site.id.len(), 4, "non-4-character id: {}", site.id);
            assert!(site.id.chars().all(|c| c.is_ascii_alphanumeric()), "non-alphanumeric id: {}", site.id);
            assert!(!site.name.is_empty(), "empty name for {}", site.id);
            assert!((-90.0..=90.0).contains(&site.lat), "implausible latitude for {}: {}", site.id, site.lat);
            assert!((-180.0..=180.0).contains(&site.lon), "implausible longitude for {}: {}", site.id, site.lon);
            // KICX (Cedar City, UT) is the highest WSR-88D at ~3279m;
            // low-lying coastal sites go slightly below sea level. Generous
            // bounds — this is a plausibility check, not a precise range.
            assert!(
                (-100..3500).contains(&site.elevation_m),
                "implausible elevation for {}: {}",
                site.id,
                site.elevation_m
            );
        }
    }

    #[test]
    fn by_id_is_case_insensitive_and_unknown_ids_are_none() {
        let kdox = by_id("KDOX").expect("KDOX must be in the bundled table");
        assert_eq!(by_id("kdox"), Some(kdox));
        assert_eq!(by_id("KdOx"), Some(kdox));
        assert!(by_id("ZZZZ").is_none());
        assert!(by_id("").is_none());
    }

    /// Cross-source check against CLAUDE.md's ground truth for KDOX, taken
    /// from a real decoded RVOL block (not from this same site list) — see
    /// `crates/nexrad-decoder/tests/decode_radial.rs`'s
    /// `start_of_volume_has_site_parameters`. Latitude/longitude only: the
    /// bundled `elevation_m` here is HOMR's published station/antenna
    /// elevation, not the bare RDA `site_amsl_m` the decoder reports (~15m
    /// for KDOX) — the two are different reference points and are not
    /// expected to match numerically.
    #[test]
    fn kdox_lat_lon_matches_decoded_ground_truth() {
        let kdox = by_id("KDOX").expect("KDOX must be in the bundled table");
        assert!((kdox.lat - 38.8258).abs() < 0.001, "lat={}", kdox.lat);
        assert!((kdox.lon - (-75.4401)).abs() < 0.001, "lon={}", kdox.lon);
    }

    #[test]
    fn ktlh_is_present_in_florida() {
        let ktlh = by_id("KTLH").expect("KTLH must be in the bundled table");
        assert_eq!(ktlh.state, "FL");
        assert!((ktlh.lat - 30.0).abs() < 2.0);
        assert!((ktlh.lon - (-84.0)).abs() < 2.0);
    }
}
