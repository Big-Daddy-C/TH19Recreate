//! ANM v8 (TH13+) sprite/animation container, with the TH19 extensions.
//!
//! An `.anm` file is a linked list of entries. Each entry describes one texture
//! (embedded as PNG/JPEG in a `THTX` block in TH19, or referenced by name),
//! a list of sprites (rectangles on that texture), and a list of scripts that
//! drive the ANM virtual machine.
//!
//! Struct layouts follow thtk's `thtypes/anm_types.h` (BSD-2-Clause).

use crate::{Error, Reader, Result};

#[derive(Clone, Debug)]
pub struct Sprite {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug)]
pub struct Instr {
    pub opcode: u16,
    pub time: i16,
    /// Bit `n` set means argument `n` is a variable reference rather than a literal.
    pub param_mask: u16,
    /// Byte offset of this instruction from the start of the script (jump targets use these).
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
}

#[derive(Clone, Debug)]
pub struct Script {
    pub id: i32,
    pub instrs: Vec<Instr>,
}

impl Script {
    /// Index of the instruction at a given byte offset (for jumps).
    pub fn index_of_offset(&self, offset: u32) -> Option<usize> {
        self.instrs.iter().position(|i| i.offset == offset)
    }
}

/// Encoded texture payload as stored in the file.
#[derive(Clone, Debug)]
pub struct Texture {
    pub format: u16,
    pub width: u16,
    pub height: u16,
    /// PNG/JPEG bytes in TH19; raw pixels in older games.
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub version: u32,
    pub name: String,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub format: u16,
    pub lowres_scale: bool,
    pub sprites: Vec<Sprite>,
    pub scripts: Vec<Script>,
    pub texture: Option<Texture>,
}

#[derive(Clone, Debug, Default)]
pub struct AnmFile {
    pub entries: Vec<Entry>,
}

const HEADER_SIZE: usize = 0x40;

impl AnmFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut entries = Vec::new();
        let mut base = 0usize;
        let mut script_index = 0i32;
        loop {
            let r = Reader::at(data, base)?;
            let h = |off: usize| r.peek_u32(off);
            let h16 = |off: usize| r.peek_u16(off);

            let version = h(0x00)?;
            if version != 8 {
                return Err(Error::Format(format!("unsupported ANM version {version} at {base:#x}")));
            }
            let n_sprites = h16(0x04)? as usize;
            let n_scripts = h16(0x06)? as usize;
            let width = h16(0x0a)?;
            let height = h16(0x0c)?;
            let format = h16(0x0e)?;
            let name_off = h(0x10)? as usize;
            let x = h16(0x14)?;
            let y = h16(0x16)?;
            let thtx_off = h(0x1c)? as usize;
            let has_data = h16(0x20)? != 0;
            let lowres_scale = r.peek_u8(0x22)? != 0;
            let next_off = h(0x24)? as usize;

            let name = r.peek_cstr(name_off)?;

            let mut sprites = Vec::with_capacity(n_sprites);
            for i in 0..n_sprites {
                let off = h(HEADER_SIZE + i * 4)? as usize;
                let s = Reader::at(data, base + off)?;
                sprites.push(Sprite {
                    id: s.peek_u32(0)?,
                    x: s.peek_f32(4)?,
                    y: s.peek_f32(8)?,
                    w: s.peek_f32(12)?,
                    h: s.peek_f32(16)?,
                });
            }

            let script_table = HEADER_SIZE + n_sprites * 4;
            let mut offsets = Vec::with_capacity(n_scripts);
            for i in 0..n_scripts {
                offsets.push((h(script_table + i * 8)? as i32, h(script_table + i * 8 + 4)? as usize));
            }

            let mut scripts = Vec::with_capacity(n_scripts);
            for (i, &(_id, start)) in offsets.iter().enumerate() {
                let limit = if let Some(&(_, next)) = offsets.get(i + 1) {
                    next
                } else if thtx_off != 0 {
                    thtx_off
                } else if next_off != 0 {
                    next_off
                } else {
                    data.len() - base
                };
                let mut instrs = Vec::new();
                let mut p = start;
                while p + 8 <= limit {
                    let opcode = r.peek_u16(p)?;
                    let len = r.peek_u16(p + 2)? as usize;
                    if opcode == 0xffff || len < 8 {
                        break;
                    }
                    instrs.push(Instr {
                        opcode,
                        time: r.peek_u16(p + 4)? as i16,
                        param_mask: r.peek_u16(p + 6)?,
                        offset: (p - start) as u32,
                        args: r.peek_bytes(p + 8, len - 8)?.to_vec(),
                    });
                    p += len;
                }
                // Scripts are numbered globally across all entries of a file.
                scripts.push(Script { id: script_index, instrs });
                script_index += 1;
            }

            let texture = if has_data && thtx_off != 0 {
                let t = Reader::at(data, base + thtx_off)?;
                if t.peek_bytes(0, 4)? != b"THTX" {
                    return Err(Error::Format("bad THTX magic".into()));
                }
                let size = t.peek_u32(12)? as usize;
                Some(Texture {
                    format: t.peek_u16(6)?,
                    width: t.peek_u16(8)?,
                    height: t.peek_u16(10)?,
                    data: t.peek_bytes(16, size)?.to_vec(),
                })
            } else {
                None
            };

            entries.push(Entry {
                version,
                name,
                x,
                y,
                width,
                height,
                format,
                lowres_scale,
                sprites,
                scripts,
                texture,
            });

            if next_off == 0 {
                break;
            }
            base += next_off;
        }
        Ok(Self { entries })
    }

    pub fn script_count(&self) -> usize {
        self.entries.iter().map(|e| e.scripts.len()).sum()
    }

    pub fn sprite_count(&self) -> usize {
        self.entries.iter().map(|e| e.sprites.len()).sum()
    }
}
