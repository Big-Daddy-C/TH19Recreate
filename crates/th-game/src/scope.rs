//! The charging aura (`AuraReimuInf` callbacks 00416d50, 00416e80, 00416f10, 00416f60; Marisa's
//! `AuraMarisaInf` 00417840, 00417980, 00417a10, 00417a60). Reimu's grows into a circle of radius
//! 64, Marisa's into a full-height column of half width 14, both over 20 frames (interpolation
//! mode 4) at the default 100% rate.
//!
//! The aura factory table 0x5a0280 gives each character a class whose vtable slot [2] starts the
//! aura, [3] updates it, [4] updates extra state every frame, [7] tests a body or bullet circle
//! (`FUN_00509030`, `FUN_004e78e0`) and [9] reports extra state that keeps [7] running while the
//! scope is not held. Aunn (`FUN_0041a280`/`0041a4b0`, [7] `0041a830`) draws an ellipse (pl04.anm
//! script 11) whose horizontal and vertical radii grow 1 -> 32 over 30 frames (mode 3), then
//! -> 64 / 192 over 10 (mode 5) and -> 80 / 16 over 10 (mode 2). Nazrin (`0041aba0`/`0041ad00`)
//! grows a circle 1 -> 32 over 30 frames (mode 6, script 10) that from its third frame flies at
//! 4 px a frame towards the nearest active spirit within 320 (`FUN_00508420`), else the player.
//! Seiran (`0041b920`/`0041ba80`) grows a circle 1 -> 48 over 30 (mode 6, script 11) 160 above the
//! player. Orin (`0041c530`/`0041c770`, [7] `0041ca40`) throws a projectile (script 13) up at
//! 6 degrees towards her horizontal movement, speed 1 accelerating by 0.1 a frame, whose
//! 2R x R box (R 1 -> 50 over 10 frames, mode 6) scopes; once above y = -60 it starts again
//! from her. Nazrin's and Seiran's [7] is `FUN_00418690` (radii summed, like Sanae's).
//!
//! The last four lean with the player's movement: an angle that starts at +-0.12217 (7 degrees)
//! when moving faster than 0.1 sideways, else 0, and then eases 10% a frame towards that target.
//! Tsukasa (`0041dba0`/`0041de20`) holds a 4R x R box on her turned by it (script 8, R 1 -> 50
//! over 10 frames, mode 6). Mamizou (`0041f470`/`0041f770`, [7] `0041fce0`) holds three circles
//! (script 10, R 1 -> 28 over 10, mode 4) at (0, -96), (64, -64) and (-64, -64) turned by the
//! angle; the side ones are drawn at 0.6 R, all three test with R. Yachie (`00421210`/`00421490`,
//! [7] `004217e0`) holds a hexagon of radius R (script 13, 1 -> 88 over 10, mode 4) turned by the
//! angle - pi/2. Saki (`004231f0`/`00423540`, [7] `00423910`) holds an ellipse 32 along the
//! angle and 96 across it (script 14, both from 1 over 10 frames, mode 4) 32 ahead of her.
use th_anm::{interp::Interp, AnmLibrary, VmId, World};
use crate::enemy::{field_to_canvas, Enemy};
use crate::character::Character;

/// Orin's skill circle (`FUN_0041d020` start, aura [4] `FUN_0041c9c0`): pl07.anm script 12 at the
/// cast position, radius 1 -> 500 over 60 frames (mode 4), active for 60 frames. Bodies inside it
/// get [7] = -2: spirits switch to their disable sub, other enemies and bullets are unaffected.
#[derive(Default)]
pub struct SkillCircle {
    pub pos: [f32; 2],
    pub radius: f32,
    frames: i32,
    growth: Interp<1>,
    vm: Option<VmId>,
}

#[derive(Default)]
pub struct Scope {
    pub radius: f32,
    pub pos: [f32; 2],
    pub vm: Option<VmId>,
    active: bool,
    growth: Interp<1>,
    character: Character,
    /// Aura timer (+0x4..+0x10), 0 on the first update.
    time: i32,
    /// Aunn's vertical radius (+0xd0; `radius` is the horizontal one, +0xcc).
    pub radius2: f32,
    growth2: Interp<1>,
    /// Orin's projectile speed (+0x30) and direction (+0x34); the lean of the later four
    /// characters' auras (+0xa4 / +0xd8).
    speed: f32,
    pub angle: f32,
    /// Mamizou's two side circles' VMs.
    side_vms: [Option<VmId>; 2],
    pub circle: Option<SkillCircle>,
    /// The player position the aura last used (Mamizou's circles are placed from it).
    anchor: [f32; 2],
}

/// What the aura's [7] callback needs, copied for the side's body and bullet tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct Probe {
    pub character: Character,
    /// The scope is held (AuraCtrl +0x18 > 0).
    pub active: bool,
    pub centre: [f32; 2],
    pub radius: f32,
    pub radius2: f32,
    /// Orin's skill circle (centre, radius) while it lasts.
    pub circle: Option<([f32; 2], f32)>,
    /// The lean (Tsukasa, Mamizou, Yachie, Saki).
    pub angle: f32,
}

impl Probe {
    /// Aura [7] for a circle of `radius` at `pos`: positive when scoped, -2 inside Orin's skill
    /// circle, 0 otherwise. Only the four characters added with this probe use it.
    pub fn test(&self, pos: [f32; 2], radius: f32) -> i32 {
        match self.character {
            Character::Aunn => ellipse_circle(pos, radius, self.centre, self.radius2, self.radius, -std::f32::consts::FRAC_PI_2) as i32,
            Character::Nazrin | Character::Seiran =>
                ((self.centre[0]-pos[0]).powi(2)+(self.centre[1]-pos[1]).powi(2) <= (self.radius+radius).powi(2)) as i32,
            Character::Orin => {
                if let Some((c, r)) = self.circle {
                    if (pos[0]-c[0]).powi(2)+(pos[1]-c[1]).powi(2) <= (r+radius).powi(2) { return -2; }
                }
                rect_circle_exact(self.centre, [self.radius*2.0, self.radius], 0.0, pos, radius) as i32
            }
            // FUN_004d44e0 on the turned 4R x R box.
            Character::Tsukasa => rect_circle_exact(self.centre, [self.radius*4.0, self.radius], self.angle, pos, radius) as i32,
            // FUN_0041fce0: any of the three circles, radii summed (strictly outside misses).
            Character::Mamizou => mamizou_circles(self.centre, self.angle).iter()
                .any(|c| (pos[0]-c[0]).powi(2)+(pos[1]-c[1]).powi(2) <= (self.radius+radius).powi(2)) as i32,
            Character::Yachie => polygon_circle(pos, radius, self.centre, self.radius, self.angle - std::f32::consts::FRAC_PI_2, 6) as i32,
            // FUN_004d4020(a = +0xd0 along the lean, b = +0xcc).
            Character::Saki => ellipse_circle(pos, radius, self.centre, self.radius2, self.radius, self.angle) as i32,
            c => bullet_hit(c, self.centre, self.radius, pos, radius) as i32,
        }
    }
    /// Whether [7] runs at all (`FUN_00509030` / `FUN_004e78e0`: the scope is held, or [9]).
    pub fn running(&self) -> bool {
        self.active || self.circle.is_some()
    }
}

impl Scope {
    pub fn active(&self) -> bool { self.active }

    pub fn probe(&self) -> Probe {
        // Mamizou's centre is the player; the circles are placed from it.
        let centre = if self.character == Character::Mamizou { self.anchor } else { self.pos };
        Probe { character: self.character, active: self.active, centre, radius: self.radius, radius2: self.radius2,
            circle: self.circle.as_ref().map(|c| (c.pos, c.radius)), angle: self.angle }
    }

    pub fn update(&mut self, held: bool, column: bool, pos: [f32; 2], file: usize,
                  world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        self.update_character(held, if column { Character::Marisa } else { Character::Reimu }, pos, 0.0, &[], file, world, lib, field);
    }

    /// `vel_x` is the player's horizontal velocity this frame (+0x20f4, 1/128 px).
    #[allow(clippy::too_many_arguments)]
    pub fn update_character(&mut self, held: bool, character: Character, pos: [f32; 2], vel_x: f32, enemies: &[Enemy],
                  file: usize, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        self.character = character;
        self.update_circle(world, field);
        let column = character == Character::Marisa;
        if held && !self.active {
            self.active = true;
            self.radius = 1.0;
            self.radius2 = 1.0;
            self.time = 0;
            // Default status +0x8c is 100 percent; ability modifiers remain unported.
            // AuraSanaeInf004184a0:128/60/mode2; AuraRanInf00419360:30/30/mode6.
            let (size, frames, mode, script) = match character {
                Character::Reimu => (64.0,20,4,15),
                Character::Marisa => (14.0,20,4,15),
                Character::Sanae => (128.0,60,2,16),
                Character::Ran => (30.0,30,6,11),
                Character::Aunn => (32.0,30,3,11),
                Character::Nazrin => (32.0,30,6,10),
                Character::Seiran => (48.0,30,6,11),
                Character::Orin => (50.0,10,6,13),
                Character::Tsukasa => (50.0,10,6,8),
                Character::Mamizou => (28.0,10,4,10),
                Character::Yachie => (88.0,10,4,13),
                // +0x6c: 1 -> 96 (+0xcc); +0x9c: 1 -> 32 (+0xd0) below.
                Character::Saki => (96.0,10,4,14),
            };
            self.growth.start([1.0], [size], frames, mode);
            if character == Character::Aunn { self.growth2.start([1.0], [32.0], 30, 3); }
            if character == Character::Saki { self.growth2.start([1.0], [32.0], 10, 4); }
            if character == Character::Orin { self.launch(pos, vel_x); }
            if leans(character) { self.angle = lean_target(vel_x); }
            let draw_pos = if column { [pos[0], 0.0] } else { pos };
            self.vm = Some(world.spawn_clipped(lib, file, script, field_to_canvas(field, draw_pos), field));
            if character == Character::Mamizou {
                self.side_vms = std::array::from_fn(|_| Some(world.spawn_clipped(lib, file, script, field_to_canvas(field, pos), field)));
            }
            // The start functions place the aura on the player (Orin's projectile too).
            self.pos = pos;
        } else if !held && self.active {
            self.active = false;
            // Orin's [7] keeps testing the last box while the skill circle lasts.
            if character != Character::Orin { self.radius = 0.0; }
            if let Some(id) = self.vm.take() { world.delete(id); }
            for id in self.side_vms.iter_mut().filter_map(|v| v.take()) { world.delete(id); }
        }
        if !self.active { return; }
        match character {
            Character::Aunn => self.update_aunn(pos, world, field),
            Character::Nazrin => self.update_nazrin(pos, enemies, world, field),
            Character::Orin => self.update_orin(pos, vel_x, world, field),
            Character::Tsukasa | Character::Mamizou | Character::Yachie | Character::Saki => self.update_leaning(pos, vel_x, world, field),
            _ => {
                self.pos = if character == Character::Seiran { [pos[0], pos[1] - 160.0] } else { pos };
                if let Some([radius]) = self.growth.step() { self.radius = radius; }
                if let Some(v) = self.vm.and_then(|id| world.get_mut(id)) {
                    // AuraMarisaInf 00417980 explicitly zeroes y; the ANM spans y=0..448.
                    let draw_pos = if column { [pos[0], 0.0] } else { self.pos };
                    let at = field_to_canvas(field, draw_pos);
                    v.origin = [at[0], at[1], 0.0];
                    // Reimu's circle scales both axes; Marisa's column, Sanae's, Nazrin's and
                    // Seiran's (+0x58 = 1) only x.
                    v.scale = if column || matches!(character, Character::Sanae | Character::Seiran) { [self.radius, 1.0] } else { [self.radius; 2] };
                }
            }
        }
        self.time += 1;
    }

    /// `FUN_0041a4b0`: both radii step, the VM takes (w, 1) / (h, 1) as its scale and scale2,
    /// then at 30 and 40 frames both restart from their current values.
    fn update_aunn(&mut self, pos: [f32; 2], world: &mut World, field: [f32; 4]) {
        if let Some([w]) = self.growth.step() { self.radius = w; }
        if let Some([h]) = self.growth2.step() { self.radius2 = h; }
        self.pos = pos;
        if let Some(v) = self.vm.and_then(|id| world.get_mut(id)) {
            v.scale = [self.radius, 1.0];
            v.scale2 = [self.radius2, 1.0];
            let at = field_to_canvas(field, pos);
            v.origin = [at[0], at[1], 0.0];
        }
        if self.time == 30 {
            self.growth.start([self.radius], [64.0], 10, 5);
            self.growth2.start([self.radius2], [192.0], 10, 5);
        } else if self.time == 40 {
            self.growth.start([self.radius], [80.0], 10, 2);
            self.growth2.start([self.radius2], [16.0], 10, 2);
        }
    }

    /// `FUN_0041ad00`: on the first two frames the circle sits on the player; then it moves
    /// 4 px a frame (the whole way when within 2) towards the nearest active spirit within 320
    /// of it, or towards the player.
    fn update_nazrin(&mut self, pos: [f32; 2], enemies: &[Enemy], world: &mut World, field: [f32; 4]) {
        if let Some([r]) = self.growth.step() { self.radius = r; }
        if let Some(v) = self.vm.and_then(|id| world.get_mut(id)) { v.scale = [self.radius, 1.0]; }
        if self.time < 2 {
            self.pos = pos;
        } else {
            let target = nearest_spirit(enemies, self.pos, 320.0).unwrap_or(pos);
            self.pos = approach(self.pos, target, 4.0, 4.0);
        }
        if let Some(v) = self.vm.and_then(|id| world.get_mut(id)) {
            let at = field_to_canvas(field, self.pos);
            v.origin = [at[0], at[1], 0.0];
        }
    }

    /// Orin's projectile restarts at the player (`FUN_0041c530`, and `FUN_0041c770` once it has
    /// passed y = -60): speed 1, straight up or 6 degrees towards her horizontal movement
    /// (|+0x20f4| > 0.1), R growing 1 -> 50 over 10 frames (mode 6).
    fn launch(&mut self, pos: [f32; 2], vel_x: f32) {
        self.pos = pos;
        self.speed = 1.0;
        self.angle = orin_angle(vel_x);
        self.growth.start([1.0], [50.0], 10, 6);
    }

    fn update_orin(&mut self, pos: [f32; 2], vel_x: f32, world: &mut World, field: [f32; 4]) {
        if self.pos[1] < -60.0 { self.launch(pos, vel_x); }
        if let Some([r]) = self.growth.step() { self.radius = r; }
        if let Some(v) = self.vm.and_then(|id| world.get_mut(id)) { v.scale = [self.radius; 2]; }
        // FUN_004d4dc0 / FUN_004d4a00 move by the speed, which then grows by 0.1.
        let (s, c) = self.angle.sin_cos();
        self.pos = [self.pos[0] + c * self.speed, self.pos[1] + s * self.speed];
        self.speed += 0.1;
        if let Some(v) = self.vm.and_then(|id| world.get_mut(id)) {
            let at = field_to_canvas(field, self.pos);
            v.origin = [at[0], at[1], 0.0];
            v.rot[2] = self.angle;
        }
    }

    /// The leaning auras' updates (`0041de20`, `0041f770`, `00421490`, `00423540`): the lean
    /// eases, the radii step, the shape is placed from the player and the VMs follow.
    fn update_leaning(&mut self, pos: [f32; 2], vel_x: f32, world: &mut World, field: [f32; 4]) {
        self.angle = lean_step(self.angle, vel_x);
        if let Some([r]) = self.growth.step() { self.radius = r; }
        if self.character == Character::Saki {
            if let Some([r]) = self.growth2.step() { self.radius2 = r; }
        }
        let (s, c) = self.angle.sin_cos();
        self.anchor = pos;
        // Saki's ellipse sits at rot(angle) (0, -32) from her.
        self.pos = if self.character == Character::Saki { [pos[0] + 32.0 * s, pos[1] - 32.0 * c] } else { pos };
        let r = self.radius;
        let (scale, scale2, rot) = match self.character {
            Character::Tsukasa => ([r, r], None, Some(self.angle)),
            Character::Mamizou => ([r, r], None, None),
            Character::Yachie => ([r, r], None, Some(crate::motion::wrap_angle(self.angle - std::f32::consts::FRAC_PI_2))),
            _ => ([r, 1.0], Some([self.radius2, 1.0]), Some(self.angle)),
        };
        let centres = if self.character == Character::Mamizou { mamizou_circles(pos, self.angle) } else { [self.pos; 3] };
        if self.character == Character::Mamizou { self.pos = centres[0]; }
        if let Some(v) = self.vm.and_then(|id| world.get_mut(id)) {
            v.scale = scale;
            if let Some(s2) = scale2 { v.scale2 = s2; }
            if let Some(a) = rot { v.rot[2] = a; }
            let at = field_to_canvas(field, centres[0]);
            v.origin = [at[0], at[1], 0.0];
        }
        for (k, id) in self.side_vms.iter().enumerate() {
            if let Some(v) = id.and_then(|id| world.get_mut(id)) {
                v.scale = [r * 0.6; 2];
                let at = field_to_canvas(field, centres[k + 1]);
                v.origin = [at[0], at[1], 0.0];
            }
        }
    }

    /// `FUN_0041d020`: Orin's skill circle at `pos`.
    pub fn start_circle(&mut self, pos: [f32; 2], file: usize, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        if let Some(id) = self.circle.take().and_then(|c| c.vm) { world.delete(id); }
        let mut growth = Interp::default();
        growth.start([1.0], [500.0], 60, 4);
        let vm = Some(world.spawn_clipped(lib, file, 12, field_to_canvas(field, pos), field));
        self.circle = Some(SkillCircle { pos, radius: 1.0, frames: 60, growth, vm });
    }

    /// Aura [4] (`FUN_0041c9c0`): while frames remain, step the radius into the VM's scale.
    fn update_circle(&mut self, world: &mut World, field: [f32; 4]) {
        let Some(c) = &mut self.circle else { return };
        if let Some([r]) = c.growth.step() { c.radius = r; }
        if let Some(v) = c.vm.and_then(|id| world.get_mut(id)) {
            v.scale = [c.radius; 2];
            let at = field_to_canvas(field, c.pos);
            v.origin = [at[0], at[1], 0.0];
        }
        c.frames -= 1;
        if c.frames == 0 { self.circle = None; }
    }

    /// Bullet callback: squared radii are added, rather than squaring their sum.
    pub fn contains_circle(&self, pos: [f32; 2], radius: f32) -> bool {
        self.active && bullet_hit(self.character,self.pos,self.radius,pos,radius)
    }

    /// Spirit callback: oriented rectangle against the scope circle (004d44e0).
    pub fn intersects_rect(&self, pos: [f32; 2], size: [f32; 2], angle: f32) -> bool {
        if !self.active { return false; }
        let (s,c) = angle.sin_cos();
        let (dx,dy) = (self.pos[0]-pos[0], self.pos[1]-pos[1]);
        let (u,v) = (dx*c+dy*s, -dx*s+dy*c);
        let x = (u.abs()-size[0]*0.5).max(0.0);
        let y = (v.abs()-size[1]*0.5).max(0.0);
        x*x+y*y < self.radius*self.radius
    }
}

/// Whether the character's aura leans with the movement.
fn leans(character: Character) -> bool {
    matches!(character, Character::Tsukasa | Character::Mamizou | Character::Yachie | Character::Saki)
}

/// The lean an aura (or Saki's bomb) starts with: +-0.12217 (7 degrees) beyond +-0.1 of
/// horizontal velocity (+0x20f4), else 0.
pub fn lean_target(vel_x: f32) -> f32 {
    if vel_x < -0.1 { -0.12217305 } else if vel_x > 0.1 { 0.12217305 } else { 0.0 }
}

/// One frame of the lean: 10% of the way to `lean_target`.
pub fn lean_step(angle: f32, vel_x: f32) -> f32 {
    crate::motion::wrap_angle(angle + (lean_target(vel_x) - angle) * 0.1)
}

/// Mamizou's three circle centres (`FUN_0041f770`): `pos` plus (0, -96), (64, -64) and (-64, -64)
/// turned by `angle`.
pub fn mamizou_circles(pos: [f32; 2], angle: f32) -> [[f32; 2]; 3] {
    let (s, c) = angle.sin_cos();
    [[pos[0] + 96.0 * s, pos[1] - 96.0 * c],
     [pos[0] + c * 64.0 + s * 64.0, pos[1] + s * 64.0 - c * 64.0],
     [pos[0] - c * 64.0 + s * 64.0, pos[1] - s * 64.0 - c * 64.0]]
}

/// `FUN_004d3ae0`: whether segment `p1`-`p2` crosses edge `a`-`b` (touching counts; collinear
/// segments by their bounding boxes).
pub fn segments_cross(p1: [f32; 2], p2: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    let f1 = (a[0]-b[0])*(p1[1]-a[1]) + (a[1]-b[1])*(a[0]-p1[0]);
    let f2 = (a[0]-b[0])*(p2[1]-a[1]) + (a[1]-b[1])*(a[0]-p2[0]);
    if f2 * f1 > 0.0 { return false; }
    if f1 != 0.0 || f2 != 0.0 {
        return ((b[1]-p1[1])*(p1[0]-p2[0]) + (p1[0]-b[0])*(p1[1]-p2[1]))
            * ((p1[0]-a[0])*(p1[1]-p2[1]) + (a[1]-p1[1])*(p1[0]-p2[0])) <= 0.0;
    }
    let (lo, hi) = if b[0] < a[0] { (b, a) } else { (a, b) };
    let (q1, q2) = if p2[0] < p1[0] { (p2, p1) } else { (p1, p2) };
    lo[0] <= q2[0] && lo[1] <= q2[1] && q1[0] <= hi[0] && q1[1] <= hi[1]
}

/// The edges of a regular polygon (`sides` vertices on a circle of `radius` round `centre`,
/// the first at `angle`).
fn polygon_edges(centre: [f32; 2], radius: f32, angle: f32, sides: u32) -> impl Iterator<Item = ([f32; 2], [f32; 2])> {
    let step = std::f32::consts::TAU / sides as f32;
    let vertex = move |t: f32| [t.cos() * radius + centre[0], t.sin() * radius + centre[1]];
    (0..sides).scan(angle, move |t, _| {
        let a = vertex(*t);
        *t = crate::motion::wrap_angle(*t + step);
        Some((a, vertex(*t)))
    })
}

/// `FUN_004d2a10`: `point` is inside the polygon when the segment from it to the centre crosses
/// none of the edges.
pub fn polygon_contains(point: [f32; 2], centre: [f32; 2], radius: f32, angle: f32, sides: u32) -> bool {
    !polygon_edges(centre, radius, angle, sides).any(|(a, b)| segments_cross(point, centre, a, b))
}

/// `FUN_004d26a0` (and Yachie's [7] `FUN_004217e0`): a circle against a regular polygon. It hits
/// when the polygon's centre lies within the circle, else when the circle's point nearest that
/// centre is inside the polygon.
pub fn polygon_circle(pos: [f32; 2], r: f32, centre: [f32; 2], radius: f32, angle: f32, sides: u32) -> bool {
    let d = [centre[0]-pos[0], centre[1]-pos[1]];
    let d2 = d[0]*d[0] + d[1]*d[1];
    if r * r >= d2 { return true; }
    let len = d2.sqrt();
    let n = if len.abs() >= 0.01 { [d[0]/len, d[1]/len] } else { d };
    polygon_contains([pos[0] + n[0]*r, pos[1] + n[1]*r], centre, radius, angle, sides)
}

/// `FUN_004d22d0`: a box (`size`, turned by `box_angle` when non-zero) against a regular
/// polygon: the polygon's centre within the box's unturned extent, the box centre inside the
/// polygon, or one of the box's edges crossing one of the polygon's (`FUN_004d27e0`).
pub fn polygon_rect(pos: [f32; 2], size: [f32; 2], box_angle: f32, centre: [f32; 2], radius: f32, angle: f32, sides: u32) -> bool {
    let (w, h) = (size[0] / 2.0, size[1] / 2.0);
    if w > (centre[0]-pos[0]).abs() && h > (centre[1]-pos[1]).abs() { return true; }
    if polygon_contains(pos, centre, radius, angle, sides) { return true; }
    let mut corners = [[w, h], [-w, h], [-w, -h], [w, -h]];
    if box_angle != 0.0 {
        let (s, c) = box_angle.sin_cos();
        for p in &mut corners { *p = [p[0]*c - p[1]*s, p[0]*s + p[1]*c]; }
    }
    let corners = corners.map(|p| [p[0] + pos[0], p[1] + pos[1]]);
    (0..4).any(|i| {
        let (p1, p2) = (corners[i], corners[(i + 1) % 4]);
        polygon_edges(centre, radius, angle, sides).any(|(a, b)| segments_cross(p1, p2, a, b))
    })
}

/// Orin's launch direction from her horizontal velocity (`FUN_0041c530`, `FUN_0054afc0`,
/// `FUN_0041bf70`): -pi/2, turned 6 degrees towards the movement beyond +-0.1.
pub fn orin_angle(vel_x: f32) -> f32 {
    if vel_x < -0.1 { -std::f32::consts::FRAC_PI_2 - 6.0f32.to_radians() }
    else if vel_x > 0.1 { 6.0f32.to_radians() - std::f32::consts::FRAC_PI_2 }
    else { -std::f32::consts::FRAC_PI_2 }
}

/// `FUN_00508420`: the nearest spirit (internal kind set) that is not disabled, hidden or
/// unhittable, strictly within `range` of `from`.
pub fn nearest_spirit(enemies: &[Enemy], from: [f32; 2], range: f32) -> Option<[f32; 2]> {
    let mut best = range * range;
    let mut found = None;
    for e in enemies {
        let d = &e.data;
        if d.delete || d.flags & (crate::enemy::flag::NO_HURTBOX | crate::enemy::flag::HIDDEN) != 0
            || d.spirit_kind == 0 || d.spirit_disabled { continue; }
        let d2 = (from[0]-d.pos[0]).powi(2)+(from[1]-d.pos[1]).powi(2);
        if d2 < best { best = d2; found = Some(d.pos); }
    }
    found
}

/// Step `from` towards `to`: when the squared distance exceeds `limit2` move `speed` along the
/// direction (normalised when the length is at least 0.01), otherwise land on `to`.
/// `FUN_0041ad00` uses (4, 4.0) and `FUN_00540aa0` (3, 9.0).
pub fn approach(from: [f32; 2], to: [f32; 2], speed: f32, limit2: f32) -> [f32; 2] {
    let d = [to[0]-from[0], to[1]-from[1]];
    let d2 = d[0]*d[0]+d[1]*d[1];
    if d2 > limit2 {
        let len = d2.sqrt();
        let n = if len.abs() >= 0.01 { [d[0]/len, d[1]/len] } else { d };
        [from[0]+n[0]*speed, from[1]+n[1]*speed]
    } else {
        to
    }
}

pub(crate) fn rect_circle(centre: [f32;2], size: [f32;2], pos: [f32;2], radius: f32) -> bool {
    let x=((pos[0]-centre[0]).abs()-size[0]*0.5).max(0.0);
    let y=((pos[1]-centre[1]).abs()-size[1]*0.5).max(0.0);
    x*x+y*y <= radius*radius
}

/// `FUN_004d44e0`: a rectangle (`size[0]` along `angle`, `size[1]` across) against a circle:
/// inside the box widened or heightened by the radius, or strictly within it of a corner.
pub fn rect_circle_exact(centre: [f32; 2], size: [f32; 2], angle: f32, pos: [f32; 2], radius: f32) -> bool {
    let (s, c) = (-angle).sin_cos();
    let (dx, dy) = (pos[0]-centre[0], pos[1]-centre[1]);
    let u = dx*s + dy*c;
    let v = dx*c - dy*s;
    let (hw, hh) = (size[0]/2.0, size[1]/2.0);
    if (v.abs() <= hw + radius && u.abs() <= hh) || (v.abs() <= hw && u.abs() <= hh + radius) { return true; }
    [(v-hw, u-hh), (v+hw, u-hh), (v-hw, u+hh), (v+hw, u+hh)].iter().any(|&(x, y)| x*x + y*y < radius*radius)
}

/// `FUN_004d4020`: an ellipse at `centre` with semi-axis `a` along `theta` and `b` across it,
/// against a circle. A circle smaller than both axes is tested exactly against the ellipse
/// shrunk by its radius; one larger than both contains it when the gap allows; otherwise points
/// are sampled from -pi: on the ellipse (max(8, (a + b) / 4) of them) against the circle when
/// the circle is the larger, else on the circle (max(8, r / 4)) against the ellipse.
pub fn ellipse_circle(pos: [f32; 2], r: f32, centre: [f32; 2], a: f32, b: f32, theta: f32) -> bool {
    use std::f32::consts::{PI, TAU};
    let (big, small) = (a.max(b), a.min(b));
    let (dx, dy) = (pos[0]-centre[0], pos[1]-centre[1]);
    let (s, c) = (-theta).sin_cos();
    if r < small {
        let u = dx*s + dy*c;
        let v = dx*c - dy*s;
        if u*u/((b-r)*(b-r)) + v*v/((a-r)*(a-r)) <= 1.0 { return true; }
    }
    let x = dx*c - dy*s;
    let y = dy*c + dx*s;
    if r > big && (r-big)*(r-big) >= x*x + y*y { return true; }
    if 1.0 > r || 1.0 > big { return false; }
    let mut t = -PI;
    if r > big {
        let n = ((a + b) as i32 / 4).max(8);
        let step = TAU / n as f32;
        for _ in 0..n {
            let q = [t.cos()*a, t.sin()*b];
            t += step;
            if (y-q[1])*(y-q[1]) + (x-q[0])*(x-q[0]) <= r*r { return true; }
        }
    } else {
        let n = ((r / 4.0) as i32).max(8);
        let step = TAU / n as f32;
        for _ in 0..n {
            let q = [x + t.cos()*r, y + t.sin()*r];
            t += step;
            if q[0]*q[0]/(a*a) + q[1]*q[1]/(b*b) <= 1.0 { return true; }
        }
    }
    false
}

/// `FUN_004d3c50`: a box (`size` = the 16-pixel laser step, `alpha` its angle) at `pos` against
/// an ellipse (`a` along `theta`, `b` across). The box centre goes into the ellipse's frame and
/// its extents are taken as `cos(beta) w - sin(beta) h` by `sin(beta) w + cos(beta) h`
/// (beta = alpha - theta, wrapped; signs kept as the original computes them). A box larger than
/// both axes checks max(8, (int)(a + b) / 4) ellipse points, starting at `alpha`, for one strictly
/// inside the box; otherwise a grid of max(3, (int)(max(w, h) / 8)) squared points spread over
/// the box is tested against the ellipse.
pub fn ellipse_rect(pos: [f32; 2], size: [f32; 2], alpha: f32, centre: [f32; 2], a: f32, b: f32, theta: f32) -> bool {
    use std::f32::consts::TAU;
    let (w, h) = (size[0], size[1]);
    let (dx, dy) = (pos[0] - centre[0], pos[1] - centre[1]);
    let (s, c) = (-theta).sin_cos();
    let x = c * dx - s * dy;
    let y = s * dx + c * dy;
    let beta = crate::motion::wrap_angle(alpha - theta);
    let (s2, c2) = beta.sin_cos();
    let wr = c2 * w - s2 * h;
    let hr = s2 * w + c2 * h;
    if w.max(h) > a.max(b) {
        let n = ((a + b) as i32 / 4).max(8);
        let step = TAU / n as f32;
        let mut t = alpha;
        for _ in 0..n {
            let p = [t.cos() * a, t.sin() * b];
            t += step;
            if (p[0] - x).abs() < wr / 2.0 && (p[1] - y).abs() < hr / 2.0 { return true; }
        }
        return false;
    }
    let m = ((w.max(h) / 8.0) as i32).max(3);
    let mut gy = -hr / 2.0;
    for _ in 0..m {
        let mut gx = -wr / 2.0;
        for _ in 0..m {
            let q = [x + gx, y + gy];
            if q[0] * q[0] / (a * a) + q[1] * q[1] / (b * b) <= 1.0 { return true; }
            gx += wr / (m - 1) as f32;
        }
        gy += hr / (m - 1) as f32;
    }
    false
}

pub(crate) fn bullet_hit(character: Character, centre: [f32;2], r: f32, pos: [f32;2], radius: f32) -> bool {
    match character {
        Character::Marisa => (pos[0]-centre[0]).abs()<r,
        Character::Sanae | Character::Nazrin | Character::Seiran => (pos[0]-centre[0]).powi(2)+(pos[1]-centre[1]).powi(2)<=(r+radius).powi(2),
        Character::Ran => rect_circle(centre,[r,7.0*r],pos,radius) || rect_circle(centre,[7.0*r,r],pos,radius),
        Character::Reimu => (pos[0]-centre[0]).powi(2)+(pos[1]-centre[1]).powi(2)<r*r+radius*radius,
        // Aunn and Orin need the whole probe (`Probe::test`).
        _ => Probe { character, active: true, centre, radius: r, radius2: r, circle: None, angle: 0.0 }.test(pos, radius) > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipse_circle_uses_the_vertical_axis_for_minus_half_pi() {
        // a = 100 along -pi/2 (vertical), b = 20 across (horizontal).
        let e = |p: [f32; 2], r: f32| ellipse_circle(p, r, [0.0, 0.0], 100.0, 20.0, -std::f32::consts::FRAC_PI_2);
        assert!(e([0.0, 90.0], 5.0));
        assert!(!e([0.0, 110.0], 5.0));
        assert!(e([15.0, 0.0], 4.0));
        assert!(!e([30.0, 0.0], 4.0));
        // A circle larger than both axes that swallows the ellipse.
        assert!(e([5.0, 5.0], 150.0));
        // Sampled edge for a circle between the axes.
        assert!(e([0.0, 140.0], 50.0));
        assert!(!e([0.0, 160.0], 50.0));
    }

    #[test]
    fn exact_rect_circle_matches_cross_and_corners() {
        let t = |p: [f32; 2], r: f32| rect_circle_exact([0.0, 0.0], [100.0, 50.0], 0.0, p, r);
        assert!(t([54.0, 0.0], 5.0));
        assert!(!t([56.0, 0.0], 5.0));
        assert!(t([0.0, 29.0], 5.0));
        assert!(t([52.0, 27.0], 5.0));
        assert!(!t([54.0, 29.0], 5.0));
    }

    #[test]
    fn approach_snaps_inside_the_step() {
        assert_eq!(approach([0.0, 0.0], [1.0, 1.0], 4.0, 4.0), [1.0, 1.0]);
        let p = approach([0.0, 0.0], [10.0, 0.0], 4.0, 4.0);
        assert!((p[0] - 4.0).abs() < 1e-5 && p[1] == 0.0);
        assert_eq!(approach([0.0, 0.0], [2.0, 2.0], 3.0, 9.0), [2.0, 2.0]);
    }

    #[test]
    fn hexagon_contains_its_inside_and_touches_by_the_nearest_point() {
        // Vertices at -pi/2 + k pi/3: points up and down, flat sides left and right at
        // x = +-R cos(30).
        let h = |p: [f32; 2], r: f32| polygon_circle(p, r, [0.0, 0.0], 100.0, -std::f32::consts::FRAC_PI_2, 6);
        assert!(h([0.0, 95.0], 1.0));
        assert!(h([85.0, 0.0], 1.0));
        assert!(!h([89.0, 0.0], 1.0));
        assert!(h([89.0, 0.0], 3.0), "the circle's nearest point reaches the side");
        assert!(!h([60.0, 80.0], 1.0), "outside a slanted side");
        assert!(h([3.0, 0.0], 5.0), "the centre within the circle");
        // A box straddling the flat side at x = 86.6.
        assert!(polygon_rect([90.0, 0.0], [10.0, 10.0], 0.0, [0.0, 0.0], 100.0, -std::f32::consts::FRAC_PI_2, 6));
        assert!(!polygon_rect([100.0, 0.0], [10.0, 10.0], 0.0, [0.0, 0.0], 100.0, -std::f32::consts::FRAC_PI_2, 6));
    }

    #[test]
    fn lean_eases_towards_seven_degrees() {
        assert_eq!(lean_target(1.0), 0.12217305);
        let a = lean_step(0.0, -1.0);
        assert!((a + 0.012217305).abs() < 1e-7);
        assert!((lean_step(0.1, 0.0) - 0.09).abs() < 1e-7);
        let c = mamizou_circles([0.0, 0.0], 0.0);
        assert_eq!(c, [[0.0, -96.0], [64.0, -64.0], [-64.0, -64.0]]);
    }

    #[test]
    fn orin_turns_six_degrees_towards_movement() {
        assert!((orin_angle(0.0) + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((orin_angle(448.0) + 1.4660766).abs() < 1e-5);
        assert!((orin_angle(-448.0) + 1.6755161).abs() < 1e-5);
    }
}
