//! Enemy bullets: the "ET" shooters scripts configure (ECL 600–620) and the bullets they fire.
//!
//! Reimplements th19.exe's bullet manager:
//! * `FUN_004f1340` fires `count2` layers × `count1` ways; `FUN_004f3910` places each bullet
//!   according to the aim mode (0–12) and sets it up;
//! * `FUN_004f1460` starts the shooter's "ex" behaviours in order (an ex waits for the
//!   previous ones to finish unless it is async);
//! * `FUN_004f5680` updates a bullet: spawn state, running ex handlers (`FUN_004f0eb0` accel,
//!   `FUN_004f08a0` step, `FUN_004efb70` bounce, ...), movement and off-field deletion.
//! Bullet sprites come from `bullet.anm` via the type table in th19.exe (`exe::bullet_types`).

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use th_anm::{AnmLibrary, VmId, World};
use th_formats::exe::BulletType;

use crate::enemy::field_to_canvas;
use crate::motion::wrap_angle;

pub const KEEP: f32 = -999999.0;

/// One "ex" behaviour slot of a shooter (`ins_609`..`ins_612`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ex {
    pub kind: u32,
    /// Starts without waiting for running behaviours to finish.
    pub asynchronous: bool,
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub d: i32,
    pub r: f32,
    pub s: f32,
    pub m: f32,
    pub n: f32,
}

pub mod ex {
    pub const SPEEDUP: u32 = 1;
    pub const SPAWN_ANIM: u32 = 2;
    pub const ACCEL: u32 = 4;
    pub const ANGLE_ACCEL: u32 = 8;
    pub const STEP: u32 = 0x10;
    pub const BOUNCE: u32 = 0x40;
    pub const INVULN: u32 = 0x80;
    pub const OFFSCREEN: u32 = 0x100;
    pub const SET_SPRITE: u32 = 0x200;
    pub const DELETE: u32 = 0x400;
    pub const SOUND: u32 = 0x800;
    pub const ACCEL_TO: u32 = 0x200000;
    /// Commands (bit 30 set): 1 = wait `a` frames.
    pub const COMMAND: u32 = 0x4000_0000;
}

/// A bullet shooter (`ET`), 16 per enemy.
#[derive(Clone, Debug)]
pub struct Et {
    pub sprite: i32,
    pub color: i32,
    /// Offset from the enemy (or an absolute position, see `absolute`).
    pub offset: [f32; 2],
    pub absolute: bool,
    pub angle: [f32; 2],
    pub speed: [f32; 2],
    /// Ways, layers.
    pub count: [i32; 2],
    pub aim: i32,
    /// Bullets start this far out along their direction (`ins_619`).
    pub distance: f32,
    pub ex: [Ex; 24],
    /// Next index for `ins_611`/`ins_612`, which append (reset by `ins_600`).
    pub ex_next: usize,
}

impl Default for Et {
    /// `ins_600`'s reset: speed 2, one bullet.
    fn default() -> Self {
        Self {
            sprite: 0,
            color: 0,
            offset: [0.0; 2],
            absolute: false,
            angle: [0.0; 2],
            speed: [2.0, 0.0],
            count: [1, 1],
            aim: 0,
            distance: 0.0,
            ex: [Ex::default(); 24],
            ex_next: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Timed {
    t: f32,
    end: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct ExState {
    speedup: Timed,
    accel: Timed,
    accel_r: f32,
    accel_vec: [f32; 2],
    accel_to: Timed,
    accel_to_r: f32,
    accel_to_vec: [f32; 2],
    angacc: Timed,
    angacc_dspeed: f32,
    angacc_dangle: f32,
    step: Timed,
    step_speed: f32,
    step_angle: f32,
    step_count: i32,
    step_max: i32,
    step_mode: i32,
    bounce_count: i32,
    bounce_max: i32,
    bounce_flags: i32,
    bounce_box: [f32; 2],
    bounce_speed: f32,
    offscreen: f32,
    wait: f32,
}

pub struct Bullet {
    pub pos: [f32; 2],
    pub vel: [f32; 2],
    pub speed: f32,
    pub angle: f32,
    pub kind: i32,
    pub color: i32,
    /// 1 live, 2 playing its spawn animation (not dangerous, half speed).
    pub state: u8,
    pub hitbox: f32,
    /// Frames during which the off-field test is skipped.
    grace: i32,
    /// Frames during which the bullet can't be cancelled (`ex` 0x80).
    pub invuln: i32,
    ex: [Ex; 24],
    ex_idx: usize,
    active: u32,
    waiting: bool,
    st: ExState,
    pub vm: Option<VmId>,
    pub delete: bool,
    /// Removed by a cancel (not by its own script or by leaving the field): plays the
    /// type's cancel effect when it goes (`FUN_004eeb30`).
    pub cancelled: bool,
    pub scope_time_scale: f32,
    timer_fraction: f32,
}

/// What a bullet needs from the world around it.
pub struct Ctx<'a> {
    pub world: &'a mut World,
    pub lib: &'a AnmLibrary,
    pub types: &'a [BulletType],
    pub anm: usize,
    pub field: [f32; 4],
    pub player: [f32; 2],
    pub rand: &'a mut dyn FnMut() -> f32,
}

impl Ctx<'_> {
    fn angle_to_player(&self, from: [f32; 2]) -> f32 {
        let (dx, dy) = (self.player[0] - from[0], self.player[1] - from[1]);
        if dx == 0.0 && dy == 0.0 {
            FRAC_PI_2
        } else {
            dy.atan2(dx)
        }
    }
}

fn polar(angle: f32, len: f32) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    [c * len, s * len]
}

/// Fire a shooter from `origin` (`FUN_004f1340` / `FUN_004f3910`).
pub fn fire(et: &Et, origin: [f32; 2], ctx: &mut Ctx, out: &mut Vec<Bullet>) {
    let aim = ctx.angle_to_player(origin);
    let (ways, layers) = (et.count[0].max(0), et.count[1].max(0));
    for j in 0..layers {
        for i in 0..ways {
            if let Some(b) = make_bullet(et, origin, i, j, aim, ctx) {
                out.push(b);
            }
        }
    }
}

fn make_bullet(et: &Et, origin: [f32; 2], i: i32, j: i32, aim: f32, ctx: &mut Ctx) -> Option<Bullet> {
    let ways = et.count[0].max(1) as f32;
    let layers = et.count[1];
    let [a1, a2] = et.angle;
    let [s1, s2] = et.speed;
    // Speed steps from s1 to s2 across the layers.
    let mut speed = if layers > 1 { s1 - (s1 - s2) * j as f32 / (layers - 1) as f32 } else { s1 };
    let ring = i as f32 * TAU / ways;
    let angle = match et.aim {
        // Fans (0 aimed, 1 absolute): ways alternate sides, a2 apart.
        0 | 1 => {
            let mut a = if et.count[0] & 1 == 0 {
                (i / 2) as f32 * a2 + a2 * 0.5
            } else {
                ((i + 1) / 2) as f32 * a2
            };
            if i & 1 != 0 {
                a = -a;
            }
            if et.aim == 0 {
                a += aim;
            }
            a + a1
        }
        // Rings (2 aimed, 3 absolute), layers turned by a2.
        2 | 3 => j as f32 * a2 + a1 + ring + if et.aim == 2 { aim } else { 0.0 },
        // Rings offset by half a step.
        4 | 5 => j as f32 * a2 + a1 + ring + PI / ways + if et.aim == 4 { aim } else { 0.0 },
        // Random angle (6), random speed (7), both (8).
        // APPROX: the range of the game's random helpers isn't visible in the decompile;
        // angle is a1 + a2 * [-1, 1], speed s1 + s2 * [0, 1].
        6 => a1 + a2 * ((ctx.rand)() * 2.0 - 1.0),
        7 => {
            speed = s1 + s2 * (ctx.rand)();
            j as f32 * a2 + a1 + ring
        }
        8 => {
            speed = s1 + s2 * (ctx.rand)();
            a1 + a2 * ((ctx.rand)() * 2.0 - 1.0)
        }
        // "Meek" fans: speed also steps across pairs of layers.
        9 | 10 => {
            let mut a;
            if layers & 1 == 0 {
                a = (j / 2) as f32 * a2 + a2 * 0.5 + ring;
                if layers > 1 {
                    speed = ((j & !1) as f32 * (s2 - s1)) / (layers - 1) as f32 + s1;
                }
            } else {
                a = ((j + 1) / 2) as f32 * a2 + ring;
                if layers > 1 {
                    speed = (((j + 1) & !1) as f32 * (s2 - s1)) / (layers - 1) as f32 + s1;
                }
            }
            if j & 1 != 0 {
                a = -a;
            }
            if et.aim == 9 {
                a += aim;
            }
            a + a1
        }
        // Ellipse: speeds s1 (x) and s2 (y) per ring direction.
        11 => {
            let (sx, sy) = (ring.cos() * s1, ring.sin() * s2);
            speed = (sx * sx + sy * sy).sqrt();
            sy.atan2(sx) + a1
        }
        // Star-ish ring: speed shrinks with |sin| of the half-step-offset direction.
        12 => {
            let d = ring + PI / ways;
            speed *= 1.0 - d.sin().abs() * s2;
            d + a1
        }
        _ => a1,
    };
    let angle = wrap_angle(wrap_angle(angle));
    let mut pos = origin;
    if et.distance != 0.0 {
        let o = polar(angle, et.distance);
        pos = [pos[0] + o[0], pos[1] + o[1]];
    }
    let ty = ctx.types.get(et.sprite.max(0) as usize)?;
    let mut b = Bullet {
        pos,
        vel: polar(angle, speed),
        speed,
        angle,
        kind: et.sprite,
        color: et.color,
        state: 1,
        hitbox: ty.hitbox,
        grace: 5,
        invuln: 0,
        ex: et.ex,
        ex_idx: 0,
        active: 0,
        waiting: false,
        st: ExState::default(),
        vm: None,
        delete: false,
        cancelled: false,
        scope_time_scale: 1.0,
        timer_fraction: 0.0,
    };
    b.set_sprite(et.sprite, et.color, ctx);
    // A leading spawn-animation ex is applied at creation; otherwise the bullet appears at once.
    if b.ex[0].kind == ex::SPAWN_ANIM {
        let style = b.ex[0].a;
        b.start_spawn_anim(style, ctx);
        b.ex_idx = 1;
    } else if let Some(id) = b.vm {
        ctx.world.interrupt(id, 2);
    }
    b.run_ex(ctx);
    Some(b)
}

/// Draw-chain priority of the enemy bullet manager's draw callback (FUN_004f6470).
pub const BULLET_DRAW_PRIORITY: i32 = 0x29;

impl Bullet {
    fn set_sprite(&mut self, kind: i32, color: i32, ctx: &mut Ctx) {
        if let Some(id) = self.vm.take() {
            ctx.world.delete(id);
        }
        let Some(ty) = ctx.types.get(kind.max(0) as usize) else { return };
        self.kind = kind;
        self.color = color;
        self.hitbox = ty.hitbox;
        let remap = ty.colours[(color.clamp(0, 15)) as usize];
        let at = field_to_canvas(ctx.field, self.pos);
        let id = ctx.world.spawn_remapped(ctx.lib, ctx.anm, ty.script as usize, at, ctx.field, remap);
        // The bullet manager draws its bullets itself, at draw-chain priority 0x29 (between
        // ANM layers 17 and 18; FUN_004f6470), whatever layer their script sets.
        ctx.world.set_draw_priority(id, BULLET_DRAW_PRIORITY);
        self.vm = Some(id);
    }

    /// Spawn animation: play the bullet's appear interrupt and back the bullet up four steps;
    /// it moves at half speed until its script flags the animation done (I0).
    fn start_spawn_anim(&mut self, style: i32, ctx: &mut Ctx) {
        if let Some(id) = self.vm {
            if style != 1 {
                ctx.world.interrupt(id, style + 7);
            }
        }
        self.state = 2;
        self.pos = [self.pos[0] - self.vel[0] * 4.0, self.pos[1] - self.vel[1] * 4.0];
    }

    /// Resolve an ex angle argument: KEEP = current angle, >= 990 = aimed (+ value - 999).
    fn resolve_angle(&self, a: f32, ctx: &Ctx) -> f32 {
        if a <= -999990.0 {
            self.angle
        } else if a >= 990.0 {
            wrap_angle(ctx.angle_to_player(self.pos) + a - 999.0)
        } else {
            a
        }
    }

    fn set_vel(&mut self) {
        self.vel = polar(self.angle, self.speed);
    }

    /// Start pending ex behaviours (`FUN_004f1460`).
    fn run_ex(&mut self, ctx: &mut Ctx) {
        while self.ex_idx < 24 {
            let e = self.ex[self.ex_idx];
            if e.kind == 0 {
                return;
            }
            // A synchronous ex waits for the running ones (offscreen-allow doesn't count).
            if !e.asynchronous && (self.active & !ex::OFFSCREEN != 0 || self.waiting) {
                return;
            }
            if e.kind & ex::COMMAND == 0 && self.active & e.kind != 0 {
                return;
            }
            self.ex_idx += 1;
            match e.kind {
                k if k == ex::COMMAND | 1 => {
                    if e.a > 0 {
                        self.waiting = true;
                        self.st.wait = e.a as f32;
                    }
                }
                ex::SPEEDUP => {
                    self.active |= ex::SPEEDUP;
                    self.st.speedup = Timed { t: 0.0, end: 17.0 };
                }
                ex::SPAWN_ANIM => self.start_spawn_anim(e.a, ctx),
                ex::ACCEL => {
                    self.active |= ex::ACCEL;
                    let ang = self.resolve_angle(e.s, ctx);
                    self.st.accel = Timed { t: 0.0, end: e.a as f32 };
                    self.st.accel_r = e.r;
                    self.st.accel_vec = polar(ang, e.r);
                }
                ex::ACCEL_TO => {
                    self.active |= ex::ACCEL_TO;
                    let ang = self.resolve_angle(e.s, ctx);
                    let r = if e.a > 0 { (e.r - self.speed) / e.a as f32 } else { 0.0 };
                    self.st.accel_to = Timed { t: 0.0, end: e.a as f32 };
                    self.st.accel_to_r = r;
                    self.st.accel_to_vec = polar(ang, r);
                }
                ex::ANGLE_ACCEL => {
                    self.active |= ex::ANGLE_ACCEL;
                    self.st.angacc = Timed { t: 0.0, end: e.a as f32 };
                    self.st.angacc_dspeed = e.r;
                    self.st.angacc_dangle = e.s;
                }
                ex::STEP => {
                    self.active |= ex::STEP;
                    self.st.step_speed = if e.s <= -999990.0 { self.speed } else { e.s };
                    let ang = self.resolve_angle(e.r, ctx);
                    self.st.step_angle = match e.c {
                        2 => wrap_angle(ctx.angle_to_player(self.pos) + ang),
                        5 | 6 => ((ctx.rand)() * 2.0 - 1.0) * e.r,
                        7 => {
                            let a = if e.r > -999990.0 {
                                if e.r < 990.0 { e.r } else { ctx.angle_to_player(self.pos) }
                            } else {
                                self.angle
                            };
                            self.st.step_speed = ((ctx.rand)() * 2.0 - 1.0) * e.s + self.speed;
                            a
                        }
                        _ => ang,
                    };
                    self.st.step = Timed { t: 0.0, end: e.a as f32 };
                    self.st.step_count = 0;
                    self.st.step_max = e.b;
                    self.st.step_mode = e.c;
                }
                ex::BOUNCE => {
                    self.active |= ex::BOUNCE;
                    self.st.bounce_speed = e.r;
                    self.st.bounce_box = if e.b & 0x20 != 0 { [e.s, e.m] } else { [296.0, 448.0] };
                    self.st.bounce_count = 0;
                    self.st.bounce_max = e.a;
                    self.st.bounce_flags = e.b;
                }
                ex::INVULN => self.invuln = e.a,
                ex::OFFSCREEN => {
                    self.active |= ex::OFFSCREEN;
                    self.st.offscreen = e.a as f32;
                }
                ex::SET_SPRITE => {
                    if e.a & 0x8000 == 0 {
                        self.set_sprite(e.a, e.b & 0x7fff, ctx);
                        if let Some(id) = self.vm {
                            ctx.world.interrupt(id, 2);
                        }
                    }
                }
                ex::DELETE => self.delete = true,
                // Not implemented yet: other kinds are skipped.
                _ => {}
            }
        }
    }

    /// One frame (`FUN_004f5680`).
    pub fn update(&mut self, ctx: &mut Ctx) { self.update_scaled(ctx, 1.0); }

    /// Scope time factor (004f5219, constant005a7320; movement004f5680).
    pub fn update_scaled(&mut self, ctx: &mut Ctx, dt: f32) {
        if self.state == 2 {
            self.pos = [self.pos[0] + self.vel[0] * dt / 2.0, self.pos[1] + self.vel[1] * dt / 2.0];
            let done = self.vm.and_then(|id| ctx.world.get(id)).is_none_or(|vm| vm.ivars[0] != 0);
            if done { self.state = 1; }
        }
        // FUN004f5680 case2 -> LAB004f57e9: once the appear flag is set, run the
        // live handlers and full movement in this same tick, after the half movement.
        if self.state == 1 {
            loop {
                self.run_ex(ctx);
                if self.handlers(ctx, dt) == 0 { break; }
            }
            self.pos = [self.pos[0] + self.vel[0] * dt, self.pos[1] + self.vel[1] * dt];
        }

        self.timer_fraction += dt;
        let timer_tick = self.timer_fraction >= 1.0;
        if timer_tick { self.timer_fraction -= 1.0; }
        if self.invuln > 0 && timer_tick {
            self.invuln -= 1;
        }
        if self.active & ex::OFFSCREEN == 0 && self.grace < 1 {
            let size = self.sprite_size(ctx);
            let (hw, hh) = (size[0] / 2.0, size[1] / 2.0);
            if self.pos[0] + hw <= -148.0 || self.pos[0] - hw >= 148.0 || self.pos[1] + hh <= -64.0 || self.pos[1] - hh >= 448.0 {
                self.delete = true;
            }
        }
        if self.grace > 0 && timer_tick {
            self.grace -= 1;
        }
        let at = field_to_canvas(ctx.field, self.pos);
        if let Some(vm) = self.vm.and_then(|id| ctx.world.get_mut(id)) {
            vm.origin = [at[0], at[1], 0.0];
            if vm.auto_rotate {
                vm.rot[2] = wrap_angle(self.angle + FRAC_PI_2);
            }
        }
    }

    /// Mark the bullet cancelled: it is removed this tick and shows its cancel effect.
    pub fn cancel(&mut self) {
        self.delete = true;
        self.cancelled = true;
    }

    /// bullet.anm script of the cancel effect (`FUN_004f11a0` +0x690 by the type's
    /// `cancel_mode`); `None` when the type has none.
    pub fn cancel_script(&self, ty: &th_formats::exe::BulletType) -> Option<i32> {
        // Table 0x598c20 (the first eight entries are scripts).
        const BY_COLOUR: [i32; 8] = [6, 10, 14, 18, 22, 26, 30, 36];
        let c = self.color.clamp(0, 15) as usize;
        match ty.cancel_mode {
            0 => Some(self.color * 2 + 6),
            1 => BY_COLOUR.get(c).copied(),
            3 => Some(0x12),
            4 => Some(8),
            5 => Some(0xe),
            6 => Some(ty.colours[c][3]),
            _ => None,
        }
    }

    /// FUN_004ee880 (ECL 615/616): live or appearing bullets inside the circle, invulnerable
    /// ones included. FUN_004ee770 (below) is the player/skill version.
    pub fn within_cancel(&self, pos: [f32;2], radius: f32, world: &World, lib: &AnmLibrary) -> bool {
        if !matches!(self.state, 1 | 2) { return false; }
        let half_width = self.vm.and_then(|id| world.get(id)).and_then(|vm|
            lib.files.get(vm.file).and_then(|f| f.sprites.get(vm.sprite.max(0) as usize))
                .map(|s| s.size[0] * vm.scale[0].abs() * 0.5)).unwrap_or(0.0);
        (self.pos[0]-pos[0]).powi(2)+(self.pos[1]-pos[1]).powi(2) <= (radius+half_width).powi(2)
    }

    /// FUN_004ee770: cancelable live/appearing bullets intersect a circle using half sprite width.
    pub fn intersects_cancel(&self, pos: [f32;2], radius: f32, world: &World, lib: &AnmLibrary) -> bool {
        if self.invuln != 0 || !matches!(self.state, 1 | 2) { return false; }
        let half_width = self.vm.and_then(|id| world.get(id)).and_then(|vm|
            lib.files.get(vm.file).and_then(|f| f.sprites.get(vm.sprite.max(0) as usize))
                .map(|s| s.size[0] * vm.scale[0].abs() * 0.5)).unwrap_or(0.0);
        (self.pos[0]-pos[0]).powi(2)+(self.pos[1]-pos[1]).powi(2) <= (radius+half_width).powi(2)
    }

    fn sprite_size(&self, ctx: &Ctx) -> [f32; 2] {
        let Some(vm) = self.vm.and_then(|id| ctx.world.get(id)) else { return [0.0; 2] };
        ctx.lib
            .files
            .get(vm.file)
            .and_then(|f| f.sprites.get(vm.sprite.max(0) as usize))
            .map_or([0.0; 2], |s| [s.size[0] * vm.scale[0].abs(), s.size[1] * vm.scale[1].abs()])
    }

    /// Run the active ex handlers; returns how many finished (they free later ex to start).
    fn handlers(&mut self, ctx: &mut Ctx, dt: f32) -> u32 {
        let mut finished = 0;
        if self.active & ex::SPEEDUP != 0 {
            let s = &mut self.st.speedup;
            if s.t > 16.0 {
                self.active &= !ex::SPEEDUP;
                finished += 1;
            } else {
                // A burst of +5 that fades out over 16 frames.
                let v = 5.0 - s.t as f32 * 5.0 / 16.0 + self.speed;
                self.vel = polar(self.angle, v);
                s.t += dt;
            }
        }
        if self.active & ex::ACCEL != 0 {
            finished += self.accel_step(false, dt) as u32;
        }
        if self.active & ex::ACCEL_TO != 0 {
            finished += self.accel_step(true, dt) as u32;
        }
        if self.active & ex::ANGLE_ACCEL != 0 {
            let s = &mut self.st.angacc;
            if s.t < s.end {
                self.angle = wrap_angle(self.angle + self.st.angacc_dangle * dt);
                self.speed += self.st.angacc_dspeed * dt;
                self.set_vel();
                self.st.angacc.t += dt;
            } else {
                self.active &= !ex::ANGLE_ACCEL;
                finished += 1;
            }
        }
        if self.active & ex::STEP != 0 {
            finished += self.step(ctx, dt) as u32;
        }
        if self.active & ex::BOUNCE != 0 {
            finished += self.bounce() as u32;
        }
        if self.active & ex::OFFSCREEN != 0 {
            self.st.offscreen -= dt;
            if self.st.offscreen <= 0.0 {
                self.active &= !ex::OFFSCREEN;
                finished += 1;
            }
        }
        if self.waiting {
            if self.st.wait < 1.0 {
                self.waiting = false;
                finished += 1;
            } else {
                self.st.wait -= dt;
            }
        }
        finished
    }

    /// Acceleration by a vector for `end` frames; the angle follows the velocity.
    fn accel_step(&mut self, to: bool, dt: f32) -> bool {
        let (timed, r, v) = if to {
            (&mut self.st.accel_to, self.st.accel_to_r, self.st.accel_to_vec)
        } else {
            (&mut self.st.accel, self.st.accel_r, self.st.accel_vec)
        };
        if timed.end <= timed.t {
            self.active &= !(if to { ex::ACCEL_TO } else { ex::ACCEL });
            return true;
        }
        timed.t += dt;
        self.speed += r * dt;
        self.vel = [self.vel[0] + v[0] * dt, self.vel[1] + v[1] * dt];
        if self.vel[0].abs() > 0.0001 || self.vel[1].abs() > 0.0001 {
            self.angle = wrap_angle(self.vel[1].atan2(self.vel[0]));
            self.speed = (self.vel[0] * self.vel[0] + self.vel[1] * self.vel[1]).sqrt();
        }
        false
    }

    /// Step: slow to a stop over `a` frames, then redirect (mode c) at the new speed; `b` times.
    fn step(&mut self, ctx: &mut Ctx, dt: f32) -> bool {
        let st = &mut self.st;
        if st.step.t < st.step.end {
            let v = self.speed - st.step.t as f32 * self.speed / st.step.end as f32;
            self.vel = polar(self.angle, v);
            st.step.t += dt;
            return false;
        }
        st.step_count += 1;
        // Redirect (jump table at 0x4f0ad8): 0/5 add, 1/6 aim + angle, 2/3/4 set.
        match st.step_mode {
            0 | 5 => self.angle = wrap_angle(self.angle + st.step_angle),
            1 | 6 => self.angle = wrap_angle(ctx.angle_to_player(self.pos) + st.step_angle),
            2..=4 => self.angle = wrap_angle(st.step_angle),
            _ => {}
        }
        self.speed = st.step_speed;
        st.step.t = 0.0;
        if st.step_count >= st.step_max {
            self.set_vel();
            self.active &= !ex::STEP;
            return true;
        }
        self.vel = polar(self.angle, self.speed);
        false
    }

    /// Bounce off the edges of a box centred at (0, 224).
    fn bounce(&mut self) -> bool {
        let st = &mut self.st;
        let [w, h] = st.bounce_box;
        let f = st.bounce_flags;
        let reflect = f & 0x10 == 0;
        let mut hit = false;
        if f & 1 != 0 && self.pos[1] < 224.0 - h / 2.0 {
            if reflect {
                self.angle = wrap_angle(-self.angle);
                self.pos[1] = (448.0 - h) - self.pos[1];
            }
            hit = true;
        }
        if f & 2 != 0 && self.pos[1] >= h / 2.0 + 224.0 {
            if reflect {
                self.angle = wrap_angle(-self.angle);
                self.pos[1] = (h + 448.0) - self.pos[1];
            }
            hit = true;
        }
        if f & 8 != 0 && self.pos[0] >= w / 2.0 {
            if reflect {
                self.angle = wrap_angle(-self.angle - PI);
                self.pos[0] = w - self.pos[0];
            }
            hit = true;
        }
        if f & 4 != 0 && self.pos[0] < -w / 2.0 {
            if reflect {
                self.angle = wrap_angle(-self.angle - PI);
                self.pos[0] = -w - self.pos[0];
            }
            hit = true;
        }
        if st.bounce_speed > -990.0 {
            self.speed = st.bounce_speed;
        }
        self.set_vel();
        if hit {
            self.st.bounce_count += 1;
        }
        if self.st.bounce_count < self.st.bounce_max {
            return false;
        }
        self.active &= !ex::BOUNCE;
        true
    }
}
