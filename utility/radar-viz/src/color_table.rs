/// Maps a physical radar value (or its absence) to an RGBA pixel.
///
/// Entries are sorted ascending by threshold. A value `v` maps to the color
/// of the last entry whose threshold is <= v. Values below the first threshold
/// use `below_min`. Below-SNR gates use `no_data`. Range-folded gates (ICD
/// raw value 1) use `range_fold` so they are visually distinct from true
/// no-data.
pub struct ColorTable {
    entries: Vec<(f32, [u8; 4])>,
    below_min: [u8; 4],
    pub no_data: [u8; 4],
    pub range_fold: [u8; 4],
}

impl ColorTable {
    pub fn new(
        mut entries: Vec<(f32, [u8; 4])>,
        below_min: [u8; 4],
        no_data: [u8; 4],
        range_fold: [u8; 4],
    ) -> Self {
        entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        Self { entries, below_min, no_data, range_fold }
    }

    pub fn lookup(&self, value: Option<f32>) -> [u8; 4] {
        let v = match value {
            None => return self.no_data,
            Some(v) => v,
        };
        match self.entries.partition_point(|&(t, _)| t <= v) {
            0 => self.below_min,
            i => self.entries[i - 1].1,
        }
    }

    /// NWS standard 16-step reflectivity palette (dBZ).
    pub fn nws_reflectivity() -> Self {
        Self::new(
            vec![
                ( 5.0, [118, 118, 118, 255]),
                (10.0, [  4, 233, 231, 255]),
                (15.0, [  1, 159, 244, 255]),
                (20.0, [  3,   0, 244, 255]),
                (25.0, [  2, 253,   2, 255]),
                (30.0, [  1, 197,   1, 255]),
                (35.0, [  0, 142,   0, 255]),
                (40.0, [253, 248,   2, 255]),
                (45.0, [229, 188,   0, 255]),
                (50.0, [253, 149,   0, 255]),
                (55.0, [253,   0,   0, 255]),
                (60.0, [212,   0,   0, 255]),
                (65.0, [188,   0,   0, 255]),
                (70.0, [248,   0, 253, 255]),
                (75.0, [152,  84, 198, 255]),
            ],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [160, 160, 160, 255],
        )
    }

    /// NWS-style 16-step radial velocity palette (m/s). Negative = toward, positive = away.
    pub fn nws_velocity() -> Self {
        Self::new(
            vec![
                (-64.0, [  0,   0, 200, 255]),
                (-54.0, [  0,  40, 255, 255]),
                (-44.0, [  0, 160, 255, 255]),
                (-34.0, [  0, 240, 255, 255]),
                (-24.0, [  0, 255, 176, 255]),
                (-14.0, [  0, 200,  80, 255]),
                ( -4.0, [  0, 100,   0, 255]),
                ( -1.0, [100, 100, 100, 255]),
                (  1.0, [100, 100,   0, 255]),
                (  4.0, [170, 170,   0, 255]),
                ( 14.0, [255, 255,   0, 255]),
                ( 24.0, [255, 160,   0, 255]),
                ( 34.0, [255,  80,   0, 255]),
                ( 44.0, [255,   0,   0, 255]),
                ( 54.0, [200,   0,   0, 255]),
            ],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [160, 160, 160, 255],
        )
    }

    /// Grayscale palette for spectrum width (m/s).
    pub fn spectrum_width() -> Self {
        Self::new(
            vec![
                ( 0.0, [ 20,  20,  20, 255]),
                ( 1.0, [ 50,  50,  50, 255]),
                ( 2.0, [ 80,  80,  80, 255]),
                ( 3.0, [110, 110, 110, 255]),
                ( 4.0, [140, 140, 140, 255]),
                ( 5.0, [170, 170, 170, 255]),
                ( 6.0, [200, 200, 200, 255]),
                ( 7.0, [220, 220, 220, 255]),
                ( 8.0, [235, 235, 235, 255]),
                (10.0, [255, 255, 255, 255]),
            ],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [160, 160, 160, 255],
        )
    }
}
