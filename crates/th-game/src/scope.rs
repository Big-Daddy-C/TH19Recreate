//! The charging aura (`AuraReimuInf` callbacks 00416d50, 00416e80, 00416f10, 00416f60; Marisa's
//! `AuraMarisaInf` 00417840, 00417980, 00417a10, 00417a60). Reimu's grows into a circle of radius
//! 64, Marisa's into a full-height column of half width 14, both over 20 frames (interpolation
//! mode 4) at the default 100% rate.
use th_anm::{interp::Interp, AnmLibrary, VmId, World};
use crate::enemy::field_to_canvas;
use crate::character::Character;

#[derive(Default)]
pub struct Scope {
    pub radius: f32,
    pub pos: [f32; 2],
    pub vm: Option<VmId>,
    active: bool,
    growth: Interp<1>,
    character: Character,
}
impl Scope {
    pub fn active(&self) -> bool { self.active }

    pub fn update(&mut self, held: bool, column: bool, pos: [f32; 2], file: usize,
                  world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        self.update_character(held, if column { Character::Marisa } else { Character::Reimu }, pos, file, world, lib, field);
    }

    pub fn update_character(&mut self, held: bool, character: Character, pos: [f32; 2], file: usize,
                  world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        self.character = character;
        let column = character == Character::Marisa;
        if held && !self.active {
            self.active = true;
            self.radius = 1.0;
            // Default status +0x8c is 100 percent; ability modifiers remain unported.
            // AuraSanaeInf004184a0:128/60/mode2; AuraRanInf00419360:30/30/mode6.
            let (size, frames, mode, script) = match character {
                Character::Reimu => (64.0,20,4,15),
                Character::Marisa => (14.0,20,4,15),
                Character::Sanae => (128.0,60,2,16),
                Character::Ran => (30.0,30,6,11),
            };
            self.growth.start([1.0], [size], frames, mode);
            let draw_pos = if column { [pos[0], 0.0] } else { pos };
            self.vm = Some(world.spawn_clipped(lib, file, script, field_to_canvas(field, draw_pos), field));
        } else if !held && self.active {
            self.active = false;
            self.radius = 0.0;
            if let Some(id) = self.vm.take() { world.delete(id); }
        }
        self.pos = pos;
        if self.active {
            if let Some([radius]) = self.growth.step() { self.radius = radius; }
            if let Some(v) = self.vm.and_then(|id| world.get_mut(id)) {
                // AuraMarisaInf 00417980 explicitly zeroes y; the ANM spans y=0..448.
                let draw_pos = if column { [pos[0], 0.0] } else { pos };
                let at = field_to_canvas(field, draw_pos);
                v.origin = [at[0], at[1], 0.0];
                // Reimu's circle scales both axes; Marisa's column only its width.
                v.scale = if column || character == Character::Sanae { [self.radius, 1.0] } else { [self.radius; 2] };
            }
        }
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

pub(crate) fn rect_circle(centre: [f32;2], size: [f32;2], pos: [f32;2], radius: f32) -> bool {
    let x=((pos[0]-centre[0]).abs()-size[0]*0.5).max(0.0);
    let y=((pos[1]-centre[1]).abs()-size[1]*0.5).max(0.0);
    x*x+y*y <= radius*radius
}

pub(crate) fn bullet_hit(character: Character, centre: [f32;2], r: f32, pos: [f32;2], radius: f32) -> bool {
    match character {
        Character::Marisa => (pos[0]-centre[0]).abs()<r,
        Character::Sanae => (pos[0]-centre[0]).powi(2)+(pos[1]-centre[1]).powi(2)<=(r+radius).powi(2),
        Character::Ran => rect_circle(centre,[r,7.0*r],pos,radius) || rect_circle(centre,[7.0*r,r],pos,radius),
        Character::Reimu => (pos[0]-centre[0]).powi(2)+(pos[1]-centre[1]).powi(2)<r*r+radius*radius,
    }
}
