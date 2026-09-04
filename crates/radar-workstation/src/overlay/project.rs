//! Projecting the bundled geometry into the render loop's world frame
//! (S5-c/S5-e, ADR-0025 §4). Pure: no GPU, no window. Called once, at
//! renderer init, behind [`project`] — a plain function, so moving the call
//! onto `spawn_blocking` later (Stage 7, once the site can change at
//! runtime) touches only the call site, not this code.

use crate::compute::geometry::az_eq_project;
use crate::event::Event;
use crate::sites::Site;

use super::Bundle;

/// The bundle's layer kind for city/site labels (§5.1's layer-kind table).
/// It carries no parts of its own — its content is the label index, walked
/// separately by [`Bundle::labels`].
const LABEL_LAYER_KIND: u32 = 5;
/// Geometry layer kinds this build knows how to draw (§5.1's table, kinds
/// 1–4). Anything else is a future bundle format's addition; skip it with
/// an event rather than treating it as an error (§5.2).
const KNOWN_GEOMETRY_KINDS: std::ops::RangeInclusive<u32> = 1..=4;

/// One geometry layer's slice of [`Projected::indices`] — a line-list draw
/// range, not a vertex range (S5-d: vertices are shared across every layer
/// in one buffer; each layer only owns a contiguous run of index pairs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedLayer {
    pub kind: u32,
    pub index_range: std::ops::Range<u32>,
}

/// One label candidate, already in world metres, ready to hand to
/// `render::labels::select` (§9.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectedLabel {
    pub world: [f32; 2],
    pub rank: u16,
    pub name: &'static str,
}

/// The whole bundle, projected once against one site. `vertices`/`indices`
/// are consumed by GPU buffer creation and dropped (ADR-0025 §4: "the
/// CPU-side projected copy is dropped after upload") — nothing reachable
/// from the per-frame render path holds a `Projected`.
pub struct Projected {
    /// World metres, one entry per projected bundle point.
    pub vertices: Vec<[f32; 2]>,
    /// Line-list pairs (`[v, v+1, v+1, v+2, …]` per part) indexing into
    /// `vertices`. Derived here, never baked (ADR-0025 §4).
    pub indices: Vec<u32>,
    pub layers: Vec<ProjectedLayer>,
    pub labels: Vec<ProjectedLabel>,
}

/// Projects every geometry layer and every label in `bundle` into world
/// metres centred on `site`, in bundle order. No culling — ADR-0025 §3
/// explicitly declines to cull part bounding boxes in v1.0, since
/// projecting everything avoids an "overlays vanish when panned far" class
/// of bug entirely, and the resulting buffers are a small fraction of the
/// GPU budget (ADR-0029).
///
/// Returns any events worth surfacing (an unrecognized layer kind) rather
/// than reporting them itself — this function has no `AppState` to report
/// into, matching `config::load`/`palette::load_all`'s `(T, Vec<Event>)`
/// shape elsewhere in this crate.
pub fn project(bundle: &'static Bundle, site: &Site) -> (Projected, Vec<Event>) {
    let mut vertices: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut layers: Vec<ProjectedLayer> = Vec::new();
    let mut labels: Vec<ProjectedLabel> = Vec::new();
    let mut events = Vec::new();

    for layer in bundle.layers() {
        if layer.kind == LABEL_LAYER_KIND {
            for label in bundle.labels() {
                let (x, y) = az_eq_project(site.lat, site.lon, label.lat, label.lon);
                labels.push(ProjectedLabel { world: [x as f32, y as f32], rank: label.rank, name: label.name });
            }
            continue;
        }
        if !KNOWN_GEOMETRY_KINDS.contains(&layer.kind) {
            events.push(Event::OverlayLayerUnknownKind { kind: layer.kind });
            continue;
        }

        let index_start = indices.len() as u32;
        for part in bundle.parts(&layer) {
            let start_vertex = vertices.len() as u32;
            let mut n: u32 = 0;
            for (lon, lat) in bundle.points(&part) {
                let (x, y) = az_eq_project(site.lat, site.lon, lat, lon);
                vertices.push([x as f32, y as f32]);
                n += 1;
            }
            if n < 2 {
                // A degenerate part (0 or 1 points after a corrupt/edge
                // read) contributes no line segments — drop its vertices
                // rather than leave an orphaned single point in the buffer.
                vertices.truncate(start_vertex as usize);
                continue;
            }
            for i in 0..n - 1 {
                indices.push(start_vertex + i);
                indices.push(start_vertex + i + 1);
            }
        }
        layers.push(ProjectedLayer { kind: layer.kind, index_range: index_start..indices.len() as u32 });
    }

    (Projected { vertices, indices, layers, labels }, events)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: Site = Site { id: "KDOX", name: "Dover", state: "DE", lat: 38.8258, lon: -75.4401, elevation_m: 15 };

    /// Hand-builds a minimal `overlay.bin`-format bundle in memory: two
    /// geometry layers (one two-part, one degenerate one-point part) plus
    /// one label, leaked to `'static` the same way `mod.rs`'s corrupt-
    /// bundle tests do.
    fn synthetic_bundle() -> &'static Bundle {
        fn fixed(deg: f64) -> i32 {
            (deg * 1e7).round() as i32
        }

        // Layer 1 (kind 1): one part, the site itself plus a point 1 degree
        // north — three points so the index pattern is checkable.
        let part_a: Vec<(f64, f64)> = vec![(SITE.lon, SITE.lat), (SITE.lon, SITE.lat + 1.0), (SITE.lon + 1.0, SITE.lat)];
        // Layer 2 (kind 2): one degenerate part (a single point) that must
        // contribute zero indices and not panic.
        let part_b: Vec<(f64, f64)> = vec![(SITE.lon, SITE.lat)];

        let layers_meta = [(1u32, vec![&part_a]), (2u32, vec![&part_b])];

        let mut parts_bytes = Vec::new();
        let mut points_bytes = Vec::new();
        let mut layer_table = Vec::new();
        let mut part_cursor = 0u32;
        let mut point_cursor = 0u32;

        for (kind, parts) in &layers_meta {
            let first_part = part_cursor;
            for part in parts {
                let min_lon = part.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
                let max_lon = part.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
                let min_lat = part.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
                let max_lat = part.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
                parts_bytes.extend_from_slice(&point_cursor.to_le_bytes());
                parts_bytes.extend_from_slice(&(part.len() as u32).to_le_bytes());
                parts_bytes.extend_from_slice(&fixed(min_lon).to_le_bytes());
                parts_bytes.extend_from_slice(&fixed(min_lat).to_le_bytes());
                parts_bytes.extend_from_slice(&fixed(max_lon).to_le_bytes());
                parts_bytes.extend_from_slice(&fixed(max_lat).to_le_bytes());
                for &(lon, lat) in part.iter() {
                    points_bytes.extend_from_slice(&fixed(lon).to_le_bytes());
                    points_bytes.extend_from_slice(&fixed(lat).to_le_bytes());
                }
                point_cursor += part.len() as u32;
                part_cursor += 1;
            }
            layer_table.extend_from_slice(&kind.to_le_bytes());
            layer_table.extend_from_slice(&first_part.to_le_bytes());
            layer_table.extend_from_slice(&(parts.len() as u32).to_le_bytes());
        }
        // Label layer (kind 5): zero parts, one label at the site itself.
        layer_table.extend_from_slice(&5u32.to_le_bytes());
        layer_table.extend_from_slice(&0u32.to_le_bytes());
        layer_table.extend_from_slice(&0u32.to_le_bytes());

        let name = b"KDOX";
        let mut label_index = Vec::new();
        label_index.extend_from_slice(&fixed(SITE.lon).to_le_bytes());
        label_index.extend_from_slice(&fixed(SITE.lat).to_le_bytes());
        label_index.extend_from_slice(&0u16.to_le_bytes()); // rank
        label_index.extend_from_slice(&0u32.to_le_bytes()); // name_off
        label_index.extend_from_slice(&(name.len() as u16).to_le_bytes());

        let mut bytes = Vec::new();
        bytes.extend_from_slice(super::super::MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes()); // version
        bytes.extend_from_slice(&3u32.to_le_bytes()); // layer_count (2 geometry + 1 label)
        bytes.extend_from_slice(&part_cursor.to_le_bytes());
        bytes.extend_from_slice(&point_cursor.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes()); // label_count
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes()); // string_bytes
        bytes.extend_from_slice(&layer_table);
        bytes.extend_from_slice(&parts_bytes);
        bytes.extend_from_slice(&points_bytes);
        bytes.extend_from_slice(&label_index);
        bytes.extend_from_slice(name);

        let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        Box::leak(Box::new(Bundle::parse(leaked).expect("synthetic bundle must be well-formed")))
    }

    #[test]
    fn the_sites_own_coordinates_land_at_the_origin() {
        let bundle = synthetic_bundle();
        let (projected, events) = project(bundle, &SITE);
        assert!(events.is_empty());
        // part_a's first vertex is the site itself.
        let [x, y] = projected.vertices[0];
        assert!(x.abs() < 1.0 && y.abs() < 1.0, "site's own point should project near the origin: ({x},{y})");
    }

    #[test]
    fn a_three_point_part_produces_the_expected_line_list_indices() {
        let bundle = synthetic_bundle();
        let (projected, _events) = project(bundle, &SITE);
        let kind1 = projected.layers.iter().find(|l| l.kind == 1).expect("kind 1 present");
        let idx = &projected.indices[kind1.index_range.start as usize..kind1.index_range.end as usize];
        // Three points -> two segments -> [0,1, 1,2].
        assert_eq!(idx, &[0, 1, 1, 2]);
    }

    #[test]
    fn a_one_point_part_produces_zero_indices_and_no_panic() {
        let bundle = synthetic_bundle();
        let (projected, _events) = project(bundle, &SITE);
        let kind2 = projected.layers.iter().find(|l| l.kind == 2).expect("kind 2 present");
        assert_eq!(kind2.index_range.start, kind2.index_range.end, "a degenerate part must emit no indices");
    }

    /// §12.1: computes the buffer sizes from the *committed* bundle and
    /// asserts they are under 16 MB — a margin over ADR-0029's measured
    /// 11.46 MB, not a re-derivation of it. Catches a bundle regenerated
    /// without the ε = 30 m primary-roads tolerance.
    #[test]
    fn overlay_vertex_and_index_bytes_are_within_the_gpu_budget() {
        let bundle = super::super::bundled().expect("committed bundle must parse");
        let site = crate::sites::by_id("KDOX").expect("KDOX is bundled");
        let (projected, events) = project(bundle, site);
        assert!(events.is_empty(), "the committed bundle must carry only known layer kinds: {events:?}");

        let vertex_bytes = projected.vertices.len() * std::mem::size_of::<[f32; 2]>();
        let index_bytes = projected.indices.len() * std::mem::size_of::<u32>();
        let total = vertex_bytes + index_bytes;
        const BUDGET: usize = 16 * 1024 * 1024;
        assert!(total < BUDGET, "overlay GPU buffers are {total} bytes, over the {BUDGET} byte budget");
    }

    #[test]
    fn the_label_layer_contributes_no_geometry_layer_entry() {
        let bundle = synthetic_bundle();
        let (projected, _events) = project(bundle, &SITE);
        assert!(projected.layers.iter().all(|l| l.kind != 5), "the label kind must not appear as a drawable layer");
        assert_eq!(projected.labels.len(), 1);
        assert_eq!(projected.labels[0].name, "KDOX");
    }
}
