//! Which music plays on which stage.
//!
//! Every character has a stage (the last int of its 0x20-byte row in the character table at
//! 0x5c5c24, which `FUN_00412d90` indexes as `0x5c5e80 + stage * 0x10`, a `worldNN.std` row with
//! NN = stage + 1). A stage plays the theme of a character who uses it; where several do, the
//! caller picks one at random. The theme of each character is its entry in the game's music room
//! (`musiccmt.txt` names the character of every `th19_NN` track); the characters are identified
//! by the boss sprite of each `plNNb.anm` (`boss_reimu` ... `boss_zanmu`).

use crate::exe::Exe;

/// `th19_NN` track of each character `pl00`..`pl18`: Reimu, Marisa, Sanae, Ran, Aunn, Nazrin,
/// Seiran, Orin, Tsukasa, Mamizou, Yachie, Saki, Yuuma, Suika, Biten, Enoko, Chiyari, Hisami,
/// Zanmu.
pub const CHARACTER_THEMES: [&str; 19] = [
    "th19_02", "th19_03", "th19_13", "th19_14", "th19_06", "th19_04", "th19_05", "th19_15", "th19_16",
    "th19_17", "th19_18", "th19_19", "th19_20", "th19_07", "th19_08", "th19_09", "th19_10", "th19_11",
    "th19_12",
];

const CHARACTER_TABLE: u32 = 0x5c5c24;

/// Each character's stage index (0-based; `world01.std` is stage 0), read from th19.exe.
pub fn character_stages(exe: &Exe) -> Vec<u32> {
    (0..CHARACTER_THEMES.len() as u32)
        .map(|c| exe.i32_at(CHARACTER_TABLE + c * 0x20 + 0x1c).unwrap_or(-1) as u32)
        .collect()
}

/// Themes of the characters whose stage is `stage` (empty when no character uses it).
pub fn stage_themes(stages: &[u32], stage: u32) -> Vec<&'static str> {
    stages.iter().zip(CHARACTER_THEMES).filter(|(s, _)| **s == stage).map(|(_, t)| t).collect()
}

/// A short English label for a track (`th19_NN`), for the song selector.
pub fn track_label(name: &str) -> String {
    let num: u32 = name.trim_start_matches("th19_").parse().unwrap_or(0);
    let who = match num {
        1 => "Title",
        21 => "Ending",
        22 => "Staff roll",
        _ => match CHARACTER_THEMES.iter().position(|t| *t == name) {
            Some(c) => CHARACTER_NAMES[c],
            None => "",
        },
    };
    if who.is_empty() { format!("{num:02}") } else { format!("{num:02} {who}") }
}

/// The nineteen characters in `pl00`..`pl18` order.
pub const CHARACTER_NAMES: [&str; 19] = [
    "Reimu", "Marisa", "Sanae", "Ran", "Aunn", "Nazrin", "Seiran", "Orin", "Tsukasa", "Mamizou", "Yachie",
    "Saki", "Yuuma", "Suika", "Biten", "Enoko", "Chiyari", "Hisami", "Zanmu",
];
