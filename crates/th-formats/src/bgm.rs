//! `thbgm.fmt`: index of the music tracks stored as raw PCM in `thbgm.dat`.
//!
//! Each 52-byte record (thtk thtypes/bgm_types.h, BSD-2-Clause):
//! `char name[16]; u32 offset; u32 preload; i32 intro_size; i32 track_size;
//!  WAVEFORMATEX (18 bytes); u16 pad`. A record with an empty name ends the list.
//! Playback: play `[offset, offset + track_size)`, then loop back to `offset + intro_size`.

use crate::{Reader, Result};

#[derive(Clone, Debug)]
pub struct Track {
    pub name: String,
    pub offset: u64,
    /// Bytes before the loop point.
    pub intro_size: u32,
    /// Total bytes in the track (intro + loop body).
    pub track_size: u32,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
}

impl Track {
    pub fn loop_start(&self) -> u64 {
        self.offset + self.intro_size as u64
    }
    pub fn end(&self) -> u64 {
        self.offset + self.track_size as u64
    }
}

const RECORD: usize = 52;

pub fn parse_fmt(data: &[u8]) -> Result<Vec<Track>> {
    let mut tracks = Vec::new();
    let mut p = 0;
    while p + RECORD <= data.len() {
        let r = Reader::at(data, p)?;
        let name = r.peek_cstr(0)?;
        if name.is_empty() {
            break;
        }
        tracks.push(Track {
            name,
            offset: r.peek_u32(16)? as u64,
            intro_size: r.peek_i32(24)? as u32,
            track_size: r.peek_i32(28)? as u32,
            channels: r.peek_u16(34)?,
            sample_rate: r.peek_u32(36)?,
            bits_per_sample: r.peek_u16(46)?,
        });
        p += RECORD;
    }
    Ok(tracks)
}
