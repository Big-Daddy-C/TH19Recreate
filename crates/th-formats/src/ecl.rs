//! ECL (`SCPT`) scripts — enemy, bullet pattern, spell card and character logic (TH10+ layout).
//!
//! Layout (from thtk's thecl10.c, BSD-2-Clause):
//! * `SCPT` header: include list offset/length and sub count.
//! * `ANIM` list: `.anm` files this script refers to by index.
//! * `ECLI` list: other `.ecl` files to include.
//! * Sub offset table, followed by NUL-terminated sub names.
//! * Each sub: `ECLH` header, then instructions until the next sub / EOF.
//!
//! Each instruction is self-sizing, so the container can be parsed without
//! knowing every opcode's signature; argument decoding is the VM's job.

use crate::{Error, Reader, Result};

#[derive(Clone, Debug)]
pub struct Instr {
    pub time: i32,
    pub opcode: u16,
    /// Bit `n` set: argument `n` is a variable reference instead of an immediate.
    pub param_mask: u16,
    /// Difficulty bitmask (`1111LHNE` in older games; TH19 uses it for rank gating too).
    pub rank_mask: u8,
    pub param_count: u8,
    /// Number of stack references in the argument list (TH13+).
    pub stack_refs: u32,
    /// Byte offset of this instruction relative to the start of its sub (jump targets use these).
    pub offset: u32,
    pub args: Vec<u8>,
}

impl Instr {
    pub fn arg_i32(&self, n: usize) -> i32 {
        self.args
            .get(n * 4..n * 4 + 4)
            .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn arg_f32(&self, n: usize) -> f32 {
        f32::from_bits(self.arg_i32(n) as u32)
    }

    pub fn is_var(&self, n: usize) -> bool {
        self.param_mask & (1 << n) != 0
    }

    /// Length-prefixed, NUL-padded string argument starting at byte `at`
    /// (used for sub names in call/spawn instructions). Returns the string and the bytes consumed.
    pub fn arg_str(&self, at: usize) -> Option<(String, usize)> {
        let len = u32::from_le_bytes(self.args.get(at..at + 4)?.try_into().ok()?) as usize;
        let raw = self.args.get(at + 4..at + 4 + len)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Some((String::from_utf8_lossy(&raw[..end]).into_owned(), 4 + len))
    }
}

#[derive(Clone, Debug)]
pub struct Sub {
    pub name: String,
    pub instrs: Vec<Instr>,
}

impl Sub {
    pub fn index_of_offset(&self, offset: u32) -> Option<usize> {
        self.instrs.iter().position(|i| i.offset == offset)
    }
}

#[derive(Clone, Debug, Default)]
pub struct EclFile {
    pub anim: Vec<String>,
    pub ecli: Vec<String>,
    pub subs: Vec<Sub>,
}

const SUB_HEADER: usize = 16;
const INSTR_HEADER: usize = 16;

fn read_list(data: &[u8], at: usize, magic: &[u8; 4]) -> Result<(Vec<String>, usize)> {
    let r = Reader::at(data, at)?;
    if r.peek_bytes(0, 4)? != magic {
        return Err(Error::Format(format!("missing {} list", String::from_utf8_lossy(magic))));
    }
    let count = r.peek_u32(4)? as usize;
    let mut p = 8;
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        let s = r.peek_cstr(p)?;
        p += s.len() + 1;
        names.push(s);
    }
    // Lists are padded to a 4-byte boundary (relative to the file start).
    let mut end = at + p;
    end = end.next_multiple_of(4);
    Ok((names, end))
}

impl EclFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let h = Reader::at(data, 0)?;
        if h.peek_bytes(0, 4)? != b"SCPT" {
            return Err(Error::Format("missing SCPT signature".into()));
        }
        let include_offset = h.peek_u32(8)? as usize;
        let sub_count = h.peek_u32(16)? as usize;

        let (anim, next) = read_list(data, include_offset, b"ANIM")?;
        let (ecli, next) = read_list(data, next, b"ECLI")?;

        let offsets_at = next;
        let mut offsets = Vec::with_capacity(sub_count);
        for i in 0..sub_count {
            offsets.push(h.peek_u32(offsets_at + i * 4)? as usize);
        }
        let mut name_at = offsets_at + sub_count * 4;
        let mut names = Vec::with_capacity(sub_count);
        for _ in 0..sub_count {
            let s = h.peek_cstr(name_at)?;
            name_at += s.len() + 1;
            names.push(s);
        }

        // Subs are laid out in file order but the table need not be sorted; the end of a
        // sub is the next sub start after it (or EOF).
        let mut sorted = offsets.clone();
        sorted.sort_unstable();

        let mut subs = Vec::with_capacity(sub_count);
        for (name, &start) in names.into_iter().zip(&offsets) {
            let s = Reader::at(data, start)?;
            if s.peek_bytes(0, 4)? != b"ECLH" {
                return Err(Error::Format(format!("sub {name}: missing ECLH")));
            }
            let end = sorted.iter().copied().find(|&o| o > start).unwrap_or(data.len());
            let mut instrs = Vec::new();
            let mut p = start + SUB_HEADER;
            while p + INSTR_HEADER <= end {
                let r = Reader::at(data, p)?;
                let size = r.peek_u16(6)? as usize;
                if size < INSTR_HEADER || p + size > end {
                    return Err(Error::Format(format!("sub {name}: bad instruction size {size} at {p:#x}")));
                }
                instrs.push(Instr {
                    time: r.peek_i32(0)?,
                    opcode: r.peek_u16(4)?,
                    param_mask: r.peek_u16(8)?,
                    rank_mask: r.peek_u8(10)?,
                    param_count: r.peek_u8(11)?,
                    stack_refs: r.peek_u32(12)?,
                    offset: (p - start) as u32,
                    args: r.peek_bytes(INSTR_HEADER, size - INSTR_HEADER)?.to_vec(),
                });
                p += size;
            }
            subs.push(Sub { name, instrs });
        }
        Ok(Self { anim, ecli, subs })
    }

    pub fn sub(&self, name: &str) -> Option<&Sub> {
        self.subs.iter().find(|s| s.name == name)
    }
}
