//! SHT player shot definitions (`plXX.sht`).
//!
//! There is no public documentation for the TH19 layout; this was worked out from
//! the files themselves and validated against all 19 characters (every byte of every
//! file is accounted for). Field meanings marked `UNCONFIRMED` still need checking
//! against th19.exe.
//!
//! Layout:
//! * `0x00` u16 `unk0` (always 6), u16 `slot_count` (always 40)
//! * `0x04` f32 × 3: radii — hitbox?, graze?, item collection? (`UNCONFIRMED`)
//! * `0x10` f32 × 4: move speed unfocused, focused, and both divided by √2 (diagonal)
//! * `0x20 .. 0xF0`: integer parameters and option positions (kept raw for now)
//! * `0xF0`: `slot_count` × i32 offsets (relative to the end of the table; -1 = unused)
//! * Each used slot: a list of 120-byte shot records terminated by a `0xFFFF` u16 (4 bytes).

use crate::{Error, Reader, Result};

pub const RECORD_SIZE: usize = 120;
const TABLE_AT: usize = 0xF0;

#[derive(Clone, Debug)]
pub struct ShotRecord {
    /// Frames between shots (`UNCONFIRMED`).
    pub interval: u16,
    /// Starting delay / phase (`UNCONFIRMED`).
    pub delay: u16,
    /// Spawn offset from the player.
    pub offset: [f32; 2],
    /// Hitbox size.
    pub hitbox: [f32; 2],
    /// Direction in radians (−π/2 = straight up).
    pub angle: f32,
    pub unk_18: f32,
    pub speed: f32,
    /// Full record bytes, for fields not decoded yet.
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ShtFile {
    pub unk0: u16,
    pub radii: [f32; 3],
    pub speed: f32,
    pub speed_focused: f32,
    pub speed_diagonal: f32,
    pub speed_focused_diagonal: f32,
    /// Bytes `0x20..0xF0`: power thresholds / option positions, not decoded yet.
    pub params_raw: Vec<u8>,
    /// One entry per slot; `None` for unused slots.
    pub slots: Vec<Option<Vec<ShotRecord>>>,
}

impl ShtFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = Reader::at(data, 0)?;
        let slot_count = r.peek_u16(2)? as usize;
        if slot_count > 256 {
            return Err(Error::Format(format!("implausible SHT slot count {slot_count}")));
        }
        let base = TABLE_AT + slot_count * 4;

        let mut slots = Vec::with_capacity(slot_count);
        for i in 0..slot_count {
            let off = r.peek_i32(TABLE_AT + i * 4)?;
            if off < 0 {
                slots.push(None);
                continue;
            }
            let mut p = base + off as usize;
            let mut records = Vec::new();
            while r.peek_u16(p)? != 0xFFFF {
                let rec = Reader::at(data, p)?;
                records.push(ShotRecord {
                    interval: rec.peek_u16(0)?,
                    delay: rec.peek_u16(2)?,
                    offset: [rec.peek_f32(4)?, rec.peek_f32(8)?],
                    hitbox: [rec.peek_f32(12)?, rec.peek_f32(16)?],
                    angle: rec.peek_f32(20)?,
                    unk_18: rec.peek_f32(24)?,
                    speed: rec.peek_f32(28)?,
                    raw: rec.peek_bytes(0, RECORD_SIZE)?.to_vec(),
                });
                p += RECORD_SIZE;
                if records.len() > 256 {
                    return Err(Error::Format("runaway SHT record list".into()));
                }
            }
            slots.push(Some(records));
        }

        Ok(Self {
            unk0: r.peek_u16(0)?,
            radii: [r.peek_f32(4)?, r.peek_f32(8)?, r.peek_f32(12)?],
            speed: r.peek_f32(16)?,
            speed_focused: r.peek_f32(20)?,
            speed_diagonal: r.peek_f32(24)?,
            speed_focused_diagonal: r.peek_f32(28)?,
            params_raw: r.peek_bytes(0x20, TABLE_AT - 0x20)?.to_vec(),
            slots,
        })
    }
}
