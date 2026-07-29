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

    /// Position a cursor at `ptr` within `body` and read the leading 4-byte
    /// block id. `ptr == 0` is the block-pointer-table sentinel for "block
    /// absent" and returns `None`, matching the other soft-fail block reads.
    pub fn for_block(body: &'a [u8], ptr: u32) -> Option<(Self, [u8; 4])> {
        if ptr == 0 {
            return None;
        }
        let mut c = Cursor::at(body, ptr as usize).ok()?;
        let block_id = c.read_array4().ok()?;
        Some((c, block_id))
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

    /// Read `N` bytes and advance the position, or fail with `context` naming the caller.
    fn read_be<const N: usize>(&mut self, context: &'static str) -> Result<[u8; N], DecodeError> {
        let s = self
            .data
            .get(self.pos..self.pos + N)
            .ok_or(DecodeError::Truncated { context })?;
        self.pos += N;
        Ok(s.try_into().expect("slice length matches N by construction"))
    }

    pub fn read_u16_be(&mut self) -> Result<u16, DecodeError> {
        self.read_be::<2>("read_u16_be").map(u16::from_be_bytes)
    }

    pub fn read_i16_be(&mut self) -> Result<i16, DecodeError> {
        self.read_u16_be().map(|v| v as i16)
    }

    pub fn read_u32_be(&mut self) -> Result<u32, DecodeError> {
        self.read_be::<4>("read_u32_be").map(u32::from_be_bytes)
    }

    pub fn read_f32_be(&mut self) -> Result<f32, DecodeError> {
        self.read_be::<4>("read_f32_be").map(f32::from_be_bytes)
    }

    pub fn read_array4(&mut self) -> Result<[u8; 4], DecodeError> {
        self.read_be::<4>("read_array4")
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
