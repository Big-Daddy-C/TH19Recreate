//! The player: movement, options and shots.
//!
//! Reimplements th19.exe's player (`FUN_00546320` state machine, `FUN_00546ed0` normal
//! state, `FUN_0042fce0` movement, `FUN_00546d10` options) and player-shot manager
//! (`FUN_0054e490` timers, `FUN_0054f990` record selection, `FUN_0054e960` bullet setup).
//! Positions are kept in 1/128 fixed point like the original.

use th_anm::{AnmLibrary, VmId, World};
use th_formats::sht::{ShotRecord, ShtFile};

use crate::enemy::{field_to_canvas, flag, Enemy};

/// Input bits, as the game stores them.
pub mod input {
    pub const SHOT: u32 = 1;
    pub const FOCUS: u32 = 8;
    pub const UP: u32 = 0x10;
    pub const DOWN: u32 = 0x20;
    pub const LEFT: u32 = 0x40;
    pub const RIGHT: u32 = 0x80;
}

/// Movement direction table at 0x5a9948: none, up, down, left, right, then diagonals.
const DIRS: [(i32, i32); 9] = [(0, 0), (0, -1), (0, 1), (-1, 0), (1, 0), (-1, -1), (1, -1), (-1, 1), (1, 1)];

fn direction(keys: u32) -> usize {
    use input::*;
    match keys & (UP | DOWN | LEFT | RIGHT) {
        k if k & (UP | LEFT) == UP | LEFT => 5,
        k if k & (DOWN | LEFT) == DOWN | LEFT => 7,
        k if k & (UP | RIGHT) == UP | RIGHT => 6,
        k if k & (DOWN | RIGHT) == DOWN | RIGHT => 8,
        k if k & DOWN != 0 => 2,
        k if k & UP != 0 => 1,
        k if k & LEFT != 0 => 3,
        k if k & RIGHT != 0 => 4,
        _ => 0,
    }
}

/// The game's frame counters: `prev` lets "the value changed this frame" be detected.
#[derive(Clone, Copy, Debug)]
struct Timer {
    prev: i32,
    cur: i32,
}

impl Timer {
    const OFF: Timer = Timer { prev: -2, cur: -1 };
    fn set(&mut self, v: i32) {
        self.cur = v;
        self.prev = v - 1;
    }
    fn tick(&mut self) {
        self.prev = self.cur;
        self.cur += 1;
    }
    fn sub(&mut self, n: i32) {
        self.prev = self.cur;
        self.cur -= n;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Rising into the field (60 frames).
    Entering,
    Normal,
}

struct PlayerOption {
    /// Current position, fixed point.
    pos: [i32; 2],
    vm: Option<VmId>,
}

pub struct PlayerBullet {
    pub pos: [f32; 2],
    pub angle: f32,
    pub speed: f32,
    pub hitbox: [f32; 2],
    pub damage: i32,
    pub homing: bool,
    pub vm: Option<VmId>,
    pub time: i32,
}

pub struct Player {
    pub sht: ShtFile,
    pub anm_file: usize,
    /// Position in 1/128 units.
    pub fixed: [i32; 2],
    pub pos: [f32; 2],
    pub state: State,
    pub state_time: i32,
    pub focused: bool,
    pub power: i32,
    prev_vx: i32,
    body: Option<VmId>,
    options: Vec<PlayerOption>,
    shot_timer: Timer,
    long_timer: Timer,
    pub bullets: Vec<PlayerBullet>,
}

impl Player {
    pub fn new(sht: ShtFile, anm_file: usize) -> Self {
        // Normal power is 200 (two options); the game raises it to 400 only for a timed
        // boost and drops back to 200 afterwards (FUN_00419ae0).
        let power = 200.min(sht.power_levels * 100).clamp(0, 400);
        Self {
            sht,
            anm_file,
            fixed: [0, 400 * 128],
            pos: [0.0, 400.0],
            state: State::Entering,
            state_time: 0,
            focused: false,
            power,
            prev_vx: 0,
            body: None,
            options: Vec::new(),
            shot_timer: Timer::OFF,
            long_timer: Timer::OFF,
            bullets: Vec::new(),
        }
    }

    fn level(&self) -> usize {
        (self.power / 100).clamp(0, 4) as usize
    }

    fn set_body_script(&mut self, world: &mut World, lib: &AnmLibrary, field: [f32; 4], script: usize) {
        if let Some(id) = self.body.take() {
            world.delete(id);
        }
        let at = field_to_canvas(field, self.pos);
        self.body = Some(world.spawn_clipped(lib, self.anm_file, script, at, field));
    }

    /// Option offsets for the current power level and focus state.
    fn option_offsets(&self) -> &[[f32; 2]] {
        let n = self.level();
        let start = n * n.saturating_sub(1) / 2;
        &self.sht.options[self.focused as usize][start..start + n]
    }

    pub fn update(&mut self, keys: u32, enemies: &mut [Enemy], world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        if self.body.is_none() {
            self.set_body_script(world, lib, field, 0);
        }
        match self.state {
            State::Entering => {
                // Rise from y = 480 to y = 400 over 60 frames.
                self.fixed[1] = 0xf000 - self.state_time * 0x2800 / 60;
                self.state_time += 1;
                if self.state_time >= 60 {
                    self.state = State::Normal;
                    self.state_time = 0;
                }
            }
            State::Normal => {
                self.focused = keys & input::FOCUS != 0;
                let dir = direction(keys);
                let s = &self.sht;
                let speed = match (self.focused, dir >= 5) {
                    (false, false) => s.speed,
                    (false, true) => s.speed_diagonal,
                    (true, false) => s.speed_focused,
                    (true, true) => s.speed_focused_diagonal,
                };
                let speed = (speed * 128.0) as i32;
                let (vx, vy) = (speed * DIRS[dir].0, speed * DIRS[dir].1);
                // Body script follows horizontal movement: 1 start left, 2 stop from left,
                // 3 start right, 4 stop from right.
                let script = match (vx.signum(), self.prev_vx.signum()) {
                    (-1, p) if p >= 0 => Some(1),
                    (0, -1) => Some(2),
                    (1, p) if p <= 0 => Some(3),
                    (0, 1) => Some(4),
                    _ => None,
                };
                if let Some(sc) = script {
                    self.set_body_script(world, lib, field, sc);
                }
                self.prev_vx = vx;
                self.fixed[0] = (self.fixed[0] + vx).clamp(-0x4600, 0x4600);
                self.fixed[1] = (self.fixed[1] + vy).clamp(0x1000, 0xd800);
                self.state_time += 1;
            }
        }
        self.pos = [self.fixed[0] as f32 / 128.0, self.fixed[1] as f32 / 128.0];

        self.update_options(world, lib, field);
        if self.state == State::Normal {
            self.update_shots(keys, world, lib, field, enemies);
        }
        self.update_bullets(enemies, world, field);

        let at = field_to_canvas(field, self.pos);
        if let Some(vm) = self.body.and_then(|id| world.get_mut(id)) {
            vm.origin = [at[0], at[1], 0.0];
        }
    }

    fn update_options(&mut self, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        let offsets: Vec<[f32; 2]> = self.option_offsets().to_vec();
        while self.options.len() > offsets.len() {
            if let Some(id) = self.options.pop().and_then(|o| o.vm) {
                world.delete(id);
            }
        }
        while self.options.len() < offsets.len() {
            let at = field_to_canvas(field, self.pos);
            let script = self.sht.option_script;
            let vm = (script >= 0).then(|| world.spawn_clipped(lib, self.anm_file, script as usize, at, field));
            self.options.push(PlayerOption { pos: self.fixed, vm });
        }
        for (o, off) in self.options.iter_mut().zip(&offsets) {
            let target = [self.fixed[0] + (off[0] * 128.0) as i32, self.fixed[1] + (off[1] * 128.0) as i32];
            // Options close 30% of the distance to their slot every frame.
            let d = [(target[0] - o.pos[0]) * 30 / 100, (target[1] - o.pos[1]) * 30 / 100];
            if d == [0, 0] {
                o.pos = target;
            } else {
                o.pos = [o.pos[0] + d[0], o.pos[1] + d[1]];
            }
            let p = field_to_canvas(field, [o.pos[0] as f32 / 128.0, o.pos[1] as f32 / 128.0]);
            if let Some(vm) = o.vm.and_then(|id| world.get_mut(id)) {
                vm.origin = [p[0], p[1], 0.0];
            }
        }
    }

    fn update_shots(&mut self, keys: u32, world: &mut World, lib: &AnmLibrary, field: [f32; 4], enemies: &[Enemy]) {
        let held = keys & input::SHOT != 0;
        if self.shot_timer.cur < 0 && held {
            if self.long_timer.cur < 0 {
                self.long_timer.set(0);
            }
            self.shot_timer.set(0);
        }
        if self.shot_timer.cur >= 0 && self.shot_timer.cur != self.shot_timer.prev {
            self.fire(self.shot_timer.cur, self.long_timer.cur, world, lib, field, enemies);
        }
        if self.shot_timer.cur >= 0 {
            if self.shot_timer.cur < 14 {
                self.shot_timer.tick();
            } else if held {
                self.shot_timer.sub(14);
            } else {
                self.shot_timer = Timer::OFF;
            }
        }
        if self.long_timer.cur >= 0 {
            if self.long_timer.cur <= 0x76 {
                self.long_timer.tick();
            } else if held {
                self.long_timer.sub(0x77);
            } else {
                self.long_timer = Timer::OFF;
            }
        }
    }

    fn fire(&mut self, timer: i32, long: i32, world: &mut World, lib: &AnmLibrary, field: [f32; 4], _enemies: &[Enemy]) {
        let mut slot = self.level();
        if self.focused && slot < 10 {
            slot += 5;
        }
        let Some(Some(records)) = self.sht.slots.get(slot) else { return };
        let records: Vec<ShotRecord> = records.clone();
        for r in &records {
            let fires = if r.alt_interval != 0 {
                long >= 0 && long % r.alt_interval as i32 == r.alt_phase as i32
            } else {
                r.interval == 0 || timer % r.interval as i32 == r.phase as i32
            };
            if fires {
                self.spawn_bullet(r, world, lib, field);
            }
        }
    }

    fn spawn_bullet(&mut self, r: &ShotRecord, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        let origin = match r.origin & 0xf {
            0 => self.pos,
            6 => [0.0, 0.0],
            k => match self.options.get(k as usize - 1) {
                Some(o) => [o.pos[0] as f32 / 128.0, o.pos[1] as f32 / 128.0],
                None => return,
            },
        };
        let (s, c) = r.angle.sin_cos();
        let vel = [c * r.speed, s * r.speed];
        // The game places the bullet one step back, so its first update lands on the offset.
        let back = if matches!(r.kind, 4..=6) { [0.0; 2] } else { vel };
        let pos = [origin[0] + r.offset[0] - back[0], origin[1] + r.offset[1] - back[1]];
        let vm = (r.anm_script >= 0).then(|| {
            let id = world.spawn_clipped(lib, self.anm_file, r.anm_script as usize, field_to_canvas(field, pos), field);
            if let Some(v) = world.get_mut(id) {
                v.rot[2] = r.angle;
            }
            id
        });
        self.bullets.push(PlayerBullet {
            pos,
            angle: r.angle,
            speed: r.speed,
            hitbox: r.hitbox,
            damage: r.damage as i32,
            homing: r.funcs[0] == 1,
            vm,
            time: 0,
        });
    }

    fn update_bullets(&mut self, enemies: &mut [Enemy], world: &mut World, field: [f32; 4]) {
        let mut i = 0;
        while i < self.bullets.len() {
            let b = &mut self.bullets[i];
            b.time += 1;
            if b.homing {
                // APPROX: steer toward the nearest damageable enemy and speed up; the real
                // amulet behaviour is callback 1 of the game's shot function table.
                let target = enemies
                    .iter()
                    .filter(|e| e.data.flags & flag::NO_HURTBOX == 0 && e.data.hurtbox[0] > 0.0 && e.data.entered)
                    .map(|e| e.data.pos)
                    .min_by(|a, c| dist2(*a, b.pos).total_cmp(&dist2(*c, b.pos)));
                if let Some(t) = target {
                    let want = (t[1] - b.pos[1]).atan2(t[0] - b.pos[0]);
                    let diff = crate::motion::wrap_angle(want - b.angle);
                    b.angle += diff.clamp(-0.2, 0.2);
                }
                b.speed = (b.speed + 0.5).min(10.0);
            }
            let (s, c) = b.angle.sin_cos();
            b.pos = [b.pos[0] + c * b.speed, b.pos[1] + s * b.speed];

            let mut dead = b.pos[0].abs() > 148.0 + 32.0 || b.pos[1] < -32.0 || b.pos[1] > 448.0 + 32.0;
            if !dead {
                for e in enemies.iter_mut() {
                    let d = &mut e.data;
                    if d.flags & flag::NO_HURTBOX != 0 || d.hurtbox[0] <= 0.0 || d.delete {
                        continue;
                    }
                    let overlap = (b.pos[0] - d.pos[0]).abs() * 2.0 < b.hitbox[0] + d.hurtbox[0]
                        && (b.pos[1] - d.pos[1]).abs() * 2.0 < b.hitbox[1] + d.hurtbox[1];
                    if overlap {
                        d.damage(b.damage, b.pos);
                        dead = true;
                        break;
                    }
                }
            }
            if dead {
                if let Some(id) = self.bullets[i].vm {
                    world.delete(id);
                }
                self.bullets.swap_remove(i);
                continue;
            }
            let b = &self.bullets[i];
            let at = field_to_canvas(field, b.pos);
            if let Some(vm) = b.vm.and_then(|id| world.get_mut(id)) {
                vm.origin = [at[0], at[1], 0.0];
                vm.rot[2] = b.angle;
            }
            i += 1;
        }
    }
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}
