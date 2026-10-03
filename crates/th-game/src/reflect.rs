//! Cancelled bullets sent to the opponent: `FUN_0051c8c0` (the kill explosion cancels live
//! small bullets), `FUN_0051c650`/`FUN_0051c250` (a flying copy of the bullet) and
//! `FUN_0051bd50` (on arrival it fires one bullet on the other field).
use std::f32::consts::{FRAC_PI_2, FRAC_PI_6};
use th_anm::{interp::Interp, AnmLibrary, VmId, World};
use th_formats::exe::BulletType;
use crate::{enemy::field_to_canvas, side::SideState};

/// A bullet cancelled by an explosion on the sender's field.
pub struct Request {
    pub kind: i32,
    pub color: i32,
    /// The cancelled bullet's speed.
    pub speed: f32,
    /// Where it was, in the sender's field coordinates.
    pub origin: [f32; 2],
}

/// The type the converted bullet takes (`FUN_0051c8c0`): the cancelled bullet's own type
/// early in the round; later a growing share of them become type 4 ("white dots"). `roll`
/// is a random number; `frames` the round's frame counter (GhostCtrl +0x28).
pub fn converted_kind(kind: i32, frames: i32, roll: u32) -> i32 {
    let keep_out_of_ten = match frames {
        ..3600 => 10,
        3600..7200 => 9,
        7200..10800 => 8,
        _ => 7,
    };
    if (roll % 10) as i32 >= keep_out_of_ten { 4 } else { kind }
}

pub struct Pending {
    kind: i32,
    color: i32,
    speed: f32,
    target: [f32; 2],
    delay: i32,
    time: i32,
    start: [f32; 2],
    curve: Interp<2>,
    vm: Option<VmId>,
}

/// A converted bullet reaching the opponent's field.
pub struct Arrival {
    pub kind: i32,
    pub color: i32,
    pub speed: f32,
    pub angle: f32,
    pub pos: [f32; 2],
}

impl Pending {
    pub fn delete(self, world: &mut World) {
        if let Some(id) = self.vm { world.delete(id); }
    }

    /// `FUN_0051c250`: 60-159 frames of flight along a Hermite curve to a point near the top of
    /// the receiving field (x within 16 of where the bullet was, y in 0..32).
    pub fn new(req: Request, source_field: [f32; 4], receiver: &mut SideState) -> Self {
        let delay = (receiver.rand() & 0xffff) as i32 % 100 + 60;
        let target = [req.origin[0] + receiver.randf2() * 16.0, receiver.randf() * 32.0];
        let start = field_to_canvas(source_field, req.origin);
        let end = field_to_canvas(receiver.field, target);
        let mut curve = Interp::default();
        curve.start_bezier(start, [receiver.randf2() * 100.0, -400.0], end, [receiver.randf2() * 100.0, 100.0], delay);
        Self { kind: req.kind, color: req.color, speed: (req.speed + 0.5).min(6.0), target, delay, time: 0, start, curve, vm: None }
    }

    /// One frame: the bullet's sprite (0.8 scale) flies; on arrival it becomes a bullet
    /// (`FUN_0051bd50`): speed random*0.2 + the carried speed + pressure/200*2, aimed down
    /// within 30 degrees from the middle of the field and inward from the sides.
    pub fn tick(&mut self, anm: Option<usize>, types: &[BulletType], receiver: &mut SideState, world: &mut World, lib: &AnmLibrary) -> Option<Arrival> {
        if self.time == 0 {
            if let (Some(file), Some(ty)) = (anm, types.get(self.kind.max(0) as usize)) {
                let remap = ty.colours[self.color.clamp(0, 15) as usize];
                let id = world.spawn_remapped(lib, file, ty.script as usize, self.start, [0.0, 0.0, 640.0, 480.0], remap);
                world.set_draw_priority(id, crate::bullet::BULLET_DRAW_PRIORITY);
                if let Some(vm) = world.get_mut(id) { vm.scale = [0.8, 0.8]; }
                self.vm = Some(id);
            }
        }
        if self.time < self.delay {
            self.time += 1;
            if let Some(pos) = self.curve.step() {
                if let Some(vm) = self.vm.and_then(|id| world.get_mut(id)) { vm.origin = [pos[0], pos[1], 0.0]; }
            }
            return None;
        }
        if let Some(id) = self.vm.take() { world.delete(id); }
        // APPROX: FUN_00404c90's result is hidden in the decompile; the side fans are read as
        // a random angle in the quarter turn that points inward.
        let angle = if self.target[0].abs() >= 64.0 {
            let r = receiver.randf() * FRAC_PI_2;
            if self.target[0] < 0.0 { r } else { FRAC_PI_2 + r }
        } else {
            FRAC_PI_2 + (receiver.randf() * 2.0 - 1.0) * FRAC_PI_6
        };
        let speed = receiver.randf() * 0.2 + self.speed + receiver.spirit_pressure as f32 / 200.0 * 2.0;
        Some(Arrival { kind: self.kind, color: self.color, speed, angle, pos: self.target })
    }
}
