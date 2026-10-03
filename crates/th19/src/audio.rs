//! Sound-effect output. The game logic only records `(sound id, pan)` events (as
//! `FUN_004bb8d0(id, pan)` does); this plays them. Sound ids and files come from th19.exe
//! (`th_formats::exe::sound_table`), the audio from th19.dat.

use std::num::NonZero;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use rodio::{DeviceSinkBuilder, MixerDeviceSink, Source};
use th_formats::{dat::Archive, exe::Exe, wav::Pcm};

pub struct Audio {
    /// Kept alive for as long as sound should play.
    sink: MixerDeviceSink,
    /// PCM and the sound's own gain (from its table volume) by sound id; `None` without a file.
    sounds: Vec<Option<(Pcm, f32)>>,
    /// One voice per sound id, as the game has one DirectSound buffer per id: playing an id again
    /// ends the previous copy instead of layering a second one. Each play bumps the id's counter;
    /// a playing copy stops when the counter has moved on.
    latest: Vec<Arc<AtomicU32>>,
    /// Extra volume 0..1 on top of the game's own level, changed with `-` / `+` (1.0 = the
    /// original's loudness).
    volume: f32,
}

impl Audio {
    /// `cfg` is th19.cfg: its byte 0xcb is the sound-effect volume setting (0..100, the game's
    /// `DAT_0062ad10`; 80 in the shipped file), 0xca the music volume.
    pub fn new(exe: &Exe, dat: &mut Archive, cfg: &std::path::Path) -> Option<Self> {
        let setting = std::fs::read(cfg).ok().and_then(|b| b.get(0xcb).copied()).unwrap_or(80).min(100) as f32;
        let sink = DeviceSinkBuilder::open_default_sink().map_err(|e| log::warn!("audio disabled: {e}")).ok()?;
        let sounds: Vec<Option<(Pcm, f32)>> = th_formats::exe::sound_table(exe)
            .into_iter()
            .map(|info| {
                let info = info?;
                let bytes = dat.read(&info.file).map_err(|e| log::warn!("sound: {e}")).ok()?;
                let pcm = th_formats::wav::parse(&bytes).map_err(|e| log::warn!("sound: {e}")).ok()?;
                // FUN_004bae70: DirectSound volume (hundredths of a dB) =
                // (table volume + 5000) * (1 - (1 - setting / 100)^3) - 5000, silent at setting 0.
                let curve = 1.0 - (1.0 - setting / 100.0).powi(3);
                let db100 = if setting == 0.0 { -10000.0 } else { (info.volume as f32 + 5000.0) * curve - 5000.0 };
                Some((pcm, 10f32.powf(db100 / 2000.0)))
            })
            .collect();
        let latest = sounds.iter().map(|_| Arc::new(AtomicU32::new(0))).collect();
        Some(Self { sink, sounds, latest, volume: 1.0 })
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn adjust_volume(&mut self, delta: f32) {
        self.volume = ((self.volume + delta) * 20.0).round().clamp(0.0, 20.0) / 20.0;
    }

    /// Play each requested id once, at the average of its pans (`FUN_004bae70` sums the queued
    /// pans and divides by their count). The game queues at most 12 distinct ids per frame.
    pub fn play_all(&mut self, events: &[(u32, i32)]) {
        let mut ids: Vec<(u32, i64, i64)> = Vec::new();
        for &(id, pan) in events {
            if let Some(e) = ids.iter_mut().find(|e| e.0 == id) {
                e.1 += pan as i64;
                e.2 += 1;
            } else if ids.len() < 12 {
                ids.push((id, pan as i64, 1));
            }
        }
        for (id, sum, count) in ids {
            self.play(id, (sum / count) as i32);
        }
    }

    /// `pan` is DirectSound's, in hundredths of a decibel: positive attenuates the left channel.
    pub fn play(&mut self, id: u32, pan: i32) {
        let Some(Some((pcm, own_gain))) = self.sounds.get(id as usize) else { return };
        if self.latest.len() <= id as usize { return; }
        let pan = pan.clamp(-10000, 10000) as f32;
        let gain = |attenuation: f32| 10f32.powf(-attenuation.max(0.0) / 2000.0);
        let (left, right) = (gain(pan), gain(-pan));
        let ch = pcm.channels as usize;
        let mut data = Vec::with_capacity(pcm.samples.len() / ch * 2);
        for frame in pcm.samples.chunks_exact(ch) {
            let mid = frame.iter().sum::<f32>() / ch as f32;
            let (l, r) = if ch >= 2 { (frame[0], frame[1]) } else { (mid, mid) };
            data.extend([l * left, r * right]);
        }
        let Some(rate) = NonZero::new(pcm.sample_rate) else { return };
        let counter = &self.latest[id as usize];
        let mine = counter.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        for x in &mut data {
            *x *= self.volume * own_gain;
        }
        self.sink.mixer().add(Voice { data, pos: 0, latest: counter.clone(), mine, rate });
    }
}

/// One playing copy of a sound: interleaved stereo that ends early when a newer copy of the same
/// id has started.
struct Voice {
    data: Vec<f32>,
    pos: usize,
    latest: Arc<AtomicU32>,
    mine: u32,
    rate: NonZero<u32>,
}

impl Iterator for Voice {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.latest.load(Ordering::Relaxed) != self.mine {
            return None;
        }
        let v = self.data.get(self.pos).copied();
        self.pos += 1;
        v
    }
}

impl Source for Voice {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(2).unwrap()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.rate
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}
