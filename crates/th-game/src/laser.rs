//! Enemy lasers (`LaserInf` family). Only the infinite laser (`LaserInfiniteInf`, vtable 0x5a873c,
//! made by ECL `ins_703` through `FUN_00530df0`) is ported: a straight beam from a point along an
//! angle that warns as a thin line, widens, holds and shrinks (`FUN_0052b550`), hurts the player
//! while widening or held (`FUN_0052b0e0` / `FUN_00549570` / `FUN_0052b250`) and is grazed.
//! The line laser (`ins_702`), curve and beam lasers, laser Ex lists and cancels are not ported.
use th_anm::{AnmLibrary, VmId, World};
use th_formats::exe::BulletType;

use crate::enemy::field_to_canvas;

/// `ins_700` / `ins_701` / `ins_608` values stored in an ET slot (`+8..+0x10`, `+0x448`,
/// `+0x458..+0x468`, `+0x478`).
/// Draw-chain priority of the laser manager.
pub const DRAW_PRIORITY: i32 = 0x27;

#[derive(Clone, Copy, Debug, Default)]
pub struct LaserEt {
    /// Length now and the length it grows to (`ins_700` floats 1 and 2).
    pub len: [f32; 2],
    /// Full width (`ins_700` float 4).
    pub width: f32,
    /// Frames of warning, widening, holding and shrinking (`ins_701`).
    pub times: [i32; 4],
    pub flags: i32,
    /// Sound ids (`ins_608`).
    pub sound: [i32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Thin warning line (state 3).
    Warn,
    /// Widening (state 4).
    Widen,
    /// Full width (state 2).
    Hold,
    /// Shrinking (state 5).
    Shrink,
}

pub struct Laser {
    pub pos: [f32; 2],
    pub angle: f32,
    pub len: f32,
    cap: f32,
    grow: f32,
    pub width: f32,
    max_width: f32,
    times: [i32; 4],
    state: State,
    timer: i32,
    /// Frames since creation (`+0x3c`).
    age: i32,
    /// Consecutive frames the player has been inside the graze ring (`+0x65c`).
    graze: i32,
    sprite_type: i32,
    color: i32,
    vm: Option<VmId>,
}

/// What a laser did to the player this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Contact {
    None,
    Graze,
    Hit,
}

impl Laser {
    /// `FUN_0052b8e0`: the laser as `ins_703` made it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        origin: [f32; 2],
        angle: f32,
        speed: f32,
        sprite_type: i32,
        color: i32,
        et: &LaserEt,
        types: &[BulletType],
        anm: usize,
        world: &mut World,
        lib: &AnmLibrary,
        field: [f32; 4],
    ) -> Self {
        let vm = types.get(sprite_type.max(0) as usize).map(|ty| {
            let remap = ty.colours[color.clamp(0, 15) as usize];
            let id = world.spawn_remapped(lib, anm, ty.script as usize, field_to_canvas(field, origin), field, remap);
            // The laser manager draws at chain priority 0x27 (`FUN_00530a30`).
            world.set_draw_priority(id, DRAW_PRIORITY);
            // The native code turns the sprite with the beam; script 161 itself draws it unrotated.
            if let Some(vm) = world.get_mut(id) {
                vm.render_mode = 1;
            }
            id
        });
        Self {
            pos: origin,
            angle: crate::motion::wrap_angle(angle),
            len: et.len[0],
            cap: et.len[1],
            grow: speed,
            // Warning lines are 2 pixels wide.
            width: 2.0,
            max_width: et.width,
            times: et.times,
            state: State::Warn,
            timer: 0,
            age: 0,
            graze: 0,
            sprite_type,
            color,
            vm,
        }
    }

    /// One frame. Returns false when the laser is over.
    pub fn update(&mut self, world: &mut World, field: [f32; 4]) -> bool {
        self.age += 1;
        self.timer += 1;
        if self.len <= self.cap && self.cap != self.len {
            self.len += self.grow;
            if self.cap <= self.len {
                self.len = self.cap;
            }
        }
        let [warn, widen, hold, shrink] = self.times;
        let mut done = false;
        match self.state {
            State::Warn => {
                if warn <= self.timer {
                    self.timer = 0;
                    self.state = State::Widen;
                }
            }
            State::Widen => {
                if self.timer < widen {
                    self.width = self.timer as f32 * self.max_width / widen as f32;
                } else {
                    self.timer = 0;
                    self.state = State::Hold;
                    self.width = self.max_width;
                }
            }
            State::Hold | State::Shrink => {}
        }
        if self.state == State::Hold && hold <= self.timer {
            self.timer = 0;
            self.state = State::Shrink;
        } else if self.state == State::Shrink {
            if shrink <= self.timer {
                done = true;
            } else {
                self.width = self.max_width - self.timer as f32 * self.max_width / shrink as f32;
            }
        }
        if let Some(vm) = self.vm.and_then(|id| world.get_mut(id)) {
            // The 16 x 14 tile has its gradient across its 16 pixels, so the tile's width is the
            // beam's width and its height the beam's length; it is centred on the beam's middle.
            let (sn, cs) = self.angle.sin_cos();
            let mid = [self.pos[0] + cs * self.len / 2.0, self.pos[1] + sn * self.len / 2.0];
            let at = field_to_canvas(field, mid);
            vm.origin = [at[0], at[1], 0.0];
            vm.rot[2] = self.angle + std::f32::consts::FRAC_PI_2;
            vm.scale = [self.width.max(0.0) / 16.0, self.len.max(0.0) / 14.0];
        }
        !done
    }

    /// `FUN_0052b0e0`: the hurting rectangle (centre, length, width, angle) while widening or
    /// held and longer than 16.
    pub fn hit_rect(&self) -> Option<([f32; 2], f32, f32, f32)> {
        if !matches!(self.state, State::Widen | State::Hold) || self.len <= 16.0 {
            return None;
        }
        let width = if self.width >= 32.0 { self.width - (self.width + 16.0) / 3.0 } else { self.width * 0.5 };
        let (s, c) = self.angle.sin_cos();
        Some(([self.pos[0] + c * self.len / 2.0, self.pos[1] + s * self.len / 2.0], self.len * 0.95, width, self.angle))
    }

    /// `FUN_00549570`: against the player's hit circle (`hit` radius) with the graze ring 30
    /// pixels wider.
    pub fn contact(&mut self, player: [f32; 2], hit: f32) -> Contact {
        let Some((centre, len, width, angle)) = self.hit_rect() else {
            self.graze = 0;
            return Contact::None;
        };
        if !rect_circle(centre, [len, width], angle, player, hit + 30.0) {
            self.graze = 0;
            return Contact::None;
        }
        if rect_circle(centre, [len, width], angle, player, hit) {
            Contact::Hit
        } else {
            self.graze += 1;
            Contact::Graze
        }
    }

    /// Whether a graze this frame earns an orb: every 8th frame of the laser's life.
    pub fn graze_tick(&self) -> bool {
        self.age % 8 == 0
    }

    pub fn delete(&mut self, world: &mut World) {
        if let Some(id) = self.vm.take() {
            world.delete(id);
        }
    }

    pub fn vm(&self) -> Option<VmId> {
        self.vm
    }

    pub fn sprite_type(&self) -> (i32, i32) {
        (self.sprite_type, self.color)
    }
}

/// An oriented rectangle (centre, size along / across `angle`) against a circle (`FUN_004d44e0`).
pub fn rect_circle(centre: [f32; 2], size: [f32; 2], angle: f32, circle: [f32; 2], radius: f32) -> bool {
    let (s, c) = angle.sin_cos();
    let (dx, dy) = (circle[0] - centre[0], circle[1] - centre[1]);
    let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
    let (eu, ev) = ((u.abs() - size[0] / 2.0).max(0.0), (v.abs() - size[1] / 2.0).max(0.0));
    eu * eu + ev * ev < radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laser(times: [i32; 4]) -> Laser {
        Laser {
            pos: [0.0, 400.0],
            angle: -std::f32::consts::FRAC_PI_2,
            len: 512.0,
            cap: 512.0,
            grow: 8.0,
            width: 2.0,
            max_width: 14.0,
            times,
            state: State::Warn,
            timer: 0,
            age: 0,
            graze: 0,
            sprite_type: 38,
            color: 6,
            vm: None,
        }
    }

    #[test]
    fn warns_widens_holds_shrinks_and_ends() {
        let mut l = laser([30, 10, 20, 10]);
        let mut w = World::default();
        let mut states = vec![];
        for _ in 0..80 {
            let alive = l.update(&mut w, [0.0; 4]);
            states.push((l.state, l.width, alive));
            if !alive {
                break;
            }
        }
        let first_widen = states.iter().position(|s| s.0 == State::Widen).unwrap();
        assert_eq!(first_widen, 29, "30 frames of warning");
        assert!(states[..first_widen].iter().all(|s| s.1 == 2.0));
        let hold = states.iter().position(|s| s.0 == State::Hold).unwrap();
        assert_eq!(states[hold].1, 14.0);
        let end = states.len();
        assert!(!states[end - 1].2, "ends after the shrink: {end}");
        assert!((70..=75).contains(&end), "{end}");
    }

    #[test]
    fn only_widening_and_held_lasers_hurt_and_the_graze_ring_is_30_wider() {
        let mut l = laser([5, 5, 100, 5]);
        let mut w = World::default();
        // Warning: harmless even on top of it.
        assert_eq!(l.contact([0.0, 300.0], 3.0), Contact::None);
        for _ in 0..12 {
            l.update(&mut w, [0.0; 4]);
        }
        assert_eq!(l.contact([0.0, 300.0], 3.0), Contact::Hit, "on the beam");
        assert_eq!(l.contact([20.0, 300.0], 3.0), Contact::Graze, "20 px to the side (beam half width 3.5 + 30 ring)");
        assert_eq!(l.contact([60.0, 300.0], 3.0), Contact::None);
        // Behind its start (the beam points up from y = 400).
        assert_eq!(l.contact([0.0, 450.0], 3.0), Contact::None);
    }
}
