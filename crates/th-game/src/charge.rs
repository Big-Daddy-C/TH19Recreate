//! Charge gauge mechanics from AuraCtrlInf::update (FUN_004e79e0).
//! Kept separate from character callbacks: no guessed thresholds or local attack effects.
//! The viewer enables all four Reimu tiers with the meter economy bypassed for practice.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Status +0x28, +0x2c, +0x30, +0x34.
    pub attack: i32,
    pub skill: i32,
    pub extra: i32,
    pub boss: i32,
    /// Status +0x88, clamped 0..200; charge advances by percent*10/100 each tick.
    pub speed_percent: i32,
}
impl Settings {
    /// FUN_005478f0: SHT offsets 0x2c, 0x30, 0x34, 0x38 multiplied by ten,
    /// then clamped to the status field range. Speed is a separate match/ability setting.
    pub fn from_sht(sht: &th_formats::sht::ShtFile, speed_percent: i32) -> Self {
        let read = |offset: usize| {
            let at = offset - 0x20;
            i32::from_le_bytes(sht.params_raw[at..at+4].try_into().unwrap())
                .saturating_mul(10).clamp(0, 2500)
        };
        Self { attack: read(0x2c), skill: read(0x30), extra: read(0x34), boss: read(0x38),
               speed_percent: speed_percent.clamp(0, 200) }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level { Attack, Skill, Extra, Boss }
#[derive(Default, Debug)]
pub struct Gauge {
    pub value: i32,
    recovery: u8,
    /// Charging has started (the controller's timer +0x18 is running).
    holding: bool,
    /// Sound requests `(id, pan)` of this tick for the side's player; ids carry
    /// [`crate::sfx::PER_SIDE`] (`side + 0x54` etc. in the controller) and pan to the side.
    pub sounds: Vec<(u32, i32)>,
}
impl Gauge {
    /// Development aid: set the gauge as if it had been held up to `value`.
    pub fn fill(&mut self, value: i32) {
        self.value = value.clamp(0, 2500);
        self.recovery = 0;
    }
    /// Hold to fill; release to select a level. Invalid player states cancel silently.
    /// `meter` is status +0x24 (0..2500). Boss-active falls back to EX, as in the original.
    /// Skill blocked represents the character callback returning nonzero.
    pub fn tick(&mut self, held: bool, allowed: bool, meter: i32, settings: Settings,
                boss_active: bool, skill_blocked: bool) -> Option<Level> {
        // FUN_004e79e0 sounds (ids are side + 0x54/0x56/0x58, panned to the side): 0x54 (ch00) when
        // charging starts, 0x56 (ch01) and 0x54 again when the gauge lands in a tier's 10-unit
        // bucket, stop 0x54 and loop 0x58 (ch03) when it reaches the meter cap, and stop 0x54 and
        // 0x58 whenever the gauge is not being charged.
        const START: u32 = 0x54 | crate::sfx::PER_SIDE;
        const TIER: u32 = 0x56 | crate::sfx::PER_SIDE;
        const FULL: u32 = 0x58 | crate::sfx::PER_SIDE;
        let sound = crate::sfx::on(crate::sfx::Cat::Charge);
        let stop = |g: &mut Self| {
            g.holding = false;
            if sound { g.sounds.extend([(START, crate::sfx::STOP), (FULL, crate::sfx::STOP)]); }
        };
        if !allowed { self.value = 0; stop(self); return None; }
        if self.recovery > 0 { self.recovery -= 1; stop(self); return None; }
        if held {
            if !self.holding {
                self.holding = true;
                if sound { self.sounds.push((START, 0)); }
            }
            let cap = meter.clamp(0, 2500);
            if self.value < cap {
                let new = self.value + settings.speed_percent.clamp(0, 200)*10/100;
                if sound && [settings.boss, settings.extra, settings.skill, settings.attack].iter().any(|t| new / 10 == t / 10) {
                    self.sounds.extend([(TIER, 0), (START, 0)]);
                }
                if cap <= new {
                    self.value = cap;
                    if sound { self.sounds.extend([(START, crate::sfx::STOP), (FULL, 0)]); }
                } else {
                    self.value = new;
                }
            }
            return None;
        }
        stop(self);
        if self.value == 0 { return None; }
        let value = std::mem::take(&mut self.value);
        self.recovery = 10;
        // Character charge attack runs alongside any higher-level dispatch.
        if value >= settings.boss && !boss_active { Some(Level::Boss) }
        else if value >= settings.extra { Some(Level::Extra) }
        else if value >= settings.skill && !skill_blocked { Some(Level::Skill) }
        else if value >= settings.attack { Some(Level::Attack) }
        else { None }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    // Synthetic thresholds exercise the controller; these are NOT claimed game defaults.
    const S: Settings = Settings { attack: 100, skill: 200, extra: 300, boss: 400, speed_percent: 100 };
    fn release(value: i32, boss: bool, blocked: bool) -> Option<Level> {
        Gauge { value, ..Default::default() }.tick(false, true, 500, S, boss, blocked)
    }
    #[test]
    fn boundaries_and_callback_gates() {
        assert_eq!(release(99, false, false), None);
        assert_eq!(release(100, false, false), Some(Level::Attack));
        assert_eq!(release(200, false, false), Some(Level::Skill));
        assert_eq!(release(300, false, false), Some(Level::Extra));
        assert_eq!(release(400, false, false), Some(Level::Boss));
        assert_eq!(release(400, true, false), Some(Level::Extra));
        assert_eq!(release(200, false, true), Some(Level::Attack));
    }
    #[test]
    fn meter_cap_and_recovery() {
        let mut g = Gauge::default();
        for _ in 0..50 { assert_eq!(g.tick(true, true, 200, S, false, false), None); }
        assert_eq!(g.value, 200);
        assert_eq!(g.tick(false, true, 200, S, false, false), Some(Level::Skill));
        for _ in 0..10 { g.tick(true, true, 200, S, false, false); assert_eq!(g.value, 0); }
        g.tick(true, true, 200, S, false, false);
        assert_eq!(g.value, 10);
    }
    #[test]
    fn interrupted_charge_cancels_without_attack() {
        let mut g = Gauge { value: 400, ..Default::default() };
        assert_eq!(g.tick(false, false, 500, S, false, false), None);
        assert_eq!(g.value, 0);
    }
    /// FUN_004e79e0: start once, a tier sound on each threshold bucket, the full-charge loop at the
    /// cap, and stops while not charging.
    #[test]
    fn charge_sounds_follow_the_controller() {
        use crate::sfx::{PER_SIDE, STOP};
        let mut g = Gauge::default();
        let mut log = Vec::new();
        for _ in 0..25 {
            g.tick(true, true, 250, S, false, false);
            log.push(std::mem::take(&mut g.sounds));
        }
        assert_eq!(log[0], vec![(0x54 | PER_SIDE, 0)], "start on the first held tick");
        // Value 100 (attack threshold) is reached on the 10th tick.
        assert_eq!(log[9], vec![(0x56 | PER_SIDE, 0), (0x54 | PER_SIDE, 0)]);
        assert_eq!(log[24], vec![(0x54 | PER_SIDE, STOP), (0x58 | PER_SIDE, 0)], "cap 250 reached");
        assert!(log[1..9].iter().all(|v| v.is_empty()));
        g.tick(false, true, 250, S, false, false);
        assert_eq!(g.sounds, vec![(0x54 | PER_SIDE, STOP), (0x58 | PER_SIDE, STOP)]);
    }
}
