//! GRLevelX `.pal` colour-table format (S3-W3, Q11, ADR-0021): a documented
//! subset, parsed by a workspace-local parser — the same posture as
//! `config` (ADR-0019), `http-ingest` (ADR-0014), and `nexrad-decoder`
//! (ADR-0008): an untrusted-input parser on a must-not-crash path is owned
//! by this project and fuzzed with the shared seeded mutator
//! (`crates/fuzz-support`), not pulled in as a dependency.
//!
//! **Loading never fails.** [`parse`] returns `(Palette, Vec<Event>)` — an
//! unparseable line or unknown directive is skipped and reported, never
//! fatal (Stability as Ethics; the same discipline `config::load` and
//! FR-CP-3 impose on configuration). [`load_all`] extends that guarantee to
//! the whole product set: every [`DisplayProduct`] always resolves to a
//! palette, bundled or user-supplied — there is no failure mode in which a
//! product has no colours.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::compute::DisplayProduct;
use crate::event::Event;

#[derive(Debug, Clone, Copy)]
struct PaletteEntry {
    threshold: f32,
    from: [u8; 4],
    /// `Some` for a gradient entry, whose colour ramps linearly to the next
    /// entry's threshold. `None` for a solid step.
    to: Option<[u8; 4]>,
}

#[derive(Debug, Clone)]
pub struct Palette {
    pub product: DisplayProduct,
    pub units: String,
    /// Legend tick spacing; Stage 4's colour scale consumes it.
    pub step: Option<f32>,
    /// Ascending by threshold.
    entries: Vec<PaletteEntry>,
    pub range_folded: [u8; 4],
    pub no_data: [u8; 4],
}

impl Palette {
    fn empty(product: DisplayProduct) -> Self {
        Self {
            product,
            units: String::new(),
            step: None,
            entries: Vec::new(),
            // GRLevelX convention: range-folded is a neutral grey, no-data
            // is fully transparent (FR-DR-4) — both overridable by RF:/ND:.
            range_folded: [160, 160, 160, 255],
            no_data: [0, 0, 0, 0],
        }
    }

    pub fn has_entries(&self) -> bool {
        !self.entries.is_empty()
    }

    /// The first and last entry thresholds (S4-W6 §9.2): the physical-value
    /// span the legend strip samples across. `None` when the palette has no
    /// entries — `load_all` guarantees that never happens for a bundled or
    /// well-formed user palette, but the accessor is honest about the empty
    /// case rather than inventing a range.
    pub fn threshold_range(&self) -> Option<(f32, f32)> {
        match (self.entries.first(), self.entries.last()) {
            (Some(first), Some(last)) => Some((first.threshold, last.threshold)),
            _ => None,
        }
    }

    /// Colour for a physical value. Below the first threshold (or when the
    /// palette has no entries at all) is [`Self::no_data`] — FR-DR-4: below
    /// the minimum displayable threshold renders fully transparent by
    /// default.
    pub fn sample(&self, value: f32) -> [u8; 4] {
        let idx = self.entries.partition_point(|e| e.threshold <= value);
        if idx == 0 {
            return self.no_data;
        }
        let entry = &self.entries[idx - 1];
        match entry.to {
            None => entry.from,
            Some(to) => {
                let Some(next) = self.entries.get(idx) else { return entry.from };
                let span = next.threshold - entry.threshold;
                if span <= 0.0 {
                    return entry.from;
                }
                let t = ((value - entry.threshold) / span).clamp(0.0, 1.0);
                lerp_color(entry.from, to, t)
            }
        }
    }
}

fn lerp_color(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round().clamp(0.0, 255.0) as u8)
}

/// Parse a `.pal` file's text for `product`. Never fails — see this
/// module's top-level doc comment.
pub fn parse(text: &str, product: DisplayProduct) -> (Palette, Vec<Event>) {
    let mut palette = Palette::empty(product);
    let mut events = Vec::new();

    for (line_no, raw_line) in text.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if !apply_directive(&mut palette, line) {
            events.push(Event::PaletteLineUnparseable { product, line: line_no + 1 });
        }
    }

    palette.entries.sort_by(|a, b| a.threshold.partial_cmp(&b.threshold).unwrap_or(std::cmp::Ordering::Equal));
    (palette, events)
}

fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// Applies one non-empty, comment-stripped line. Returns `false` if the
/// line was not a recognized, well-formed directive.
fn apply_directive(palette: &mut Palette, line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(directive) = parts.next() else { return false };
    let rest: Vec<&str> = parts.collect();

    match directive {
        "Product:" => true, // informational; the caller already fixed `product`
        "Units:" => {
            palette.units = rest.join(" ");
            true
        }
        "Step:" => match rest.first().and_then(|s| s.parse::<f32>().ok()) {
            Some(v) => {
                palette.step = Some(v);
                true
            }
            None => false,
        },
        "Color:" => apply_color(palette, &rest, false, false),
        "Color4:" => apply_color(palette, &rest, true, false),
        "SolidColor:" => apply_color(palette, &rest, false, true),
        "SolidColor4:" => apply_color(palette, &rest, true, true),
        "RF:" => apply_fixed_color(&rest).is_some_and(|c| {
            palette.range_folded = c;
            true
        }),
        "ND:" => apply_fixed_color(&rest).is_some_and(|c| {
            palette.no_data = c;
            true
        }),
        _ => false,
    }
}

/// Parses `Color:`/`Color4:`/`SolidColor:`/`SolidColor4:`'s shared shape:
/// a leading threshold, one colour (3 or 4 numbers depending on `has_alpha`),
/// and — unless `solid_only` — an optional second colour of the same width
/// making this a gradient entry.
fn apply_color(palette: &mut Palette, rest: &[&str], has_alpha: bool, solid_only: bool) -> bool {
    let Some(nums) = parse_numbers(rest) else { return false };
    let width = if has_alpha { 4 } else { 3 };
    let solid_len = 1 + width;
    let gradient_len = 1 + width * 2;

    if nums.len() == solid_len {
        let threshold = nums[0];
        let from = color_from(&nums[1..], has_alpha);
        palette.entries.push(PaletteEntry { threshold, from, to: None });
        true
    } else if !solid_only && nums.len() == gradient_len {
        let threshold = nums[0];
        let from = color_from(&nums[1..1 + width], has_alpha);
        let to = color_from(&nums[1 + width..], has_alpha);
        palette.entries.push(PaletteEntry { threshold, from, to: Some(to) });
        true
    } else {
        false
    }
}

/// Parses `RF:`/`ND:`'s shared shape: a colour with no leading threshold,
/// alpha optional (defaulting to fully opaque for `RF:`, fully transparent
/// for `ND:` — the caller decides which by leaving the field at
/// [`Palette::empty`]'s default when alpha is omitted... in practice both
/// directives here take an explicit default via the 3-number branch below,
/// matching the GRLevelX convention that RF is normally opaque and ND is
/// normally not drawn at all).
fn apply_fixed_color(rest: &[&str]) -> Option<[u8; 4]> {
    let nums = parse_numbers(rest)?;
    match nums.len() {
        3 => Some([clamp_u8(nums[0]), clamp_u8(nums[1]), clamp_u8(nums[2]), 255]),
        4 => Some([clamp_u8(nums[0]), clamp_u8(nums[1]), clamp_u8(nums[2]), clamp_u8(nums[3])]),
        _ => None,
    }
}

fn color_from(nums: &[f32], has_alpha: bool) -> [u8; 4] {
    if has_alpha {
        [clamp_u8(nums[0]), clamp_u8(nums[1]), clamp_u8(nums[2]), clamp_u8(nums[3])]
    } else {
        [clamp_u8(nums[0]), clamp_u8(nums[1]), clamp_u8(nums[2]), 255]
    }
}

fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn parse_numbers(tokens: &[&str]) -> Option<Vec<f32>> {
    tokens.iter().map(|t| t.parse::<f32>().ok()).collect()
}

/// One evaluation per possible cell value (S3-a): this is the entirety of
/// the application's colour mapping — 256 evaluations per product, not one
/// per gate. `scale`/`offset` are a grid's *effective* values (§4.4), so
/// this needs nothing else to invert a cell back to a physical value.
pub type ColorLut = [[u8; 4]; 256];

pub fn compile_lut(palette: &Palette, scale: f32, offset: f32) -> ColorLut {
    let mut lut = [[0u8; 4]; 256];
    lut[0] = palette.no_data;
    lut[1] = palette.range_folded;
    for raw in 2..=255u16 {
        let physical = (raw as f32 - offset) / scale;
        lut[raw as usize] = palette.sample(physical);
    }
    lut
}

/// The bundled default palette for `product`, with no user override applied
/// — `load_all` is the production path (it checks for an override first);
/// this is the seam `utility/radar-viz`'s grid render path uses to draw
/// with the same palettes the application ships, without duplicating them.
pub fn bundled_default(product: DisplayProduct) -> Palette {
    parse(bundled_pal_text(product), product).0
}

fn bundled_pal_text(product: DisplayProduct) -> &'static str {
    match product {
        DisplayProduct::Reflectivity => include_str!("palettes/reflectivity.pal"),
        DisplayProduct::Velocity => include_str!("palettes/velocity.pal"),
        DisplayProduct::SpectrumWidth => include_str!("palettes/spectrum_width.pal"),
        DisplayProduct::Zdr => include_str!("palettes/zdr.pal"),
        DisplayProduct::Cc => include_str!("palettes/cc.pal"),
        DisplayProduct::EchoTops => include_str!("palettes/echo_tops.pal"),
        DisplayProduct::Vil => include_str!("palettes/vil.pal"),
    }
}

fn user_palette_path(product: DisplayProduct) -> Option<PathBuf> {
    crate::paths::data_dir().map(|dir| dir.join("palettes").join(format!("{product}.pal")))
}

/// `None` for "use the bundled default" — either no user override exists
/// (the common case, silent, matching `config::load`'s missing-file
/// treatment) or the file couldn't be read (reported).
fn read_user_palette(product: DisplayProduct, events: &mut Vec<Event>) -> Option<String> {
    let path = user_palette_path(product)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            events.push(Event::UserPaletteUnreadable {
                product,
                path: path.display().to_string(),
                reason: e.to_string(),
            });
            None
        }
    }
}

/// Every [`DisplayProduct`] resolved to a palette: bundled defaults,
/// overridden by a readable, well-formed user palette from
/// `paths::data_dir()/palettes/<product>.pal` (FR-CT-3). A malformed user
/// palette (one that parses to zero colour entries) falls back to the
/// bundled default and is reported — never a colourless product.
pub fn load_all() -> (BTreeMap<DisplayProduct, Palette>, Vec<Event>) {
    let mut palettes = BTreeMap::new();
    let mut events = Vec::new();

    for product in DisplayProduct::ALL {
        let (bundled, bundled_events) = parse(bundled_pal_text(product), product);
        events.extend(bundled_events);

        let mut resolved = bundled;
        if let Some(user_text) = read_user_palette(product, &mut events) {
            let (user_palette, user_events) = parse(&user_text, product);
            events.extend(user_events);
            if user_palette.has_entries() {
                resolved = user_palette;
            } else if let Some(path) = user_palette_path(product) {
                events.push(Event::UserPaletteMalformed { product, path: path.display().to_string() });
            }
        }
        palettes.insert(product, resolved);
    }

    (palettes, events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_default_parses_with_entries_and_no_events() {
        for product in DisplayProduct::ALL {
            let (palette, events) = parse(bundled_pal_text(product), product);
            assert!(events.is_empty(), "{product} bundled palette reported unexpected events: {events:?}");
            assert!(palette.has_entries(), "{product} bundled palette must have colour entries");
        }
    }

    #[test]
    fn solid_color_step_function_matches_last_threshold_at_or_below() {
        let text = "SolidColor: 0 0 0 0\nSolidColor: 10 100 100 100\nSolidColor: 20 200 200 200\n";
        let (palette, events) = parse(text, DisplayProduct::Reflectivity);
        assert!(events.is_empty());
        assert_eq!(palette.sample(15.0), [100, 100, 100, 255]);
        assert_eq!(palette.sample(20.0), [200, 200, 200, 255]);
        assert_eq!(palette.sample(25.0), [200, 200, 200, 255]);
    }

    #[test]
    fn gradient_interpolates_at_an_exact_threshold_and_midway() {
        let text = "Color: 0 0 0 0 100 100 100\nColor: 10 200 200 200\n";
        let (palette, events) = parse(text, DisplayProduct::Reflectivity);
        assert!(events.is_empty());
        assert_eq!(palette.sample(0.0), [0, 0, 0, 255], "exact lower threshold");
        assert_eq!(palette.sample(5.0), [50, 50, 50, 255], "midway through the gradient");
        assert_eq!(palette.sample(10.0), [200, 200, 200, 255], "at the next entry's own threshold");
    }

    #[test]
    fn value_below_the_first_threshold_is_transparent() {
        let text = "SolidColor: 0 10 10 10\n";
        let (palette, _events) = parse(text, DisplayProduct::Reflectivity);
        assert_eq!(palette.sample(-5.0), [0, 0, 0, 0]);
    }

    #[test]
    fn rf_and_nd_land_at_lut_indices_one_and_zero() {
        let text = "SolidColor: 0 10 10 10\nRF: 50 50 50\nND: 1 2 3 4\n";
        let (palette, events) = parse(text, DisplayProduct::Reflectivity);
        assert!(events.is_empty());
        let lut = compile_lut(&palette, 1.0, 0.0);
        assert_eq!(lut[0], [1, 2, 3, 4]);
        assert_eq!(lut[1], [50, 50, 50, 255]);
    }

    #[test]
    fn a_real_community_style_palette_parses() {
        // A representative excerpt in the documented subset, hand-written
        // in the style of a real GRLevelX reflectivity table.
        let text = "\
            Product: BR\n\
            Units: dBZ\n\
            Step: 5\n\
            Color4: 5 118 118 118 255 4 233 231 255\n\
            SolidColor: 70 248 0 253\n\
            RF: 160 160 160\n\
            ND: 0 0 0 0\n";
        let (palette, events) = parse(text, DisplayProduct::Reflectivity);
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(palette.units, "dBZ");
        assert_eq!(palette.step, Some(5.0));
        assert!(palette.has_entries());
    }

    #[test]
    fn unknown_directives_are_skipped_and_reported() {
        let text = "SolidColor: 0 1 1 1\nFutureDirective: 1 2 3\n";
        let (palette, events) = parse(text, DisplayProduct::Reflectivity);
        assert!(palette.has_entries(), "a following unknown directive must not discard prior entries");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Event::PaletteLineUnparseable { line: 2, .. }));
    }

    #[test]
    fn malformed_known_directive_is_skipped_and_reported() {
        let text = "SolidColor: not-a-number 1 1 1\n";
        let (_palette, events) = parse(text, DisplayProduct::Reflectivity);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn threshold_range_spans_first_to_last_entry() {
        let text = "SolidColor: -32 0 0 0\nSolidColor: 0 128 128 128\nSolidColor: 75 255 255 255\n";
        let (palette, _events) = parse(text, DisplayProduct::Reflectivity);
        assert_eq!(palette.threshold_range(), Some((-32.0, 75.0)));
    }

    #[test]
    fn threshold_range_of_an_empty_palette_is_none() {
        assert_eq!(Palette::empty(DisplayProduct::Reflectivity).threshold_range(), None);
    }

    #[test]
    fn compile_lut_evaluates_every_cell_value_once() {
        let text = "SolidColor: -32 0 0 0\nSolidColor: 0 128 128 128\n";
        let (palette, _events) = parse(text, DisplayProduct::Reflectivity);
        let lut = compile_lut(&palette, 2.0, 66.0);
        // raw=66 -> physical=(66-66)/2=0.0 -> the second entry's colour.
        assert_eq!(lut[66], [128, 128, 128, 255]);
    }

    /// §12's "palette parse time for the full bundled set at startup"
    /// measurement, kept as a regression guard like
    /// `config::tests::load_of_a_realistic_file_is_fast` rather than a
    /// one-off. This is on the < 2s first-render path from Stage 4 on.
    #[test]
    fn load_all_of_the_bundled_set_is_fast() {
        let start = std::time::Instant::now();
        let (palettes, events) = load_all();
        let elapsed = start.elapsed();

        println!("palette::load_all of the bundled set took {elapsed:?}");
        assert!(elapsed < std::time::Duration::from_millis(50), "took {elapsed:?}, expected well under 50ms");
        assert_eq!(palettes.len(), DisplayProduct::ALL.len(), "every product must resolve to a palette");
        assert!(events.is_empty(), "the bundled set itself must not report anything: {events:?}");
    }
}
