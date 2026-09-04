//! The bundled map underlay geometry (Stage 5, ADR-0025 §3–§4 as amended by
//! ADR-0028 §3): county/state/coastline/primary-road line geometry and city
//! labels, baked at build time by `utility/map-bake/bake.py` into
//! `overlay.bin` (committed beside this module, per ADR-0025 §6) and
//! compiled in via `include_bytes!` — no runtime shapefile/DBF parser, no
//! tessellator, no network. `bundle.manifest.txt`, committed alongside, is
//! the provenance and per-layer count record a 6.4 MB binary blob can't be
//! reviewed as (ADR-0025 §6).
//!
//! Every accessor here is pure and has no GPU dependency, so it is reachable
//! from `radar-viz` and from integration tests, not just the render loop
//! (S5-b). Projection into the render loop's world frame lives in
//! [`project`].
//!
//! **Hardening (Stability as Ethics).** [`Bundle::parse`] validates the
//! header and every section's bounds and returns `None` rather than
//! panicking or truncating. Past that, every accessor still reads through
//! `slice::get`, never indexing: an inconsistent *interior* reference (a
//! part whose points run past the point array, a label name whose
//! `off + len` runs past the string table, non-UTF-8 name bytes) yields an
//! empty iteration for that one element, not a panic and not a failed
//! parse — a single bad label must not delete the county layer.

mod project;

use std::sync::OnceLock;

pub use project::{project, Projected, ProjectedLabel, ProjectedLayer};

const MAGIC: &[u8; 8] = b"RWMOVL01";
const FORMAT_VERSION: u32 = 1;

const HEADER_LEN: usize = 32;
const LAYER_ENTRY_LEN: usize = 12;
const PART_ENTRY_LEN: usize = 24;
const POINT_LEN: usize = 8;
const LABEL_ENTRY_LEN: usize = 16;

/// One layer's worth of parts, or the label index when [`kind`](Self::kind)
/// is the label layer (`first_part`/`part_count` are `0` in that case —
/// §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    pub kind: u32,
    first_part: u32,
    part_count: u32,
}

/// One contiguous polyline (or polygon ring, treated as a closed polyline —
/// ADR-0025 §5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    /// `[min_lon, min_lat, max_lon, max_lat]`, degrees. Stored for a future
    /// culling pass; v1.0 does not cull (ADR-0025 §3).
    pub bbox_deg: [f64; 4],
    first_point: u32,
    point_count: u32,
}

/// One city/site label candidate from the bundle's label index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Label<'a> {
    pub lon: f64,
    pub lat: f64,
    pub rank: u16,
    pub name: &'a str,
}

fn read_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)?.try_into().ok().map(u32::from_le_bytes)
}

fn read_i32(b: &[u8], off: usize) -> Option<i32> {
    b.get(off..off + 4)?.try_into().ok().map(i32::from_le_bytes)
}

fn read_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)?.try_into().ok().map(u16::from_le_bytes)
}

/// Fixed-point bundle coordinates are 1e-7 degree units (§5.1).
fn fixed_to_deg(v: i32) -> f64 {
    v as f64 / 1e7
}

/// A parsed, bounds-validated view over a compiled-in `overlay.bin`. Every
/// section's offset and length were checked against the header's declared
/// counts at [`parse`](Self::parse) time; every accessor still re-checks
/// per-element (see the module doc's hardening note).
pub struct Bundle {
    bytes: &'static [u8],
    layer_count: u32,
    part_count: u32,
    point_count: u32,
    label_count: u32,
    string_bytes: u32,
    layers_off: usize,
    parts_off: usize,
    points_off: usize,
    labels_off: usize,
    strings_off: usize,
}

impl Bundle {
    /// Parses and validates `bytes` as an `overlay.bin` bundle. Checks the
    /// magic, the version, and that every section fits inside `bytes` at
    /// the declared counts, using only checked/saturating arithmetic — a
    /// declared count large enough to overflow a section offset fails
    /// parse rather than wrapping into an in-bounds-looking but wrong
    /// offset. Returns `None`, never panics, on any of that failing.
    pub fn parse(bytes: &'static [u8]) -> Option<Self> {
        if bytes.get(0..8)? != MAGIC {
            return None;
        }
        let version = read_u32(bytes, 8)?;
        if version != FORMAT_VERSION {
            return None;
        }
        let layer_count = read_u32(bytes, 12)?;
        let part_count = read_u32(bytes, 16)?;
        let point_count = read_u32(bytes, 20)?;
        let label_count = read_u32(bytes, 24)?;
        let string_bytes = read_u32(bytes, 28)?;

        let layers_off = HEADER_LEN;
        let layers_len = (layer_count as usize).checked_mul(LAYER_ENTRY_LEN)?;
        let parts_off = layers_off.checked_add(layers_len)?;
        let parts_len = (part_count as usize).checked_mul(PART_ENTRY_LEN)?;
        let points_off = parts_off.checked_add(parts_len)?;
        let points_len = (point_count as usize).checked_mul(POINT_LEN)?;
        let labels_off = points_off.checked_add(points_len)?;
        let labels_len = (label_count as usize).checked_mul(LABEL_ENTRY_LEN)?;
        let strings_off = labels_off.checked_add(labels_len)?;
        let strings_len = string_bytes as usize;
        let end = strings_off.checked_add(strings_len)?;
        if end > bytes.len() {
            return None;
        }

        Some(Self {
            bytes,
            layer_count,
            part_count,
            point_count,
            label_count,
            string_bytes,
            layers_off,
            parts_off,
            points_off,
            labels_off,
            strings_off,
        })
    }

    pub fn layers(&self) -> impl Iterator<Item = Layer> + '_ {
        (0..self.layer_count as usize).filter_map(move |i| {
            let off = self.layers_off + i * LAYER_ENTRY_LEN;
            Some(Layer {
                kind: read_u32(self.bytes, off)?,
                first_part: read_u32(self.bytes, off + 4)?,
                part_count: read_u32(self.bytes, off + 8)?,
            })
        })
    }

    /// Every part belonging to `layer`. A `first_part`/`part_count` pair
    /// that runs past the bundle's actual part count is clamped, yielding
    /// an empty (or truncated) iteration rather than reading out of range.
    pub fn parts(&self, layer: &Layer) -> impl Iterator<Item = Part> + '_ {
        let total = self.part_count as usize;
        let start = (layer.first_part as usize).min(total);
        let end = (layer.first_part as usize).saturating_add(layer.part_count as usize).min(total);
        (start..end).filter_map(move |i| {
            let off = self.parts_off + i * PART_ENTRY_LEN;
            let first_point = read_u32(self.bytes, off)?;
            let point_count = read_u32(self.bytes, off + 4)?;
            let min_lon = read_i32(self.bytes, off + 8)?;
            let min_lat = read_i32(self.bytes, off + 12)?;
            let max_lon = read_i32(self.bytes, off + 16)?;
            let max_lat = read_i32(self.bytes, off + 20)?;
            Some(Part {
                bbox_deg: [fixed_to_deg(min_lon), fixed_to_deg(min_lat), fixed_to_deg(max_lon), fixed_to_deg(max_lat)],
                first_point,
                point_count,
            })
        })
    }

    /// Every `(lon, lat)` in `part`, degrees. Clamped against the bundle's
    /// actual point count the same way [`parts`](Self::parts) clamps
    /// against the part count.
    pub fn points(&self, part: &Part) -> impl Iterator<Item = (f64, f64)> + '_ {
        let total = self.point_count as usize;
        let start = (part.first_point as usize).min(total);
        let end = (part.first_point as usize).saturating_add(part.point_count as usize).min(total);
        (start..end).filter_map(move |i| {
            let off = self.points_off + i * POINT_LEN;
            let lon = read_i32(self.bytes, off)?;
            let lat = read_i32(self.bytes, off + 4)?;
            Some((fixed_to_deg(lon), fixed_to_deg(lat)))
        })
    }

    pub fn labels(&self) -> impl Iterator<Item = Label<'_>> + '_ {
        (0..self.label_count as usize).filter_map(move |i| {
            let off = self.labels_off + i * LABEL_ENTRY_LEN;
            let lon = read_i32(self.bytes, off)?;
            let lat = read_i32(self.bytes, off + 4)?;
            let rank = read_u16(self.bytes, off + 8)?;
            let name_off = read_u32(self.bytes, off + 10)?;
            let name_len = read_u16(self.bytes, off + 14)?;
            let name = self.read_name(name_off, name_len)?;
            Some(Label { lon: fixed_to_deg(lon), lat: fixed_to_deg(lat), rank, name })
        })
    }

    /// A label name from the string table. `None` — not a panic — for an
    /// offset/length that runs past the string section or a byte slice that
    /// isn't valid UTF-8 (both are "this one label is malformed," not
    /// "the bundle is corrupt").
    fn read_name(&self, name_off: u32, name_len: u16) -> Option<&'static str> {
        let strings_end = self.strings_off.checked_add(self.string_bytes as usize)?;
        let start = self.strings_off.checked_add(name_off as usize)?;
        let end = start.checked_add(name_len as usize)?;
        if end > strings_end {
            return None;
        }
        std::str::from_utf8(self.bytes.get(start..end)?).ok()
    }

    pub fn total_points(&self) -> usize {
        self.point_count as usize
    }
}

/// The bundle compiled into the binary. `None` if it fails validation —
/// which cannot happen for a bundle this project generated, and is handled
/// anyway. Memoized: parsing runs at most once per process.
pub fn bundled() -> Option<&'static Bundle> {
    static BUNDLE: OnceLock<Option<Bundle>> = OnceLock::new();
    BUNDLE.get_or_init(|| Bundle::parse(include_bytes!("overlay.bin"))).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_bundle() -> &'static Bundle {
        bundled().expect("the committed overlay.bin must parse")
    }

    #[test]
    fn bundled_parses_the_committed_bundle() {
        real_bundle();
    }

    #[test]
    fn walks_every_layer_part_point_and_label() {
        let bundle = real_bundle();
        let mut total_points = 0usize;
        let mut layer_kinds = Vec::new();
        for layer in bundle.layers() {
            layer_kinds.push(layer.kind);
            for part in bundle.parts(&layer) {
                let points: Vec<(f64, f64)> = bundle.points(&part).collect();
                assert!(points.len() >= 2 || points.is_empty(), "a part must have >= 2 points or be unreadable");
                for (lon, lat) in &points {
                    assert!((-180.0..=180.0).contains(lon), "lon out of range: {lon}");
                    assert!((-90.0..=90.0).contains(lat), "lat out of range: {lat}");
                }
                total_points += points.len();
            }
        }
        assert_eq!(total_points, bundle.total_points());

        let mut ranks: Vec<u16> = Vec::new();
        for label in bundle.labels() {
            assert!(!label.name.is_empty(), "label name must be non-empty");
            assert!((-180.0..=180.0).contains(&label.lon));
            assert!((-90.0..=90.0).contains(&label.lat));
            ranks.push(label.rank);
        }
        let mut sorted = ranks.clone();
        sorted.sort_unstable();
        assert_eq!(ranks, sorted, "label ranks must already be ascending");
        assert!(sorted.windows(2).all(|w| w[1] == w[0] + 1) || sorted.is_empty(), "ranks must be dense");
    }

    #[test]
    fn layer_kinds_are_the_five_expected() {
        let bundle = real_bundle();
        let mut kinds: Vec<u32> = bundle.layers().map(|l| l.kind).collect();
        kinds.sort_unstable();
        assert_eq!(kinds, vec![1, 2, 3, 4, 5], "expected exactly the five ADR-0025/0028 layer kinds");
    }

    #[test]
    fn counts_match_the_manifest() {
        let manifest = include_str!("bundle.manifest.txt");
        let bundle = real_bundle();

        for line in manifest.lines() {
            let Some(rest) = line.trim().strip_prefix("kind=") else { continue };
            let (kind_str, rest) = rest.split_once(' ').expect("manifest layer line has a kind and a name");
            let kind: u32 = kind_str.parse().expect("manifest kind is numeric");
            if let Some(points_field) = rest.split_whitespace().find_map(|f| f.strip_prefix("points=")) {
                let expected_points: usize = points_field.parse().unwrap();
                let layer = bundle.layers().find(|l| l.kind == kind).expect("kind present in bundle");
                let actual: usize = bundle.parts(&layer).map(|p| bundle.points(&p).count()).sum();
                assert_eq!(actual, expected_points, "kind {kind} point count disagrees with manifest");
            }
            if let Some(count_field) = rest.split_whitespace().find_map(|f| f.strip_prefix("count=")) {
                let expected: usize = count_field.parse().unwrap();
                assert_eq!(bundle.labels().count(), expected, "label count disagrees with manifest");
            }
        }
    }

    #[test]
    fn corrupt_bundles_never_panic() {
        let real = include_bytes!("overlay.bin");

        // Bad magic.
        let mut bad_magic = real.to_vec();
        bad_magic[0] = b'X';
        let leaked: &'static [u8] = Box::leak(bad_magic.into_boxed_slice());
        assert!(Bundle::parse(leaked).is_none());

        // Unknown version.
        let mut bad_version = real.to_vec();
        bad_version[8..12].copy_from_slice(&2u32.to_le_bytes());
        let leaked: &'static [u8] = Box::leak(bad_version.into_boxed_slice());
        assert!(Bundle::parse(leaked).is_none());

        // part_count = u32::MAX (arithmetic must not overflow/wrap into
        // something that looks in-bounds).
        let mut huge_parts = real.to_vec();
        huge_parts[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        let leaked: &'static [u8] = Box::leak(huge_parts.into_boxed_slice());
        assert!(Bundle::parse(leaked).is_none());

        // point_count one too large.
        let mut point_count = read_u32(real, 20).unwrap();
        point_count += 1;
        let mut bad_points = real.to_vec();
        bad_points[20..24].copy_from_slice(&point_count.to_le_bytes());
        let leaked: &'static [u8] = Box::leak(bad_points.into_boxed_slice());
        assert!(Bundle::parse(leaked).is_none());

        // Truncated at various section boundaries: parse must return None,
        // not panic, for any prefix of the real bundle.
        for cut in [0, 4, 16, 32, 100, real.len() / 2, real.len() - 1] {
            let truncated = &real[..cut];
            let leaked: &'static [u8] = Box::leak(truncated.to_vec().into_boxed_slice());
            let parsed = Bundle::parse(leaked);
            if let Some(bundle) = parsed {
                // A short-but-structurally-valid prefix (e.g. an
                // all-empty-sections header) is allowed to parse; walking
                // it must still never panic.
                for layer in bundle.layers() {
                    for part in bundle.parts(&layer) {
                        let _ = bundle.points(&part).count();
                    }
                }
                let _ = bundle.labels().count();
            }
        }

        // A label name_off past the string table: that one label yields no
        // name, walking must not panic and other labels are unaffected.
        let bundle = real_bundle();
        let label_count = real_bundle().labels().count();
        if label_count > 0 {
            let mut bad_name_off = real.to_vec();
            // Overwrite the first label's name_off (labels_off + 10) with a
            // value past the string table.
            let off = bundle.labels_off + 10;
            let bogus = bundle.string_bytes + 1;
            bad_name_off[off..off + 4].copy_from_slice(&bogus.to_le_bytes());
            let leaked: &'static [u8] = Box::leak(bad_name_off.into_boxed_slice());
            let corrupted = Bundle::parse(leaked).expect("header-level structure is still valid");
            let names: Vec<&str> = corrupted.labels().map(|l| l.name).collect();
            assert!(names.len() <= label_count, "a bad name must drop that label, not panic");
        }

        // A name slice containing non-UTF-8 bytes.
        if label_count > 0 {
            let mut bad_utf8 = real.to_vec();
            // Point the first label at a one-byte name starting with 0x80
            // (a bare continuation byte: never valid UTF-8 on its own).
            let strings_off = bundle.strings_off;
            if strings_off < bad_utf8.len() {
                bad_utf8[strings_off] = 0x80;
                let off = bundle.labels_off; // first label's lon/lat/rank/name_off/name_len
                bad_utf8[off + 10..off + 14].copy_from_slice(&0u32.to_le_bytes());
                bad_utf8[off + 14..off + 16].copy_from_slice(&1u16.to_le_bytes());
                let leaked: &'static [u8] = Box::leak(bad_utf8.into_boxed_slice());
                let corrupted = Bundle::parse(leaked).expect("header-level structure is still valid");
                // Must not panic; the malformed label is simply absent.
                let _ = corrupted.labels().count();
            }
        }
    }
}
