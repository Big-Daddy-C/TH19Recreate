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
    /// Orin's projectile speed (+0x30) and direction (+0x34).
    speed: f32,
    angle: f32,
    pub circle: Option<SkillCircle>,
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
        Probe { character: self.character, active: self.active, centre: self.pos, radius: self.radius, radius2: self.radius2,
            circle: self.circle.as_ref().map(|c| (c.pos, c.radius)) }
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
            };
            self.growth.start([1.0], [size], frames, mode);
            if character == Character::Aunn { self.growth2.start([1.0], [32.0], 30, 3); }
            if character == Character::Orin { self.launch(pos, vel_x); }
            let draw_pos = if column { [pos[0], 0.0] } else { pos };
            self.vm = Some(world.spawn_clipped(lib, file, script, field_to_canvas(field, draw_pos), field));
            // The start functions place the aura on the player (Orin's projectile too).
            self.pos = pos;
        } else if !held && self.active {
            self.active = false;
            // Orin's [7] keeps testing the last box while the skill circle lasts.
            if character != Character::Orin { self.radius = 0.0; }
            if let Some(id) = self.vm.take() { world.delete(id); }
        }
        if !self.active { return; }
        match character {
            Character::Aunn => self.update_aunn(pos, world, field),
            Character::Nazrin => self.update_nazrin(pos, enemies, world, field),
            Character::Orin => self.update_orin(pos, vel_x, world, field),
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

pub(crate) fn bullet_hit(character: Character, centre: [f32;2], r: f32, pos: [f32;2], radius: f32) -> bool {
    match character {
        Character::Marisa => (pos[0]-centre[0]).abs()<r,
        Character::Sanae | Character::Nazrin | Character::Seiran => (pos[0]-centre[0]).powi(2)+(pos[1]-centre[1]).powi(2)<=(r+radius).powi(2),
        Character::Ran => rect_circle(centre,[r,7.0*r],pos,radius) || rect_circle(centre,[7.0*r,r],pos,radius),
        Character::Reimu => (pos[0]-centre[0]).powi(2)+(pos[1]-centre[1]).powi(2)<r*r+radius*radius,
        // Aunn and Orin need the whole probe (`Probe::test`).
        Character::Aunn | Character::Orin => Probe { character, active: true, centre, radius: r, radius2: r, circle: None }.test(pos, radius) > 0,
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
    fn orin_turns_six_degrees_towards_movement() {
        assert!((orin_angle(0.0) + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((orin_angle(448.0) + 1.4660766).abs() < 1e-5);
        assert!((orin_angle(-448.0) + 1.6755161).abs() < 1e-5);
    }
}
