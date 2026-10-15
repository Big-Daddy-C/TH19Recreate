//! The game's random number generator (`FUN_00402540` on `DAT_005c6620`, the replay RNG).
//!
//! A Park-Miller "minimal standard" generator: `state = state * 48271 mod (2^31 - 1)`, and each
//! draw returns `state % divisor` (RNG +0x10; 0xffff00 for the replay RNG, set by
//! `FUN_005636xx` together with the seed `seed mod (2^31 - 1)`, 0 replaced). The float helpers
//! inline the same three shapes everywhere (`FUN_00404b70`, `FUN_00404bd0`, the open-coded
//! copies): `r / (d - 1)`, `r / (d / 2 - 1) - 1` and `r / (d / 2pi - pi) - pi`, in single
//! precision.
//!
//! APPROX (sequencing): the game draws every side's numbers from this one global generator, in
//! its own call order; the port keeps one per side (plus local ones) and calls them in its own
//! order, so individual values differ even though each draw has the original's distribution.

pub const MODULUS: u64 = 0x7fff_ffff;
pub const MULTIPLIER: u64 = 48271;
/// The replay RNG's divisor (+0x10).
pub const DIVISOR: u32 = 0x00ff_ff00;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u32,
}

impl Rng {
    /// Seeded like the replay RNG: `seed mod (2^31 - 1)`, with 0 replaced by 1.
    pub fn new(seed: u32) -> Self {
        let s = (seed as u64 % MODULUS) as u32;
        Self { state: if s == 0 { 1 } else { s } }
    }

    /// `FUN_00402540`: the next state, reduced by the divisor.
    pub fn next(&mut self) -> u32 {
        self.state = (self.state as u64 * MULTIPLIER % MODULUS) as u32;
        self.state % DIVISOR
    }

    /// `FUN_00401960`: the same step, returning the state's low 16 bits (no divisor).
    pub fn next_u16(&mut self) -> u16 {
        self.state = (self.state as u64 * MULTIPLIER % MODULUS) as u32;
        self.state as u16
    }

    /// `FUN_004042b0`: `(r / (d / 2 - 1) - 1) * pi`.
    pub fn signed_pi(&mut self) -> f32 {
        self.signed() * std::f32::consts::PI
    }

    /// `r / (d - 1)`: [0, 1].
    pub fn unit(&mut self) -> f32 {
        self.next() as f32 / (DIVISOR as f32 - 1.0)
    }

    /// `FUN_00404b70`: `r / (d / 2 - 1) - 1`, [-1, 1].
    pub fn signed(&mut self) -> f32 {
        self.next() as f32 / (DIVISOR as f32 / 2.0 - 1.0) - 1.0
    }

    /// `r / (d / 2pi - pi) - pi`: an angle in [-pi, pi].
    pub fn angle(&mut self) -> f32 {
        use std::f32::consts::{PI, TAU};
        self.next() as f32 / (DIVISOR as f32 / TAU - PI) - PI
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_standard_sequence() {
        // 48271 is the multiplier; with state 1 the first raw states are 48271 and 48271^2 mod m.
        let mut r = Rng::new(1);
        assert_eq!(r.next(), 48271);
        assert_eq!(r.next(), (48271u64 * 48271 % 0x7fff_ffff % 0xffff00) as u32);
        assert_eq!(Rng::new(0), Rng::new(0x7fff_ffff), "zero seeds become 1");
    }

    #[test]
    fn helpers_cover_their_ranges() {
        let mut r = Rng::new(12345);
        for _ in 0..10000 {
            let (u, s, a) = (r.unit(), r.signed(), r.angle());
            assert!((0.0..=1.0).contains(&u));
            assert!((-1.0..=1.0000002).contains(&s));
            assert!((-std::f32::consts::PI..=std::f32::consts::PI + 1e-5).contains(&a));
        }
    }
}
