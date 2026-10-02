//! THA1 archive reader (`th19.dat`), the container format used by Touhou 9.5 onwards.
//!
//! Format knowledge ported from thtk (thdat95.c / thcrypt.c / thlzss.c),
//! BSD-2-Clause, https://github.com/thpatch/thtk.
//!
//! Layout:
//! * 16-byte header, encrypted with (0x1b, 0x37, 16, 16):
//!   `"THA1"`, list size + 123456789, list zsize + 987654321, entry count + 135792468.
//! * File bodies, each encrypted with one of 8 parameter sets picked by a name hash,
//!   then LZSS-compressed unless `zsize == size`.
//! * File list at the very end: encrypted with (0x3e, 0x9b, 0x80), LZSS-compressed.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::{Error, Result};

#[derive(Clone, Copy, Debug)]
struct CryptParams {
    key: u8,
    step: u8,
    block: usize,
    limit: usize,
}

const fn cp(key: u8, step: u8, block: usize, limit: usize) -> CryptParams {
    CryptParams { key, step, block, limit }
}

/// Parameter table used by TH14 and every later main-series game, including TH19.
const TH14_CRYPT_PARAMS: [CryptParams; 8] = [
    cp(0x1b, 0x73, 0x100, 0x3800),
    cp(0x12, 0x43, 0x200, 0x3e00),
    cp(0x35, 0x79, 0x400, 0x3c00),
    cp(0x03, 0x91, 0x80, 0x6400),
    cp(0xab, 0xdc, 0x80, 0x7000),
    cp(0x51, 0x9e, 0x100, 0x4000),
    cp(0xc1, 0x15, 0x400, 0x2c00),
    cp(0x99, 0x7d, 0x80, 0x4400),
];

fn crypt_index(name: &str) -> usize {
    // The game sums the bytes into a signed char; only the low 3 bits matter.
    name.bytes().fold(0u8, |acc, b| acc.wrapping_add(b)) as usize & 7
}

/// In-place block-interleave XOR decryption.
pub fn decrypt(data: &mut [u8], mut key: u8, step: u8, mut block: usize, mut limit: usize) {
    let mut size = data.len();
    if size < block >> 2 {
        size = 0;
    } else {
        let rem = size % block;
        size -= (if rem < block >> 2 { rem } else { 0 }) + size % 2;
    }
    if limit % block != 0 {
        limit += block - limit % block;
    }
    let end = size.min(limit);

    let mut temp = vec![0u8; block];
    let mut pos = 0;
    while pos < end {
        if end - pos < block {
            block = end - pos;
        }
        let increment = (block >> 1) + (block & 1);
        let chunk = &data[pos..pos + block];

        let mut inp = 0;
        let mut out = block as isize - 1;
        while out > 0 {
            temp[out as usize] = chunk[inp] ^ key;
            temp[out as usize - 1] =
                chunk[inp + increment] ^ key.wrapping_add(step.wrapping_mul(increment as u8));
            out -= 2;
            inp += 1;
            key = key.wrapping_add(step);
        }
        if block & 1 == 1 {
            temp[0] = chunk[inp] ^ key;
            key = key.wrapping_add(step);
        }
        key = key.wrapping_add(step.wrapping_mul(increment as u8));

        data[pos..pos + block].copy_from_slice(&temp[..block]);
        pos += block;
    }
}

/// MSB-first bit reader.
struct Bits<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u8,
}

impl Bits<'_> {
    fn read(&mut self, count: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..count {
            let b = *self.data.get(self.byte)?;
            v = (v << 1) | ((b >> (7 - self.bit)) & 1) as u32;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.byte += 1;
            }
        }
        Some(v)
    }
}

/// ZUN's LZSS: 8 KiB dictionary starting at index 1, 13-bit offsets, 4-bit lengths (+3).
pub fn unlzss(input: &[u8], output_size: usize) -> Vec<u8> {
    const MASK: usize = 0x1fff;
    let mut dict = [0u8; 0x2000];
    let mut head = 1usize;
    let mut out = Vec::with_capacity(output_size);
    let mut bits = Bits { data: input, byte: 0, bit: 0 };

    while out.len() < output_size {
        let Some(flag) = bits.read(1) else { break };
        if flag == 1 {
            let Some(c) = bits.read(8) else { break };
            out.push(c as u8);
            dict[head] = c as u8;
            head = (head + 1) & MASK;
        } else {
            let Some(offset) = bits.read(13) else { break };
            if offset == 0 {
                break;
            }
            let Some(len) = bits.read(4) else { break };
            for i in 0..len as usize + 3 {
                let c = dict[(offset as usize + i) & MASK];
                out.push(c);
                dict[head] = c;
                head = (head + 1) & MASK;
            }
        }
    }
    out.truncate(output_size);
    out
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub size: usize,
    pub zsize: usize,
}

/// A THA1 archive opened from disk. Files are decoded lazily on request.
pub struct Archive {
    file: File,
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut file = File::open(path)?;
        let file_len = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        let mut header = [0u8; 16];
        file.read_exact(&mut header)?;
        decrypt(&mut header, 0x1b, 0x37, 16, 16);
        if &header[0..4] != b"THA1" {
            return Err(Error::Format("not a THA1 archive (bad magic)".into()));
        }
        let word = |i: usize| u32::from_le_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
        let list_size = word(1).wrapping_sub(123456789) as usize;
        let list_zsize = word(2).wrapping_sub(987654321) as usize;
        let count = word(3).wrapping_sub(135792468) as usize;
        if list_zsize as u64 > file_len {
            return Err(Error::Format("file list larger than archive".into()));
        }

        let list_start = file_len - list_zsize as u64;
        let mut zlist = vec![0u8; list_zsize];
        file.seek(SeekFrom::Start(list_start))?;
        file.read_exact(&mut zlist)?;
        decrypt(&mut zlist, 0x3e, 0x9b, 0x80, list_zsize);
        let list = unlzss(&zlist, list_size);

        let mut entries = Vec::with_capacity(count);
        let mut p = 0;
        let rd = |p: usize| -> Result<u32> {
            list.get(p..p + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .ok_or_else(|| Error::Format("truncated file list".into()))
        };
        for _ in 0..count {
            let nul = list[p..]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| Error::Format("unterminated entry name".into()))?;
            let name = String::from_utf8_lossy(&list[p..p + nul]).into_owned();
            p += nul + (4 - nul % 4);
            let offset = rd(p)? as u64;
            let size = rd(p + 4)? as usize;
            p += 12;
            entries.push(Entry { name, offset, size, zsize: 0 });
        }
        for i in 0..entries.len() {
            let next = entries.get(i + 1).map_or(list_start, |e| e.offset);
            entries[i].zsize = (next - entries[i].offset) as usize;
        }

        let index = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.to_ascii_lowercase(), i))
            .collect();
        Ok(Self { file, entries, index })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(&name.to_ascii_lowercase())
    }

    /// Read and decode a file by name (case-insensitive).
    pub fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        let i = *self
            .index
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        self.read_entry(i)
    }

    pub fn read_entry(&mut self, i: usize) -> Result<Vec<u8>> {
        let e = self.entries[i].clone();
        let mut data = vec![0u8; e.zsize];
        self.file.seek(SeekFrom::Start(e.offset))?;
        self.file.read_exact(&mut data)?;
        let p = TH14_CRYPT_PARAMS[crypt_index(&e.name)];
        decrypt(&mut data, p.key, p.step, p.block, p.limit);
        if e.zsize == e.size {
            Ok(data)
        } else {
            Ok(unlzss(&data, e.size))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lzss_literals() {
        // flag=1 then byte 'A' (0x41), flag=1 then 'B', then flag=0 offset=0 terminator.
        // bits: 1 01000001 1 01000010 0 0000000000000
        let bits = "1010000011010000100000000000000000";
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (i, c) in bits.chars().enumerate() {
            if c == '1' {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        assert_eq!(unlzss(&bytes, 2), b"AB");
    }
}
