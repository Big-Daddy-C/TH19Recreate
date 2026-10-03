//! Which sound effects are switched on. The sound hooks are ported from the original one group
//! at a time so each can be checked by ear; add a [`Cat`] to [`ENABLED`] to turn a group on.

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
    /// Skill / charge releases (Reimu, Marisa, Ran) and the Reimu skill end.
    Skill,
    Graze,
    PlayerHit,
    Barrier,
    Item,
    Laser,
    /// Bullet bounces, steps and the ex sound command.
    BulletEvent,
    /// Marisa's beam start.
    BeamStart,
    /// Charge-shot hit callbacks (0x41).
    ChargeHit,
}

/// Groups currently audible.
pub const ENABLED: &[Cat] =
    &[Cat::PlayerShot, Cat::EnemyHit, Cat::EnemyDeath, Cat::Bomb, Cat::Graze, Cat::PlayerHit, Cat::Barrier, Cat::Item];

pub fn on(c: Cat) -> bool {
    ENABLED.contains(&c)
}
