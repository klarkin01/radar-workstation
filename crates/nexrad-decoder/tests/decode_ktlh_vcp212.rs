use nexrad_decoder::{parse_radial_stream, Radial, RadialStatus};

// Fixtures extracted from a real KTLH (Tallahassee, FL) VCP 212 volume
// (2026-07-31, volume 998), by
// `utility/nexrad-inspect/gen_fixtures.py --by-elevation`. See S1-W4d in
// docs/plans/stage-0-1-close-the-acquisition-path.md and the corrected
// radial status table in docs/architecture/nexrad-binary-format.md §6.1.
//
// This VCP settles two questions FR-ND-8 fixture breadth needed answered:
//
// 1. Does `elevation_number` repeat within a volume on a SAILS/MRLE
//    precipitation VCP? No. The volume's SAILS/MRLE-inserted low-level cuts
//    (elevation 9, angle 0.66° — a near-repeat of elevation 1's 0.65°) got a
//    new, incrementing `elevation_number`, never a reused one. ADR-0012's
//    late-data discard rule (keyed on elevation number) is safe as designed.
// 2. What does radial status code 5 mean? Not "SAILS supplemental low-level
//    cut" as this repo's docs previously stated — it appeared exactly once,
//    on the volume's single highest elevation (16, angle 9.84°), matching
//    MetPy's `START_ELEVATION | LAST_ELEVATION`.
//
// It also gives the decoder its first non-KDOX site and its first
// standard-resolution fixture: elevation 16 (the last elevation) carries
// `azimuth_spacing_code == 2`, confirmed by direct measurement across the
// full downloaded volume to mean 1.0° spacing (360 radials per 360° sweep) —
// standard resolution — while elevations 1 and 9 (`azimuth_spacing_code ==
// 1`) measured 0.5° (720 radials per sweep), i.e. super-resolution. This is
// the *reverse* of what `nexrad-binary-format.md` §6.1 previously stated for
// this field's code meaning; that table has been corrected in the same pass
// (S1-W4d).

macro_rules! fixture {
    ($name:expr) => {
        include_bytes!(concat!("fixtures/", $name))
    };
}

fn first_radial(data: &[u8]) -> Radial {
    parse_radial_stream(data)
        .expect("parse failed")
        .into_iter()
        .next()
        .expect("no radials")
}

#[test]
fn second_site_and_precip_vcp_are_recognized() {
    let r = first_radial(fixture!("ktlh_vcp212_start_of_volume.bin"));
    assert_eq!(&r.site_id, b"KTLH");
    assert_eq!(r.radial_status, RadialStatus::StartOfVolume);
    assert_eq!(r.elevation_number, 1);
    let sp = r.site_parameters.as_ref().expect("RVOL block absent");
    assert_eq!(sp.vcp_number, 212);
}

#[test]
fn start_of_volume_is_super_resolution() {
    let r = first_radial(fixture!("ktlh_vcp212_start_of_volume.bin"));
    assert_eq!(r.azimuth_spacing_code, 1, "elevation 1 measured 0.5° spacing (super-resolution)");
    assert!((r.elevation_deg - 0.65).abs() < 0.1, "el={}", r.elevation_deg);
}

#[test]
fn sails_repeated_low_elevation_gets_a_new_elevation_number() {
    let r = first_radial(fixture!("ktlh_vcp212_sails_repeated_low_elevation.bin"));

    // Ordinary StartOfElevation, not a special SAILS code — and a distinct,
    // higher elevation_number than the original low cut, even though the
    // elevation angle repeats it.
    assert_eq!(r.radial_status, RadialStatus::StartOfElevation);
    assert_eq!(r.elevation_number, 9);
    assert!(
        (r.elevation_deg - 0.65).abs() < 0.1,
        "SAILS cut should repeat elevation 1's angle (~0.65°), got {}",
        r.elevation_deg
    );
    assert_eq!(r.azimuth_spacing_code, 1);
}

#[test]
fn last_elevation_status_code_means_start_of_last_elevation_not_sails() {
    let r = first_radial(fixture!("ktlh_vcp212_last_elevation.bin"));

    assert_eq!(r.radial_status, RadialStatus::StartOfLastElevation);
    assert_eq!(r.elevation_number, 16, "should be the volume's highest elevation number");
    assert!(
        r.elevation_deg > 9.0,
        "the 'last elevation' status should land on the highest tilt, got {}",
        r.elevation_deg
    );
    // The highest tilt in VCP 212 is Doppler-capable, unlike elevation 1.
    assert!(r.products.contains_key(&nexrad_decoder::ProductKind::Vel));
    // Measured 1.0° spacing across the full volume — the standard-resolution
    // fixture (see this file's module doc comment on the az_spacing correction).
    assert_eq!(r.azimuth_spacing_code, 2);
}
