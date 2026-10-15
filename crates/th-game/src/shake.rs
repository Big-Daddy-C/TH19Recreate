//! Screen shake (`ScreenInf` effect type 1: `FUN_004b9f00` -> `FUN_004b9d40` -> `FUN_004ba250`).
//!
//! Each frame the effect advances its timer and stops once the timer reaches its duration;
//! otherwise its size runs linearly `start + (end - start) * t / duration`. Each axis then
//! picks -s, 0 or +s at random (`rand % 3`): case 1 is +s, case 2 is -s, except that case 2 on
//! the x axis still writes +s to the field cameras (only the unscaled copy `DAT_0062c414`
//! gets -s). The values go to slot `index` of the three field cameras (0x62be14, 0x62bfa0,
//! 0x62c12c, +0x14c, scaled by the resolution) and unconditionally to `DAT_0062c414/418`,
//! which `FUN_004ec1e0` copies into the stage camera. A task at priority 0x68 (`0x532090`)
//! zeroes them all every frame, so nothing outlives the effect.
//!
//! Field drawing selects slot 0 or 1 by side (`FUN_004cc030`, `FUN_00405690`); slot 2 is never
//! read for a field, so ECL 517 (`FUN_004fecc0` case 0x3e: `(duration, start, end)` on slot 2)
//! shakes only the stage behind both fields. A bomb ending (`FUN_0053fac0` and the other bomb
//! updates) shakes its own field: 6 for 15 frames on slot = side. The offsets reach sprite
//! quads (`FUN_004c9380` adds manager +0xc4 / +0xc8 in pixels); primitives do not use them.
#[derive(Clone, Copy, Debug)]
pub struct Shake {
    pub duration: i32,
    pub start: i32,
    pub end: i32,
    /// Camera slot: 0 / 1 = that side's field, 2 = neither field.
    pub index: usize,
    timer: i32,
}

impl Shake {
    pub fn new(duration: i32, start: i32, end: i32, index: usize) -> Self {
        Self { duration, start, end, index, timer: 0 }
    }
    /// The bomb's end (`FUN_004b9f00(1, 15, 6, 6, .., side)`).
    pub fn bomb(side: usize) -> Self {
        Self::new(15, 6, 6, side)
    }
}

/// This frame's offsets: per field (in canvas units, the cameras' scaled values) and for the
/// stage (the unscaled value, in screen pixels).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Offsets {
    pub field: [[f32; 2]; 2],
    pub stage_pixels: [f32; 2],
}

#[derive(Clone, Debug)]
pub struct Shakes {
    list: Vec<Shake>,
    rng: crate::rng::Rng,
}

impl Default for Shakes {
    fn default() -> Self {
        Self { list: Vec::new(), rng: crate::rng::Rng::new(0x2545f491) }
    }
}

impl Shakes {
    pub fn add(&mut self, shake: Shake) {
        self.list.push(shake);
    }

    pub fn active(&self) -> bool {
        !self.list.is_empty()
    }

    /// `FUN_00402540` (see `rng.rs` for the sequencing approximation).
    fn rand(&mut self) -> u32 {
        self.rng.next()
    }

    /// One frame: every effect runs in creation order (a later one overwrites an earlier one's
    /// slots), then finished effects are removed.
    pub fn tick(&mut self) -> Offsets {
        let mut out = Offsets::default();
        let mut list = std::mem::take(&mut self.list);
        list.retain_mut(|s| {
            s.timer += 1;
            if s.duration <= s.timer {
                return false;
            }
            let size = s.start as f32 + (s.end - s.start) as f32 * s.timer as f32 / s.duration as f32;
            let (field_x, stage_x) = match self.rand() % 3 {
                1 => (size, size),
                2 => (size, -size),
                _ => (0.0, 0.0),
            };
            let y = match self.rand() % 3 {
                1 => size,
                2 => -size,
                _ => 0.0,
            };
            if s.index < 2 {
                out.field[s.index] = [field_x, y];
            }
            out.stage_pixels = [stage_x, y];
            true
        });
        self.list = list;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecl_517_shakes_only_the_stage_and_fades() {
        let mut s = Shakes::default();
        s.add(Shake::new(30, 12, 0, 2));
        let mut seen = 0.0f32;
        for t in 1..30 {
            let o = s.tick();
            assert_eq!(o.field, [[0.0; 2]; 2]);
            let size = 12.0 - 12.0 * t as f32 / 30.0;
            for v in o.stage_pixels { assert!(v == 0.0 || (v.abs() - size).abs() < 1e-4); seen = seen.max(v.abs()); }
        }
        assert!(seen > 0.0);
        assert_eq!(s.tick(), Offsets::default(), "frame 30 ends it with nothing written");
        assert!(!s.active());
    }

    #[test]
    fn bomb_shake_moves_its_field_by_six_for_fourteen_frames() {
        let mut s = Shakes::default();
        s.add(Shake::bomb(1));
        for _ in 0..14 {
            let o = s.tick();
            assert_eq!(o.field[0], [0.0; 2]);
            assert!(o.field[1].iter().all(|v| *v == 0.0 || v.abs() == 6.0));
            // The x quirk: the field never moves left.
            assert!(o.field[1][0] >= 0.0);
        }
        s.tick();
        assert!(!s.active());
    }
}
