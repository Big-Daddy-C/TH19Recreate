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
//!
//! The later characters (bomb factory 0x5a02d0): Aunn's (`FUN_0053fe30` / `FUN_0053fac0`) is
//! an ellipse on her, its horizontal radius 1 -> meter * 100 / 4000 + 128 and vertical one
//! 1 -> meter * 400 / 4000 + 128 (mode 1) over max(120, 300 * meter / 2500) frames, cancelling
//! bullets (`FUN_004ee500`) and damaging for 10 (collider type 5). Nazrin's (`FUN_00540f00` /
//! `FUN_00540aa0`) is Reimu's ring (radius meter * 80 / 4000 + 80, band meter * 16 / 4000 + 16,
//! mode 4, max(200, 370 * meter / 2500) frames) whose centre starts on her and moves 3 px a frame
//! towards the nearest active spirit within 320, or back to her. Seiran's (`FUN_0041b5c0` /
//! `FUN_00541b10`) is the same ring fixed 160 above where she bombed, max(120, 300 * meter /
//! 2500) frames. Orin's (`FUN_0041bf70` / `FUN_0041c200`) is a box thrown up (6 degrees
//! towards her movement) at speed 1, +0.05 a frame for 60 frames and then -0.035 while
//! positive, R 1 -> meter / 2500 * 64 + 64 over 20 frames (mode 4), 2R long and R wide,
//! cancelling bullets on the field (`FUN_004ee0d0`) and lasers and damaging for 20; it lasts
//! max(220, 320 * meter / 2500) frames.
//!
//! Tsukasa's (`FUN_0041d650` / `FUN_0041d870`) is a 4R x R box (R 1 -> meter / 2500 * 40 + 64
//! over 20 frames, mode 4) that starts on her, rises 1 px a frame and spins from -pi/2 by 5
//! degrees a frame, cancelling bullets and lasers and damaging for 20; max(200, 370 * meter /
//! 2500) frames. Mamizou's (`FUN_0041ed00` / `FUN_0041efa0`) is three circles at (0, -96),
//! (-64, -64) and (64, -64) from where she bombed, rising 0.1 px a frame, R 24 -> meter / 2500 *
//! 32 + 48 over the whole max(180, 400 * meter / 2500) frames (mode 4), each cancelling and
//! damaging for 20. Yachie's (`FUN_00420c80` / `FUN_00420ef0`) is a hexagon (a vertex at -pi/2)
//! of radius 24 -> meter / 2500 * 16 + 160 over max(140, 280 * meter / 2500) frames (mode 4),
//! rising 0.1 px a frame, damaging for 20. Saki's (`FUN_004228b0` / `FUN_00422c60`) is an
//! ellipse 64 ahead of her along her lean (as her aura's), its axes 24 -> meter / 2500 * 16 + 48
//! along the lean and 24 -> meter / 2500 * 48 + 144 across it over max(160, 280 * meter / 2500)
//! frames (mode 4), damaging for 10 (`FUN_0051f4e0`).
use crate::enemy::{field_to_canvas, Enemy};
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

/// The bullet and laser cancelling shapes a bomb produces this frame.
pub struct Areas<'a> {
    pub circles: &'a mut Vec<([f32; 2], f32)>,
    pub columns: &'a mut Vec<(f32, f32)>,
    pub rects: &'a mut Vec<([f32; 2], [f32; 2])>,
    pub ellipses: &'a mut Vec<([f32; 2], f32, f32, f32)>,
    pub orects: &'a mut Vec<([f32; 2], [f32; 2], f32)>,
    /// Regular polygons (centre, radius, angle of the first vertex, sides): Yachie's hexagons.
    pub polygons: &'a mut Vec<([f32; 2], f32, f32, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind { Ring, Column, Cross, Ellipse, Seeker, Thrown, Spinner, Triple, Hexagon, Leaning }

pub struct Bomb {
    kind: Kind,
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
    /// The meter when the bomb started (BombInf +0x88), drained to 0 over the duration.
    meter_start: i32,
    /// The flash (+0x7c) follows the player while the shape keeps its own centre.
    flash_follows: bool,
    /// The shape's own centre (BombInf +0x30) for Seiran's and the moving bombs, and Orin's
    /// speed (+0x48) and direction (+0x4c).
    centre: [f32; 2],
    speed: f32,
    angle: f32,
    /// Aunn's second (vertical) radius.
    radius2: Interp<1>,
    pub current_radius2: f32,
    /// Mamizou's side rings.
    side_rings: [Option<VmId>; 2],
    /// The player's horizontal velocity this frame (+0x20f4), for Saki's lean.
    pub vel_x: f32,
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
        Self { kind: Kind::Column, locked: true, column_pos, time: 0, duration, radius, squash: Interp::default(), ring, flash, current_radius: 1.0, meter_start: meter,
            flash_follows: false, centre: column_pos, speed: 0.0, angle: 0.0, radius2: Interp::default(), current_radius2: 1.0, side_rings: [None; 2], vel_x: 0.0 }
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
        Self { kind: Kind::Ring, locked: false, column_pos: [0.0; 2], time: 0, duration, radius, squash, ring, flash, current_radius: 1.0, meter_start: meter,
            flash_follows: false, centre: pos, speed: 0.0, angle: 0.0, radius2: Interp::default(), current_radius2: 1.0, side_rings: [None; 2], vel_x: 0.0 }
    }

    /// BombSanaeInf00417f10 and BombRanInf00418e60 use a fixed launch position; Seiran's
    /// (`FUN_0041b5c0`) is fixed 160 above it with the flash following her.
    pub fn new_character(character: crate::character::Character, pos: [f32;2], meter: i32, file: usize,
        world: &mut World, lib: &AnmLibrary, field: [f32;4]) -> Self {
        use crate::character::Character::*;
        let cross=character==Ran;
        let seiran=character==Seiran;
        let meter=meter.clamp(0,MAX_METER);
        let duration=((if cross || seiran {300} else {320})*meter/MAX_METER).max(120);
        let centre = if seiran { [pos[0], pos[1] - 160.0] } else { pos };
        let flash=world.spawn_clipped(lib,file,match character { Ran => 16, Seiran => 15, _ => 20 },field_to_canvas(field,pos),field);
        let ring=world.spawn_clipped(lib,file,match character { Ran => 13, Seiran => 12, _ => 17 },field_to_canvas(field,centre),field);
        let mut radius=Interp::default();
        let mut squash=Interp::default();
        if cross {
            radius.start([1.0],[meter as f32/2500.0*64.0+32.0],duration,4);
        } else if seiran {
            radius.start([1.0],[meter as f32*80.0/4000.0+80.0],duration,4);
            squash.start([1.0],[meter as f32*16.0/4000.0+16.0],duration,4);
        } else {
            radius.start([1.0],[meter as f32/2500.0*224.0+128.0],duration,4);
            squash.start([1.0],[meter as f32/2500.0*32.0+16.0],duration,4);
        }
        Self {kind: if cross {Kind::Cross} else {Kind::Ring},locked:true,column_pos:centre,time:0,duration,radius,squash,ring,flash,current_radius:1.0,meter_start:meter,
            flash_follows: seiran, centre, speed: 0.0, angle: 0.0, radius2: Interp::default(), current_radius2: 1.0, side_rings: [None; 2], vel_x: 0.0 }
    }

    /// The bombs that move: Aunn's ellipse (on her), Nazrin's seeking ring and Orin's thrown box.
    /// `vel_x` is the player's horizontal velocity (+0x20f4) for Orin's direction.
    #[allow(clippy::too_many_arguments)]
    pub fn new_moving(character: crate::character::Character, pos: [f32;2], vel_x: f32, meter: i32, file: usize,
        world: &mut World, lib: &AnmLibrary, field: [f32;4]) -> Self {
        use crate::character::Character::*;
        let meter=meter.clamp(0,MAX_METER);
        let m = meter as f32;
        let at=field_to_canvas(field,pos);
        let (kind, flash, ring, duration) = match character {
            Aunn => (Kind::Ellipse, 15, 12, (300*meter/MAX_METER).max(120)),
            Nazrin => (Kind::Seeker, 14, 11, (370*meter/MAX_METER).max(200)),
            _ => (Kind::Thrown, 17, 14, (320*meter/MAX_METER).max(220)),
        };
        let flash=world.spawn_clipped(lib,file,flash,at,field);
        let ring=world.spawn_clipped(lib,file,ring,at,field);
        let mut radius=Interp::default();
        let mut radius2=Interp::default();
        let mut squash=Interp::default();
        match kind {
            Kind::Ellipse => {
                radius.start([1.0],[m*100.0/4000.0+128.0],duration,1);
                radius2.start([1.0],[m*400.0/4000.0+128.0],duration,1);
            }
            Kind::Seeker => {
                radius.start([1.0],[m*80.0/4000.0+80.0],duration,4);
                squash.start([1.0],[m*16.0/4000.0+16.0],duration,4);
            }
            _ => radius.start([1.0],[m/2500.0*64.0+64.0],20,4),
        }
        let angle = if kind == Kind::Thrown { crate::scope::orin_angle(vel_x) } else { 0.0 };
        Self {kind,locked:false,column_pos:pos,time:0,duration,radius,squash,ring,flash,current_radius:1.0,meter_start:meter,
            flash_follows: true, centre: pos, speed: 1.0, angle, radius2, current_radius2: 1.0, side_rings: [None; 2], vel_x: 0.0 }
    }

    /// The last four characters' bombs.
    #[allow(clippy::too_many_arguments)]
    pub fn new_later(character: crate::character::Character, pos: [f32;2], vel_x: f32, meter: i32, file: usize,
        world: &mut World, lib: &AnmLibrary, field: [f32;4]) -> Self {
        use crate::character::Character::*;
        let meter = meter.clamp(0, MAX_METER);
        let m = meter as f32 / 2500.0;
        let at = field_to_canvas(field, pos);
        // (kind, flash script, ring script, FUN_004ed3d0(min, max) duration)
        let (kind, flash, ring, duration) = match character {
            Tsukasa => (Kind::Spinner, 13, 9, (370 * meter / MAX_METER).max(200)),
            Mamizou => (Kind::Triple, 14, 11, (400 * meter / MAX_METER).max(180)),
            Yachie => (Kind::Hexagon, 17, 14, (280 * meter / MAX_METER).max(140)),
            _ => (Kind::Leaning, 18, 15, (280 * meter / MAX_METER).max(160)),
        };
        let flash = world.spawn_clipped(lib, file, flash, at, field);
        let ring = world.spawn_clipped(lib, file, ring, at, field);
        let side_rings = if kind == Kind::Triple { std::array::from_fn(|_| Some(world.spawn_clipped(lib, file, 11, at, field))) } else { [None; 2] };
        let mut radius = Interp::default();
        let mut radius2 = Interp::default();
        match kind {
            Kind::Spinner => radius.start([1.0], [m * 40.0 + 64.0], 20, 4),
            Kind::Triple => radius.start([24.0], [m * 32.0 + 48.0], duration, 4),
            Kind::Hexagon => radius.start([24.0], [m * 16.0 + 160.0], duration, 4),
            _ => {
                // +0xc8: across the lean (A); +0xf8: along it (B).
                radius.start([24.0], [m * 48.0 + 144.0], duration, 4);
                radius2.start([24.0], [m * 16.0 + 48.0], duration, 4);
            }
        }
        let start = if matches!(kind, Kind::Triple | Kind::Hexagon) { 24.0 } else { 1.0 };
        let angle = match kind {
            Kind::Leaning => crate::scope::lean_target(vel_x),
            _ => -std::f32::consts::FRAC_PI_2,
        };
        // The flash follows the player except Saki's (FUN_00422c60 leaves it where she bombed).
        let leaning = kind == Kind::Leaning;
        Self { kind, locked: leaning, column_pos: pos, time: 0, duration, radius, squash: Interp::default(), ring, flash,
            current_radius: start, meter_start: meter, flash_follows: !leaning, centre: pos, speed: 0.0, angle, radius2,
            current_radius2: start, side_rings, vel_x }
    }

    /// One frame (`FUN_0053c510`). Returns false once the bomb has finished.
    pub fn tick(&mut self, pos: [f32; 2], world: &mut World, field: [f32; 4], cancel: &mut Vec<([f32; 2], f32)>, columns: &mut Vec<(f32, f32)>, damage: &mut Vec<DamageCircle>) -> bool {
        self.tick_character(pos, &[], world, field, &mut Areas { circles: cancel, columns, rects: &mut Vec::new(),
            ellipses: &mut Vec::new(), orects: &mut Vec::new(), polygons: &mut Vec::new() }, damage)
    }

    pub fn tick_character(&mut self, pos: [f32;2], enemies: &[Enemy], world: &mut World, field:[f32;4],
        areas: &mut Areas, damage:&mut Vec<DamageCircle>) -> bool {
        // Orin's box moves before anything else (FUN_004d4dc0 / FUN_004d4a00), even after the
        // duration.
        if self.kind == Kind::Thrown {
            let (s, c) = self.angle.sin_cos();
            self.centre = [self.centre[0] + c * self.speed, self.centre[1] + s * self.speed];
        }
        // Tsukasa's box turns 5 degrees a frame (motion flag 0x10, +0x54 = 0.0872665).
        if self.kind == Kind::Spinner {
            self.angle = crate::motion::wrap_angle(self.angle + 0.087266475);
        }
        self.time += 1;
        if self.time < self.duration {
            match self.kind {
                Kind::Seeker => {
                    let target = crate::scope::nearest_spirit(enemies, self.centre, 320.0).unwrap_or(pos);
                    self.centre = crate::scope::approach(self.centre, target, 3.0, 9.0);
                }
                Kind::Ellipse => self.centre = pos,
                Kind::Thrown => {
                    if self.time < 60 { self.speed += 0.05; } else if self.speed > 0.0 { self.speed -= 0.035; }
                }
                Kind::Spinner => self.centre[1] -= 1.0,
                Kind::Triple | Kind::Hexagon => self.centre[1] -= 0.1,
                Kind::Leaning => {
                    self.angle = crate::scope::lean_step(self.angle, self.vel_x);
                    let (s, c) = self.angle.sin_cos();
                    self.centre = [pos[0] + 64.0 * s, pos[1] - 64.0 * c];
                }
                _ => {}
            }
            if let Some([r]) = self.radius.step() { self.current_radius = r; }
            if let Some([r]) = self.radius2.step() { self.current_radius2 = r; }
            let squash = self.squash.step().map_or(1.0, |[s]| s);
            let player = pos;
            let pos = if self.locked { self.column_pos } else { pos };
            let centre = match self.kind { Kind::Ellipse | Kind::Seeker | Kind::Thrown | Kind::Spinner | Kind::Triple
                | Kind::Hexagon | Kind::Leaning => self.centre, _ => pos };
            let triple = crate::scope::mamizou_circles(centre, 0.0);
            // FUN_0041efa0: (0, -96), (-64, -64), (64, -64) from the centre, not turned.
            let triple = [triple[0], triple[2], triple[1]];
            let flash_at = field_to_canvas(field, if self.flash_follows { player } else { pos });
            if let Some(vm) = world.get_mut(self.flash) { vm.origin = [flash_at[0], flash_at[1], 0.0]; }
            for (k, id) in self.side_rings.iter().enumerate() {
                if let Some(vm) = id.and_then(|id| world.get_mut(id)) {
                    let c = field_to_canvas(field, triple[k + 1]);
                    vm.origin = [c[0], c[1], 0.0];
                    vm.scale = [self.current_radius; 2];
                }
            }
            if let Some(vm) = world.get_mut(self.ring) {
                let c = field_to_canvas(field, if self.kind == Kind::Triple { triple[0] } else { centre });
                vm.origin = [c[0], c[1], 0.0];
                vm.scale = match self.kind {
                    Kind::Column => [self.current_radius, 1.0],
                    Kind::Cross | Kind::Thrown | Kind::Spinner | Kind::Triple | Kind::Hexagon => [self.current_radius;2],
                    // FUN_00422c60: scale (A, 30), scale2 (B, 30), turned by the lean.
                    Kind::Leaning => [self.current_radius, 30.0],
                    // FUN_0053fac0: scale (A, 30), scale2 (B, 30), rotation -pi/2.
                    Kind::Ellipse => [self.current_radius, 30.0],
                    _ => [self.current_radius, squash],
                };
                if self.kind == Kind::Ellipse {
                    vm.scale2 = [self.current_radius2, 30.0];
                    vm.rot[2] = -std::f32::consts::FRAC_PI_2;
                }
                if self.kind == Kind::Leaning { vm.scale2 = [self.current_radius2, 30.0]; }
                if matches!(self.kind, Kind::Thrown | Kind::Spinner | Kind::Hexagon | Kind::Leaning) { vm.rot[2] = self.angle; }
            }
            match self.kind {
                Kind::Column => {
                    // The band is centred on the player's x, 2 * width wide and the whole field tall.
                    areas.columns.push((pos[0], self.current_radius));
                    damage.push(DamageCircle { pos: [pos[0], 224.0], radius: 0.0, damage: COLUMN_DAMAGE, remaining: 1, growth: 0.0,
                        rect: Some([self.current_radius * 2.0, 448.0]), ellipse: None, orect: None, polygon: None });
                }
                Kind::Cross => {
                    for size in [[self.current_radius*7.0,self.current_radius],[self.current_radius,self.current_radius*7.0]] {
                        areas.rects.push((pos,size));
                        damage.push(DamageCircle {pos,radius:0.0,damage:20,remaining:1,growth:0.0,rect:Some(size),ellipse:None,orect:None,polygon:None});
                    }
                }
                Kind::Ellipse => {
                    // a = the vertical radius, along -pi/2; b = the horizontal one.
                    let e = (self.current_radius2, self.current_radius, -std::f32::consts::FRAC_PI_2);
                    areas.ellipses.push((centre, e.0, e.1, e.2));
                    damage.push(DamageCircle { pos: centre, radius: 0.0, damage: DAMAGE, remaining: 1, growth: 0.0, rect: None, ellipse: Some(e), orect: None, polygon: None });
                }
                Kind::Thrown | Kind::Spinner => {
                    let size = if self.kind == Kind::Spinner { [self.current_radius * 4.0, self.current_radius] } else { [self.current_radius * 2.0, self.current_radius] };
                    areas.orects.push((centre, size, self.angle));
                    damage.push(DamageCircle { pos: centre, radius: 0.0, damage: COLUMN_DAMAGE, remaining: 1, growth: 0.0, rect: None, ellipse: None, orect: Some((size, self.angle)), polygon: None });
                }
                Kind::Triple => {
                    for c in triple {
                        areas.circles.push((c, self.current_radius));
                        damage.push(DamageCircle { pos: c, radius: self.current_radius, damage: COLUMN_DAMAGE, remaining: 1, growth: 0.0, rect: None, ellipse: None, orect: None, polygon: None });
                    }
                }
                Kind::Hexagon => {
                    areas.polygons.push((centre, self.current_radius, self.angle, 6));
                    damage.push(DamageCircle { pos: centre, radius: 0.0, damage: COLUMN_DAMAGE, remaining: 1, growth: 0.0, rect: None, ellipse: None, orect: None,
                        polygon: Some((self.current_radius, self.angle, 6)) });
                }
                Kind::Leaning => {
                    // a = B along the lean, b = A (as Aunn's +0xf8 / +0xc8 pair).
                    let e = (self.current_radius2, self.current_radius, self.angle);
                    areas.ellipses.push((centre, e.0, e.1, e.2));
                    damage.push(DamageCircle { pos: centre, radius: 0.0, damage: DAMAGE, remaining: 1, growth: 0.0, rect: None, ellipse: Some(e), orect: None, polygon: None });
                }
                _ => {
                    areas.circles.push((centre, self.current_radius));
                    damage.push(DamageCircle { pos: centre, radius: self.current_radius, damage: DAMAGE, remaining: 1, growth: 0.0, rect: None, ellipse: None, orect: None, polygon: None });
                }
            }
            return true;
        }
        if self.time == self.duration {
            world.interrupt(self.ring, 1);
            world.interrupt(self.flash, 1);
            // Mamizou's update interrupts only the first ring (FUN_0041efa0); APPROX: the other
            // two are deleted with it below, where they are released was not traced.
        }
        if self.time >= self.duration + 10 {
            world.delete(self.ring);
            world.delete(self.flash);
            for id in self.side_rings.iter().flatten() { world.delete(*id); }
            return false;
        }
        true
    }

    /// The meter this frame (FUN_00403250, mode 0 at BombInf +0x88): linear from the starting
    /// meter to 0 over the duration, truncated.
    pub fn meter(&self) -> i32 {
        drained_meter(self.meter_start, self.time, self.duration)
    }

    /// Frames elapsed and the frame count the circle lasts.
    pub fn progress(&self) -> (i32, i32) { (self.time, self.duration) }

    /// The frame the duration ran out (the update functions' `time == duration` block: VMs
    /// interrupted, screen shake, sound 0x31 stopped and 0x2c played).
    pub fn ended_now(&self) -> bool { self.time == self.duration }

    /// Whether the circle is still growing (the player is protected meanwhile).
    pub fn protecting(&self) -> bool { self.time < self.duration }
}

/// FUN_00403250 mode 0 from `start` to 0: `(int)((0 - start) * t / duration + start)`, 0 once
/// the duration has passed.
pub fn drained_meter(start: i32, time: i32, duration: i32) -> i32 {
    if time >= duration || duration <= 0 {
        return 0;
    }
    let f = time as f32 / duration as f32;
    ((0 - start) as f32 * f + start as f32) as i32
}

#[cfg(test)]
mod tests {
    #[test]
    fn meter_drains_linearly_over_the_bomb() {
        use super::drained_meter;
        assert_eq!(drained_meter(2500, 1, 250), 2490);
        assert_eq!(drained_meter(2500, 125, 250), 1250);
        assert_eq!(drained_meter(2500, 249, 250), 10);
        assert_eq!(drained_meter(2500, 250, 250), 0);
        assert_eq!(drained_meter(1000, 60, 120), 500);
    }
}
