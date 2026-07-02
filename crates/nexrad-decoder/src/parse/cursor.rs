use crate::DecodeError;

/// Bounds-checked cursor over a borrowed byte slice. All reads advance the
/// internal position; random-access is done by creating a sub-cursor via
/// [`Cursor::at`]. Every read returns `Err(DecodeError::Truncated)` rather
/// than panicking on out-of-bounds access.
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    /// Create a cursor positioned at `offset` within `data`.
    pub fn at(data: &'a [u8], offset: usize) -> Result<Self, DecodeError> {
        if offset > data.len() {
            return Err(DecodeError::Truncated { context: "cursor::at" });
        }
        Ok(Cursor { data, pos: offset })
    }

    #[allow(dead_code)]
    pub fn position(&self) -> usize {
        self.pos
    }

    #[allow(dead_code)]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    #[allow(dead_code)]
    pub fn skip(&mut self, n: usize) -> Result<(), DecodeError> {
        let new_pos = self.pos
            .checked_add(n)
            .filter(|&p| p <= self.data.len())
            .ok_or(DecodeError::Truncated { context: "skip" })?;
        self.pos = new_pos;
        Ok(())
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or(DecodeError::Truncated { context: "read_u8" })?;
        self.pos += 1;
        Ok(b)
    }

    pub fn read_u16_be(&mut self) -> Result<u16, DecodeError> {
        let s = self
            .data
            .get(self.pos..self.pos + 2)
            .ok_or(DecodeError::Truncated { context: "read_u16_be" })?;
        self.pos += 2;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }

    pub fn read_i16_be(&mut self) -> Result<i16, DecodeError> {
        self.read_u16_be().map(|v| v as i16)
    }

    pub fn read_u32_be(&mut self) -> Result<u32, DecodeError> {
        let s = self
            .data
            .get(self.pos..self.pos + 4)
            .ok_or(DecodeError::Truncated { context: "read_u32_be" })?;
        self.pos += 4;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub fn read_f32_be(&mut self) -> Result<f32, DecodeError> {
        let s = self
            .data
            .get(self.pos..self.pos + 4)
            .ok_or(DecodeError::Truncated { context: "read_f32_be" })?;
        self.pos += 4;
        Ok(f32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub fn read_array4(&mut self) -> Result<[u8; 4], DecodeError> {
        let s = self
            .data
            .get(self.pos..self.pos + 4)
            .ok_or(DecodeError::Truncated { context: "read_array4" })?;
        self.pos += 4;
        Ok([s[0], s[1], s[2], s[3]])
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let s = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or(DecodeError::Truncated { context: "read_bytes" })?;
        self.pos += n;
        Ok(s)
    }
}
