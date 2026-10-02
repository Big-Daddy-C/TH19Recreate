//! Gameplay: enemies driven by ECL, the wave system, the player and player shots.

pub mod bullet;
pub mod charge;
pub mod round;
pub mod enemy;
pub mod motion;
pub mod player;
pub mod side;

pub use enemy::{Enemy, EnemyData};
pub use side::{Side, SideState};

/// Playfield rectangles on the 640x480 canvas `[x, y, w, h]` (from th19.exe `FUN_004ba5f0`:
/// x 16..312 / 328..624, y 16..464). Field coordinates have their origin at the top centre,
/// x in [-148, 148], y in [0, 448].
pub const FIELDS: [[f32; 4]; 2] = [[16.0, 16.0, 296.0, 448.0], [328.0, 16.0, 296.0, 448.0]];
