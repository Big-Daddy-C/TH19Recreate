//! Enemy lasers (`LaserInf` family), made by ECL `ins_702` / `ins_703` through `FUN_00530df0`.
//!
//! * The infinite laser (`LaserInfiniteInf`, vtable 0x5a873c, `ins_703`): a straight beam from a
//!   point along an angle that warns as a thin line, widens, holds and shrinks (`FUN_0052b550`),
//!   hurts the player while widening or held (`FUN_0052b0e0` / `FUN_00549570` /
//!   `FUN_0052b250`) and is grazed.
//! * The line laser (`LaserLineInf`, vtable 0x5a885c, `ins_702`): a segment that grows from its
//!   tail by its speed per frame up to its length, then travels along its angle at that speed
//!   until both ends are off the field (`FUN_0052e0c0`), at full width from the start.
//!
//! Circle/rectangle cancellation cuts 16-pixel sections and preserves the surviving pieces.
//! Curved lasers execute analytical path commands (00525d50/005274f0). Beam lasers remain unported.
use th_anm::{AnmLibrary, VmId, World};
use th_formats::exe::BulletType;

use crate::enemy::field_to_canvas;

/// `ins_700` / `ins_701` / `ins_608` values stored in an ET slot (`+8..+0x10`, `+0x448`,
/// `+0x458..+0x468`, `+0x478`).
/// Draw-chain priority of the laser manager.
pub const DRAW_PRIORITY: i32 = 0x27;

#[derive(Clone, Copy, Debug, Default)]
pub struct LaserEt {
    /// Length now and the length it grows to (`ins_700` floats 1 and 2).
    pub len: [f32; 2],
    /// Full width (`ins_700` float 4).
    pub width: f32,
    /// `ins_700` float 3 (a line laser's travel limit).
    pub f3: f32,
    /// Frames of warning, widening, holding and shrinking (`ins_701`).
    pub times: [i32; 4],
    pub flags: i32,
    /// Sound ids (`ins_608`).
    pub sound: [i32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Infinite,
    Line,
    Curve,
}

#[derive(Clone, Copy, Debug)]
pub enum CancelArea {
    All,
    Circle { pos: [f32; 2], radius: f32 },
    Column { x: f32, half_width: f32 },
    Rect { pos: [f32;2], size: [f32;2] },
    /// Orin's bomb box (`FUN_005303f0`): `size[0]` along `angle`.
    ORect { pos: [f32;2], size: [f32;2], angle: f32 },
    /// Aunn's bomb ellipse (`FUN_00530180`): `a` along `angle`, `b` across.
    Ellipse { pos: [f32;2], a: f32, b: f32, angle: f32 },
    /// Yachie's hexagons (`FUN_00530320`, vtable +0x34): line lasers test each step box with
    /// `FUN_004d22d0`, curved ones each node with `FUN_004d2a10`.
    Polygon { pos: [f32;2], radius: f32, angle: f32, sides: u32 },
}

pub struct CancelContext<'a> {
    pub world: &'a mut World,
    pub lib: &'a AnmLibrary,
    pub types: &'a [BulletType],
    pub anm: usize,
    pub field: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Thin warning line (state 3).
    Warn,
    /// Widening (state 4).
    Widen,
    /// Full width (state 2).
    Hold,
    /// Shrinking (state 5).
    Shrink,
}

pub struct Laser {
    pub kind: Kind,
    /// Pixel size of the laser's sprite: the beam's width is scaled from tile[0], its length from tile[1].
    tile: [f32; 2],
    /// A line laser's total travel limit (`ins_700` float 3; 0 = none) and distance travelled.
    limit: f32,
    travelled: f32,
    pub pos: [f32; 2],
    pub angle: f32,
    pub len: f32,
    cap: f32,
    grow: f32,
    pub width: f32,
    max_width: f32,
    times: [i32; 4],
    state: State,
    timer: i32,
    /// Frames since creation (`+0x3c`).
    age: i32,
    /// Consecutive frames the player has been inside the graze ring (`+0x65c`).
    graze: i32,
    /// Native +0x85c: player cancellation respects this; script cancellation can bypass it.
    pub invuln: i32,
    /// Line lasers get 30 frames before the off-field deletion test (0052fca0).
    offscreen_grace: i32,
    sprite_type: i32,
    color: i32,
    vm: Option<VmId>,
    curve: Option<Curve>,
}

/// What a laser did to the player this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Contact {
    None,
    Graze,
    Hit,
}

impl Laser {
    /// `FUN_0052b8e0`: the laser as `ins_703` made it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: Kind,
        origin: [f32; 2],
        angle: f32,
        speed: f32,
        sprite_type: i32,
        color: i32,
        et: &LaserEt,
        types: &[BulletType],
        anm: usize,
        world: &mut World,
        lib: &AnmLibrary,
        field: [f32; 4],
    ) -> Self {
        let vm = types.get(sprite_type.max(0) as usize).map(|ty| {
            let remap = ty.colours[color.clamp(0, 15) as usize];
            let id = world.spawn_remapped(lib, anm, ty.script as usize, field_to_canvas(field, origin), field, remap);
            // The laser manager draws at chain priority 0x27 (`FUN_00530a30`).
            world.set_draw_priority(id, DRAW_PRIORITY);
            // The native code turns the sprite with the beam; script 161 itself draws it unrotated.
            if let Some(vm) = world.get_mut(id) {
                vm.render_mode = 1;
            }
            id
        });
        let line = kind == Kind::Line;
        let tile = vm
            .and_then(|id| world.get(id))
            .and_then(|v| lib.files.get(v.file).and_then(|f| f.sprites.get(v.sprite.max(0) as usize)))
            .map_or([16.0, 14.0], |sp| [sp.size[0].max(1.0), sp.size[1].max(1.0)]);
        Self {
            kind,
            tile,
            limit: et.f3,
            travelled: if line && et.len[1] < et.len[0] { 0.01 } else { 0.0 },
            pos: origin,
            angle: crate::motion::wrap_angle(angle),
            len: et.len[0],
            cap: et.len[1],
            grow: speed,
            // Infinite lasers warn as 2 pixel lines; line lasers start at full width.
            width: if line { et.width } else { 2.0 },
            max_width: et.width,
            times: et.times,
            state: if line { State::Hold } else { State::Warn },
            timer: 0,
            age: 0,
            graze: 0,
            invuln: 0,
            offscreen_grace: if line { 30 } else { 0 },
            sprite_type,
            color,
            vm,
            curve: None,
        }
    }

    /// One frame. Returns false when the laser is over.
    pub fn update(&mut self, world: &mut World, field: [f32; 4]) -> bool {
        if self.invuln > 0 { self.invuln -= 1; }
        if self.kind == Kind::Curve { return self.update_curve(world, field); }
        if self.kind == Kind::Line {
            return self.update_line(world, field);
        }
        self.age += 1;
        self.timer += 1;
        if self.len <= self.cap && self.cap != self.len {
            self.len += self.grow;
            if self.cap <= self.len {
                self.len = self.cap;
            }
        }
        let [warn, widen, hold, shrink] = self.times;
        let mut done = false;
        match self.state {
            State::Warn => {
                if warn <= self.timer {
                    self.timer = 0;
                    self.state = State::Widen;
                }
            }
            State::Widen => {
                if self.timer < widen {
                    self.width = self.timer as f32 * self.max_width / widen as f32;
                } else {
                    self.timer = 0;
                    self.state = State::Hold;
                    self.width = self.max_width;
                }
            }
            State::Hold | State::Shrink => {}
        }
        if self.state == State::Hold && hold <= self.timer {
            self.timer = 0;
            self.state = State::Shrink;
        } else if self.state == State::Shrink {
            if shrink <= self.timer {
                done = true;
            } else {
                self.width = self.max_width - self.timer as f32 * self.max_width / shrink as f32;
            }
        }
        self.sync_sprite(world, field);
        !done
    }

    /// `FUN_0052e0c0`: the line laser grows, then travels, until it has left the field.
    fn update_line(&mut self, world: &mut World, field: [f32; 4]) -> bool {
        self.age += 1;
        let (s, c) = self.angle.sin_cos();
        let step = self.grow;
        if self.cap <= self.len {
            self.travelled += step;
            self.pos = [self.pos[0] + c * step, self.pos[1] + s * step];
            if self.limit > 0.0 && self.limit <= self.travelled + self.len && self.limit != self.travelled + self.len {
                self.len = self.limit - self.travelled;
                self.cap = self.len;
                if self.len <= 0.0 {
                    return false;
                }
            }
        } else {
            self.len += step;
            if self.cap <= self.len {
                self.len = self.cap;
            }
        }
        let head = [self.pos[0] + c * self.len, self.pos[1] + s * self.len];
        let w = self.width;
        let outside = |p: [f32; 2]| p[0] + w <= -148.0 || 148.0 <= p[0] - w || p[1] + w <= 0.0 || 448.0 <= p[1] - w;
        if self.offscreen_grace > 0 { self.offscreen_grace -= 1; }
        let alive = self.offscreen_grace > 0 || !outside(self.pos) || !outside(head);
        self.sync_sprite(world, field);
        alive
    }

    fn sync_sprite(&self, world: &mut World, field: [f32; 4]) {
        if let Some(vm) = self.vm.and_then(|id| world.get_mut(id)) {
            let (s, c) = self.angle.sin_cos();
            let mid = [self.pos[0] + c * self.len / 2.0, self.pos[1] + s * self.len / 2.0];
            let at = field_to_canvas(field, mid);
            vm.origin = [at[0], at[1], 0.0];
            vm.rot[2] = self.angle + std::f32::consts::FRAC_PI_2;
            vm.scale = [self.width.max(0.0) / self.tile[0], self.len.max(0.0) / self.tile[1]];
        }
    }

    /// Native cancellation samples at 8, 24, 40, ... pixels, then cuts at 16-pixel boundaries
    /// (0052c9f0/0052d350 and 0052a040/0052a970; splitters 0052d680/0052ac90).
    /// Returns whether the original survives, plus newly detached line lasers.
    pub fn cancel(&mut self, area: CancelArea, respect_invuln: bool, ctx: &mut CancelContext<'_>) -> (bool, Vec<Self>) {
        if respect_invuln && self.invuln != 0 { return (true, Vec::new()); }
        if self.kind == Kind::Curve { return self.cancel_curve(area,ctx); }
        let strict_end = matches!(area, CancelArea::All)
            || (self.kind == Kind::Infinite && matches!(area, CancelArea::Circle { .. }));
        let (s, c) = self.angle.sin_cos();
        let point = |distance: f32| [self.pos[0] + c * distance, self.pos[1] + s * distance];
        let mut mask = Vec::new();
        let mut distance = 8.0;
        while mask.len() < 512 && if strict_end { distance + 8.0 < self.len } else { distance + 8.0 <= self.len } {
            let at = point(distance);
            let hit = match area {
                CancelArea::All => true,
                CancelArea::Circle { pos, radius } => (at[0]-pos[0]).powi(2)+(at[1]-pos[1]).powi(2) <= radius*radius,
                // The original rectangle test uses the projected 16-pixel step as the local
                // rectangle dimensions (0052d350/0052a970), not the displayed beam width.
                CancelArea::Rect {pos,size} => rect_rect(at,[16.0*c,16.0*s],self.angle,pos,size),
                CancelArea::Column { x, half_width } => rect_rect(
                    at, [16.0*c, 16.0*s], self.angle, [x, 224.0], [2.0*half_width, 448.0]),
                // FUN_004d2c30 takes the area's angle too: test in the area's frame.
                CancelArea::ORect { pos, size, angle } => {
                    let (sa, ca) = angle.sin_cos();
                    let (dx, dy) = (at[0]-pos[0], at[1]-pos[1]);
                    rect_rect([dx*ca+dy*sa, -dx*sa+dy*ca], [16.0*c, 16.0*s], self.angle-angle, [0.0, 0.0], size)
                }
                // The ellipse callback (vtable +0x3c, FUN_00529820 / FUN_0052c2c0) tests the
                // 16-pixel step box against the ellipse (FUN_004d3c50).
                CancelArea::Ellipse { pos, a, b, angle } => crate::scope::ellipse_rect(at, [16.0*c, 16.0*s], self.angle, pos, a, b, angle),
                CancelArea::Polygon { pos, radius, angle, sides } => crate::scope::polygon_rect(at, [16.0*c, 16.0*s], self.angle, pos, radius, angle, sides),
            };
            mask.push(hit);
            if hit {
                let script = match self.sprite_type {
                    0..=17 | 34 | 38 => Some(212 + self.color*2),
                    // The line-rectangle callback uses <31 here; the other callbacks use <32.
                    31 if self.kind == Kind::Line && matches!(area, CancelArea::Column { .. } | CancelArea::Rect { .. } | CancelArea::ORect { .. }) => Some(284 + self.color*2),
                    18..=31 => Some(260 + self.color*2),
                    32..=33 => Some(284 + self.color*2),
                    _ => None,
                };
                if let Some(script) = script {
                    ctx.world.spawn_clipped(ctx.lib, ctx.anm, script as usize, field_to_canvas(ctx.field, at), ctx.field);
                }
            }
            distance += 16.0;
        }
        if matches!(area, CancelArea::All) { return (false, Vec::new()); }
        if !mask.iter().any(|&x| x) { return (true, Vec::new()); }

        let original_pos = self.pos;
        let original_len = self.len;
        let original_cap = self.cap;
        let mut fragments = Vec::new();
        let first = mask.iter().position(|&x| !x).unwrap_or(mask.len());
        let mut cursor;
        let alive;
        if self.kind == Kind::Infinite {
            // Keep the emitter-side part. The emitter can grow it back on later ticks.
            cursor = mask.iter().position(|&x| x).unwrap();
            self.len = cursor as f32 * 16.0;
            alive = true;
        } else if first == mask.len() {
            return (false, Vec::new());
        } else {
            let offset = first as f32 * 16.0;
            if first != 0 {
                self.pos = [original_pos[0] + c*offset, original_pos[1] + s*offset];
                self.len = original_len-offset;
                self.cap = self.len;
                self.travelled = offset;
            }
            cursor = first;
            while cursor < mask.len() && !mask[cursor] { cursor += 1; }
            if cursor < mask.len() {
                let len = (cursor-first) as f32*16.0;
                self.cap -= self.len-len;
                self.len = len;
                alive = len >= 24.0;
            } else {
                alive = first == 0 || self.len > 24.0;
            }
        }
        while cursor < mask.len() {
            if mask[cursor] { cursor += 1; continue; }
            let start = cursor;
            while cursor < mask.len() && !mask[cursor] { cursor += 1; }
            let len = (cursor-start) as f32*16.0;
            if self.kind == Kind::Line && len <= 24.0 { continue; }
            let offset = start as f32*16.0;
            let et = LaserEt { len: [len, len], width: self.width,
                f3: if self.kind == Kind::Infinite { original_cap-offset } else { self.limit },
                ..Default::default() };
            let fragment = Self::new(Kind::Line, [original_pos[0]+c*offset, original_pos[1]+s*offset],
                self.angle, if self.kind == Kind::Infinite { 8.0 } else { self.grow },
                self.sprite_type, self.color, &et, ctx.types, ctx.anm, ctx.world, ctx.lib, ctx.field);
            fragment.sync_sprite(ctx.world, ctx.field);
            fragments.push(fragment);
        }
        self.sync_sprite(ctx.world, ctx.field);
        (alive, fragments)
    }

    /// `FUN_0052b0e0`: the hurting rectangle (centre, length, width, angle) while widening or
    /// held and longer than 16.
    pub fn hit_rect(&self) -> Option<([f32; 2], f32, f32, f32)> {
        if self.kind == Kind::Line {
            // FUN_0052db90: longer than 16 and wider than 3; 0.8 of the length.
            if self.len <= 16.0 || self.width <= 3.0 {
                return None;
            }
            let width = if self.width >= 32.0 { self.width - (self.width + 16.0) / 2.0 } else { self.width * 0.5 };
            let (s, c) = self.angle.sin_cos();
            return Some(([self.pos[0] + c * self.len / 2.0, self.pos[1] + s * self.len / 2.0], self.len * 0.8, width, self.angle));
        }
        if !matches!(self.state, State::Widen | State::Hold) || self.len <= 16.0 {
            return None;
        }
        let width = if self.width >= 32.0 { self.width - (self.width + 16.0) / 3.0 } else { self.width * 0.5 };
        let (s, c) = self.angle.sin_cos();
        Some(([self.pos[0] + c * self.len / 2.0, self.pos[1] + s * self.len / 2.0], self.len * 0.95, width, self.angle))
    }

    /// `FUN_00549570`: against the player's hit circle (`hit` radius) with the graze ring 30
    /// pixels wider.
    pub fn contact(&mut self, player: [f32; 2], hit: f32) -> Contact {
        if let Some(curve)=&self.curve {
            let mut accumulated=0.0; let mut graze=false;
            for n in curve.nodes.iter().take(curve.nodes.len().saturating_sub(1)) {
                accumulated+=n.speed; if accumulated<16.0 {continue;}
                // 005279d0 adds half a node velocity plus half the initial heading velocity.
                let centre=[n.pos[0]+(n.angle.cos()+self.angle.cos())*n.speed*0.5,
                    n.pos[1]+(n.angle.sin()+self.angle.sin())*n.speed*0.5];
                let size=[n.speed*1.1,self.width*0.5*curve.hit_scale];
                if rect_circle(centre,size,n.angle,player,hit) {return Contact::Hit;}
                graze |= rect_circle(centre,size,n.angle,player,hit+30.0);
            }
            return if graze {Contact::Graze} else {Contact::None};
        }
        let Some((centre, len, width, angle)) = self.hit_rect() else {
            self.graze = 0;
            return Contact::None;
        };
        if !rect_circle(centre, [len, width], angle, player, hit + 30.0) {
            self.graze = 0;
            return Contact::None;
        }
        if rect_circle(centre, [len, width], angle, player, hit) {
            Contact::Hit
        } else {
            self.graze += 1;
            Contact::Graze
        }
    }

    /// Whether a graze this frame earns an orb: every 8th frame of the laser's life.
    pub fn graze_tick(&self) -> bool {
        self.age % 8 == 0
    }

    /// Where the graze effect appears (`FUN_004d3580`): the closest point of the hit segment to
    /// the player. Curved lasers use the player's position (their segment test is per node).
    pub fn graze_point(&self, player: [f32; 2]) -> [f32; 2] {
        match (&self.curve, self.hit_rect()) {
            (None, Some((centre, len, _, angle))) => crate::graze::closest_on_segment(centre, len, angle, player),
            _ => player,
        }
    }

    pub fn delete(&mut self, world: &mut World) {
        if let Some(id) = self.vm.take() {
            world.delete(id);
        }
    }

    pub fn vm(&self) -> Option<VmId> {
        self.vm
    }

    pub fn sprite_type(&self) -> (i32, i32) {
        (self.sprite_type, self.color)
    }
}

/// Where a ray from `from` along `angle` meets the field rectangle (x -148..148, y 0..448): the
/// entry point when it starts outside and is heading in, otherwise the exit (`ECL ins_569`).
pub fn ray_to_field_edge(from: [f32; 2], angle: f32) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    let (mut t_in, mut t_out) = (f32::NEG_INFINITY, f32::INFINITY);
    for (p, d, lo, hi) in [(from[0], c, -148.0, 148.0), (from[1], s, 0.0, 448.0)] {
        if d.abs() < 1e-6 {
            if p < lo || p > hi { return [0.0; 2]; }
            continue;
        }
        let (a, b) = ((lo - p) / d, (hi - p) / d);
        t_in = t_in.max(a.min(b));
        t_out = t_out.min(a.max(b));
    }
    if t_in > t_out || t_out < 0.0 { return [0.0; 2]; }
    let t = if t_in >= 0.0 && t_in <= t_out { t_in } else { t_out.max(0.0) };
    [from[0] + c * t, from[1] + s * t]
}

/// SAT equivalent of the game's rotated rectangle overlap (004d2c30).
pub(crate) fn rect_rect(a: [f32; 2], size: [f32; 2], angle: f32, b: [f32; 2], box_size: [f32; 2]) -> bool {
    let (s,c) = angle.sin_cos();
    let axes = [[1.0,0.0], [0.0,1.0], [c,s], [-s,c]];
    for [x,y] in axes {
        let separation = ((a[0]-b[0])*x+(a[1]-b[1])*y).abs();
        let reach_a = (size[0]*(c*x+s*y)).abs()/2.0 + (size[1]*(-s*x+c*y)).abs()/2.0;
        let reach_b = (box_size[0]*x).abs()/2.0 + (box_size[1]*y).abs()/2.0;
        if separation > reach_a+reach_b { return false; }
    }
    true
}

pub fn cancel_lasers(lasers: &mut Vec<Laser>, area: CancelArea, respect_invuln: bool, ctx: &mut CancelContext<'_>) {
    let mut detached = Vec::new();
    lasers.retain_mut(|laser| {
        let (alive, fragments) = laser.cancel(area, respect_invuln, ctx);
        detached.extend(fragments);
        if !alive { laser.delete(ctx.world); }
        alive
    });
    for mut laser in detached {
        if lasers.len() < 512 { lasers.push(laser); } else { laser.delete(ctx.world); }
    }
}

/// An oriented rectangle (centre, size along / across `angle`) against a circle (`FUN_004d44e0`).
pub fn rect_circle(centre: [f32; 2], size: [f32; 2], angle: f32, circle: [f32; 2], radius: f32) -> bool {
    let (s, c) = angle.sin_cos();
    let (dx, dy) = (circle[0] - centre[0], circle[1] - centre[1]);
    let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
    let (eu, ev) = ((u.abs() - size[0] / 2.0).max(0.0), (v.abs() - size[1] / 2.0).max(0.0));
    eu * eu + ev * ev < radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_leave_the_field_through_its_edges() {
        let p = ray_to_field_edge([0.0, 224.0], -std::f32::consts::FRAC_PI_2);
        assert!((p[0]).abs() < 1e-3 && p[1].abs() < 1e-3, "{p:?}");
        let p = ray_to_field_edge([0.0, 100.0], 0.0);
        assert!((p[0] - 148.0).abs() < 1e-3 && (p[1] - 100.0).abs() < 1e-3, "{p:?}");
        // From above the field, straight down: enters at the top edge.
        let p = ray_to_field_edge([10.0, -50.0], std::f32::consts::FRAC_PI_2);
        assert!((p[1]).abs() < 1e-3, "{p:?}");
        assert_eq!(ray_to_field_edge([200.0, 100.0], std::f32::consts::FRAC_PI_2), [0.0; 2], "a parallel miss keeps zero outputs");
        assert_eq!(ray_to_field_edge([200.0, 100.0], 0.0), [0.0; 2], "an outward miss must not fabricate a boundary point");
    }

    #[test]
    fn a_line_laser_grows_then_travels_and_ends_off_the_field() {
        let mut l = laser([0, 0, 0, 0]);
        l.kind = Kind::Line;
        l.len = 0.0;
        l.cap = 40.0;
        l.grow = 8.0;
        l.width = 16.0;
        l.pos = [0.0, 100.0];
        l.angle = std::f32::consts::FRAC_PI_2; // down
        let mut w = World::default();
        let mut lens = vec![];
        let mut ys = vec![];
        while l.update(&mut w, [0.0; 4]) {
            lens.push(l.len);
            ys.push(l.pos[1]);
            assert!(lens.len() < 200);
        }
        assert_eq!(lens[0], 8.0, "grows by its speed");
        assert!(lens.iter().any(|&x| x == 40.0), "up to its length");
        assert_eq!(ys[0], 100.0, "the tail stays while it grows");
        assert!(*ys.last().unwrap() > 400.0, "then the tail advances until both ends are off: {:?}", ys.last());
        // A grown laser hurts only when longer than 16 and wider than 3.
        let mut h = laser([0, 0, 0, 0]);
        h.kind = Kind::Line;
        h.len = 40.0;
        h.width = 16.0;
        h.pos = [0.0, 100.0];
        h.angle = std::f32::consts::FRAC_PI_2;
        assert_eq!(h.contact([0.0, 120.0], 3.0), Contact::Hit);
        assert_eq!(h.contact([0.0, 200.0], 3.0), Contact::None);
    }

    fn laser(times: [i32; 4]) -> Laser {
        Laser {
            kind: Kind::Infinite,
            tile: [16.0, 14.0],
            limit: 0.0,
            travelled: 0.0,
            pos: [0.0, 400.0],
            angle: -std::f32::consts::FRAC_PI_2,
            len: 512.0,
            cap: 512.0,
            grow: 8.0,
            width: 2.0,
            max_width: 14.0,
            times,
            state: State::Warn,
            timer: 0,
            age: 0,
            graze: 0,
            invuln: 0,
            offscreen_grace: 0,
            sprite_type: 38,
            color: 6,
            vm: None,
            curve: None,
        }
    }

    #[test]
    fn warns_widens_holds_shrinks_and_ends() {
        let mut l = laser([30, 10, 20, 10]);
        let mut w = World::default();
        let mut states = vec![];
        for _ in 0..80 {
            let alive = l.update(&mut w, [0.0; 4]);
            states.push((l.state, l.width, alive));
            if !alive {
                break;
            }
        }
        let first_widen = states.iter().position(|s| s.0 == State::Widen).unwrap();
        assert_eq!(first_widen, 29, "30 frames of warning");
        assert!(states[..first_widen].iter().all(|s| s.1 == 2.0));
        let hold = states.iter().position(|s| s.0 == State::Hold).unwrap();
        assert_eq!(states[hold].1, 14.0);
        let end = states.len();
        assert!(!states[end - 1].2, "ends after the shrink: {end}");
        assert!((70..=75).contains(&end), "{end}");
    }

    #[test]
    fn only_widening_and_held_lasers_hurt_and_the_graze_ring_is_30_wider() {
        let mut l = laser([5, 5, 100, 5]);
        let mut w = World::default();
        // Warning: harmless even on top of it.
        assert_eq!(l.contact([0.0, 300.0], 3.0), Contact::None);
        for _ in 0..12 {
            l.update(&mut w, [0.0; 4]);
        }
        assert_eq!(l.contact([0.0, 300.0], 3.0), Contact::Hit, "on the beam");
        assert_eq!(l.contact([20.0, 300.0], 3.0), Contact::Graze, "20 px to the side (beam half width 3.5 + 30 ring)");
        assert_eq!(l.contact([60.0, 300.0], 3.0), Contact::None);
        // Behind its start (the beam points up from y = 400).
        assert_eq!(l.contact([0.0, 450.0], 3.0), Contact::None);
    }
}

// LaserCurveInf path nodes: +8/+c absolute frame interval, +20 position,
// +2c heading, +30 speed, +34/+38 speed/angular acceleration (00415c60).
#[derive(Clone, Copy)]
struct PathSample { pos: [f32;2], angle: f32, speed: f32 }
#[derive(Clone)]
struct PathSegment { start: f32, end: f32, initial: PathSample, accel: f32, turn: f32 }
impl PathSegment {
    fn sample(&self, time:f32)->PathSample {
        let t=(time-self.start).max(0.0);
        let mut n=self.initial;
        if self.accel==0.0 && self.turn==0.0 {n.pos[0]+=n.angle.cos()*n.speed*t;n.pos[1]+=n.angle.sin()*n.speed*t;return n;}
        // 005274f0 mode2 integrates whole ticks, then the remaining fraction.
        for _ in 0..t.floor() as usize {
            n.pos[0]+=n.angle.cos()*n.speed;n.pos[1]+=n.angle.sin()*n.speed;
            n.angle=crate::motion::wrap_angle(n.angle+self.turn);n.speed+=self.accel;
        }
        n.pos[0]+=n.angle.cos()*n.speed*t.fract();n.pos[1]+=n.angle.sin()*n.speed*t.fract();n
    }
}
#[derive(Clone)]
struct Curve { path:Vec<PathSegment>, nodes:Vec<PathSample>, hit_scale:f32 }
impl Curve {
    fn sample(&self,t:f32)->PathSample {
        if t<0.0 {return self.path[0].initial;}
        self.path.iter().find(|p|p.start<=t && t<p.end).unwrap_or_else(||self.path.last().unwrap()).sample(t)
    }
}
impl Laser {
    /// ECL711 -> LaserCurveInf, 005286c0 initialization and 00525d50 Ex path setup.
    pub fn new_curve(origin:[f32;2],et:&crate::bullet::Et,types:&[BulletType],anm:usize,
        world:&mut World,lib:&AnmLibrary,field:[f32;4])->Self {
        let origin=[origin[0]+et.angle[0].cos()*et.distance,origin[1]+et.angle[0].sin()*et.distance];
        let mut l=Self::new(Kind::Line,origin,et.angle[0],et.speed[0],et.sprite,et.color,&et.laser,types,anm,world,lib,field);
        l.delete(world);l.kind=Kind::Curve;l.width=et.laser.width;l.offscreen_grace=30;
        let initial=PathSample {pos:origin,angle:et.angle[0],speed:et.speed[0]};
        let mut path=vec![PathSegment {start:0.0,end:999999.0,initial,accel:0.0,turn:0.0}];
        for e in &et.ex {
            match e.kind {
                0=>break,
                crate::bullet::ex::INVULN=>l.invuln=e.a,
                crate::bullet::ex::ANGLE_ACCEL=>{
                    let start=e.b as f32;
                    let last=path.last_mut().unwrap();
                    let initial=last.sample(start);last.end=start;
                    let end=if e.a<0 {999999.0} else {start+e.a as f32};
                    let segment=PathSegment {start,end,initial,accel:e.r,turn:e.s};
                    let finish=segment.sample(end.min(start+e.a.max(0) as f32));
                    path.push(segment);
                    if e.a>=0 {path.push(PathSegment {start:end,end:999999.0,initial:finish,accel:0.0,turn:0.0});}
                }
                other=>eprintln!("unported curved laser Ex {other:#x}"),
            }
        }
        let script=match et.sprite {1=>325,2=>11,n=>n+145};
        let remap=types.get(et.sprite.max(0) as usize).map(|t|t.colours[et.color.clamp(0,15) as usize]);
        let id=if let Some(remap)=remap {world.spawn_remapped(lib,anm,script as usize,field_to_canvas(field,origin),field,remap)}
            else {world.spawn_clipped(lib,anm,script as usize,field_to_canvas(field,origin),field)};
        // Sprite callback3 (00530f00) ignores the script sprite and returns color+524.
        if et.sprite!=1 && et.sprite!=2 {if let Some(v)=world.get_mut(id) {v.sprite=et.color+524;}}
        world.interrupt(id,2);world.set_draw_priority(id,DRAW_PRIORITY);
        if let Some(v)=world.get_mut(id) {v.visible=false;}
        l.vm=Some(id);
        l.curve=Some(Curve {path,nodes:vec![initial;et.laser.times[0].max(2) as usize],hit_scale:if et.sprite==2 {0.55} else {1.1}});
        l
    }
    fn update_curve(&mut self,world:&mut World,_field:[f32;4])->bool {
        self.age+=1;
        let c=self.curve.as_mut().unwrap();
        for i in 0..c.nodes.len() {c.nodes[i]=c.sample(self.age as f32-i as f32);}
        self.pos=c.nodes[0].pos;
        if let Some(v)=self.vm.and_then(|id|world.get_mut(id)) {v.visible=false;}
        if self.offscreen_grace>0 {self.offscreen_grace-=1;return true;}
        !c.nodes.iter().all(|n|n.pos[0]+self.width<=-148.0 || n.pos[0]-self.width>=148.0 || n.pos[1]+self.width<=0.0 || n.pos[1]-self.width>=448.0)
    }
    /// 00526990: textured triangle strip with averaged normals at interior nodes.
    pub fn curve_batch(&self,world:&World,lib:&AnmLibrary,field:[f32;4])->Option<th_anm::DrawBatch> {
        let curve=self.curve.as_ref()?;let vm=world.get(self.vm?)?;
        let sp=lib.files.get(vm.file)?.sprites.get(vm.sprite.max(0) as usize)?;
        let color=[vm.color[0]/255.0,vm.color[1]/255.0,vm.color[2]/255.0,vm.alpha/255.0];
        let mut strip=Vec::new();let [u0,v0,u1,v1]=sp.uv;
        for (i,n) in curve.nodes.iter().enumerate() {
            let angle=if i==0 {n.angle} else {n.angle+crate::motion::wrap_angle(curve.nodes[i-1].angle-n.angle)*0.5};
            let at=field_to_canvas(field,n.pos);let w=self.width*0.5;
            let uvx=u0+(u1-u0)*i as f32/(curve.nodes.len()-1) as f32;
            strip.push([th_anm::Vertex {pos:[at[0]-angle.sin()*w,at[1]+angle.cos()*w],uv:[uvx,v0],color,w:1.0,fog:0.0,depth:0.0},
                th_anm::Vertex {pos:[at[0]+angle.sin()*w,at[1]-angle.cos()*w],uv:[uvx,v1],color,w:1.0,fog:0.0,depth:0.0}]);
        }
        let mut vertices=Vec::new();for n in strip.windows(2) {vertices.extend([n[0][0],n[0][1],n[1][0],n[1][0],n[0][1],n[1][1]]);}
        Some(th_anm::DrawBatch {texture:Some(sp.texture),blend:vm.blend,depth:th_anm::DepthMode::Off,layer:DRAW_PRIORITY,clip:Some(field),vertices})
    }
    fn cancel_curve(&mut self,area:CancelArea,ctx:&mut CancelContext<'_>)->(bool,Vec<Self>) {
        let curve=self.curve.as_ref().unwrap();
        let mask:Vec<_>=curve.nodes.iter().map(|n|match area {
            CancelArea::All=>true,
            CancelArea::Circle {pos,radius}=>(n.pos[0]-pos[0]).powi(2)+(n.pos[1]-pos[1]).powi(2)<=radius*radius,
            CancelArea::Rect {pos,size}=>crate::scope::rect_circle(pos,size,n.pos,0.0),
            CancelArea::Column {x,half_width}=>(n.pos[0]-x).abs()<=half_width,
            CancelArea::ORect {pos,size,angle}=>crate::scope::rect_circle_exact(pos,size,angle,n.pos,0.0),
            // FUN_00524160 -> FUN_004d4420: each node as a point in the ellipse.
            CancelArea::Ellipse {pos,a,b,angle}=>crate::scope::ellipse_circle(n.pos,0.0,pos,a,b,angle),
            CancelArea::Polygon {pos,radius,angle,sides}=>crate::scope::polygon_contains(n.pos,pos,radius,angle,sides),
        }).collect();
        if !mask.iter().any(|&x|x) {return (true,Vec::new());}
        let mut runs=Vec::new();let mut i=0;
        while i<mask.len() {if mask[i] {i+=1;continue;}let first=i;while i<mask.len()&&!mask[i] {i+=1;}
            if i-first>3 {runs.push((first,i));}}
        // 00525270 discards surviving runs shorter than four nodes and splits at cut nodes.
        let Some((first,end))=runs.first().copied() else {return (false,Vec::new());};
        let original=curve.clone();let age=self.age;
        let mut fragments=Vec::new();
        for &(start,end) in runs.iter().skip(1) {
            let et=crate::bullet::Et {sprite:self.sprite_type,color:self.color,angle:[self.angle,0.0],speed:[self.grow,0.0],
                laser:LaserEt {width:self.width,times:[(end-start) as i32,0,0,0],..Default::default()},..Default::default()};
            let mut fragment=Self::new_curve(self.pos,&et,ctx.types,ctx.anm,ctx.world,ctx.lib,ctx.field);
            fragment.curve=Some(Curve {nodes:original.nodes[start..end].to_vec(),..original.clone()});
            fragment.age=age-start as i32;fragment.invuln=self.invuln;fragments.push(fragment);
        }
        self.curve=Some(Curve {nodes:original.nodes[first..end].to_vec(),..original});self.age=age-first as i32;
        (true,fragments)
    }
}

#[cfg(test)]
mod curve_tests {
    use super::*;
    #[test]
    fn native_curve_path_uses_absolute_times_and_discrete_integration() {
        let initial=PathSample {pos:[0.0;2],angle:std::f32::consts::FRAC_PI_2,speed:0.0};
        let ramp=PathSegment {start:0.0,end:40.0,initial,accel:0.1,turn:0.0};
        let a=ramp.sample(40.0);
        assert!((a.pos[1]-78.0).abs()<0.001);assert!((a.speed-4.0).abs()<0.001);
        let straight=PathSegment {start:40.0,end:130.0,initial:a,accel:0.0,turn:0.0};
        let b=straight.sample(130.0);assert!((b.pos[1]-438.0).abs()<0.002);
        let slow=PathSegment {start:130.0,end:140.0,initial:b,accel:-0.4,turn:0.0};
        let c=slow.sample(140.0);assert!(c.speed.abs()<0.001);assert!((c.pos[1]-460.0).abs()<0.002);
        let turn=PathSegment {start:140.0,end:148.0,initial:c,accel:0.0,turn:std::f32::consts::PI/16.0};
        assert!((turn.sample(148.0).angle.abs()-std::f32::consts::PI).abs()<0.001);
    }
    #[test]
    fn original_sanae_ex_uses_laser_gradient_color_and_native_tail_length() {
        let mut w=World::default();let lib=AnmLibrary::default();
        let et=crate::bullet::Et {color:6,speed:[0.0;2],laser:LaserEt {width:24.0,times:[32,-1,-1,-1],..Default::default()},
            ex:std::array::from_fn(|i|if i==0 {crate::bullet::Ex {kind:0x80,a:300,..Default::default()}}
                else if i==1 {crate::bullet::Ex {kind:8,a:40,b:0,r:0.1,asynchronous:true,..Default::default()}}
                else {Default::default()}),..Default::default()};
        let mut l=Laser::new_curve([0.0;2],&et,&[],0,&mut w,&lib,[0.0;4]);
        for _ in 0..40 {l.update(&mut w,[0.0;4]);}
        assert_eq!(l.invuln,260);assert_eq!(l.curve.as_ref().unwrap().nodes.len(),32);
        assert!((l.pos[0]-78.0).abs()<0.001);
        assert_eq!(w.get(l.vm.unwrap()).unwrap().sprite,530);
        assert!(!w.get(l.vm.unwrap()).unwrap().visible);
        let mut ctx=CancelContext {world:&mut w,lib:&lib,types:&[],anm:0,field:[0.0;4]};
        assert!(l.cancel(CancelArea::All,true,&mut ctx).0);
        assert!(!l.cancel(CancelArea::All,false,&mut ctx).0);
    }
}
