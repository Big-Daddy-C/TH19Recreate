//! Charge gauge mechanics from AuraCtrlInf::update (FUN_004e79e0).
//! Kept separate from character callbacks: no guessed thresholds or local attack effects.
//! The viewer currently enables the first tier; higher tiers await their character callbacks.
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
}
impl Gauge {
    /// Hold to fill; release to select a level. Invalid player states cancel silently.
    /// `meter` is status +0x24 (0..2500). Boss-active falls back to EX, as in the original.
    /// Skill blocked represents the character callback returning nonzero.
    pub fn tick(&mut self, held: bool, allowed: bool, meter: i32, settings: Settings,
                boss_active: bool, skill_blocked: bool) -> Option<Level> {
        if !allowed { self.value = 0; return None; }
        if self.recovery > 0 { self.recovery -= 1; return None; }
        if held {
            let cap = meter.clamp(0, 2500);
            self.value = (self.value + settings.speed_percent.clamp(0, 200)*10/100).min(cap);
            return None;
        }
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
        Gauge { value, recovery: 0 }.tick(false, true, 500, S, boss, blocked)
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
        let mut g = Gauge { value: 400, recovery: 0 };
        assert_eq!(g.tick(false, false, 500, S, false, false), None);
        assert_eq!(g.value, 0);
    }
}
