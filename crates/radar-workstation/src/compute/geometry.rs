//! Beam geometry for Echo Tops/VIL (S3-W4 §7.1): standard 4/3-effective-earth-
//! radius conversions between (ground range, elevation angle) and (slant
//! range, height above the radar). Pure math, no I/O, no radar-specific
//! types — this module knows nothing about `SweepGrid` or moments.

/// Mean Earth radius, meters (WGS84-adjacent spherical approximation — fine
/// at radar ranges, where the ellipsoid's flattening is negligible against
/// the beam-height uncertainty this whole model already carries).
const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// Standard-atmosphere effective-earth-radius factor (the textbook "4/3
/// earth" approximation for standard atmospheric refraction).
const REFRACTION_K: f64 = 4.0 / 3.0;
const KE_A: f64 = REFRACTION_K * EARTH_RADIUS_M;

/// Slant range and height above the radar for a target at ground range
/// `ground_m` along the surface, on a beam at elevation `elev_deg`.
///
/// Closed form, from the triangle (earth centre, radar, target) with
/// central angle `φ = ground / (ke·a)`:
///   `r = ke·a · sin(φ) / cos(θ + φ)`
///   `h = ke·a · (cos(θ) / cos(θ + φ) − 1)`
pub fn slant_range_and_height(ground_m: f64, elev_deg: f64) -> (f64, f64) {
    let theta = elev_deg.to_radians();
    let phi = ground_m / KE_A;
    let denom = (theta + phi).cos();
    if denom.abs() < 1e-9 {
        // Not reachable at any realistic WSR-88D range/elevation combination
        // (theta + phi stays well under 30° even at max range and the
        // highest VCP tilt) — guarded anyway per Stability as Ethics: a
        // degenerate result must never become a division that produces an
        // silently-wrong finite number.
        return (f64::INFINITY, f64::INFINITY);
    }
    let r = KE_A * phi.sin() / denom;
    let h = KE_A * (theta.cos() / denom - 1.0);
    (r, h)
}

/// Forward direction: ground range and height for a target at slant range
/// `slant_m` on a beam at elevation `elev_deg`. The standard
/// Doviak-and-Zrnic slant-range-to-height/ground-range equations, inverse
/// of [`slant_range_and_height`]. Used by tests and by a future cursor
/// readout.
pub fn ground_range_and_height(slant_m: f64, elev_deg: f64) -> (f64, f64) {
    let theta = elev_deg.to_radians();
    let h = (slant_m * slant_m + KE_A * KE_A + 2.0 * slant_m * KE_A * theta.sin()).sqrt() - KE_A;
    let s = KE_A * (slant_m * theta.cos() / (KE_A + h)).asin();
    (s, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_against_ground_range_and_height() {
        for &ground_km in &[10.0, 50.0, 100.0, 230.0, 460.0] {
            for &elev_deg in &[0.0, 0.5, 1.5, 4.0, 10.0, 19.5] {
                let ground_m = ground_km * 1000.0;
                let (slant, height) = slant_range_and_height(ground_m, elev_deg);
                let (ground_back, height_back) = ground_range_and_height(slant, elev_deg);
                assert!(
                    (ground_back - ground_m).abs() < 1.0,
                    "ground={ground_km}km elev={elev_deg}deg: ground round-trip off by {}",
                    (ground_back - ground_m).abs()
                );
                assert!(
                    (height_back - height).abs() < 1.0,
                    "ground={ground_km}km elev={elev_deg}deg: height round-trip off by {}",
                    (height_back - height).abs()
                );
            }
        }
    }

    #[test]
    fn beam_height_matches_the_standard_four_thirds_figure() {
        // At 0° elevation, the small-angle approximation to the 4/3-earth
        // model is h ≈ ground² / (2 · ke·a). At 100 km ground range:
        // 100000² / (2 * 8_494_666.67) ≈ 588.6 m — this closed form's own
        // output, cross-checked against that textbook approximation (and
        // against `ground_range_and_height`'s independent DZ-formula
        // derivation via the round-trip test above).
        let (_, h) = slant_range_and_height(100_000.0, 0.0);
        let approx = 100_000f64.powi(2) / (2.0 * KE_A);
        assert!((h - approx).abs() < 5.0, "expected ~{approx} m (small-angle approximation), got {h} m");
    }

    #[test]
    fn phi_near_zero_does_not_divide_by_zero() {
        let (r, h) = slant_range_and_height(0.0, 0.5);
        assert!(r.abs() < 1.0, "r={r}");
        assert!(h.abs() < 1.0, "h={h}");
    }
}
