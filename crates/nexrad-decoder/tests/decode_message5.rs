use nexrad_decoder::parse_metadata_stream;

// Fixture extracted from a real KDOX -S chunk's decompressed message
// stream (downloads/KDOX_20260629_1801/20260629-180100-001-S, message 5 at
// stream offset 321048). See docs/architecture/nexrad-binary-format.md §15
// and S1-W2 in docs/plans/stage-0-1-close-the-acquisition-path.md.

#[test]
fn parses_vcp_35_from_a_real_message5_record() {
    let data = include_bytes!("fixtures/kdox_vcp35_message5.bin");
    let metadata = parse_metadata_stream(data).expect("parse failed");

    let vcp = metadata.vcp.expect("Message 5 should have been found");
    assert_eq!(vcp.vcp_number, 35);
    assert_eq!(vcp.pattern_type, 2);
    assert_eq!(vcp.elevations.len(), 16);

    // Lowest cut: confirmed commanded angle 0.308deg, a surveillance
    // (waveform=1) cut immediately followed by its Doppler pair at the
    // same angle.
    assert!((vcp.elevations[0].elevation_deg - 0.308).abs() < 0.01);
    assert_eq!(vcp.elevations[0].waveform, 1);
    assert!((vcp.elevations[1].elevation_deg - 0.308).abs() < 0.01);
    assert_eq!(vcp.elevations[1].waveform, 2);

    // The repeated low-angle insert at cut index 6 (same structure as the
    // SAILS/MRLE finding confirmed for KTLH VCP 212).
    assert!((vcp.elevations[6].elevation_deg - 0.308).abs() < 0.01);

    // Highest cut: commanded 6.416deg, batch waveform.
    let last = vcp.elevations.last().unwrap();
    assert!((last.elevation_deg - 6.416).abs() < 0.01);
    assert_eq!(last.waveform, 4);
}

#[test]
fn metadata_stream_with_no_message5_yields_none() {
    let metadata = parse_metadata_stream(&[]).expect("empty stream should not error");
    assert!(metadata.vcp.is_none());
}
