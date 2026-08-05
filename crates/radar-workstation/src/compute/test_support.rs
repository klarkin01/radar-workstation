//! Shared synthetic-data builders for compute-layer unit tests, mirroring
//! the established pattern in `assembly::tests` and `state::apply`'s own
//! tests: domain objects are constructed directly rather than decoded from
//! fixture bytes. Gridding and derivation operate on already-decoded
//! `Sweep`/`Radial`/`ProductData` values, not on the decoder's byte-level
//! framing — that has its own fixture-based suite in `crates/nexrad-decoder`
//! and does not need re-testing here.

use nexrad_decoder::{ProductData, ProductKind, ProductMap, Radial, RadialStatus, Sweep};

/// One moment to attach to a synthetic radial: `(kind, gate_count,
/// first_gate_m, gate_width_m, word_size, scale, offset)`.
pub type MomentSpec = (ProductKind, u16, u16, u16, u8, f32, f32);

/// The raw gate value gridding sees at index `gate`, shared by every 8-bit
/// synthetic moment: gate 0 is the ICD no-data code, gate 1 is the ICD
/// range-fold code, and gate `g >= 2` is `g` itself (clamped into the valid
/// `2..=255` range) — so a test that wants "the gate whose raw value is
/// 133" just asks for gate index 133, with no second helper needed.
fn synthetic_raw(gate: u16) -> u16 {
    match gate {
        0 => 0,
        1 => 1,
        g => ((g - 2) % 254) + 2,
    }
}

/// A synthetic radial carrying the given moments, with gate data from
/// [`synthetic_raw`].
pub fn radial(elevation_number: u8, azimuth_deg: f32, azimuth_spacing_code: u8, moments: &[MomentSpec]) -> Radial {
    let mut products = ProductMap::new();
    for &(kind, gate_count, first_gate_m, gate_width_m, word_size, scale, offset) in moments {
        let data: Vec<u8> = (0..gate_count).map(|g| synthetic_raw(g) as u8).collect();
        products.insert(kind, ProductData { gate_count, first_gate_m, gate_width_m, word_size, scale, offset, data });
    }
    bare_radial(elevation_number, azimuth_deg, azimuth_spacing_code, products)
}

/// A synthetic radial carrying one explicit 16-bit moment, gate values
/// given verbatim rather than through [`synthetic_raw`] — needed for tests
/// that must control a raw value above 255 (e.g. ZDR requantisation).
#[allow(clippy::too_many_arguments)]
pub fn radial16(
    elevation_number: u8,
    azimuth_deg: f32,
    azimuth_spacing_code: u8,
    kind: ProductKind,
    gate_count: u16,
    first_gate_m: u16,
    gate_width_m: u16,
    scale: f32,
    offset: f32,
    raw_gates: &[u16],
) -> Radial {
    let data: Vec<u8> = raw_gates.iter().flat_map(|v| v.to_be_bytes()).collect();
    let mut products = ProductMap::new();
    products.insert(kind, ProductData { gate_count, first_gate_m, gate_width_m, word_size: 16, scale, offset, data });
    bare_radial(elevation_number, azimuth_deg, azimuth_spacing_code, products)
}

fn bare_radial(elevation_number: u8, azimuth_deg: f32, azimuth_spacing_code: u8, products: ProductMap) -> Radial {
    Radial {
        site_id: *b"TEST",
        scan_time_ms: 0,
        julian_date: 0,
        azimuth_deg,
        elevation_deg: elevation_number as f32 * 0.5,
        azimuth_number: 0,
        azimuth_spacing_code,
        radial_status: RadialStatus::Intermediate,
        elevation_number,
        unambiguous_range_km: Some(300.0),
        nyquist_velocity_mps: Some(8.0),
        site_parameters: None,
        products,
    }
}

pub fn sweep(elevation_number: u8, elevation_deg: f32, radials: Vec<Radial>) -> Sweep {
    Sweep {
        elevation_number,
        elevation_deg,
        nyquist_velocity_mps: Some(8.0),
        unambiguous_range_km: Some(300.0),
        radials,
        complete: true,
    }
}
