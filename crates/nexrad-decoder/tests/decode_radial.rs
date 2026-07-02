use nexrad_decoder::{parse_radial_stream, MomentKind, RadialStatus};

// Each fixture is a complete Message 31 record (CTM header + message header +
// body), extracted from real KDOX VCP 35 chunks by gen_fixtures.py. Passing
// fixture bytes directly to parse_radial_stream works because the stream
// walker expects data starting at the first CTM header (no volume header).

macro_rules! fixture {
    ($name:expr) => {
        include_bytes!(concat!("fixtures/", $name))
    };
}

// ---------------------------------------------------------------------------
// Start of Volume
// ---------------------------------------------------------------------------

#[test]
fn start_of_volume_status_and_geometry() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let radials = parse_radial_stream(data).expect("parse failed");
    assert_eq!(radials.len(), 1);

    let r = &radials[0];
    assert_eq!(r.radial_status, RadialStatus::StartOfVolume);
    assert_eq!(r.elevation_number, 1);
    assert_eq!(&r.site_id, b"KDOX");
    assert!((r.azimuth_deg - 226.25).abs() < 0.1, "az={}", r.azimuth_deg);
    assert!((r.elevation_deg - 0.39).abs() < 0.1, "el={}", r.elevation_deg);
}

#[test]
fn start_of_volume_rrad_values() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    assert!((r.unamb_range_km - 583.75).abs() < 0.01, "unamb={}", r.unamb_range_km);
    assert!((r.nyquist_vel_ms - 8.37).abs() < 0.01, "nyquist={}", r.nyquist_vel_ms);
}

#[test]
fn start_of_volume_has_volume_constants() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    let vc = r.volume_constants.as_ref().expect("volume_constants absent on StartOfVolume");
    assert_eq!(vc.vcp_number, 35);
    assert!((vc.latitude - 38.8258).abs() < 0.001, "lat={}", vc.latitude);
    assert!((vc.longitude - (-75.4401)).abs() < 0.001, "lon={}", vc.longitude);
    assert_eq!(vc.site_amsl_m, 15);
}

#[test]
fn start_of_volume_moments_present() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    // Tilt 1 in VCP 35 has dual-pol moments but no velocity/width
    for kind in [MomentKind::Ref, MomentKind::Zdr, MomentKind::Phi, MomentKind::Rho, MomentKind::Cfp] {
        assert!(r.moments.contains_key(&kind), "missing moment {kind:?}");
    }
    assert!(!r.moments.contains_key(&MomentKind::Vel), "unexpected VEL on tilt 1");
    assert!(!r.moments.contains_key(&MomentKind::Sw), "unexpected SW on tilt 1");
}

#[test]
fn start_of_volume_dref_gate_geometry() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    let dref = r.moments.get(&MomentKind::Ref).expect("no DREF");
    assert_eq!(dref.gate_count, 1832);
    assert_eq!(dref.first_gate_m, 2125);
    assert_eq!(dref.gate_width_m, 250);
    assert_eq!(dref.word_size, 8);
    assert!((dref.scale - 2.0).abs() < 1e-6);
    assert!((dref.offset - 66.0).abs() < 1e-6);
}

#[test]
fn start_of_volume_drho_gate_geometry() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    let drho = r.moments.get(&MomentKind::Rho).expect("no DRHO");
    assert_eq!(drho.gate_count, 1192);
    assert_eq!(drho.word_size, 8);
    assert!((drho.scale - 300.0).abs() < 1e-4);
    assert!((drho.offset - (-60.5)).abs() < 1e-4);
}

#[test]
fn start_of_volume_dzdr_is_16bit() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    let dzdr = r.moments.get(&MomentKind::Zdr).expect("no DZDR");
    assert_eq!(dzdr.word_size, 16);
    assert_eq!(dzdr.gate_count, 1192);
}

#[test]
fn start_of_volume_physical_value_conversion() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];
    let dref = r.moments.get(&MomentKind::Ref).expect("no DREF");

    // At least one gate across 1832 must be non-reserved (even in clear air).
    // Gate 0 is near the radar — typically below SNR (raw=0 or 1), returns None.
    // Scan all gates to find at least one valid physical value.
    let valid_count = (0..dref.gate_count as usize)
        .filter_map(|i| dref.physical_value(i))
        .count();

    // Check that physical values, when present, are in a plausible dBZ range
    for i in 0..dref.gate_count as usize {
        if let Some(dbz) = dref.physical_value(i) {
            assert!(
                dbz > -35.0 && dbz < 90.0,
                "gate {i}: dBZ={dbz} out of physical range"
            );
        }
    }

    // There should be at least some data in 1832 gates (even in clear air, ground
    // clutter returns appear near the radar). If all 1832 are below-SNR, the fixture
    // or the decoder has a problem.
    assert!(valid_count > 0, "no valid DREF gates found in 1832-gate sweep");
}

// ---------------------------------------------------------------------------
// Intermediate
// ---------------------------------------------------------------------------

#[test]
fn intermediate_status_and_geometry() {
    let data = fixture!("kdox_vcp35_intermediate.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    assert_eq!(r.radial_status, RadialStatus::Intermediate);
    assert_eq!(r.elevation_number, 1);
    // RVOL is present on every radial in observed KDOX data (vol_ptr is always 72)
    let vc = r.volume_constants.as_ref().expect("RVOL block absent");
    assert_eq!(vc.vcp_number, 35);
}

#[test]
fn intermediate_rrad_values() {
    let data = fixture!("kdox_vcp35_intermediate.bin");
    let r = &parse_radial_stream(data).unwrap()[0];
    assert!((r.unamb_range_km - 583.75).abs() < 0.01);
    assert!((r.nyquist_vel_ms - 8.37).abs() < 0.01);
}

// ---------------------------------------------------------------------------
// End of Elevation
// ---------------------------------------------------------------------------

#[test]
fn end_of_elevation_status() {
    let data = fixture!("kdox_vcp35_end_of_elevation.bin");
    let r = &parse_radial_stream(data).unwrap()[0];
    assert_eq!(r.radial_status, RadialStatus::EndOfElevation);
    assert_eq!(r.elevation_number, 1);
}

// ---------------------------------------------------------------------------
// Start of Elevation (tilt 2 — Doppler-only, 3 moments)
// ---------------------------------------------------------------------------

#[test]
fn start_of_elevation_status_and_moments() {
    let data = fixture!("kdox_vcp35_start_of_elevation.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    assert_eq!(r.radial_status, RadialStatus::StartOfElevation);
    assert_eq!(r.elevation_number, 2);

    // Tilt 2 in VCP 35 is a Doppler-only tilt: REF + VEL + SW, no dual-pol
    assert!(r.moments.contains_key(&MomentKind::Ref));
    assert!(r.moments.contains_key(&MomentKind::Vel));
    assert!(r.moments.contains_key(&MomentKind::Sw));
    assert!(!r.moments.contains_key(&MomentKind::Zdr));
    assert!(!r.moments.contains_key(&MomentKind::Phi));
    assert!(!r.moments.contains_key(&MomentKind::Rho));
}

#[test]
fn start_of_elevation_dvel_gate_geometry() {
    let data = fixture!("kdox_vcp35_start_of_elevation.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    // Tilt 2: all moments have 1192 gates (confirmed from binary inspection)
    let dvel = r.moments.get(&MomentKind::Vel).expect("no DVEL");
    assert_eq!(dvel.gate_count, 1192);
    assert_eq!(dvel.word_size, 8);
    assert!((dvel.scale - 2.0).abs() < 1e-6);
    assert!((dvel.offset - 129.0).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// End of Volume (tilt 16 — all 7 moments)
// ---------------------------------------------------------------------------

#[test]
fn end_of_volume_status_and_all_moments() {
    let data = fixture!("kdox_vcp35_end_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    assert_eq!(r.radial_status, RadialStatus::EndOfVolume);

    for kind in [
        MomentKind::Ref, MomentKind::Vel, MomentKind::Sw,
        MomentKind::Zdr, MomentKind::Phi, MomentKind::Rho, MomentKind::Cfp,
    ] {
        assert!(r.moments.contains_key(&kind), "missing moment {kind:?}");
    }
}

#[test]
fn end_of_volume_dphi_is_16bit() {
    let data = fixture!("kdox_vcp35_end_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    let dphi = r.moments.get(&MomentKind::Phi).expect("no DPHI");
    assert_eq!(dphi.word_size, 16);
}

// ---------------------------------------------------------------------------
// Reserved gate value semantics (raw 0 = below SNR, raw 1 = range folded)
// ---------------------------------------------------------------------------

#[test]
fn reserved_raw_values_return_none() {
    // Synthesise a minimal MomentData to test the physical_value logic in isolation.
    use nexrad_decoder::MomentData;

    let md = MomentData {
        gate_count: 4,
        first_gate_m: 2125,
        gate_width_m: 250,
        word_size: 8,
        scale: 2.0,
        offset: 66.0,
        data: vec![0, 1, 2, 133], // below-SNR, range-folded, min valid, valid
    };

    assert!(md.physical_value(0).is_none(), "raw=0 should be None");
    assert!(md.physical_value(1).is_none(), "raw=1 should be None");
    assert!(md.physical_value(2).is_some(), "raw=2 should be Some");
    // raw=133 → (133 - 66) / 2 = 33.5 dBZ
    let v = md.physical_value(3).unwrap();
    assert!((v - 33.5).abs() < 1e-5, "expected 33.5 dBZ, got {v}");
}

#[test]
fn raw_gate_out_of_range_returns_none() {
    use nexrad_decoder::MomentData;

    let md = MomentData {
        gate_count: 2,
        first_gate_m: 2125,
        gate_width_m: 250,
        word_size: 8,
        scale: 2.0,
        offset: 66.0,
        data: vec![100, 200],
    };

    assert!(md.raw_gate(0).is_some());
    assert!(md.raw_gate(1).is_some());
    assert!(md.raw_gate(2).is_none(), "out-of-range index should return None");
}

// ---------------------------------------------------------------------------
// Dual-pol calibration constants (scale/offset) — tilt 1 fixture
// ---------------------------------------------------------------------------

#[test]
fn start_of_volume_dzdr_scale_and_offset() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    let dzdr = r.moments.get(&MomentKind::Zdr).expect("no DZDR");
    // Confirmed from binary: scale=32.0, offset=418.0 (CLAUDE.md §Confirmed Test File Values)
    assert!((dzdr.scale - 32.0).abs() < 1e-4, "scale={}", dzdr.scale);
    assert!((dzdr.offset - 418.0).abs() < 1e-4, "offset={}", dzdr.offset);
}

#[test]
fn start_of_volume_dphi_scale_and_offset() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    let dphi = r.moments.get(&MomentKind::Phi).expect("no DPHI");
    // Confirmed from binary: scale=2.8361, offset=2.0 (CLAUDE.md §Confirmed Test File Values)
    assert!((dphi.scale - 2.8361).abs() < 1e-4, "scale={}", dphi.scale);
    assert!((dphi.offset - 2.0).abs() < 1e-4, "offset={}", dphi.offset);
}

#[test]
fn start_of_volume_dcfp_gate_geometry() {
    let data = fixture!("kdox_vcp35_start_of_volume.bin");
    let r = &parse_radial_stream(data).unwrap()[0];

    // DCFP on tilt 1 covers the same range as DREF: 1832 gates at 8 bits each.
    let dcfp = r.moments.get(&MomentKind::Cfp).expect("no DCFP");
    assert_eq!(dcfp.gate_count, 1832);
    assert_eq!(dcfp.word_size, 8);
}

// ---------------------------------------------------------------------------
// 16-bit physical value computation (DZDR path through physical_value)
// ---------------------------------------------------------------------------

#[test]
fn physical_value_16bit_conversion() {
    use nexrad_decoder::MomentData;

    // DZDR calibration constants from the KDOX fixture.
    // raw=0 → None (below SNR); raw=450 → (450 - 418.0) / 32.0 = 1.0 dB ZDR
    let md = MomentData {
        gate_count: 2,
        first_gate_m: 0,
        gate_width_m: 0,
        word_size: 16,
        scale: 32.0,
        offset: 418.0,
        data: vec![
            0x00, 0x00, // gate 0: raw=0 → None (below SNR)
            0x01, 0xC2, // gate 1: raw=450 → 1.0 dB ZDR
        ],
    };

    assert!(md.physical_value(0).is_none(), "raw=0 (16-bit) should be None");
    let v = md.physical_value(1).unwrap();
    assert!((v - 1.0).abs() < 1e-5, "expected 1.0 dB ZDR, got {v}");
}

// ---------------------------------------------------------------------------
// Stream-level edge cases
// ---------------------------------------------------------------------------

#[test]
fn empty_stream_returns_empty_vec() {
    let radials = parse_radial_stream(&[]).unwrap();
    assert!(radials.is_empty());
}

#[test]
fn legacy_size_records_are_skipped() {
    // size_hw=0 in the message header triggers a 2432-byte legacy skip.
    // A buffer of exactly 2432 zero bytes produces zero radials.
    let data = vec![0u8; 2432];
    let radials = parse_radial_stream(&data).unwrap();
    assert!(radials.is_empty());
}

#[test]
fn truncated_msg31_record_returns_error() {
    // Craft a header that claims msg_type=31 with size_hw=5000 (10000 byte record),
    // but the buffer is only 100 bytes — record slice will fail.
    let mut data = vec![0u8; 100];
    // Message header sits after 12-byte CTM header.
    // Bytes 12-13: size_hw = 5000 big-endian
    data[12] = 0x13;
    data[13] = 0x88;
    // Byte 15: msg_type = 31
    data[15] = 31;

    assert!(parse_radial_stream(&data).is_err(), "expected Err for truncated record");
}
