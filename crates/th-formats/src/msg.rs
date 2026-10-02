//! MSG dialogue scripts (TH9+ layout, with the TH19 header extension).
//!
//! Layout from thtk's thmsg06.c (BSD-2-Clause):
//! * `u32` entry count
//! * TH19 only: 0x50 bytes of extra header values
//! * `count` pairs of `(i32 offset, i32 flags)`
//! * instructions: `u16 time, u8 opcode, u8 length, data[length]`;
//!   an entry ends at an instruction with `time == 0 && opcode == 0`.
//!
//! Text arguments are obfuscated with a rolling XOR; [`decode_text`] undoes it.
//! The text itself is the game's script and is only ever decoded for display
//! at runtime — it is never stored in this repository.

use crate::{Error, Reader, Result};

pub const TH19_EXTRA_HEADER: usize = 0x50;

#[derive(Clone, Debug)]
pub struct Instr {
    pub time: u16,
    pub opcode: u8,
    pub data: Vec<u8>,
}

impl Instr {
    pub fn arg_i32(&self, n: usize) -> i32 {
        self.data
            .get(n * 4..n * 4 + 4)
            .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub flags: i32,
    pub instrs: Vec<Instr>,
}

#[derive(Clone, Debug, Default)]
pub struct MsgFile {
    pub header: Vec<i32>,
    pub entries: Vec<Entry>,
}

/// Undo the rolling XOR applied to text arguments (key 0x77, step 7, step-step 16).
pub fn decode_text(data: &[u8]) -> Vec<u8> {
    let (mut key, mut step) = (0x77u8, 7u8);
    data.iter()
        .map(|&b| {
            let out = b ^ key;
            key = key.wrapping_add(step);
            step = step.wrapping_add(16);
            out
        })
        .take_while(|&b| b != 0)
        .collect()
}

impl MsgFile {
    /// Parse a stage/VS dialogue file (`plXXst.msg`, `plXXvs.msg`).
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::parse_with(data, TH19_EXTRA_HEADER)
    }

    /// Parse an ending/staff-roll file (`endXX.msg`, `staff.msg`), which lacks the TH19 header block.
    pub fn parse_ending(data: &[u8]) -> Result<Self> {
        Self::parse_with(data, 0)
    }

    fn parse_with(data: &[u8], extra: usize) -> Result<Self> {
        let r = Reader::at(data, 0)?;
        let count = r.peek_u32(0)? as usize;
        if count > 10_000 {
            return Err(Error::Format(format!("implausible MSG entry count {count}")));
        }
        let header = (0..extra / 4)
            .map(|i| r.peek_i32(4 + i * 4))
            .collect::<Result<Vec<_>>>()?;
        let table = 4 + extra;

        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let offset = r.peek_i32(table + i * 8)? as usize;
            let flags = r.peek_i32(table + i * 8 + 4)?;
            let mut instrs = Vec::new();
            let mut p = offset;
            // Unused entry slots point at the shared terminator / end of file.
            while p + 4 <= data.len() {
                let time = r.peek_u16(p)?;
                let opcode = r.peek_u8(p + 2)?;
                let len = r.peek_u8(p + 3)? as usize;
                if time == 0 && opcode == 0 {
                    break;
                }
                instrs.push(Instr { time, opcode, data: r.peek_bytes(p + 4, len)?.to_vec() });
                p += 4 + len;
            }
            entries.push(Entry { flags, instrs });
        }
        Ok(Self { header, entries })
    }
}
