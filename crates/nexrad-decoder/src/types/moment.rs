#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MomentKind {
    Ref,
    Vel,
    Sw,
    Zdr,
    Phi,
    Rho,
    Cfp,
}

impl MomentKind {
    /// Map an ICD block_id (e.g. `b"DREF"`) to the corresponding variant.
    pub fn from_block_id(id: &[u8; 4]) -> Option<Self> {
        match id {
            b"DREF" => Some(Self::Ref),
            b"DVEL" => Some(Self::Vel),
            b"DSW " => Some(Self::Sw),
            b"DZDR" => Some(Self::Zdr),
            b"DPHI" => Some(Self::Phi),
            b"DRHO" => Some(Self::Rho),
            b"DCFP" => Some(Self::Cfp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MomentData {
    pub gate_count: u16,
    pub first_gate_m: u16,
    pub gate_width_m: u16,
    /// Bits per gate: 8 for most moments, 16 for ZDR and PHI.
    pub word_size: u8,
    pub scale: f32,
    pub offset: f32,
    /// Raw gate bytes. 16-bit moments store each gate as two consecutive big-endian bytes.
    pub data: Vec<u8>,
}

impl MomentData {
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
