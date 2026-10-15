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
    /// Where it lands on the receiving field; `None` picks the explosion-cancel rule (x near
    /// where the bullet was, y in 0..32).
    pub target: Option<[f32; 2]>,
    /// A character's special bullet: on arrival it becomes `ExtraAttack00` (Reimu: a yin-yang
    /// orb, `EXOnmyoInf`) at the sender's EX rank instead of an ordinary bullet.
    pub special_rank: Option<i32>,
    /// Side whose character's program runs a special bullet's attack.
    pub owner: usize,
    /// Fly as this `bullet.anm` script instead of the bullet's own sprite (Marisa's laser
    /// projectile, script 63).
    pub script: Option<i32>,
    /// Hermite tangents (start, end) instead of the random ones (Marisa's skill).
    pub tangents: Option<([f32; 2], [f32; 2])>,
    /// Land at the opponent player's x on the bottom row (y 448) instead of `target`.
    pub aim_player: bool,
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
    special_rank: Option<i32>,
    owner: usize,
    script: Option<i32>,
    character: crate::character::Character,
}

/// A converted bullet reaching the opponent's field.
pub struct Arrival {
    pub kind: i32,
    pub color: i32,
    pub speed: f32,
    pub angle: f32,
    pub pos: [f32; 2],
    /// Set for a special bullet: the EX rank its `ExtraAttack00` plays at.
    pub special: Option<i32>,
    /// Side whose program runs it (special bullets).
    pub owner: usize,
    pub character: crate::character::Character,
}

impl Pending {
    /// Whether this is a special bullet (counts against the sender's special limit).
    pub fn is_special(&self) -> bool { self.special_rank.is_some() }

    /// Where it will land, in the receiving field's coordinates, and whether it is still queued fresh.
    pub fn landing(&self) -> ([f32; 2], bool) { (self.target, self.time == 0) }

    pub fn delete(self, world: &mut World) {
        if let Some(id) = self.vm { world.delete(id); }
    }

    /// `FUN_0051c250`: 60-159 frames of flight along a Hermite curve to a point near the top of
    /// the receiving field (x within 16 of where the bullet was, y in 0..32).
    pub fn new(req: Request, source_field: [f32; 4], receiver: &mut SideState) -> Self {
        Self::new_character(req,source_field,receiver,crate::character::Character::Reimu)
    }
    /// A character's special bullet flies like `FUN_0051c250` but with its EX class's init
    /// (vtable [1]): Sanae 80-159 frames (`FUN_004186e0`), Ran 50-89, Orin 70-129
    /// (`FUN_0041cbb0`, whose tangents are always (r100, 200), (r100, -300)); Aunn, Nazrin and
    /// Seiran 60-159 (`FUN_0053f2c0`, `FUN_0041af40`, `FUN_0041bb20`) with the default tangents.
    pub fn new_character(req: Request, source_field: [f32;4], receiver: &mut SideState, character: crate::character::Character) -> Self {
        use crate::character::Character::*;
        let special = req.special_rank.is_some();
        let delay = match character {
            Sanae if special => receiver.rand16() as i32%80+80,
            Ran if special => receiver.rand16() as i32%40+50,
            Orin if special => receiver.rand16() as i32%60+70,
            _ => receiver.rand16() as i32%100+60,
        };
        let jitter = [receiver.randf2() * 16.0, receiver.randf() * 32.0];
        let target = req.target.unwrap_or([req.origin[0] + jitter[0], jitter[1]]);
        let start = field_to_canvas(source_field, req.origin);
        let end = field_to_canvas(receiver.field, target);
        let mut curve = Interp::default();
        let (t0, t1) = req.tangents.unwrap_or_else(|| if matches!(character,Sanae|Ran|Orin) && special {
            ([receiver.randf2()*100.0,200.0],[receiver.randf2()*100.0,-300.0])
        } else {([receiver.randf2() * 100.0, -400.0], [receiver.randf2() * 100.0, 100.0])});
        curve.start_bezier(start, t0, end, t1, delay);
        Self { kind: req.kind, color: req.color, speed: (req.speed + 0.5).min(6.0), target, delay, time: 0, start, curve, vm: None, special_rank: req.special_rank, owner: req.owner, script: req.script, character }
    }

    /// One frame: the bullet's sprite (0.8 scale) flies; on arrival it becomes a bullet
    /// (`FUN_0051bd50`): speed random*0.2 + the carried speed + pressure/200*2, aimed down
    /// within 30 degrees from the middle of the field and inward from the sides.
    pub fn tick(&mut self, anm: Option<usize>, types: &[BulletType], receiver: &mut SideState, world: &mut World, lib: &AnmLibrary) -> Option<Arrival> {
        if self.time == 0 && self.script.is_some() {
            use crate::character::Character::*;
            // Ran's and the later characters' specials use the sender's plNNb.anm
            // (EnemyManager +0x4adc); Sanae's and Marisa's come from bullet.anm.
            let own_file=match self.character {
                Ran | Aunn | Nazrin | Seiran | Orin => lib.file_id(&format!("{}b.anm", self.character.prefix())),
                _ => anm,
            };
            if let (Some(file), Some(script)) = (own_file, self.script) {
                let id = world.spawn_clipped(lib, file, script as usize, self.start, [0.0, 0.0, 640.0, 480.0]);
                world.set_draw_priority(id, crate::bullet::BULLET_DRAW_PRIORITY);
                let special = self.special_rank.is_some();
                if let Some(vm) = world.get_mut(id) {
                    // The flight updates (FUN_00418a80, FUN_0053f150) hold Sanae's and Aunn's
                    // sprite at alpha 0xc0, scale 0.8; Nazrin's and Seiran's (FUN_0041b2e0) and
                    // Orin's (FUN_0041cf10) keep their script's. Orin's VM gets resolution mode 6.
                    match self.character {
                        Nazrin | Seiran | Orin if special => {}
                        _ => vm.scale = [0.8, 0.8],
                    }
                    if special && matches!(self.character, Sanae | Ran | Aunn) { vm.alpha = 192.0; }
                    if special && self.character == Orin { vm.res_mode = Some(6); }
                }
                self.vm = Some(id);
            }
        } else if self.time == 0 {
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
        if self.special_rank.is_some() {
            return Some(Arrival { kind: self.kind, color: self.color, speed: 0.0, angle: 0.0, pos: self.target, special: self.special_rank, owner: self.owner, character:self.character });
        }
        // FUN_0051bd50: at |x| >= 64 the bullet aims down and up to 30 degrees inward
        // (pi/2 -/+ FUN_00404c90(pi/6), i.e. unit * pi/6); nearer the middle down +- 30 degrees.
        let angle = if self.target[0].abs() >= 64.0 {
            let r = receiver.randf() * FRAC_PI_6;
            if self.target[0] < 0.0 { FRAC_PI_2 - r } else { FRAC_PI_2 + r }
        } else {
            FRAC_PI_2 + receiver.randf2() * FRAC_PI_6
        };
        let speed = receiver.randf() * 0.2 + self.speed + receiver.spirit_pressure as f32 / 200.0 * 2.0;
        Some(Arrival { kind: self.kind, color: self.color, speed, angle, pos: self.target, special: None, owner: self.owner, character:self.character })
    }
}

/// White dot bullets (type 1, or type 4 later) come from the dot counter (`FUN_00548bc0`, GhostCtrl
/// +0x50, `FUN_005450e0`): the type is 4 with probability 0 / 1/5 / 2/5 / 1/2 as the round passes
/// 3600, 7200 and 10800 frames.
pub fn dot_kind(frames: i32, roll: u32) -> i32 {
    let white = match frames {
        ..3600 => false,
        3600..7200 => roll % 5 > 3,
        7200..10800 => roll % 5 > 2,
        _ => roll % 6 > 2,
    };
    if white { 4 } else { 1 }
}

/// Counter gain per kill from the round time (VS values of FUN_0051d4e0): dots 0/30/110/200.
pub fn dot_time_bonus(frames: i32) -> i32 {
    match frames { ..3600 => 0, 3600..7200 => 30, 7200..10800 => 110, _ => 200 }
}

/// Special-bullet gain per spirit kill from the round time (VS): 0/20/40/100.
pub fn special_time_bonus(frames: i32) -> i32 {
    match frames { ..3600 => 0, 3600..7200 => 20, 7200..10800 => 40, _ => 100 }
}

/// How many special bullets one side may have on the other (ExtraAttackInf +0x38): 20, doubling
/// at 3600 and 7200 frames, unlimited from 10800.
pub fn special_limit(frames: i32) -> usize {
    match frames { ..3600 => 20, 3600..7200 => 40, 7200..10800 => 80, _ => 999 }
}
