//! Minimal RIFF/WAVE reader for the game's sound effects (16-bit or 8-bit PCM).

use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct Pcm {
    pub channels: u16,
    pub sample_rate: u32,
    /// Interleaved samples in -1..1.
    pub samples: Vec<f32>,
}

pub fn parse(data: &[u8]) -> Result<Pcm> {
    let bad = |m: &str| Error::Format(format!("wav: {m}"));
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(bad("not a RIFF/WAVE file"));
    }
    let (mut p, mut fmt, mut pcm) = (12usize, None, None);
    while p + 8 <= data.len() {
        let size = u32::from_le_bytes(data[p + 4..p + 8].try_into().unwrap()) as usize;
        let body = &data[p + 8..(p + 8 + size).min(data.len())];
        match &data[p..p + 4] {
            b"fmt " if body.len() >= 16 => {
                let tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                if tag != 1 || channels == 0 || !matches!(bits, 8 | 16) {
                    return Err(bad("only PCM 8/16-bit is supported"));
                }
                fmt = Some((channels, rate, bits));
            }
            b"data" => pcm = Some(body),
            _ => {}
        }
        p += 8 + size + (size & 1);
    }
    let ((channels, sample_rate, bits), body) = fmt.zip(pcm).ok_or_else(|| bad("missing fmt or data chunk"))?;
    let samples = if bits == 16 {
        body.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
    } else {
        body.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect()
    };
    Ok(Pcm { channels, sample_rate, samples })
}
