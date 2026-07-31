use nexrad_decoder::{parse_radial_stream, ProductKind, Radial, RadialStatus};

// Fixtures extracted directly from a real archive-bucket volume file,
// KTLH20100601_000029_V03.gz (unidata-nexrad-level2, 2010-06-01, VCP 121) —
// the non-dual-pol era half of FR-ND-8's fixture breadth (S1-W4d in
// docs/plans/stage-0-1-close-the-acquisition-path.md).
//
// Unlike the chunk-stream fixtures elsewhere in this suite, archive volume
// files of this era are NOT internally BZ2-block-wrapped — the message
// stream begins directly after the 24-byte volume header (`V03` predates
// the BZ2-per-block Message 32 wrapping described for the 2026 KABR sample
// in CLAUDE.md). Extraction was a one-off `utility/nexrad-inspect` script
// reusing `gen_fixtures.py`'s `iter_msg31` walker directly against the raw
// (gunzipped) file bytes — no BZ2 decompression step needed, unlike every
// other fixture in this crate. This is exactly the "different envelope"
// the plan calls out as a utility job, not production code; nothing in
// `crates/radar-workstation` or `crates/nexrad-decoder` needs to parse this
// envelope, since the message stream itself (CTM header + message header +
// body) is identical to what the chunk-stream decoder already consumes.
//
// VCP 121 here has no ZDR/PHI/RHO/CFP blocks on any elevation — confirming
// the decoder's "moment block absent" path against real non-dual-pol data
// rather than only a synthetic corpus case.

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
fn non_dual_pol_start_of_volume_has_only_reflectivity() {
    let r = first_radial(fixture!("ktlh_vcp121_start_of_volume.bin"));
    assert_eq!(&r.site_id, b"KTLH");
    assert_eq!(r.radial_status, RadialStatus::StartOfVolume);
    let sp = r.site_parameters.as_ref().expect("RVOL block absent");
    assert_eq!(sp.vcp_number, 121);

    assert!(r.products.contains_key(&ProductKind::Ref));
    for kind in [
        ProductKind::Vel, ProductKind::SpectrumWidth,
        ProductKind::Zdr, ProductKind::Phi, ProductKind::Rho, ProductKind::Cfp,
    ] {
        assert!(!r.products.contains_key(&kind), "non-dual-pol era should not carry {kind:?}");
    }
}

#[test]
fn non_dual_pol_second_elevation_adds_velocity_and_spectrum_width_but_no_dual_pol() {
    let r = first_radial(fixture!("ktlh_vcp121_start_of_elevation.bin"));
    assert_eq!(r.elevation_number, 2);
    assert!(r.products.contains_key(&ProductKind::Ref));
    assert!(r.products.contains_key(&ProductKind::Vel));
    assert!(r.products.contains_key(&ProductKind::SpectrumWidth));
    assert!(!r.products.contains_key(&ProductKind::Zdr));
    assert!(!r.products.contains_key(&ProductKind::Rho));
}

#[test]
fn non_dual_pol_intermediate_and_end_of_elevation_decode() {
    let inter = first_radial(fixture!("ktlh_vcp121_intermediate.bin"));
    assert_eq!(inter.radial_status, RadialStatus::Intermediate);
    assert_eq!(inter.elevation_number, 1);

    let end_el = first_radial(fixture!("ktlh_vcp121_end_of_elevation.bin"));
    assert_eq!(end_el.radial_status, RadialStatus::EndOfElevation);
    assert_eq!(end_el.elevation_number, 1);
}

#[test]
fn non_dual_pol_end_of_volume_is_the_highest_elevation() {
    let r = first_radial(fixture!("ktlh_vcp121_end_of_volume.bin"));
    assert_eq!(r.radial_status, RadialStatus::EndOfVolume);
    assert_eq!(r.elevation_number, 20);
    assert!(r.elevation_deg > 19.0, "el={}", r.elevation_deg);

    // This era's highest tilt uses a coarser reflectivity gate spacing
    // (1.0 km) than the 250 m used throughout the KDOX/KTLH super-res data
    // elsewhere in this suite — real evidence of gate geometry variety.
    let dref = r.products.get(&ProductKind::Ref).expect("no DREF");
    assert_eq!(dref.gate_width_m, 1000);
}
