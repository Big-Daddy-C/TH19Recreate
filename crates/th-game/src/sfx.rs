//! Sound-effect groups and the stop request. Every group is ported from a `FUN_004bb8d0` call
//! site; a group can be muted at run time (see [`toggle`]) while checking others by ear.

/// Pan value of a request that stops sound `id` instead of playing it (`FUN_004bb7d0(id)`). In
/// the game a stop queued in the same frame as a play of that id wins.
pub const STOP: i32 = i32::MIN;

/// Id flag: add the player's side (0 left, 1 right) to the id, as the charge controller does
/// (`side + 0x54`). Such requests pan to the side.
pub const PER_SIDE: u32 = 0x8000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    /// The player's shots (SHT record sound ids, `FUN_0054e960`).
    PlayerShot,
    /// Enemy bullet firing (ET sound, `FUN_004f1340`).
    EnemyFire,
    /// An enemy taking damage (`FUN_00509980`).
    EnemyHit,
    /// An enemy's death burst (`FUN_0050c1f0`).
    EnemyDeath,
    /// Bomb start and end sounds.
    Bomb,
    /// Skill releases (Reimu, Marisa, Ran) and the Reimu skill end.
    Skill,
    Graze,
    PlayerHit,
    Barrier,
    Item,
    /// Laser creation (line, infinite and curved laser inits).
    Laser,
    /// Bullet bounces, steps, ex starts and the ex sound command.
    BulletEvent,
    /// Shot init callbacks that play 0x14 (Marisa's beams).
    BeamStart,
    /// Charge-shot hit callbacks (0x41).
    ChargeHit,
    /// The charge gauge (`FUN_004e79e0`): start, tier crossings, full charge loop.
    Charge,
    /// An EX or boss attack being sent (`FUN_0050db70`, 0x4d).
    Release,
    /// Spell card declaration (ECL 522, `FUN_004f67c0`).
    Spell,
    /// ECL 516: a script-chosen sound.
    Script,
    /// Opening the pause menu (`FUN_00539470`, 0xe).
    Pause,
}

impl Cat {
    pub const ALL: [Cat; 19] = [
        Cat::PlayerShot, Cat::EnemyFire, Cat::EnemyHit, Cat::EnemyDeath, Cat::Bomb, Cat::Skill, Cat::Graze,
        Cat::PlayerHit, Cat::Barrier, Cat::Item, Cat::Laser, Cat::BulletEvent, Cat::BeamStart, Cat::ChargeHit,
        Cat::Charge, Cat::Release, Cat::Spell, Cat::Script, Cat::Pause,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Cat::PlayerShot => "shot",
            Cat::EnemyFire => "enemy fire",
            Cat::EnemyHit => "enemy hit",
            Cat::EnemyDeath => "enemy death",
            Cat::Bomb => "bomb",
            Cat::Skill => "skill",
            Cat::Graze => "graze",
            Cat::PlayerHit => "player hit",
            Cat::Barrier => "barrier",
            Cat::Item => "item",
            Cat::Laser => "laser",
            Cat::BulletEvent => "bullet event",
            Cat::BeamStart => "beam",
            Cat::ChargeHit => "charge hit",
            Cat::Charge => "charge",
            Cat::Release => "release",
            Cat::Spell => "spell",
            Cat::Script => "script",
            Cat::Pause => "pause",
        }
    }

    fn bit(self) -> u32 {
        1 << Cat::ALL.iter().position(|&c| c == self).unwrap()
    }
}

/// Groups muted at run time (a bit per [`Cat::ALL`] entry); the viewer toggles them with F1..F12
/// (Shift for the rest) to find which group a sound belongs to.
static MUTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn on(c: Cat) -> bool {
    MUTED.load(std::sync::atomic::Ordering::Relaxed) & c.bit() == 0
}

/// Mute or unmute group number `index` of [`Cat::ALL`]; false when there is no such group.
pub fn toggle(index: usize) -> bool {
    let Some(c) = Cat::ALL.get(index) else { return false };
    MUTED.fetch_xor(c.bit(), std::sync::atomic::Ordering::Relaxed);
    true
}

/// Names of the muted groups.
pub fn muted() -> Vec<&'static str> {
    Cat::ALL.iter().filter(|&&c| !on(c)).map(|c| c.name()).collect()
}
