//! Sound-effect and music output. The game logic only records `(sound id, pan)` events (as
//! `FUN_004bb8d0(id, pan)` does); this plays them. Sound ids and files come from th19.exe
//! (`th_formats::exe::sound_table`), the audio from th19.dat; music tracks are raw PCM in
//! thbgm.dat, listed by thbgm.fmt.

use std::num::NonZero;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use rodio::{DeviceSinkBuilder, MixerDeviceSink, Source};
use th_formats::{dat::Archive, exe::Exe, wav::Pcm};

pub struct Audio {
    /// Kept alive for as long as sound should play.
    sink: MixerDeviceSink,
    /// PCM, table volume (hundredths of a dB) and loop flag by sound id; `None` without a file.
    sounds: Vec<Option<(Pcm, i16, bool)>>,
    /// One voice per sound id, as the game has one DirectSound buffer per id: playing an id again
    /// ends the previous copy instead of layering a second one. Each play bumps the id's counter;
    /// a playing copy stops when the counter has moved on.
    latest: Vec<Arc<AtomicU32>>,
    /// Sound-effect and music volume settings, 0..=100 (the game's `DAT_0062bddf` /
    /// `DAT_0062bdde`, saved in th19.cfg bytes 0xcb / 0xca).
    se_setting: u8,
    bgm_setting: u8,
    /// Music: the track list, the file, the running track's file name, and the values the playing
    /// source reads (a stop counter and its gain as `f32` bits).
    tracks: Vec<th_formats::bgm::Track>,
    bgm_path: PathBuf,
    music: Option<String>,
    music_latest: Arc<AtomicU32>,
    music_gain: Arc<AtomicU32>,
    /// 2/3 while the menu is open (everything is 1/3 quieter), otherwise 1.
    duck: f32,
}

/// `FUN_004bb970`: DirectSound volume (hundredths of a dB) of the music for a setting,
/// `(-5000 - trunc((1 - (1 - s / 100)^4) * -5000)) * 9 / 10`. The game also silences the music when
/// the sound-effect setting is 0 (a quirk the settings menu here does not copy).
pub fn music_db100(setting: u8) -> i32 {
    let s = setting as f32 / 100.0;
    let v = -5000 - ((1.0 - (1.0 - s).powi(4)) * -5000.0) as i32;
    v * 9 / 10
}

/// `FUN_004bae70`: DirectSound volume (hundredths of a dB) of a sound effect with table volume
/// `volume`, `(volume + 5000) * (1 - (1 - s / 100)^3) - 5000`, silent at setting 0.
pub fn se_db100(volume: i16, setting: u8) -> f32 {
    if setting == 0 {
        return -10000.0;
    }
    let curve = 1.0 - (1.0 - setting as f32 / 100.0).powi(3);
    (volume as f32 + 5000.0) * curve - 5000.0
}

/// Amplitude of a DirectSound volume in hundredths of a decibel.
fn gain(db100: f32) -> f32 {
    if db100 <= -10000.0 { 0.0 } else { 10f32.powf(db100 / 2000.0) }
}

impl Audio {
    /// `cfg` is th19.cfg (settings default to its bytes 0xcb / 0xca, 80 and 100 in the shipped
    /// file, rounded to the nearest 10); `bgm` is thbgm.dat.
    pub fn new(exe: &Exe, dat: &mut Archive, cfg: &std::path::Path, bgm: &std::path::Path) -> Option<Self> {
        let bytes = std::fs::read(cfg).unwrap_or_default();
        let setting = |at: usize, default: u8| (bytes.get(at).copied().unwrap_or(default).min(100) + 5) / 10 * 10;
        let sink = DeviceSinkBuilder::open_default_sink().map_err(|e| log::warn!("audio disabled: {e}")).ok()?;
        let sounds: Vec<Option<(Pcm, i16, bool)>> = th_formats::exe::sound_table(exe)
            .into_iter()
            .map(|info| {
                let info = info?;
                let bytes = dat.read(&info.file).map_err(|e| log::warn!("sound: {e}")).ok()?;
                let pcm = th_formats::wav::parse(&bytes).map_err(|e| log::warn!("sound: {e}")).ok()?;
                Some((pcm, info.volume, info.looped))
            })
            .collect();
        let latest = sounds.iter().map(|_| Arc::new(AtomicU32::new(0))).collect();
        let tracks = dat.read("thbgm.fmt").ok().and_then(|b| th_formats::bgm::parse_fmt(&b).ok()).unwrap_or_default();
        let bgm_setting = setting(0xca, 100);
        Some(Self {
            sink,
            sounds,
            latest,
            se_setting: setting(0xcb, 80),
            bgm_setting,
            tracks,
            bgm_path: bgm.to_path_buf(),
            music: None,
            music_latest: Arc::new(AtomicU32::new(0)),
            music_gain: Arc::new(AtomicU32::new(gain(music_db100(bgm_setting) as f32).to_bits())),
            duck: 1.0,
        })
    }

    pub fn se_setting(&self) -> u8 {
        self.se_setting
    }

    pub fn bgm_setting(&self) -> u8 {
        self.bgm_setting
    }

    pub fn set_se_setting(&mut self, setting: u8) {
        self.se_setting = setting.min(100);
    }

    /// Takes effect immediately on the running track.
    pub fn set_bgm_setting(&mut self, setting: u8) {
        self.bgm_setting = setting.min(100);
        self.apply_music_gain();
    }

    fn apply_music_gain(&self) {
        let g = gain(music_db100(self.bgm_setting) as f32) * self.duck;
        self.music_gain.store(g.to_bits(), Ordering::Relaxed);
    }

    /// Lower everything by a third while the menu is open.
    pub fn set_duck(&mut self, on: bool) {
        self.duck = if on { 2.0 / 3.0 } else { 1.0 };
        self.apply_music_gain();
    }

    /// Names (`th19_NN`) of the music tracks in thbgm.fmt order.
    pub fn track_names(&self) -> Vec<String> {
        self.tracks.iter().map(|t| t.name.trim_end_matches(".wav").to_string()).collect()
    }


    /// Start the track `name` (`th19_NN`) from the beginning, looping at its loop point. Playing
    /// the track that is already running does nothing.
    pub fn play_music(&mut self, name: &str) {
        let file = format!("{name}.wav");
        if self.music.as_deref() == Some(file.as_str()) {
            return;
        }
        let Some(track) = self.tracks.iter().find(|t| t.name == file).cloned() else {
            log::warn!("music {file} is not in thbgm.fmt");
            return;
        };
        let read = || -> std::io::Result<Vec<u8>> {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = std::fs::File::open(&self.bgm_path)?;
            f.seek(SeekFrom::Start(track.offset))?;
            let mut raw = vec![0u8; track.track_size as usize];
            f.read_exact(&mut raw)?;
            Ok(raw)
        };
        let raw = match read() {
            Ok(raw) => raw,
            Err(e) => return log::warn!("music {file}: {e}"),
        };
        let Some(rate) = NonZero::new(track.sample_rate) else { return };
        let data: Vec<i16> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let mine = self.music_latest.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let loop_start = (track.intro_size as usize / 2).min(data.len());
        self.sink.mixer().add(MusicSource {
            data,
            pos: 0,
            loop_start,
            rate,
            channels: NonZero::new(track.channels.max(1)).unwrap(),
            latest: self.music_latest.clone(),
            mine,
            gain: self.music_gain.clone(),
        });
        self.music = Some(file);
    }

    pub fn stop_music(&mut self) {
        self.music_latest.fetch_add(1, Ordering::Relaxed);
        self.music = None;
    }

    /// One frame of requests (`FUN_004bae70`): each id plays once at the average of its queued
    /// pans, restarting it; a stop request ([`th_game::sfx::STOP`]) for an id in the same frame
    /// wins over its plays. The game queues at most 12 distinct ids per frame.
    pub fn play_all(&mut self, events: &[(u32, i32)]) {
        // (id, pan sum, count, stopped)
        let mut ids: Vec<(u32, i64, i64, bool)> = Vec::new();
        for &(id, pan) in events {
            let entry = match ids.iter().position(|e| e.0 == id) {
                Some(i) => &mut ids[i],
                None if ids.len() < 12 => {
                    ids.push((id, 0, 0, false));
                    ids.last_mut().unwrap()
                }
                None => continue,
            };
            if pan == th_game::sfx::STOP {
                entry.3 = true;
            } else {
                entry.1 += pan as i64;
                entry.2 += 1;
            }
        }
        for (id, sum, count, stopped) in ids {
            if stopped {
                self.stop(id);
            } else if count > 0 {
                self.play(id, (sum / count) as i32);
            }
        }
    }

    /// End whatever copy of `id` is playing.
    pub fn stop(&mut self, id: u32) {
        if let Some(counter) = self.latest.get(id as usize) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `pan` is DirectSound's, in hundredths of a decibel: positive attenuates the left channel.
    pub fn play(&mut self, id: u32, pan: i32) {
        let Some(Some((pcm, volume, looped))) = self.sounds.get(id as usize) else { return };
        if self.latest.len() <= id as usize {
            return;
        }
        let own_gain = gain(se_db100(*volume, self.se_setting)) * self.duck;
        let pan = pan.clamp(-10000, 10000) as f32;
        let side = |attenuation: f32| 10f32.powf(-attenuation.max(0.0) / 2000.0);
        let (left, right) = (side(pan), side(-pan));
        let ch = pcm.channels as usize;
        let mut data = Vec::with_capacity(pcm.samples.len() / ch * 2);
        for frame in pcm.samples.chunks_exact(ch) {
            let mid = frame.iter().sum::<f32>() / ch as f32;
            let (l, r) = if ch >= 2 { (frame[0], frame[1]) } else { (mid, mid) };
            data.extend([l * left * own_gain, r * right * own_gain]);
        }
        let Some(rate) = NonZero::new(pcm.sample_rate) else { return };
        let counter = &self.latest[id as usize];
        let mine = counter.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        self.sink.mixer().add(Voice { data, pos: 0, latest: counter.clone(), mine, rate, looped: *looped });
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
    /// DSBPLAY_LOOPING (table +0xc): repeat until stopped.
    looped: bool,
}

impl Iterator for Voice {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.latest.load(Ordering::Relaxed) != self.mine {
            return None;
        }
        if self.looped && self.pos >= self.data.len() {
            self.pos = 0;
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

/// The running music track: interleaved 16-bit PCM that plays once from the start and then loops
/// from `loop_start`, scaled by the shared gain; ends when a newer track or a stop bumps `latest`.
struct MusicSource {
    data: Vec<i16>,
    pos: usize,
    loop_start: usize,
    rate: NonZero<u32>,
    channels: NonZero<u16>,
    latest: Arc<AtomicU32>,
    mine: u32,
    gain: Arc<AtomicU32>,
}

impl Iterator for MusicSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.latest.load(Ordering::Relaxed) != self.mine || self.data.is_empty() {
            return None;
        }
        if self.pos >= self.data.len() {
            self.pos = self.loop_start.min(self.data.len().saturating_sub(1));
        }
        let v = self.data[self.pos] as f32 / 32768.0 * f32::from_bits(self.gain.load(Ordering::Relaxed));
        self.pos += 1;
        Some(v)
    }
}

impl Source for MusicSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.rate
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn music_and_effect_levels_follow_the_game_formulas() {
        // Music: setting 100 is 0 dB; the others follow FUN_004bb970.
        assert_eq!(music_db100(100), 0);
        assert_eq!(music_db100(50), (-5000 - (-4687.5f32 as i32)) * 9 / 10);
        assert_eq!(music_db100(0), (-5000) * 9 / 10);
        // Sound effects: setting 100 leaves the table volume, 0 is silent.
        assert_eq!(se_db100(-2800, 100), -2800.0);
        assert_eq!(se_db100(-2800, 0), -10000.0);
        assert!(se_db100(-300, 50) < -300.0 && se_db100(-300, 50) > se_db100(-300, 20));
    }

    #[test]
    fn music_plays_the_intro_once_then_loops() {
        let mut m = MusicSource {
            data: (0..10).map(|i| (i * 1000) as i16).collect(),
            pos: 0,
            loop_start: 6,
            rate: NonZero::new(44100).unwrap(),
            channels: NonZero::new(2).unwrap(),
            latest: Arc::new(AtomicU32::new(1)),
            mine: 1,
            gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        };
        let got: Vec<i32> = (0..14).map(|_| (m.next().unwrap() * 32768.0).round() as i32).collect();
        assert_eq!(got, [0, 1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 9000, 6000, 7000, 8000, 9000]);
        m.latest.store(2, Ordering::Relaxed);
        assert!(m.next().is_none(), "a newer track or a stop ends it");
    }
}
