//! The graze effect: a short random-walk lightning line (`FUN_00549fb0` queues pool entry type 4
//! on the bullet manager with a 0-7 tick delay; `FUN_00505cf0` spawns it through
//! `FUN_00506940`, effect-table row 4: effect.anm script 0 with the hooks `FUN_004dc010`
//! (set-up), `FUN_004dc250` (grow/fade each tick) and `FUN_004dc440` (draw through
//! `FUN_004c3f40`, a D3D line strip), plus the per-bullet gate that decides when a bullet grazes
//! (`FUN_004f5130`). The graze sound (`FUN_004bb8d0(0x2a, x * 1000 / 192)`) has no audio output
//! here yet.
use th_anm::{BlendMode, DepthMode, DrawBatch, Vertex};

/// Points in the strip (`FUN_004dc250` stops at index 0x13).
const POINTS: usize = 20;

/// Default colour of bullets and lasers (`0xffd08080`, +0x694 / +0x860).
pub const DEFAULT_RGB: u32 = 0xd08080;

/// The two timers and graze allowance a bullet keeps (`+0xfd8`, `+0x1000`, `+0x1018`).
#[derive(Clone, Copy, Debug)]
pub struct Gate {
    /// Graze allowance: one graze, regained after 60 ticks out of range.
    left: u8,
    /// Timer +0xfd8 `(cur, prev)`; ticks only while in range.
    contact: (i32, i32),
    /// Timer +0x1000 `(cur)`; counts ticks out of range.
    apart: i32,
}

impl Default for Gate {
    fn default() -> Self {
        // The game never sets the flag at manager +0x815718, so the allowance is 1 and the
        // interval 0x78 (`FUN_004f44d7`: `(flag != 0) + 1`, `flag ? 0x3c : 0x78`).
        Self { left: 1, contact: (0, -1), apart: 0 }
    }
}

impl Gate {
    /// One bullet tick; `in_range` is the ring test (`FUN_00549e00` returned 2). True when this
    /// tick grazes.
    pub fn tick(&mut self, in_range: bool) -> bool {
        const INTERVAL: i32 = 0x78;
        if in_range {
            // FUN_0040e130: the timer moved this tick and sits on a multiple of the interval.
            let grazed = self.left != 0 && self.contact.0 != self.contact.1 && self.contact.0 % INTERVAL == 0;
            if grazed {
                self.contact = (0, -1);
                self.left -= 1;
            }
            self.contact = (self.contact.0 + 1, self.contact.0);
            self.apart = 0;
            grazed
        } else {
            self.apart += 1;
            if self.apart >= 0x3c && self.left == 0 {
                self.left = 1;
                self.contact = (0, -1);
            }
            false
        }
    }
}

struct Trail {
    pts: [[f32; 2]; POINTS],
    alpha: [u8; POINTS],
    /// Timer +0xf8: points shown so far.
    cur: usize,
    prev: usize,
    angle: f32,
}

pub struct Spark {
    delay: i32,
    pos: [f32; 2],
    rgb: u32,
    trail: Option<Trail>,
}

impl Spark {
    /// `delay` is `rand & 7` ticks.
    pub fn new(pos: [f32; 2], rgb: u32, delay: i32) -> Self {
        Self { delay, pos, rgb, trail: None }
    }

    /// One tick (`FUN_00505cf0` counts the delay down, then the VM hooks run). `rand` is the
    /// game's uniform 0..1 generator. False once the line has finished.
    pub fn tick(&mut self, rand: &mut impl FnMut() -> f32) -> bool {
        if self.delay > 0 {
            self.delay -= 1;
            return true;
        }
        let tr = self.trail.get_or_insert_with(|| {
            // FUN_004dc010: random heading, alpha 0xff for the first ten points, then
            // ~(k * 0x18) for the rest.
            let mut alpha = [0xffu8; POINTS];
            for (k, a) in alpha[10..].iter_mut().enumerate() {
                *a = !((k as u32 * 0x18) as u8);
            }
            Trail { pts: [[0.0; 2]; POINTS], alpha, cur: 1, prev: 0, angle: (rand() * 2.0 - 1.0) * std::f32::consts::PI }
        });
        if tr.cur >= POINTS {
            return false;
        }
        if tr.cur != tr.prev {
            for a in &mut tr.alpha[..tr.cur] {
                *a = a.saturating_sub(0x10);
            }
            let step = rand() * 8.0 + 1.0;
            let at = tr.cur;
            tr.pts[at] = [tr.angle.cos() * step + tr.pts[at - 1][0], tr.angle.sin() * step + tr.pts[at - 1][1]];
            let mut a = (rand() * 2.0 - 1.0) * std::f32::consts::PI + tr.angle;
            while a > std::f32::consts::PI {
                a -= std::f32::consts::TAU;
            }
            while a < -std::f32::consts::PI {
                a += std::f32::consts::TAU;
            }
            tr.angle = a;
        }
        tr.prev = tr.cur;
        tr.cur += 1;
        true
    }

    /// The line strip as one-pixel quads in canvas space, clipped to the field.
    pub fn batch(&self, field: [f32; 4]) -> Option<DrawBatch> {
        let tr = self.trail.as_ref()?;
        let [r, g, b] = [(self.rgb >> 16) & 0xff, (self.rgb >> 8) & 0xff, self.rgb & 0xff].map(|c| c as f32 / 255.0);
        let origin = crate::enemy::field_to_canvas(field, self.pos);
        let mut vertices = Vec::new();
        let v = |p: [f32; 2], a: u8| Vertex { pos: p, uv: [0.0; 2], color: [r, g, b, a as f32 / 255.0], w: 1.0, fog: 0.0, depth: 0.0 };
        for i in 1..tr.cur.min(POINTS) {
            let (p0, p1) = (tr.pts[i - 1], tr.pts[i]);
            let a = [origin[0] + p0[0], origin[1] + p0[1]];
            let b2 = [origin[0] + p1[0], origin[1] + p1[1]];
            let (dx, dy) = (b2[0] - a[0], b2[1] - a[1]);
            let len = (dx * dx + dy * dy).sqrt().max(1e-3);
            let n = [-dy / len * 0.5, dx / len * 0.5];
            let (ca, cb) = (tr.alpha[i - 1], tr.alpha[i]);
            vertices.extend([
                v([a[0] + n[0], a[1] + n[1]], ca), v([a[0] - n[0], a[1] - n[1]], ca), v([b2[0] + n[0], b2[1] + n[1]], cb),
                v([b2[0] + n[0], b2[1] + n[1]], cb), v([a[0] - n[0], a[1] - n[1]], ca), v([b2[0] - n[0], b2[1] - n[1]], cb),
            ]);
        }
        (!vertices.is_empty()).then_some(DrawBatch { texture: None, blend: BlendMode::Normal, depth: DepthMode::Off, layer: i32::MAX - 1, clip: Some(field), vertices })
    }
}

/// Closest point to `p` on the segment centred at `c` with full length `len` along `angle`
/// (`FUN_004d3580`), where a laser's graze effect appears.
pub fn closest_on_segment(c: [f32; 2], len: f32, angle: f32, p: [f32; 2]) -> [f32; 2] {
    let (s, co) = angle.sin_cos();
    let along = ((p[0] - c[0]) * co + (p[1] - c[1]) * s).clamp(-len / 2.0, len / 2.0);
    [c[0] + co * along, c[1] + s * along]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_grazes_once_and_regains_after_sixty_ticks_apart() {
        let mut g = Gate::default();
        assert!(g.tick(true), "first contact grazes at once");
        assert!((0..200).all(|_| !g.tick(true)), "no allowance left while it stays in range");
        let mut g = Gate::default();
        assert!(g.tick(true));
        for _ in 0..59 {
            assert!(!g.tick(false));
        }
        assert!(!g.tick(true), "59 ticks apart is too few: still no allowance");
        for _ in 0..60 {
            g.tick(false);
        }
        assert!(g.tick(true), "after 60 ticks apart the allowance and timer are restored");
    }

    #[test]
    fn spark_grows_one_point_a_tick_then_ends() {
        let mut n = 0.3f32;
        let mut rand = move || { n = (n * 7.7 + 0.13).fract(); n };
        let mut s = Spark::new([10.0, 20.0], DEFAULT_RGB, 2);
        for _ in 0..2 { assert!(s.tick(&mut rand)); assert!(s.batch([0.0, 0.0, 296.0, 448.0]).is_none()); }
        for want in 2..=20 { assert!(s.tick(&mut rand)); assert_eq!(s.trail.as_ref().unwrap().cur, want); }
        assert!(!s.tick(&mut rand));
    }
}
