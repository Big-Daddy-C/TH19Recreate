//! Marisa's beam shots: SHT records of kind 2 with callbacks `[2, 2, 0, 2]` (init
//! `FUN_0054d9e0`, update `FUN_0054d370`, hit `FUN_0054cd70`) and her charge beams, records with
//! callbacks `[12, 9, 0, 8]` (`FUN_0054ccf0`, `FUN_0054c830`, `FUN_0054c7c0`). A beam is a long rotated
//! rectangle collider that grows 18 pixels a frame up to about 512 from its origin (the player
//! or an option) along the record's angle, pierces every enemy it overlaps, is cut short at the
//! one it hits, and lasts while the shot button is held. At most one beam per SHT group is alive.
//!
//! Saki's beams are records `[18, 2, 0, 8]`: the same update (they follow the player), but init
//! `FUN_0054d960` gives the collider a hit interval of 4 and the hit callback is the charge
//! beam's `FUN_0054c7c0` (never cut short). A record with a life (+0x3c, 60 for her charge
//! slots 10 and 11) lasts that many frames whatever the shot button does (`FUN_0054d370`).
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
    /// Hit callback 2 (`FUN_0054cd70`): cut short at what it hits, damage every 4th frame of
    /// its age; otherwise (`FUN_0054c7c0`) never cut, damage on each collider hit.
    cut: bool,
    /// The collider's hit interval (+0xa4): 4 after init 18, else 1.
    interval: i32,
    /// SHT record +0x3c: frames the beam lasts regardless of the button (0: tied to it).
    life: i32,
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
            cut: r.funcs[3] == 2,
            interval: if r.funcs[0] == 18 { 4 } else { 1 },
            life: i32::from_le_bytes(r.raw[0x3c..0x40].try_into().unwrap()),
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

    /// The record's life (+0x3c): a beam with one is not tied to the shot button.
    pub fn life(&self) -> i32 {
        self.life
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
        // 0054d370 follows the option each tick; 0054c830 retains the charge beam's
        // launch position (+0x54) for its entire lifetime.
        if !self.charge {
            self.pos = [origin[0] + self.offset[0], origin[1] + self.offset[1]];
        }
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
        if !keep || (self.charge && self.time > CHARGE_FRAMES) || (self.life > 0 && self.time >= self.life) {
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
            if !self.cut {
                // APPROX (phase): the collider's own life counter (+0x80) picks the frames of an
                // interval above 1; the beam's age stands in for it.
                if self.time % self.interval == 0 { out.push((index, self.damage, centre)); }
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
        if self.hit && self.cut && self.time % 2 == 0 && self.impact_script >= 0 {
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
    fn original_marisa_beams_survive_contact_repeat_damage_and_reach_field_top() {
        let Some(home) = std::env::var_os("USERPROFILE") else { return };
        let path = std::env::var_os("TH19_DAT").map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(home).join("Downloads/th19/th19/th19.dat"));
        if !path.is_file() { eprintln!("Original assets unavailable"); return; }
        let mut dat = th_formats::dat::Archive::open(path).unwrap();
        let sht = th_formats::sht::ShtFile::parse(&dat.read("pl01.sht").unwrap()).unwrap();
        let mut lib = AnmLibrary::default();
        let file = lib.load("pl01.anm", &dat.read("pl01.anm").unwrap()).unwrap();
        for (slot, charge) in [(2, false), (10, true)] {
            let r = sht.slots[slot].as_ref().unwrap().iter()
                .find(|r| r.funcs[0] == if charge {12} else {2}).unwrap();
            let mut world = World::default();
            let field = crate::FIELDS[0];
            let origin = [0.0, 440.0];
            let mut beam = Beam::new(r, file, &mut world, &lib, field,
                [origin[0] + r.offset[0], origin[1] + r.offset[1]]);
            let vm = beam.vm().unwrap();
            let enemy = Enemy {
                threads: th_ecl::Threads::new(th_ecl::Context::new(th_ecl::SubRef {file:0, sub:0})),
                data: crate::EnemyData {pos:[r.offset[0], 240.0], hurtbox:[20.0;2], hp:10000, ..Default::default()}
            };
            let mut hit_frames = Vec::new();
            for frame in 1..=65 {
                beam.update(origin, true, field, &mut world);
                let mut hits = Vec::new();
                beam.collide(std::slice::from_ref(&enemy), &mut world, &lib, field, file, &mut hits);
                if !hits.is_empty() { hit_frames.push(frame); }
                world.tick(&lib);
                assert!(world.get(vm).is_some(), "beam sprite vanished after contact: charge={charge}, frame={frame}");
                assert!(!beam.ended);
            }
            assert!(hit_frames.len() > 10);
            for pair in hit_frames.windows(2) {
                assert_eq!(pair[1]-pair[0], if charge {1} else {4});
            }
            // Removing the obstruction restores the original >512 px reach, including its drawing.
            for _ in 0..30 { beam.update(origin, true, field, &mut world); world.tick(&lib); }
            if charge { assert_eq!(beam.len, 522.0); }
            else { assert!(beam.len > 512.0 && beam.len <= 530.0); }
            let texture = lib.files[file].sprites[if charge {29} else {28}].texture;
            let drawing = world.draw_list(&lib, th_anm::Pass::Ui);
            let top = drawing.iter().filter(|b| b.texture == Some(texture)).flat_map(|b| &b.vertices)
                .map(|v| v.pos[1]).fold(f32::INFINITY, f32::min);
            assert!(top < field[1], "beam stops before field top: {top}");
            if charge {
                let launch = beam.pos;
                beam.update([80.0, 300.0], true, field, &mut world);
                assert_eq!(beam.pos, launch, "charge beam remains at the native launch position");
                while beam.time < CHARGE_FRAMES {
                    beam.update(origin, true, field, &mut world);
                    assert!(!beam.ended);
                }
                beam.update(origin, true, field, &mut world);
                assert!(beam.ended, "charge beam ends after age 0x77");
            } else {
                beam.update(origin, false, field, &mut world);
                assert!(beam.ended, "normal beam ends on release");
            }
            for _ in 0..30 { world.tick(&lib); }
            assert!(world.get(vm).is_none(), "native ending interrupt still fades and deletes");
        }
    }

    #[test]
    fn ray_enters_a_circle_ahead() {
        // Straight up (-pi/2) from the origin at a circle of radius 10 centred 100 above.
        let d = ray_entry([0.0, 0.0], -std::f32::consts::FRAC_PI_2, [0.0, -100.0], 10.0);
        assert!((d - 90.0).abs() < 1e-3, "{d}");
        assert_eq!(ray_entry([0.0, 0.0], -std::f32::consts::FRAC_PI_2, [50.0, -100.0], 10.0), 0.0);
    }
}
