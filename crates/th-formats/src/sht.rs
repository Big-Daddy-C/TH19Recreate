//! SHT player shot definitions (`plXX.sht`).
//!
//! There is no public documentation for the TH19 layout; it was worked out from the files
//! and from th19.exe's player code (`FUN_005478f0` loads it, `FUN_0054f990` picks records,
//! `FUN_0054e960` turns a record into a bullet).
//!
//! Layout:
//! * `0x00` u16 `unk0` (always 6), u16 `slot_count` (always 40)
//! * `0x04` f32 × 3: radii — hitbox, graze?, item collection? (2nd/3rd `UNCONFIRMED`)
//! * `0x10` f32 × 4: move speed unfocused, focused, and both divided by √2 (diagonal)
//! * `0x20` i32 power levels (the game starts at `levels * 100` power)
//! * `0x3C` i32 option sprite script in `plXX.anm` (`FUN_005483d0`)
//! * `0x50` option positions, unfocused: level 1 has 1 option, level 2 has 2, ... (f32 pairs)
//! * `0xA0` the same for focused
//! * `0xF0`: `slot_count` × i32 offsets (relative to the end of the table; -1 = unused).
//!   Slot `power / 100` is used unfocused, slot `power / 100 + 5` focused.
//! * Each used slot: a list of 120-byte shot records terminated by a `0xFFFF` u16 (4 bytes).

use crate::{Error, Reader, Result};

pub const RECORD_SIZE: usize = 120;
const TABLE_AT: usize = 0xF0;

#[derive(Clone, Debug)]
pub struct ShotRecord {
    /// Fires when `timer % interval == phase` (interval 0: every tick of the timer).
    pub interval: i8,
    pub phase: i8,
    pub damage: i16,
    /// Spawn offset from the origin (player or option).
    pub offset: [f32; 2],
    /// Hitbox size.
    pub hitbox: [f32; 2],
    /// Direction in radians (−π/2 = straight up).
    pub angle: f32,
    pub unk_18: f32,
    pub speed: f32,
    /// Low nibble: 0 = player, 1..=15 = option index, 6 (on some types) = field origin.
    pub origin: u8,
    /// Shot behaviour class (2: one live at a time per `group`; 4–6: offset not velocity-corrected).
    pub kind: u8,
    /// ANM script in the player's `plXX.anm`.
    pub anm_script: i16,
    /// Sound effect id, -1 for none.
    pub sound: i16,
    /// When non-zero, fire on the long timer instead: `long_timer % alt_interval == alt_phase`.
    pub alt_interval: i8,
    pub alt_phase: i8,
    pub group: i32,
    /// Indexes into the game's tables of per-shot behaviour callbacks (e.g. homing).
    pub funcs: [i32; 4],
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
    pub power_levels: i32,
    pub option_script: i32,
    /// Option positions for level `n` (1-based): `options[0][n*(n-1)/2 ..][..n]`; `[1]` focused.
    pub options: [[[f32; 2]; 10]; 2],
    /// Bytes `0x20..0xF0`, including fields not decoded yet.
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
                let b = rec.peek_bytes(0, RECORD_SIZE)?;
                records.push(ShotRecord {
                    interval: b[0] as i8,
                    phase: b[1] as i8,
                    damage: rec.peek_u16(2)? as i16,
                    offset: [rec.peek_f32(4)?, rec.peek_f32(8)?],
                    hitbox: [rec.peek_f32(12)?, rec.peek_f32(16)?],
                    angle: rec.peek_f32(20)?,
                    unk_18: rec.peek_f32(24)?,
                    speed: rec.peek_f32(28)?,
                    origin: b[0x28],
                    kind: b[0x29],
                    anm_script: rec.peek_u16(0x2a)? as i16,
                    sound: rec.peek_u16(0x2c)? as i16,
                    alt_interval: b[0x32] as i8,
                    alt_phase: b[0x33] as i8,
                    group: rec.peek_i32(0x38)?,
                    funcs: [rec.peek_i32(0x48)?, rec.peek_i32(0x4c)?, rec.peek_i32(0x50)?, rec.peek_i32(0x54)?],
                    raw: b.to_vec(),
                });
                p += RECORD_SIZE;
                if records.len() > 256 {
                    return Err(Error::Format("runaway SHT record list".into()));
                }
            }
            slots.push(Some(records));
        }

        let mut options = [[[0.0; 2]; 10]; 2];
        for (set, at) in [(0, 0x50), (1, 0xa0)] {
            for (i, o) in options[set].iter_mut().enumerate() {
                *o = [r.peek_f32(at + i * 8)?, r.peek_f32(at + i * 8 + 4)?];
            }
        }
        Ok(Self {
            unk0: r.peek_u16(0)?,
            power_levels: r.peek_i32(0x20)?,
            option_script: r.peek_i32(0x3c)?,
            options,
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
