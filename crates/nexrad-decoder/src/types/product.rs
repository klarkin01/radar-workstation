#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProductKind {
    Ref,
    Vel,
    SpectrumWidth,
    Zdr,
    Phi,
    Rho,
    Cfp,
}

/// Number of [`ProductKind`] variants. Backs [`ProductMap`]'s fixed storage.
const PRODUCT_KIND_COUNT: usize = 7;

impl ProductKind {
    /// Map an ICD block_id (e.g. `b"DREF"`) to the corresponding variant.
    pub fn from_block_id(id: &[u8; 4]) -> Option<Self> {
        match id {
            b"DREF" => Some(Self::Ref),
            b"DVEL" => Some(Self::Vel),
            b"DSW " => Some(Self::SpectrumWidth),
            b"DZDR" => Some(Self::Zdr),
            b"DPHI" => Some(Self::Phi),
            b"DRHO" => Some(Self::Rho),
            b"DCFP" => Some(Self::Cfp),
            _ => None,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Ref => 0,
            Self::Vel => 1,
            Self::SpectrumWidth => 2,
            Self::Zdr => 3,
            Self::Phi => 4,
            Self::Rho => 5,
            Self::Cfp => 6,
        }
    }
}

const ALL_PRODUCT_KINDS: [ProductKind; PRODUCT_KIND_COUNT] = [
    ProductKind::Ref,
    ProductKind::Vel,
    ProductKind::SpectrumWidth,
    ProductKind::Zdr,
    ProductKind::Phi,
    ProductKind::Rho,
    ProductKind::Cfp,
];

/// Per-radial product storage, keyed by [`ProductKind`].
///
/// Backed by a fixed array instead of a `HashMap` — the key space is a small
/// closed enum, so array indexing removes a per-radial heap allocation and
/// SipHash lookup from the hottest path in the decoder (thousands of radials
/// per volume).
#[derive(Debug, Clone, Default)]
pub struct ProductMap {
    slots: [Option<ProductData>; PRODUCT_KIND_COUNT],
}

impl ProductMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, kind: ProductKind, data: ProductData) {
        self.slots[kind.index()] = Some(data);
    }

    pub fn get(&self, kind: &ProductKind) -> Option<&ProductData> {
        self.slots[kind.index()].as_ref()
    }

    pub fn contains_key(&self, kind: &ProductKind) -> bool {
        self.slots[kind.index()].is_some()
    }

    pub fn keys(&self) -> impl Iterator<Item = &ProductKind> + '_ {
        self.iter().map(|(k, _)| k)
    }

    pub fn iter(&self) -> ProductMapIter<'_> {
        ProductMapIter { map: self, idx: 0 }
    }
}

pub struct ProductMapIter<'a> {
    map: &'a ProductMap,
    idx: usize,
}

impl<'a> Iterator for ProductMapIter<'a> {
    type Item = (&'a ProductKind, &'a ProductData);

    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < PRODUCT_KIND_COUNT {
            let kind = &ALL_PRODUCT_KINDS[self.idx];
            let slot = &self.map.slots[self.idx];
            self.idx += 1;
            if let Some(data) = slot {
                return Some((kind, data));
            }
        }
        None
    }
}

impl<'a> IntoIterator for &'a ProductMap {
    type Item = (&'a ProductKind, &'a ProductData);
    type IntoIter = ProductMapIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Debug, Clone)]
pub struct ProductData {
    pub gate_count: u16,
    pub first_gate_m: u16,
    pub gate_width_m: u16,
    /// Bits per gate: 8 for most products, 16 for ZDR and PHI.
    pub word_size: u8,
    pub scale: f32,
    pub offset: f32,
    /// Raw gate bytes. 16-bit products store each gate as two consecutive big-endian bytes.
    pub data: Vec<u8>,
}

impl ProductData {
    /// Raw encoded value at `index`. Returns `None` if `index` is out of range.
    pub fn raw_gate(&self, index: usize) -> Option<u16> {
        match self.word_size {
            8 => self.data.get(index).map(|&b| b as u16),
            16 => {
                let i = index * 2;
                let hi = *self.data.get(i)?;
                let lo = *self.data.get(i + 1)?;
                Some(u16::from_be_bytes([hi, lo]))
            }
            _ => None,
        }
    }

    /// Physical value at `index` in calibrated units (dBZ, m/s, dB, etc.).
    /// Returns `None` for below-SNR (raw=0) and range-folded (raw=1) gates.
    pub fn physical_value(&self, index: usize) -> Option<f32> {
        let raw = self.raw_gate(index)?;
        // ICD: raw values 0 and 1 are reserved flag codes, not data.
        if raw < 2 {
            return None;
        }
        Some((raw as f32 - self.offset) / self.scale)
    }
}
