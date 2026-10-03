//! Sound-effect output. The game logic only records `(sound id, pan)` events (as
//! `FUN_004bb8d0(id, pan)` does); this plays them. Sound ids and files come from th19.exe
//! (`th_formats::exe::sound_table`), the audio from th19.dat.

use std::collections::HashSet;

use rodio::{buffer::SamplesBuffer, DeviceSinkBuilder, MixerDeviceSink, Player};
use th_formats::{dat::Archive, exe::Exe, wav::Pcm};

pub struct Audio {
    /// Kept alive for as long as sound should play.
    sink: MixerDeviceSink,
    /// PCM by sound id (`None` for ids without a file).
    sounds: Vec<Option<Pcm>>,
    /// One voice per sound id, as the game has one DirectSound buffer per id: playing an id again
    /// restarts it instead of layering a second copy.
    voices: Vec<Option<Player>>,
    /// Sound volume 0..1, changed with `-` / `+`. APPROX: the game's own volume setting
    /// (`th19.cfg`) is not read yet.
    volume: f32,
}

impl Audio {
    pub fn new(exe: &Exe, dat: &mut Archive) -> Option<Self> {
        let sink = DeviceSinkBuilder::open_default_sink().map_err(|e| log::warn!("audio disabled: {e}")).ok()?;
        let sounds: Vec<Option<Pcm>> = th_formats::exe::sound_table(exe)
            .into_iter()
            .map(|name| {
                let bytes = dat.read(&name?).map_err(|e| log::warn!("sound: {e}")).ok()?;
                th_formats::wav::parse(&bytes).map_err(|e| log::warn!("sound: {e}")).ok()
            })
            .collect();
        let voices: Vec<Option<Player>> = sounds.iter().map(|_| None).collect();
        Some(Self { sink, sounds, voices, volume: 0.1 })
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn adjust_volume(&mut self, delta: f32) {
        self.volume = ((self.volume + delta) * 20.0).round().clamp(0.0, 20.0) / 20.0;
        for v in self.voices.iter().flatten() {
            v.set_volume(self.volume);
        }
    }

    /// Play every distinct sound once (the game queues each id at most once per frame, with
    /// one pan per request; the first is used here).
    pub fn play_all(&mut self, events: &[(u32, i32)]) {
        // FUN_004bb8d0 keeps at most 12 distinct ids per frame.
        let mut seen = HashSet::new();
        for &(id, pan) in events {
            if seen.len() < 12 && seen.insert(id) {
                self.play(id, pan);
            }
        }
    }

    /// `pan` is DirectSound's, in hundredths of a decibel: positive attenuates the left channel.
    pub fn play(&mut self, id: u32, pan: i32) {
        let Some(Some(pcm)) = self.sounds.get(id as usize) else { return };
        if self.voices.len() <= id as usize { return; }
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
        let (Some(channels), Some(rate)) = (std::num::NonZero::new(2u16), std::num::NonZero::new(pcm.sample_rate)) else { return };
        let mixer = self.sink.mixer();
        let voice = self.voices[id as usize].get_or_insert_with(|| Player::connect_new(mixer));
        voice.clear();
        voice.set_volume(self.volume);
        voice.append(SamplesBuffer::new(channels, rate, data));
        voice.play();
    }
}
