//! Parsers for Touhou 19 (Unfinished Dream of All Living Ghost) data formats.
//!
//! None of the game's assets are included here; everything is read from the
//! user's own copy of `th19.dat` / `thbgm.dat`.

pub mod dat;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("format error: {0}")]
    Format(String),
    #[error("file not found in archive: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod anm;

/// Bounds-checked little-endian reader over a byte slice, positioned at a base offset.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    base: usize,
}

impl<'a> Reader<'a> {
    pub fn at(data: &'a [u8], base: usize) -> Result<Self> {
        if base > data.len() {
            return Err(Error::Format(format!("offset {base:#x} past end of data")));
        }
        Ok(Self { data, base })
    }

    pub fn peek_bytes(&self, off: usize, len: usize) -> Result<&'a [u8]> {
        let start = self.base + off;
        self.data
            .get(start..start + len)
            .ok_or_else(|| Error::Format(format!("read of {len} bytes at {start:#x} out of bounds")))
    }

    pub fn peek_u8(&self, off: usize) -> Result<u8> {
        Ok(self.peek_bytes(off, 1)?[0])
    }

    pub fn peek_u16(&self, off: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.peek_bytes(off, 2)?.try_into().unwrap()))
    }

    pub fn peek_u32(&self, off: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.peek_bytes(off, 4)?.try_into().unwrap()))
    }

    pub fn peek_i32(&self, off: usize) -> Result<i32> {
        Ok(self.peek_u32(off)? as i32)
    }

    pub fn peek_f32(&self, off: usize) -> Result<f32> {
        Ok(f32::from_bits(self.peek_u32(off)?))
    }

    /// NUL-terminated string; Shift-JIS bytes are kept as lossy UTF-8.
    pub fn peek_cstr(&self, off: usize) -> Result<String> {
        let start = self.base + off;
        let tail = self.data.get(start..).ok_or_else(|| Error::Format("string out of bounds".into()))?;
        let len = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        Ok(String::from_utf8_lossy(&tail[..len]).into_owned())
    }
}
