//! Reimu's and Marisa's bombs: Reimu's `BombReimuInf` (`FUN_0053c7d0` start, `FUN_0053c510` per frame), started by
//! the player state machine (`FUN_00546320`: button bit 4, `FUN_004ed6d0`/`FUN_004ed7d0`).
//! It is a circle that grows around the player for as long as the meter allowed: at the
//! maximum meter (2500) 250 frames, radius 1 -> 268 decelerating (interpolation mode 4) with
//! a squashed ring (y scale 1 -> 36). Every frame it cancels bullets (the player's cancel
//! rules), damages enemies for 10, and keeps the player protected; lasers (`FUN_00530c60`)
//! are not ported. Marisa's (`BombMarisaInf`, `FUN_0053dc50` start, `FUN_0053d9b0` per frame) is a
//! full-height column centred on her that widens from 1 to meter * 64 / 4000 + 16 pixels each side
//! over 350 * meter / 2500 (at least 180) frames, cancelling bullets (`FUN_004ee0d0`) and damaging
//! enemies for 20 a frame.
use crate::enemy::field_to_canvas;
use crate::skill::DamageCircle;
use th_anm::{interp::Interp, AnmLibrary, VmId, World};

/// Meter ceiling (status +0x24 is clamped to 0..2500).
pub const MAX_METER: i32 = 2500;
/// Frames the player stays protected after each bomb frame (`player+0x2074` block: 0x28).
pub const PROTECTION: i32 = 40;
/// Damage per frame to every enemy inside (`FUN_005200f0(.., lifetime 1, damage 10)`).
pub const DAMAGE: i32 = 10;
/// Damage per frame of Marisa's column (`FUN_0051f320(.., 1, 0x14)`).
pub const COLUMN_DAMAGE: i32 = 20;

pub struct Bomb {
    /// Marisa's column instead of Reimu's circle.
    column: bool,
    cross: bool,
    locked: bool,
    /// BombMarisaInf +0x30: launch x, y explicitly zero (0053dc50).
    column_pos: [f32; 2],
    time: i32,
    duration: i32,
    radius: Interp<1>,
    squash: Interp<1>,
    ring: VmId,
    flash: VmId,
    pub current_radius: f32,
}

impl Bomb {
    /// `FUN_004ed3d0(0x78, 0xfa)`: the longer of 120 frames and 250 scaled by the meter.
    pub fn duration(meter: i32) -> i32 {
        (250 * meter.clamp(0, MAX_METER) / MAX_METER).max(120)
    }

    /// `FUN_004ed3d0(0xb4, 0x15e)`: Marisa's bomb lasts at least 180 frames, up to 350.
    pub fn duration_marisa(meter: i32) -> i32 {
        (350 * meter.clamp(0, MAX_METER) / MAX_METER).max(180)
    }

    /// `FUN_0053dc50`: the flash (pl01.anm script 23) and the column (script 18).
    pub fn new_marisa(pos: [f32; 2], meter: i32, file: usize, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) -> Self {
        let meter = meter.clamp(0, MAX_METER);
        let duration = Self::duration_marisa(meter);
        let column_pos = [pos[0], 0.0];
        let at = field_to_canvas(field, column_pos);
        let flash = world.spawn_clipped(lib, file, 23, at, field);
        let ring = world.spawn_clipped(lib, file, 18, at, field);
        let mut radius = Interp::default();
        radius.start([1.0], [meter as f32 * 64.0 / 4000.0 + 16.0], duration, 4);
        Self { column: true, cross: false, locked: true, column_pos, time: 0, duration, radius, squash: Interp::default(), ring, flash, current_radius: 1.0 }
    }

    /// `FUN_0053c7d0`: a flash (pl00.anm script 19) and the ring (script 16) at the player.
    pub fn new(pos: [f32; 2], meter: i32, file: usize, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) -> Self {
        let meter = meter.clamp(0, MAX_METER);
        let duration = Self::duration(meter);
        let at = field_to_canvas(field, pos);
        let flash = world.spawn_clipped(lib, file, 19, at, field);
        let ring = world.spawn_clipped(lib, file, 16, at, field);
        let mut radius = Interp::default();
        radius.start([1.0], [meter as f32 * 224.0 / 4000.0 + 128.0], duration, 4);
        let mut squash = Interp::default();
        squash.start([1.0], [meter as f32 * 32.0 / 4000.0 + 16.0], duration, 4);
        Self { column: false, cross: false, locked: false, column_pos: [0.0; 2], time: 0, duration, radius, squash, ring, flash, current_radius: 1.0 }
    }

    /// BombSanaeInf00417f10 and BombRanInf00418e60 use a fixed launch position.
    pub fn new_character(character: crate::character::Character, pos: [f32;2], meter: i32, file: usize,
        world: &mut World, lib: &AnmLibrary, field: [f32;4]) -> Self {
        let cross=character==crate::character::Character::Ran;
        let meter=meter.clamp(0,MAX_METER);
        let duration=((if cross {300} else {320})*meter/MAX_METER).max(120);
        let at=field_to_canvas(field,pos);
        let flash=world.spawn_clipped(lib,file,if cross {16} else {20},at,field);
        let ring=world.spawn_clipped(lib,file,if cross {13} else {17},at,field);
        let mut radius=Interp::default();
        radius.start([1.0],[if cross {meter as f32/2500.0*64.0+32.0} else {meter as f32/2500.0*224.0+128.0}],duration,4);
        let mut squash=Interp::default();
        if !cross {squash.start([1.0],[meter as f32/2500.0*32.0+16.0],duration,4);}
        Self {column:false,cross,locked:true,column_pos:pos,time:0,duration,radius,squash,ring,flash,current_radius:1.0}
    }

    /// One frame (`FUN_0053c510`). Returns false once the bomb has finished.
    pub fn tick(&mut self, pos: [f32; 2], world: &mut World, field: [f32; 4], cancel: &mut Vec<([f32; 2], f32)>, columns: &mut Vec<(f32, f32)>, damage: &mut Vec<DamageCircle>) -> bool {
        self.tick_character(pos,world,field,cancel,columns,&mut Vec::new(),damage)
    }

    pub fn tick_character(&mut self, pos: [f32;2], world: &mut World, field:[f32;4],
        cancel:&mut Vec<([f32;2],f32)>, columns:&mut Vec<(f32,f32)>,
        rects:&mut Vec<([f32;2],[f32;2])>, damage:&mut Vec<DamageCircle>) -> bool {
        self.time += 1;
        if self.time < self.duration {
            if let Some([r]) = self.radius.step() { self.current_radius = r; }
            let squash = self.squash.step().map_or(1.0, |[s]| s);
            let pos = if self.locked { self.column_pos } else { pos };
            let at = field_to_canvas(field, pos);
            if let Some(vm) = world.get_mut(self.flash) { vm.origin = [at[0], at[1], 0.0]; }
            if let Some(vm) = world.get_mut(self.ring) {
                vm.origin = [at[0], at[1], 0.0];
                vm.scale = if self.column { [self.current_radius, 1.0] } else if self.cross { [self.current_radius;2] } else { [self.current_radius, squash] };
            }
            if self.column {
                // The band is centred on the player's x, 2 * width wide and the whole field tall.
                columns.push((pos[0], self.current_radius));
                damage.push(DamageCircle { pos: [pos[0], 224.0], radius: 0.0, damage: COLUMN_DAMAGE, remaining: 1, growth: 0.0,
                    rect: Some([self.current_radius * 2.0, 448.0]) });
            } else if self.cross {
                for size in [[self.current_radius*7.0,self.current_radius],[self.current_radius,self.current_radius*7.0]] {
                    rects.push((pos,size));
                    damage.push(DamageCircle {pos,radius:0.0,damage:20,remaining:1,growth:0.0,rect:Some(size)});
                }
            } else {
                cancel.push((pos, self.current_radius));
                damage.push(DamageCircle { pos, radius: self.current_radius, damage: DAMAGE, remaining: 1, growth: 0.0, rect: None });
            }
            return true;
        }
        if self.time == self.duration {
            world.interrupt(self.ring, 1);
            world.interrupt(self.flash, 1);
        }
        if self.time >= self.duration + 10 {
            world.delete(self.ring);
            world.delete(self.flash);
            return false;
        }
        true
    }

    /// Frames elapsed and the frame count the circle lasts.
    pub fn progress(&self) -> (i32, i32) { (self.time, self.duration) }

    /// Whether the circle is still growing (the player is protected meanwhile).
    pub fn protecting(&self) -> bool { self.time < self.duration }
}
