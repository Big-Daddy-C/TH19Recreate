//! Marisa's beam shots: SHT records of kind 2 with callbacks `[2, 2, 0, 2]` (init
//! `FUN_0054d9e0`, update `FUN_0054d370`, hit `FUN_0054cd70`) and her charge beams, records with
//! callbacks `[12, 9, 0, 8]` (`FUN_0054ccf0`, `FUN_0054c830`, `FUN_0054c7c0`). A beam is a long rotated
//! rectangle collider that grows 18 pixels a frame up to about 512 from its origin (the player
//! or an option) along the record's angle, pierces every enemy it overlaps, is cut short at the
//! one it hits, and lasts while the shot button is held. At most one beam per SHT group is alive.
use th_anm::{AnmLibrary, VmId, World};
use th_formats::sht::ShotRecord;

use crate::enemy::{field_to_canvas, flag, Enemy};

/// Length gained per frame and the length it stops growing at (`+0xb0`).
const GROW: f32 = 18.0;
const FULL: f32 = 512.0;
/// A beam hits (its callback returns the record damage) on every 4th frame of its age.
const HIT_PERIOD: i32 = 4;
/// A charge beam ends once its age passes 0x77 frames.
const CHARGE_FRAMES: i32 = 0x77;

pub struct Beam {
    pub group: i32,
    /// A charge beam: lasts 120 frames whatever the shot button does, is not cut by what it hits
    /// and damages on every frame.
    charge: bool,
    /// SHT origin nibble: 0 the player, n the n-th option.
    origin: u8,
    offset: [f32; 2],
    angle: f32,
    width: f32,
    damage: i32,
    impact_script: i32,
    pub pos: [f32; 2],
    pub len: f32,
    /// Frames alive (`+0x1c`).
    time: i32,
    /// Overlapping an enemy this frame (`+0xa4`) and whether the impact state is on (`+0xa8`).
    hit: bool,
    impacting: bool,
    pub ended: bool,
    vm: Option<VmId>,
    /// Frames left before an ended beam's VM is removed.
    fade: i32,
}

impl Beam {
    pub fn new(r: &ShotRecord, anm: usize, world: &mut World, lib: &AnmLibrary, field: [f32; 4], pos: [f32; 2]) -> Self {
        let vm = (r.anm_script >= 0).then(|| world.spawn_clipped(lib, anm, r.anm_script as usize, field_to_canvas(field, pos), field));
        Self {
            group: r.group,
            charge: r.funcs[0] == 12,
            origin: r.origin & 0xf,
            offset: r.offset,
            angle: crate::motion::wrap_angle(r.angle),
            width: r.hitbox[1],
            damage: r.damage as i32,
            impact_script: i16::from_le_bytes([r.raw[0x2e], r.raw[0x2f]]) as i32,
            pos,
            len: 0.0,
            time: 0,
            hit: false,
            impacting: false,
            ended: false,
            vm,
            fade: 0,
        }
    }

    /// The beam's sprite VM.
    pub fn vm(&self) -> Option<VmId> {
        self.vm
    }

    /// Whether it is a charge beam (not tied to the shot button).
    pub fn is_charge(&self) -> bool {
        self.charge
    }

    pub fn origin(&self) -> u8 {
        self.origin
    }

    /// An ended beam is kept until its fade-out has played.
    pub fn faded(&mut self, world: &World) -> bool {
        self.fade -= 1;
        self.fade > 0 && self.vm.is_some_and(|id| world.get(id).is_some())
    }

    /// The beam's collider: centre, length and width, as a rotated rectangle.
    pub fn collider(&self) -> ([f32; 2], f32, f32, f32) {
        let (s, c) = self.angle.sin_cos();
        ([self.pos[0] + c * self.len / 2.0, self.pos[1] + s * self.len / 2.0], self.len, self.width, self.angle)
    }

    /// Whether the beam goes on: the shot button is held, the player is up and not charging.
    pub fn update(
        &mut self,
        origin: [f32; 2],
        keep: bool,
        field: [f32; 4],
        world: &mut World,
    ) -> bool {
        self.pos = [origin[0] + self.offset[0], origin[1] + self.offset[1]];
        if self.ended {
            return false;
        }
        self.time += 1;
        if self.len <= FULL && self.len != FULL {
            self.len += GROW;
        }
        if let Some(vm) = self.vm.and_then(|id| world.get_mut(id)) {
            let at = field_to_canvas(field, self.pos);
            vm.origin = [at[0], at[1], 0.0];
            vm.rot[2] = self.angle;
            vm.scale2[0] = self.len / FULL;
        }
        if !self.hit && self.impacting {
            if let Some(id) = self.vm {
                world.interrupt(id, 3);
            }
            self.impacting = false;
        }
        if !keep || (self.charge && self.time > CHARGE_FRAMES) {
            self.ended = true;
            self.fade = 30;
            if let Some(id) = self.vm {
                world.interrupt(id, 1);
            }
        }
        self.hit = false;
        true
    }

    /// Enemies the beam damages this frame, as (index, damage). The beam is cut short at the
    /// last overlapped enemy (callback order), 8 pixels beyond where it enters it.
    pub fn collide(&mut self, enemies: &[Enemy], world: &mut World, lib: &AnmLibrary, field: [f32; 4], anm: usize, out: &mut Vec<(usize, i32, [f32; 2])>) {
        if self.ended {
            return;
        }
        let (centre, len, width, angle) = self.collider();
        let mut cut = None;
        for (index, e) in enemies.iter().enumerate() {
            let d = &e.data;
            if d.flags & flag::NO_HURTBOX != 0 || d.hurtbox[0] <= 0.0 || d.delete {
                continue;
            }
            if !crate::player::shot_hits(centre, [len, width], angle, d) {
                continue;
            }
            self.hit = true;
            if !self.impacting {
                if let Some(id) = self.vm {
                    world.interrupt(id, 2);
                }
                self.impacting = true;
            }
            if self.charge {
                out.push((index, self.damage, centre));
                continue;
            }
            let radius = d.hurtbox[0] / 2.0;
            let entry = ray_entry(self.pos, angle, d.pos, width / 2.0 + radius);
            cut = Some((entry + 8.0).max(0.0));
            if self.time % HIT_PERIOD == 0 {
                out.push((index, self.damage, centre));
            }
        }
        if let Some(l) = cut {
            self.len = l;
        }
        // An impact glow at the end of the beam on every even frame (record +0x2e).
        if self.hit && !self.charge && self.time % 2 == 0 && self.impact_script >= 0 {
            let (s, c) = angle.sin_cos();
            let end = [self.pos[0] + c * self.len, self.pos[1] + s * self.len];
            world.spawn_rotated(lib, anm, self.impact_script as usize, field_to_canvas(field, end), field, angle);
        }
    }

    pub fn delete(&mut self, world: &mut World) {
        if let Some(id) = self.vm.take() {
            world.delete(id);
        }
    }
}

/// Distance along a ray from `from` at `angle` to where it enters the circle (`FUN_004d3720`);
/// 0 when it starts inside or misses.
pub fn ray_entry(from: [f32; 2], angle: f32, centre: [f32; 2], radius: f32) -> f32 {
    let (s, c) = angle.sin_cos();
    let (lx, ly) = (centre[0] - from[0], centre[1] - from[1]);
    let along = c * lx + s * ly;
    let across = -s * lx + c * ly;
    if across.abs() > radius {
        return 0.0;
    }
    along - (radius * radius - across * across).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_enters_a_circle_ahead() {
        // Straight up (-pi/2) from the origin at a circle of radius 10 centred 100 above.
        let d = ray_entry([0.0, 0.0], -std::f32::consts::FRAC_PI_2, [0.0, -100.0], 10.0);
        assert!((d - 90.0).abs() < 1e-3, "{d}");
        assert_eq!(ray_entry([0.0, 0.0], -std::f32::consts::FRAC_PI_2, [50.0, -100.0], 10.0), 0.0);
    }
}
