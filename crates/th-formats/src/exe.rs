//! Static game tables read from the user's own `th19.exe` (v1.10c).
//!
//! Some gameplay data is compiled into the executable rather than stored in `th19.dat`.
//! It is read at runtime from the user's copy, like every other asset.

use crate::{Error, Result};

/// A loaded PE image, addressed by virtual address.
pub struct Exe {
    data: Vec<u8>,
    image_base: u32,
    /// (virtual address, virtual size, file offset, raw size)
    sections: Vec<(u32, u32, u32, u32)>,
}

impl Exe {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let bad = || Error::Format("not a PE executable".into());
        if data.get(0..2) != Some(b"MZ") {
            return Err(bad());
        }
        let pe = u32_at(0x3c).ok_or_else(bad)? as usize;
        if data.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err(bad());
        }
        let count = u16_at(pe + 6).ok_or_else(bad)? as usize;
        let opt_size = u16_at(pe + 20).ok_or_else(bad)? as usize;
        let image_base = u32_at(pe + 24 + 28).ok_or_else(bad)?;
        let mut sections = Vec::with_capacity(count);
        for i in 0..count {
            let s = pe + 24 + opt_size + i * 40;
            let f = |k: usize| u32_at(s + k).ok_or_else(bad);
            sections.push((f(12)?, f(8)?, f(20)?, f(16)?));
        }
        Ok(Self { data, image_base, sections })
    }

    /// A memory image (e.g. a Ghidra export of the loaded program) laid out as consecutive blocks.
    /// `map` lines are `name va_hex size_hex offset_hex`; `#` lines are ignored. Development aid
    /// for when the executable itself is unavailable.
    pub fn from_image(data: Vec<u8>, map: &str) -> Result<Self> {
        let mut sections = Vec::new();
        for line in map.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let f: Vec<&str> = line.split_whitespace().collect();
            let hex = |s: &str| u32::from_str_radix(s, 16).map_err(|_| Error::Format(format!("bad image map line: {line}")));
            let (va, size, pos) = (hex(f.get(1).copied().unwrap_or(""))?, hex(f.get(2).copied().unwrap_or(""))?, hex(f.get(3).copied().unwrap_or(""))?);
            sections.push((va - 0x40_0000, size, pos, size));
        }
        Ok(Self { data, image_base: 0x40_0000, sections })
    }

    /// File bytes backing `len` bytes at virtual address `va` (only initialised data).
    pub fn bytes(&self, va: u32, len: usize) -> Option<&[u8]> {
        let rva = va.checked_sub(self.image_base)?;
        let &(sva, _, raw, raw_size) = self.sections.iter().find(|s| s.0 <= rva && rva < s.0 + s.3)?;
        let off = (raw + rva - sva) as usize;
        if rva - sva + len as u32 > raw_size {
            return None;
        }
        self.data.get(off..off + len)
    }

    pub fn i32_at(&self, va: u32) -> Option<i32> {
        self.bytes(va, 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn f32_at(&self, va: u32) -> Option<f32> {
        self.i32_at(va).map(|v| f32::from_bits(v as u32))
    }
}

/// One enemy bullet sprite type (table at 0x5C6650, 0x158 bytes per type).
#[derive(Clone, Debug)]
pub struct BulletType {
    /// `bullet.anm` script.
    pub script: i32,
    /// Per colour: the 5 sprites the script's sprite numbers 0..4 map to (`FUN_004f66a0`).
    /// For some types the 4th/5th values are the cancel-effect script and an RGB colour.
    pub colours: [[i32; 5]; 16],
    /// Collision radius.
    pub hitbox: f32,
    pub unk_148: i32,
    /// How the cancel effect is chosen.
    pub cancel_mode: i32,
    pub unk_150: i32,
    /// Extra ANM script spawned with the bullet (0 = none).
    pub extra_script: i32,
}

const BULLET_TYPES_VA: u32 = 0x5c6650;
const BULLET_TYPE_SIZE: u32 = 0x158;

/// Read the bullet type table (stops at the first entry that isn't plausible).
pub fn bullet_types(exe: &Exe) -> Vec<BulletType> {
    let mut out = Vec::new();
    for t in 0..64u32 {
        let base = BULLET_TYPES_VA + t * BULLET_TYPE_SIZE;
        let Some(script) = exe.i32_at(base) else { break };
        if !(0..2000).contains(&script) {
            break;
        }
        let mut colours = [[0; 5]; 16];
        for (c, col) in colours.iter_mut().enumerate() {
            for (n, v) in col.iter_mut().enumerate() {
                *v = exe.i32_at(base + 4 + c as u32 * 0x14 + n as u32 * 4).unwrap_or(0);
            }
        }
        out.push(BulletType {
            script,
            colours,
            hitbox: exe.f32_at(base + 0x144).unwrap_or(0.0),
            unk_148: exe.i32_at(base + 0x148).unwrap_or(0),
            cancel_mode: exe.i32_at(base + 0x14c).unwrap_or(0),
            unk_150: exe.i32_at(base + 0x150).unwrap_or(0),
            extra_script: exe.i32_at(base + 0x154).unwrap_or(0),
        });
    }
    out
}

/// What the game knows about one sound id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundInfo {
    /// The wav in th19.dat.
    pub file: String,
    /// DirectSound volume in hundredths of a decibel (0 = full, negative = quieter) at the game's
    /// full sound-effect setting.
    pub volume: i16,
    /// Plays looped (DSBPLAY_LOOPING).
    pub looped: bool,
}

/// The sound table by sound id (`None` when an id has no file).
///
/// `FUN_004bc050` loads the 0x48 wavs named by the pointers at 0x596210 into buffers 0..0x48 and
/// the init after it (0x004bd... loop at decompile line 163814) gives every sound id N the
/// buffer `row.field1` of the first row of the 0x14-byte table at 0x5c4e00 whose first dword is
/// N. The row also holds the volume (`short` at +8, used by `FUN_004bae70` as
/// `(volume + 5000) * curve - 5000`) and the loop flag (+0xc). Only some ids equal their row's
/// file index (menu ids 9 and 10 are cancel00 and select00).
pub fn sound_table(exe: &Exe) -> Vec<Option<SoundInfo>> {
    const FILES: u32 = 0x596210;
    const ROWS: u32 = 0x5c4e00;
    let names: Vec<Option<String>> = (0..0x48u32)
        .map(|i| {
            let ptr = exe.i32_at(FILES + i * 4)? as u32;
            let raw = exe.bytes(ptr, 32)?;
            Some(String::from_utf8_lossy(raw.split(|&b| b == 0).next()?).into_owned())
        })
        .collect();
    (0..0x5au32)
        .map(|id| {
            let row = (0..0x60u32).map(|r| ROWS + r * 0x14).find(|&a| exe.i32_at(a) == Some(id as i32))?;
            let file = names.get(exe.i32_at(row + 4)? as usize)?.clone()?;
            let volume = exe.bytes(row + 8, 2).map(|b| i16::from_le_bytes([b[0], b[1]]))?;
            Some(SoundInfo { file, volume, looped: exe.i32_at(row + 0xc)? != 0 })
        })
        .collect()
}
