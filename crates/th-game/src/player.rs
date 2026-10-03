//! The player: movement, options and shots.
//!
//! Reimplements th19.exe's player (`FUN_00546320` state machine, `FUN_00546ed0` normal
//! state, `FUN_0042fce0` movement, `FUN_00546d10` options) and player-shot manager
//! (`FUN_0054e490` timers, `FUN_0054f990` record selection, `FUN_0054e960` bullet setup).
//! Positions are kept in 1/128 fixed point like the original.

use th_anm::{AnmLibrary, VmId, World};
use th_formats::sht::{ShotRecord, ShtFile};

use crate::character::Character;
use crate::enemy::{field_to_canvas, flag, Enemy};

/// Input bits, as the game stores them.
pub mod input {
    pub const SHOT: u32 = 1;
    pub const CHARGE: u32 = 2;
    /// The bomb button (`+0x2a8 & 4` in FUN_00546320).
    pub const BOMB: u32 = 4;
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
    /// Rising into the field (60 frames); after a death it also clears nearby bullets.
    Entering,
    Normal,
    /// Just hit: a short window (8 frames) before the death is processed (state 4).
    Hit,
    /// The barrier absorbed a hit: pushed away from the bullet for 40 frames (state 5).
    Knockback,
    /// Dead, waiting 30 frames to respawn (state 2).
    Dead,
    /// Out of hearts.
    GameOver,
}

struct PlayerOption {
    /// Current position, fixed point.
    pos: [i32; 2],
    vm: Option<VmId>,
    /// Second per-option script (SHT +0x40, Ran's rotating clock) while power is 400.
    extra: Option<VmId>,
    /// Snap-to-target ticks left (option +0x100; Ran's callback sets 10).
    hold: i32,
}

pub struct PlayerBullet {
    pub pos: [f32; 2],
    pub angle: f32,
    pub speed: f32,
    pub hitbox: [f32; 2],
    pub damage: i32,
    pub homing: bool,
    /// Callback 1 retains the selected enemy until it disappears or becomes invalid.
    pub homing_target: Option<i32>,
    angular_velocity: f32,
    acceleration: f32,
    pub vm: Option<VmId>,
    pub time: i32,
    /// Reimu slot 10: init callback 11 sets piercing, update callback 8 controls speed.
    pub charge_shot: bool,
    /// FUN_0054fba0 state 2: collider removed, ANM interrupt 1 plays the impact.
    pub impacting: bool,
    hit_callback: i32,
    snake_turn: Option<(i32, f32)>,
    snake: bool,
    explosion_growth: f32,
}

struct SnakeExplosion {pos:[f32;2],angle:f32,radius:f32,growth:f32,damage:i32,time:i32}
pub struct Player {
    pub character: crate::character::Character,
    /// Development toggle: the player can't be hit.
    pub godmode: bool,
    /// Set when a heart is lost; the side consumes it (EX/boss level catch-up).
    pub just_died: bool,
    pub sht: ShtFile,
    pub anm_file: usize,
    /// Position in 1/128 units.
    pub fixed: [i32; 2],
    pub pos: [f32; 2],
    pub state: State,
    pub state_time: i32,
    pub focused: bool,
    pub scope: crate::scope::Scope,
    pub power: i32,
    pub charge_settings: crate::charge::Settings,
    pub charge: crate::charge::Gauge,
    /// Status +0x24. Starts at the first SHT threshold (FUN_005478f0).
    pub meter: i32,
    /// Practice override requested by the user: all tiers, no meter earnings/costs.
    pub free_charge: bool,
    pub opponent_boss_active: bool,
    pub charge_event: Option<crate::charge::Level>,
    /// Sound requests `(id, pan)`; `None` pans to this player's side. Drained by the side.
    pub sounds: Vec<(u32, Option<i32>)>,
    pub last_charge: Option<crate::charge::Level>,
    pub skills: Vec<crate::skill::Skill>,
    /// The running bomb (`BombInf`), at most one.
    pub bomb: Option<crate::bomb::Bomb>,
    bomb_prev: bool,
    pub cancel_circles: Vec<([f32;2], f32)>,
    /// Marisa's bomb: full-height bands (centre x, half width) that cancel bullets.
    pub cancel_columns: Vec<(f32, f32)>,
    damage_circles: Vec<crate::skill::DamageCircle>,
    snake_explosions: Vec<SnakeExplosion>,
    skill_rng: u32,
    charge_timer: Option<i32>,
    /// Marisa: a skill release also fires the two side charge beams (SHT slot 11).
    wide_charge: bool,
    charge_origin: [f32;2],
    skill_volley: bool,
    boost_frames: i32,
    pub cancel_rects: Vec<([f32;2],[f32;2])>,
    pub aura_anm: Option<usize>,
    charge_vm: Option<VmId>,
    focus_vm: Option<VmId>,
    prev_vx: i32,
    body: Option<VmId>,
    options: Vec<PlayerOption>,
    /// Player position last seen while unfocused (player +0x6a0/+0x6a4), Ran's option anchor.
    option_anchor: [i32; 2],
    shot_timer: Timer,
    long_timer: Timer,
    pub bullets: Vec<PlayerBullet>,
    /// Marisa's beams and which SHT groups have one alive.
    pub beams: Vec<crate::beam::Beam>,
    beam_groups: [bool; 64],
    /// Frames of invulnerability left.
    pub invuln: i32,
    /// Times hit so far.
    pub hits: u32,
    /// Hearts left (status +0x54) and heart slots (status +0x58, at most 7).
    pub lives: i32,
    pub max_lives: i32,
    /// Bullets within this radius of the player should be cancelled this frame.
    pub cancel_radius: Option<f32>,
    /// The death effect (script 21): a glow that bursts on interrupt 1.
    death_vm: Option<VmId>,
    /// `effect.anm`, which holds the barrier sprite (script 1).
    pub effect_anm: Option<usize>,
    pub barrier: Barrier,
    barrier_vm: Option<VmId>,
    /// Direction of the knockback push.
    knock_angle: f32,
}

/// The barrier (th19.exe `FUN_00544be0` / `FUN_00544c90` / `FUN_00544dd0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Barrier {
    /// Up: absorbs the next hit.
    Up,
    /// Broken.
    Broken,
}

/// Starting hearts: TH19's default match setting (configurable in the game's options).
pub const DEFAULT_LIVES: i32 = 2;

impl Player {
    pub fn new(sht: ShtFile, anm_file: usize) -> Self {
        // Normal power is 200 (two options); the game raises it to 400 only for a timed
        // boost and drops back to 200 afterwards (FUN_00419ae0).
        let power = 200.min(sht.power_levels * 100).clamp(0, 400);
        // APPROX: use 100% charge speed until match settings / card modifiers are loaded.
        let charge_settings = crate::charge::Settings::from_sht(&sht, 100);
        Self {
            character: crate::character::Character::Reimu,
            sht,
            anm_file,
            fixed: [0, 400 * 128],
            pos: [0.0, 400.0],
            state: State::Entering,
            state_time: 0,
            focused: false,
            scope: crate::scope::Scope::default(),
            power,
            charge_settings,
            charge: crate::charge::Gauge::default(),
            meter: charge_settings.attack,
            free_charge: false, opponent_boss_active: false, charge_event: None, sounds: Vec::new(), last_charge: None,
            skills: Vec::new(), bomb: None, bomb_prev: false, cancel_circles: Vec::new(), cancel_columns: Vec::new(), damage_circles: Vec::new(), snake_explosions: Vec::new(), skill_rng: 0x1cafe777,
            charge_timer: None,
            wide_charge: false, charge_origin: [0.0;2], skill_volley: false, boost_frames: 0, cancel_rects: Vec::new(),
            aura_anm: None,
            charge_vm: None,
            focus_vm: None,
            prev_vx: 0,
            body: None,
            options: Vec::new(),
            option_anchor: [0, 400 * 128],
            shot_timer: Timer::OFF,
            long_timer: Timer::OFF,
            bullets: Vec::new(),
            beams: Vec::new(),
            beam_groups: [false; 64],
            invuln: 0,
            hits: 0,
            lives: DEFAULT_LIVES,
            max_lives: DEFAULT_LIVES,
            cancel_radius: None,
            death_vm: None,
            godmode: false,
            just_died: false,
            effect_anm: None,
            barrier: Barrier::Up,
            barrier_vm: None,
            knock_angle: 0.0,
        }
    }

    /// Collision radius against bullets: 3.0 (player +0x20c4/+0x20c8, scaled by a percentage
    /// that is 100 unless an ability card changes it). 0 while the player can't be hit.
    pub fn hitbox_radius(&self) -> f32 {
        if self.state == State::Normal && self.invuln < 1 { 3.0 } else { 0.0 }
    }

    /// Hit by a bullet at `from`. With the barrier up it breaks instead (`FUN_00544c90`):
    /// 100 frames of invulnerability and a knockback away from the bullet. Otherwise
    /// (`FUN_00545e10`): hit effect, then an 8-frame window before death.
    pub fn hit(&mut self, from: [f32; 2], world: &mut World, lib: &AnmLibrary, field: [f32; 4]) -> bool {
        // Godmode (a development toggle, not a game feature): hits are ignored.
        if self.godmode || self.state != State::Normal || self.invuln > 0 {
            return false;
        }
        if self.barrier == Barrier::Up {
            self.barrier = Barrier::Broken;
            // FUN_00544c90: sound 0x1b, panned to this player's side.
            if crate::sfx::on(crate::sfx::Cat::Barrier) { self.sounds.push((0x1b, None)); }
            if let Some(id) = self.barrier_vm.take() {
                world.interrupt(id, 1);
            }
            self.invuln += 100;
            let (dx, dy) = (self.pos[0] - from[0], self.pos[1] - from[1]);
            self.knock_angle = dy.atan2(dx);
            self.state = State::Knockback;
            self.state_time = 0;
            return true;
        }
        self.hits += 1;
        // FUN_00545e10: sound 2 (centre pan) unless player flag 0x8 is set (not traced).
        if crate::sfx::on(crate::sfx::Cat::PlayerHit) { self.sounds.push((2, Some(0))); }
        world.spawn_clipped(lib, self.anm_file, 22, field_to_canvas(field, self.pos), field);
        self.state = State::Hit;
        self.state_time = 0;
        self.invuln = 6;
        true
    }

    /// The death proper (`FUN_00549830`): effect, one heart lost, options gone, 180 frames
    /// of invulnerability.
    fn die(&mut self, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        self.death_vm = Some(world.spawn_clipped(lib, self.anm_file, 21, field_to_canvas(field, self.pos), field));
        self.lives -= 1;
        self.just_died = true;
        self.state = State::Dead;
        self.state_time = 0;
        self.invuln = 180;
        for o in self.options.drain(..) {
            for id in [o.vm, o.extra].into_iter().flatten() {
                world.interrupt(id, 1);
            }
        }
        // Losing a heart restores the barrier (FUN_00549830 -> FUN_00544be0).
        self.barrier = Barrier::Up;
        if let Some(id) = self.body.take() {
            world.delete(id);
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
        self.cancel_radius = None;
        if self.boost_frames > 0 {
            self.boost_frames -= 1;
            if self.boost_frames == 0 { self.power = 200; }
        }
        self.charge_event = None;
        if self.body.is_none() && matches!(self.state, State::Entering | State::Normal) {
            self.set_body_script(world, lib, field, 0);
        }
        match self.state {
            State::Hit => {
                self.state_time += 1;
                if self.state_time >= 8 {
                    self.die(world, lib, field);
                }
            }
            State::Dead => {
                self.state_time += 1;
                if self.state_time >= 30 {
                    // APPROX: the glow waits for interrupt 1 to burst; what sends it in the
                    // game hasn't been found, so it bursts here, as the player comes back.
                    if let Some(id) = self.death_vm.take() {
                        world.interrupt(id, 1);
                    }
                    if self.lives > 0 {
                        // Respawn: back to the bottom centre and rise in again.
                        self.state = State::Entering;
                        self.state_time = 0;
                        self.fixed = [0, 0xf000];
                    } else {
                        self.state = State::GameOver;
                    }
                }
            }
            State::GameOver => {}
            State::Knockback => {
                // Case 5 of FUN_00546320: pushed along the angle at speed 2 -> 0 over 40 frames.
                let speed = 2.0 - self.state_time as f32 * 2.0 / 40.0;
                let (s, c) = self.knock_angle.sin_cos();
                self.fixed[0] = (self.fixed[0] + (c * speed * 128.0) as i32).clamp(-0x4600, 0x4600);
                self.fixed[1] = (self.fixed[1] + (s * speed * 128.0) as i32).clamp(0x1000, 0xd800);
                self.state_time += 1;
                if self.state_time >= 40 {
                    self.state = State::Normal;
                    self.state_time = 0;
                }
            }
            State::Entering => {
                // Rise from y = 480 to y = 400 over 60 frames; from frame 30 bullets within
                // 160 of the player are cancelled (case 0 of FUN_00546320).
                self.fixed[1] = 0xf000 - self.state_time * 0x2800 / 60;
                if self.state_time >= 30 {
                    self.cancel_radius = Some(160.0);
                }
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
        if self.invuln > 0 {
            self.invuln -= 1;
        }

        if matches!(self.state, State::Entering | State::Normal | State::Knockback) {
            self.update_options(world, lib, field);
        }
        // Barrier sprite (effect.anm script 1) follows the player while the barrier is up.
        let alive = matches!(self.state, State::Entering | State::Normal | State::Knockback);
        if self.barrier == Barrier::Up && alive && self.barrier_vm.is_none() {
            if let Some(file) = self.effect_anm {
                self.barrier_vm = Some(world.spawn_clipped(lib, file, 1, field_to_canvas(field, self.pos), field));
                // FUN_00544dd0 creates the barrier with sound 0x3b, panned to this side.
                if crate::sfx::on(crate::sfx::Cat::Barrier) { self.sounds.push((0x3b, None)); }
            }
        }
        if let Some(vm) = self.barrier_vm.and_then(|id| world.get_mut(id)) {
            let at = field_to_canvas(field, self.pos);
            vm.origin = [at[0], at[1], 0.0];
        }
        if self.state == State::Normal {
            self.update_shots(keys, world, lib, field, enemies);
        }
        self.update_charge(keys, world, lib, field);
        self.update_focus_marker(world, lib, field);
        self.scope.update_character(self.charge.value > 0 && keys & input::CHARGE != 0
            && self.state == State::Normal, self.character, self.pos, self.anm_file, world, lib, field);
        self.cancel_circles.clear();
        self.cancel_columns.clear();
        self.cancel_rects.clear();
        self.skills.retain_mut(|s| {
            let alive = s.tick(world, field, &mut self.cancel_circles, &mut self.damage_circles);
            if std::mem::take(&mut s.ended) {
                // FUN_0053b3a0: stop 0x31, then 0x2c panned to the side.
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x31, Some(crate::sfx::STOP))); self.sounds.push((0x2c, None)); }
            }
            alive
        });
        // The bomb button (FUN_00546320 states 1 and 4): pressing it starts the bomb, and during
        // the 8-frame hit window it also cancels the death (a deathbomb). Practice economy: no
        // stock is used and the meter counts as full (FUN_004ed6d0/FUN_004ed7d0 spend one bomb
        // and read the meter, both deferred).
        let bomb_pressed = keys & input::BOMB != 0 && !self.bomb_prev;
        self.bomb_prev = keys & input::BOMB != 0;
        if bomb_pressed && self.bomb.is_none() && matches!(self.state, State::Normal | State::Hit) {
            if self.state == State::Hit {
                self.state = State::Normal;
                self.state_time = 0;
            }
            let meter = if self.free_charge { crate::bomb::MAX_METER } else { self.meter };
            self.bomb = Some(if self.character == Character::Marisa {
                crate::bomb::Bomb::new_marisa(self.pos, meter, self.anm_file, world, lib, field)
            } else if matches!(self.character, Character::Sanae | Character::Ran) {
                crate::bomb::Bomb::new_character(self.character,self.pos,meter,self.anm_file,world,lib,field)
            } else {
                crate::bomb::Bomb::new(self.pos, meter, self.anm_file, world, lib, field)
            });
            // The bomb starts with sound 0x31 (all four characters' start functions).
            if crate::sfx::on(crate::sfx::Cat::Bomb) { self.sounds.push((0x31, Some(0))); }
        }
        if let Some(b) = &mut self.bomb {
            let protecting = b.protecting();
            if !b.tick_character(self.pos, world, field, &mut self.cancel_circles, &mut self.cancel_columns, &mut self.cancel_rects, &mut self.damage_circles) {
                // The update functions end by stopping 0x31 and playing 0x2c (centre pan).
                if crate::sfx::on(crate::sfx::Cat::Bomb) { self.sounds.push((0x31, Some(crate::sfx::STOP))); self.sounds.push((0x2c, Some(0))); }
                self.bomb = None;
            }
            if protecting {
                self.invuln = self.invuln.max(crate::bomb::PROTECTION);
            }
        }
        let mut damage = vec![(0i32, self.pos); enemies.len()];
        // 0054c2f0 copies shot motion with speed2; collider hit interval +a4 is4.
        for x in &mut self.snake_explosions {
            x.time+=1;x.pos[0]+=x.angle.cos()*2.0;x.pos[1]+=x.angle.sin()*2.0;x.radius+=x.growth;
            if x.time%4==0 {self.damage_circles.push(crate::skill::DamageCircle {pos:x.pos,radius:x.radius,
                damage:x.damage,remaining:1,growth:0.0,rect:None});}
        }
        self.snake_explosions.retain(|x|x.time<20);
        for circle in &mut self.damage_circles {
            for (index, enemy) in enemies.iter_mut().enumerate() {
                let d = &mut enemy.data;
                if d.delete || d.flags & flag::NO_HURTBOX != 0 || d.hurtbox[0] <= 0.0 { continue; }
                let hit = if let Some(size) = circle.rect {
                    (circle.pos[0] - d.pos[0]).abs() <= size[0] / 2.0 + d.hurtbox[0] / 2.0
                        && (circle.pos[1] - d.pos[1]).abs() <= size[1] / 2.0 + d.hurtbox[1] / 2.0
                } else {
                    let x = circle.pos[0].clamp(d.pos[0]-d.hurtbox[0]/2.0,d.pos[0]+d.hurtbox[0]/2.0);
                    let y = circle.pos[1].clamp(d.pos[1]-d.hurtbox[1]/2.0,d.pos[1]+d.hurtbox[1]/2.0);
                    dist2(circle.pos,[x,y]) <= circle.radius.powi(2)
                };
                if hit {
                    damage[index].0 += circle.damage;
                    damage[index].1 = circle.pos;
                }
            }
            circle.radius += circle.growth; circle.remaining -= 1;
        }
        self.damage_circles.retain(|c| c.remaining > 0);
        self.update_bullets(enemies, world, lib, field, &mut damage);
        self.update_beams(enemies, world, lib, field, &mut damage);
        // FUN00520450 caps the sum of colliders at player+2274, loaded from SHT+28.
        let cap = i32::from_le_bytes(self.sht.params_raw[8..12].try_into().unwrap()).max(0);
        for (enemy, (amount, from)) in enemies.iter_mut().zip(damage) {
            let amount = amount.min(cap);
            // FUN00509980: shooter states 0 (entering) and 2 (dead) divide damage by five.
            let amount = if matches!(self.state, State::Entering | State::Dead) {amount/5} else {amount};
            if amount > 0 { enemy.data.damage(amount, from); }
        }

        let at = field_to_canvas(field, self.pos);
        if let Some(vm) = self.body.and_then(|id| world.get_mut(id)) {
            vm.origin = [at[0], at[1], 0.0];
            // While invulnerable the body is tinted 0xff0000ff every third frame
            // (end of FUN_00546320).
            if self.invuln > 0 && self.state_time % 3 == 0 {
                vm.color2 = [0.0, 0.0, 255.0];
                vm.alpha2 = 255.0;
                vm.color_mode = 1;
            } else {
                vm.color_mode = 0;
            }
        }
    }

    /// Development aid: fill the gauge to `level`'s threshold so the next frame without
    /// the charge button releases it through the normal dispatch (same state checks and
    /// boss-to-EX fallback as a held charge).
    pub fn instant_charge(&mut self, level: crate::charge::Level) {
        use crate::charge::Level;
        if self.state != State::Normal {
            return;
        }
        let s = self.charge_settings;
        self.charge.fill(match level {
            Level::Attack => s.attack,
            Level::Skill => s.skill,
            Level::Extra => s.extra,
            Level::Boss => s.boss,
        });
    }

    /// `FUN_00546ed0`: while focus is held (and the player is alive) effect.anm script 19 (the
    /// spiked ring and red/white hit dot, spawning script 18) follows the player; releasing
    /// sends it interrupt 1.
    fn update_focus_marker(&mut self, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        let shown = self.focused && matches!(self.state, State::Normal | State::Entering | State::Knockback);
        let at = field_to_canvas(field, self.pos);
        if shown && self.focus_vm.is_none() {
            self.focus_vm = self.effect_anm.map(|file| world.spawn_clipped(lib, file, 19, at, field));
        } else if !shown {
            if let Some(id) = self.focus_vm.take() { world.interrupt(id, 1); }
        }
        if let Some(vm) = self.focus_vm.and_then(|id| world.get_mut(id)) {
            vm.origin = [at[0], at[1], 0.0];
        }
    }

    /// AuraCtrl release dispatch. Free-charge practice bypasses only the resource economy.
    fn update_charge(&mut self, keys: u32, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        let previous = self.charge.value;
        let released = self.charge.tick(keys & input::CHARGE != 0, self.state == State::Normal,
            if self.free_charge { 2500 } else { self.meter }, self.charge_settings, self.opponent_boss_active, false);
        if self.charge.value > 0 {
            if self.charge_vm.is_none() {
                self.charge_vm = self.aura_anm.map(|file| world.spawn_clipped(lib, file, 29,
                    field_to_canvas(field, self.pos), field));
            }
            for (threshold, interrupt) in [(self.charge_settings.attack,8),(self.charge_settings.skill,9),
                (self.charge_settings.extra,10),(self.charge_settings.boss,11)] {
                if previous < threshold && self.charge.value >= threshold {
                    if let Some(id) = self.charge_vm { world.interrupt(id, interrupt); }
                }
            }
        } else if let Some(id) = self.charge_vm.take() {
            world.delete(id);
        }
        if let Some(vm) = self.charge_vm.and_then(|id| world.get_mut(id)) {
            let at = field_to_canvas(field, self.pos);
            vm.origin = [at[0], at[1], 0.0];
        }
        if let Some(level) = released {
            use crate::charge::Level;
            self.last_charge = Some(level);
            self.charge_event = Some(level);
            // The basic local shot accompanies every higher-tier release.
            if !self.skill_volley { self.charge_timer = Some(0); }
            if !self.skill_volley || level==Level::Skill {self.charge_origin = self.pos;}
            let threshold = match level {
                Level::Attack => self.charge_settings.attack,
                Level::Skill => self.charge_settings.skill,
                Level::Extra => self.charge_settings.extra,
                Level::Boss => self.charge_settings.boss,
            };
            if !self.free_charge { self.meter = (self.meter-threshold+self.charge_settings.attack).clamp(0,2500); }
            if level == Level::Skill && self.character == Character::Marisa {
                // FUN_00417b90: sound 0x33, panned to the player's side.
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x33, None)); }
                self.wide_charge = true;
            }
            if level == Level::Skill && self.character == Character::Reimu {
                // APPROX: deterministic local RNG instead of the game's replay RNG.
                self.skill_rng ^= self.skill_rng << 13; self.skill_rng ^= self.skill_rng >> 17; self.skill_rng ^= self.skill_rng << 5;
                let phase = (self.skill_rng as f64/u32::MAX as f64) as f32*std::f32::consts::TAU-std::f32::consts::PI;
                // FUN_0053b920 (skill init) plays 0x31 at centre pan.
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x31, Some(0))); }
                self.skills.push(crate::skill::Skill::new(self.pos,phase,self.anm_file,world,lib,field));
                self.invuln = self.invuln.max(40);
            }
            if level == Level::Skill && self.character == Character::Sanae {
                self.skill_volley = true; self.charge_timer = Some(0);
            }
            if level == Level::Skill && self.character == Character::Ran && self.boost_frames == 0 {
                // EXRanInf00419ae0: power400 for600 ticks, then restore200.
                // FUN_00419ae0: sound 0xd, panned to the player's side.
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0xd, None)); }
                self.power = 400; self.boost_frames = 600;
                self.update_options(world,lib,field);
            }
            if matches!(level, Level::Extra | Level::Boss) { self.barrier = Barrier::Up; }
        }
        if let Some(t) = self.charge_timer {
            let (slot, fire_frames, stop) = match self.character {
                Character::Sanae => (if self.skill_volley {11} else {10}, if self.skill_volley {60} else {30}, if self.skill_volley {70} else {40}),
                Character::Ran => (if self.boost_frames>0 {11} else {10},50,61),
                _ => (10,21,21),
            };
            if t < fire_frames { self.fire_slot(slot, t, 0, world, lib, field); }
            if t == 0 && std::mem::take(&mut self.wide_charge) { self.fire_slot(11,t,0,world,lib,field); }
            self.charge_timer = (t+1 < stop).then_some(t+1);
            if self.charge_timer.is_none() { self.skill_volley=false; }
        }
    }

    /// Native option update (`FUN_00546d10`) with the per-character callback installed by
    /// `FUN_005483d0`. Ran (character 3) installs `FUN_0054a120`: unfocused, the player position is
    /// stored (+0x6a0/+0x6a4) every tick; focused, the target is the focused slot offset added to
    /// that stored position, so the orbs stay where the last unfocused tick left them. Everyone
    /// else's target is the player position plus the slot offset.
    fn update_options(&mut self, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        let offsets: Vec<[f32; 2]> = self.option_offsets().to_vec();
        let latching = self.character == Character::Ran;
        if !self.focused {
            self.option_anchor = self.fixed;
        }
        if self.options.len() != offsets.len() {
            // `FUN_005483d0` rebuilds every option when the count changes; each starts on its
            // slot relative to the live player position (focused offsets when focus is held).
            for o in self.options.drain(..) {
                for id in [o.vm, o.extra].into_iter().flatten() {
                    world.delete(id);
                }
            }
            for off in &offsets {
                let at = [self.fixed[0] + (off[0] * 128.0) as i32, self.fixed[1] + (off[1] * 128.0) as i32];
                let script = self.sht.option_script;
                let canvas = field_to_canvas(field, [at[0] as f32 / 128.0, at[1] as f32 / 128.0]);
                let vm = (script >= 0).then(|| world.spawn_clipped(lib, self.anm_file, script as usize, canvas, field));
                self.options.push(PlayerOption { pos: at, vm, extra: None, hold: if latching { 10 } else { 0 } });
            }
        }
        // FUN_005483d0: at full power (400) every option carries the SHT +0x40 script; below it
        // they are removed.
        let full = self.power >= 400;
        for o in &mut self.options {
            if !full {
                if let Some(id) = o.extra.take() { world.interrupt(id, 1); }
            } else if o.extra.is_none() && self.sht.option_script2 >= 0 {
                let canvas = field_to_canvas(field, [o.pos[0] as f32 / 128.0, o.pos[1] as f32 / 128.0]);
                o.extra = Some(world.spawn_clipped(lib, self.anm_file, self.sht.option_script2 as usize, canvas, field));
            }
        }
        let anchor = if latching { self.option_anchor } else { self.fixed };
        for (o, off) in self.options.iter_mut().zip(&offsets) {
            let target = [anchor[0] + (off[0] * 128.0) as i32, anchor[1] + (off[1] * 128.0) as i32];
            if o.hold == 0 {
                // Options close 30% of the distance to their slot every frame.
                let d = [(target[0] - o.pos[0]) * 30 / 100, (target[1] - o.pos[1]) * 30 / 100];
                if d == [0, 0] {
                    o.pos = target;
                } else {
                    o.pos = [o.pos[0] + d[0], o.pos[1] + d[1]];
                }
            } else {
                o.hold -= 1;
                o.pos = target;
            }
            let p = field_to_canvas(field, [o.pos[0] as f32 / 128.0, o.pos[1] as f32 / 128.0]);
            for id in [o.vm, o.extra].into_iter().flatten() {
                if let Some(vm) = world.get_mut(id) {
                    vm.origin = [p[0], p[1], 0.0];
                }
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
        self.fire_slot(slot, timer, long, world, lib, field);
    }

    fn fire_slot(&mut self, slot: usize, timer: i32, long: i32, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        let Some(Some(records)) = self.sht.slots.get(slot) else { return };
        let records: Vec<ShotRecord> = records.clone();
        for r in &records {
            if r.interval < 0 { break; }
            if r.kind == 2 && self.beam_groups.get(r.group.max(0) as usize).copied().unwrap_or(false) {
                continue;
            }
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

    /// Where a record's origin nibble points: the player, the field origin or an option.
    fn origin_of(&self, nibble: u8) -> Option<[f32; 2]> {
        match nibble & 0xf {
            // 0054f990 passes current player+68c for every slot; origin5 reads+c18.
            0 => Some(self.pos),
            5 => Some(self.charge_origin),
            6 => Some([0.0, 0.0]),
            k => self.options.get(k as usize - 1).map(|o| [o.pos[0] as f32 / 128.0, o.pos[1] as f32 / 128.0]),
        }
    }

    fn spawn_bullet(&mut self, r: &ShotRecord, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        let Some(origin) = self.origin_of(r.origin) else { return };
        // FUN_0054e960 runs the record's init callback (SHT +0x48 through 0x5a0440; all return 0),
        // then plays the record's sound (SHT +0x2c) panned by the shot's x: origin + offset, one
        // velocity step back unless the kind is 4..6. Init callbacks 2, 12 and 18 (0054d9e0,
        // 0054ccf0, 0054d960) play 0x14 panned by the player's x.
        if matches!(r.funcs[0], 2 | 12 | 18) && crate::sfx::on(crate::sfx::Cat::BeamStart) {
            self.sounds.push((0x14, Some((self.pos[0] * 1000.0 / 192.0) as i32)));
        }
        if r.sound >= 0 && crate::sfx::on(crate::sfx::Cat::PlayerShot) {
            let back = if matches!(r.kind, 4..=6) { 0.0 } else { r.angle.cos() * r.speed };
            self.sounds.push((r.sound as u32, Some(((origin[0] + r.offset[0] - back) * 1000.0 / 192.0) as i32)));
        }
        if r.kind == 2 || r.funcs[0] == 12 {
            let pos = [origin[0] + r.offset[0], origin[1] + r.offset[1]];
            self.beams.push(crate::beam::Beam::new(r, self.anm_file, world, lib, field, pos));
            if r.kind == 2 {
                if let Some(g) = self.beam_groups.get_mut(r.group.max(0) as usize) { *g = true; }
            }
            return;
        }
        let (s, c) = r.angle.sin_cos();
        let vel = [c * r.speed, s * r.speed];
        // The game places the bullet one step back, so its first update lands on the offset.
        let back = if matches!(r.kind, 4..=6) { [0.0; 2] } else { vel };
        let pos = [origin[0] + r.offset[0] - back[0], origin[1] + r.offset[1] - back[1]];
        let vm = (r.anm_script >= 0).then(|| {
            let id = world.spawn_clipped(lib, self.anm_file, r.anm_script as usize, field_to_canvas(field, pos), field);
            if let Some(v) = world.get_mut(id) {
                // Charge orbs rotate through script 10's angular velocity. Their ANM
                // does not request direction-following rotation (0054df10, +0x4cc).
                if v.auto_rotate { v.rot[2] = r.angle; }
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
            homing_target: None,
            angular_velocity: r.angular_velocity,
            acceleration: r.acceleration,
            vm,
            time: 0,
            charge_shot: r.funcs == [11, 8, 0, 7],
            impacting: false,
            hit_callback: r.funcs[3], snake_turn: None, snake: r.funcs[1]==4,
            explosion_growth: f32::from_le_bytes(r.raw[0x68..0x6c].try_into().unwrap()),
        });
    }

    /// Marisa's beams (`FUN_0054d370` / `FUN_0054cd70`): grow, hit, and end with the shot button.
    fn update_beams(&mut self, enemies: &[Enemy], world: &mut World, lib: &AnmLibrary, field: [f32; 4], damage: &mut [(i32, [f32; 2])]) {
        let keep = self.shot_timer.cur >= 0
            && !matches!(self.state, State::Dead | State::Hit | State::GameOver)
            && self.charge.value < 1
            && self.bomb.is_none();
        let mut beams = std::mem::take(&mut self.beams);
        let mut hits = Vec::new();
        for b in &mut beams {
            let origin = self.origin_of(b.origin()).unwrap_or(self.pos);
            let alive = if b.is_charge() { !matches!(self.state, State::GameOver) } else { keep };
            b.update(origin, alive, field, world);
            b.collide(enemies, world, lib, field, self.anm_file, &mut hits);
            if b.ended && !b.is_charge() { if let Some(g) = self.beam_groups.get_mut(b.group.max(0) as usize) { *g = false; } }
        }
        for (index, amount, from) in hits {
            damage[index].0 += amount;
            damage[index].1 = from;
        }
        beams.retain_mut(|b| {
            let alive = !b.ended || b.faded(world);
            if !alive { b.delete(world); }
            alive
        });
        self.beams = beams;
    }

    fn update_bullets(&mut self, enemies: &mut [Enemy], world: &mut World, lib: &AnmLibrary, field: [f32; 4], damage: &mut [(i32, [f32; 2])]) {
        let mut i = 0;
        while i < self.bullets.len() {
            let b = &mut self.bullets[i];
            // Callback 8 at 0054b380, read from the original executable's table
            // 005a0400 (not present as a function in the Ghidra export).
            if !b.impacting && b.charge_shot {
                b.speed = if b.time < 120 { (b.speed - 0.95).max(0.2) } else { b.speed + 0.2 };
            }
            if !b.impacting && b.homing {
                update_homing(b, enemies);
            }
            if !b.impacting && b.snake { update_snake(b,enemies,world); }
            // FUN_0054df10: callback, then SHT angular/speed increments, then movement.
            if !b.impacting {
                b.angle = crate::motion::wrap_angle(b.angle + b.angular_velocity);
                b.speed += b.acceleration;
            }
            b.time += 1;
            let (s, c) = b.angle.sin_cos();
            b.pos = [b.pos[0] + c * b.speed, b.pos[1] + s * b.speed];

            let mut dead = b.pos[0].abs() > 148.0 + 32.0 || b.pos[1] < -32.0 || b.pos[1] > 448.0 + 32.0;
            dead |= b.vm.is_some_and(|id| world.get(id).is_none());
            if !dead && !b.impacting {
                for (index, e) in enemies.iter_mut().enumerate() {
                    let d = &mut e.data;
                    if d.flags & flag::NO_HURTBOX != 0 || d.hurtbox[0] <= 0.0 || d.delete {
                        continue;
                    }
                    let overlap = shot_hits(b.pos, b.hitbox, b.angle, d);
                    if overlap {
                        // A collider remembers the last enemy it hit (+0xa8) only to avoid a
                        // second hit within one frame: the per-frame collider tick (0051fa30)
                        // clears it, and the hit interval (+0xa4) is 1. A piercing charge
                        // orb (callback 7 leaves it alive, 0054fba0) therefore hits every
                        // overlapping enemy on every frame.
                        damage[index].0 += b.damage;
                        damage[index].1 = b.pos;
                        if !b.charge_shot {
                            // Hit callbacks 3, 4 and 9 (0054c5f0, 0054c2f0, 0054a7b0) end with sound 0x41,
                            // panned by the shot's x.
                            if matches!(b.hit_callback, 3 | 4 | 9) {
                                if crate::sfx::on(crate::sfx::Cat::ChargeHit) { self.sounds.push((0x41, Some((b.pos[0] * 1000.0 / 192.0) as i32))); }
                            }
                            // Callback 1 adds effect 142 before the shared impact transition.
                            if b.hit_callback == 1 || b.hit_callback == 5 {
                                if let Some(file) = self.effect_anm {
                                    let mut random = || {
                                        let x = &mut self.skill_rng;
                                        *x ^= *x << 13; *x ^= *x >> 17; *x ^= *x << 5;
                                        *x
                                    };
                                    // Original replay RNG is not yet shared by this controller.
                                    let spread = random() as f64 / u32::MAX as f64 * 2.0 - 1.0;
                                    let angle = crate::motion::wrap_angle(b.angle + spread as f32 * 0.34906584 + std::f32::consts::PI);
                                    let id = world.spawn_rotated(lib, file, 142, field_to_canvas(field, b.pos), field, angle);
                                    if let Some(vm) = world.get_mut(id) {
                                        vm.color = [(random() & 127) as f32 + 127.0, (random() & 63) as f32 + 64.0, (random() & 63) as f32 + 64.0];
                                        vm.alpha = (random() & 63) as f32 + 96.0;
                                    }
                                }
                            }
                            if b.hit_callback == 4 {
                                //0054c2f0: replace the shot collider with radius24,+SHT68/frame,life20.
                                self.snake_explosions.push(SnakeExplosion {pos:b.pos,angle:b.angle,radius:24.0,
                                    damage:b.damage,time:0,growth:b.explosion_growth});
                            }
                            b.impacting = true;
                            b.speed = if b.hit_callback==4 {2.0} else {b.speed/8.0};
                            if let Some(id) = b.vm { world.interrupt(id, 1); }
                            else { dead = true; }
                            break;
                        }
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
                vm.origin = [at[0], at[1], if b.impacting { 0.1 } else { 0.0 }];
                if vm.auto_rotate { vm.rot[2] = b.angle; }
            }
            i += 1;
        }
    }
}

/// Sanae callback4 (0054c080): detect a body within +/-16y and outside +/-16x;
/// interrupt2, stop for four ticks, then head horizontally toward its sampled x at14.
fn update_snake(b: &mut PlayerBullet, enemies: &[Enemy], world: &mut World) {
    if let Some((ticks,x))=b.snake_turn {
        let ticks=ticks+1;
        if ticks==4 {
            b.angle=if b.pos[0]>x {std::f32::consts::PI} else {0.0};
            b.speed=14.0;b.snake=false;
        }
        b.snake_turn=Some((ticks,x));
        return;
    }
    if let Some(e)=enemies.iter().find(|e| !e.data.delete && !e.data.killed
        && e.data.flags & (flag::NO_HURTBOX|flag::HIDDEN)==0
        && (b.pos[1]-e.data.pos[1]).abs()<=16.0 && (b.pos[0]-e.data.pos[0]).abs()>=16.0) {
        b.speed=0.0; b.snake_turn=Some((1,e.data.pos[0]));
        if let Some(id)=b.vm {world.interrupt(id,2);}
    }
}

/// Shot callback 1 (0054dc40), nearest-target query00508530, initializer0054def0.
fn update_homing(b: &mut PlayerBullet, enemies: &[Enemy]) {
    // Original additionally tests internal status bits 0xc00, which are not modeled
    // individually yet. Deleted/killed enemies cannot be retained by this runtime.
    let valid = |e: &&Enemy| e.data.flags & (flag::NO_HURTBOX | flag::HIDDEN) == 0
        && !e.data.delete && !e.data.killed;
    if b.homing_target.is_none() {
        let mut distance = 256.0 * 256.0;
        for e in enemies.iter().filter(valid) {
            let d = dist2(e.data.pos, b.pos);
            if d < distance {
                distance = d;
                b.homing_target = Some(e.data.id);
            }
        }
    }
    let target = b.homing_target.and_then(|id| enemies.iter().find(|e| e.data.id == id).filter(valid));
    if let Some(target) = target {
        if b.time < 60 {
            let want = (target.data.pos[1] - b.pos[1]).atan2(target.data.pos[0] - b.pos[0]);
            let diff = crate::motion::wrap_angle(want - b.angle);
            if diff.abs() >= std::f32::consts::FRAC_PI_4 {
                b.speed = (b.speed - 0.2).max(4.0);
            } else if diff.abs() < std::f32::consts::PI / 12.0 {
                b.speed = (b.speed + 0.2).min(16.0);
            }
            b.angle = crate::motion::wrap_angle(b.angle + diff * 0.08);
        } else {
            // After 60 ticks, the locked shot travels straight and accelerates uncapped.
            b.speed += 0.2;
        }
    } else {
        b.homing_target = None;
        b.speed = (b.speed + 0.1).min(16.0);
    }
}

/// Shot collider against an enemy (FUN_00509980 -> FUN_00520450 -> FUN_0051e670). The
/// shot is a rectangle rotated to its angle, `size[0]` along its direction (collider
/// +0x2c/+0x30 from record +0xc/+0x10). Enemies are a circle of radius `hurtbox[0] / 2`
/// unless flag 0x1000 selects their rectangle (FUN_004d44e0 / FUN_004d2c30).
pub(crate) fn shot_hits(pos: [f32; 2], size: [f32; 2], angle: f32, d: &crate::enemy::EnemyData) -> bool {
    if d.flags & 0x1000 != 0 {
        // APPROX: the rectangle-rectangle test ignores the shot's rotation.
        return (pos[0] - d.pos[0]).abs() * 2.0 < size[0] + d.hurtbox[0]
            && (pos[1] - d.pos[1]).abs() * 2.0 < size[1] + d.hurtbox[1];
    }
    let r = d.hurtbox[0] / 2.0;
    let (s, c) = angle.sin_cos();
    let (dx, dy) = (d.pos[0] - pos[0], d.pos[1] - pos[1]);
    // The enemy centre in the shot's frame, then its distance to the rectangle.
    let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
    let (eu, ev) = ((u.abs() - size[0] / 2.0).max(0.0), (v.abs() - size[1] / 2.0).max(0.0));
    eu * eu + ev * ev < r * r || (eu == 0.0 && ev <= r) || (ev == 0.0 && eu <= r)
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

#[cfg(test)]
mod tests {
use super::*;

    #[test]
    fn snake_turns_on_the_fourth_native_tick_and_keeps_the_sampled_target() {
        let mut b=amulet(-std::f32::consts::FRAC_PI_2,8.0,0);b.snake=true;b.homing=false;
        let mut w=World::default();
        let enemies=[homing_enemy(1,[80.0,300.0])];
        update_snake(&mut b,&enemies,&mut w);assert_eq!(b.speed,0.0);
        update_snake(&mut b,&[],&mut w);assert_eq!(b.speed,0.0);
        update_snake(&mut b,&[],&mut w);assert_eq!(b.speed,0.0);
        update_snake(&mut b,&[],&mut w);assert_eq!(b.speed,14.0);assert_eq!(b.angle,0.0);
        assert!(!b.snake);
    }

    #[test]
    fn new_character_charge_records_keep_current_and_captured_origins_distinct() {
        let path=std::path::PathBuf::from(std::env::var_os("TH19_DAT").unwrap_or_else(||
            std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("Downloads/th19/th19/th19.dat").into_os_string()));
        if !path.is_file() {return;}
        let mut dat=th_formats::dat::Archive::open(path).unwrap();
        for character in [Character::Sanae,Character::Ran] {
            let sht=th_formats::sht::ShtFile::parse(&dat.read(&format!("{}.sht",character.prefix())).unwrap()).unwrap();
            let mut p=Player::new(sht,0);p.character=character;
            p.pos=[50.0,400.0];p.charge_origin=[-20.0,400.0];
            assert_eq!(p.origin_of(0),Some([50.0,400.0]));
            assert_eq!(p.origin_of(5),Some([-20.0,400.0]));
        }
    }

    /// Ran's options (`FUN_0054a120`) hold the last unfocused position while Shift is down and
    /// shots leave from them; other characters' options keep following the player.
    #[test]
    fn ran_options_latch_while_focused_and_shots_follow_them() {
        let path=std::path::PathBuf::from(std::env::var_os("TH19_DAT").unwrap_or_else(||
            std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("Downloads/th19/th19/th19.dat").into_os_string()));
        if !path.is_file() {return;}
        let mut dat=th_formats::dat::Archive::open(path).unwrap();
        let (mut world,lib,field)=(World::default(),AnmLibrary::default(),[0.0,0.0,384.0,448.0]);
        for character in [Character::Ran,Character::Reimu] {
            let mut sht=th_formats::sht::ShtFile::parse(&dat.read(&format!("{}.sht",character.prefix())).unwrap()).unwrap();
            sht.option_script=-1;
            let mut p=Player::new(sht,0);p.character=character;p.state=State::Normal;p.power=200;
            for _ in 0..30 {p.update(0,&mut [],&mut world,&lib,field);}
            let free=p.options.iter().map(|o|o.pos).collect::<Vec<_>>();
            let off=p.sht.options[0][2];
            // Unfocused: slot offsets are relative to the player.
            assert_eq!(free[1],[p.fixed[0]+(off[0]*128.0) as i32,p.fixed[1]+(off[1]*128.0) as i32]);
            let held_at=p.fixed;
            // Hold Shift and move left; Ran's options stay on the latched anchor.
            for _ in 0..40 {p.update(input::FOCUS|input::LEFT,&mut [],&mut world,&lib,field);}
            assert!(p.fixed[0]<held_at[0]);
            let foc=p.sht.options[1][2];
            let want=if character==Character::Ran {
                [held_at[0]+(foc[0]*128.0) as i32,held_at[1]+(foc[1]*128.0) as i32]
            } else {
                [p.fixed[0]+(foc[0]*128.0) as i32,p.fixed[1]+(foc[1]*128.0) as i32]
            };
            let got=p.options[1].pos;
            if character==Character::Ran {
                assert!((got[0]-want[0]).abs()<=1&&(got[1]-want[1]).abs()<=1,"{character:?} {got:?} {want:?}");
            } else {
                // Still trailing the moving player by the 30% ease, not held in place.
                assert!(got[0]<held_at[0]+(foc[0]*128.0) as i32-1000,"{got:?}");
            }
            assert_eq!(p.origin_of(2),Some([got[0] as f32/128.0,got[1] as f32/128.0]));
            // Release: the options ease back (30% a tick) to the live player slots.
            for _ in 0..60 {p.update(0,&mut [],&mut world,&lib,field);}
            let back=p.options[1].pos;let free_off=p.sht.options[0][2];
            assert_eq!(back,[p.fixed[0]+(free_off[0]*128.0) as i32,p.fixed[1]+(free_off[1]*128.0) as i32]);
        }
    }

    fn amulet(angle: f32, speed: f32, time: i32) -> PlayerBullet {
        PlayerBullet { pos: [0.0, 300.0], angle, speed, hitbox: [4.0; 2], damage: 1,
            homing: true, homing_target: None, angular_velocity: 0.0, acceleration: 0.0,
            vm: None, time, charge_shot: false, impacting: false, hit_callback: 0, snake_turn: None, snake: false, explosion_growth: 0.0 }
    }

    fn homing_enemy(id: i32, pos: [f32; 2]) -> Enemy {
        Enemy { threads: th_ecl::Threads::new(th_ecl::Context::new(th_ecl::SubRef { file: 0, sub: 0 })),
            data: crate::EnemyData { id, pos, hurtbox: [10.0; 2], hp: 100, ..Default::default() } }
    }

    #[test]
    fn original_shot_impacts_keep_animation_stop_damage_and_expire() {
        let path=std::path::PathBuf::from(std::env::var_os("TH19_DAT").unwrap_or_else(||
            std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("Downloads/th19/th19/th19.dat").into_os_string()));
        if !path.is_file() { return; }
        let mut dat=th_formats::dat::Archive::open(path).unwrap();
        let mut lib=AnmLibrary::default();
        let file=lib.load("pl00.anm",&dat.read("pl00.anm").unwrap()).unwrap();
        let effect=lib.load("effect.anm",&dat.read("effect.anm").unwrap()).unwrap();
        let sht=ShtFile::parse(&dat.read("pl00.sht").unwrap()).unwrap();
        for (script,impact_sprite) in [(6,26),(7,29),(8,35)] {
            let mut world=World::default();
            let mut p=Player::new(sht.clone(),file); p.pos=[0.0,100.0]; p.effect_anm=Some(effect);
            let mut record=sht.slots.iter().flatten().flatten().find(|r| r.anm_script==script).unwrap().clone();
            record.origin=0; record.offset=[0.0;2];
            p.spawn_bullet(&record,&mut world,&lib,crate::FIELDS[0]);
            let id=p.bullets[0].vm.unwrap();
            let mut enemies=[homing_enemy(1,p.pos)]; enemies[0].data.hurtbox=[100.0;2];
            let mut damage=[(0,[0.0;2])];
            p.update_bullets(&mut enemies,&mut world,&lib,crate::FIELDS[0],&mut damage);
            assert_eq!(damage[0].0,record.damage as i32);
            assert!(p.bullets[0].impacting);
            assert!(p.bullets[0].speed < record.speed / 4.0);
            world.tick(&lib);
            assert_eq!(world.get(id).unwrap().sprite,impact_sprite);
            for _ in 0..40 {
                damage[0].0=0;
                p.update_bullets(&mut enemies,&mut world,&lib,crate::FIELDS[0],&mut damage);
                assert_eq!(damage[0].0,0,"impact has no collider");
                world.tick(&lib);
            }
            assert!(world.get(id).is_none()); assert!(p.bullets.is_empty());
        }
    }

    #[test]
    fn amulet_slows_on_sharp_turn_and_accelerates_only_when_aligned() {
        let enemy = [homing_enemy(7, [100.0,300.0])];
        let mut b = amulet(-std::f32::consts::FRAC_PI_2, 8.0, 0);
        update_homing(&mut b, &enemy);
        assert!((b.speed - 7.8).abs() < 0.001);
        assert!((b.angle - (-std::f32::consts::FRAC_PI_2 * 0.92)).abs() < 0.001);
        b.angle = -0.4; b.speed = 8.0;
        update_homing(&mut b, &enemy);
        assert_eq!(b.speed, 8.0, "15-45 degree errors preserve speed");
        b.angle = -0.1;
        update_homing(&mut b, &enemy);
        assert!((b.speed - 8.2).abs() < 0.001);
        b.angle = -std::f32::consts::FRAC_PI_2; b.speed = 4.0;
        update_homing(&mut b, &enemy);
        assert_eq!(b.speed, 4.0);
    }

    #[test]
    fn amulet_keeps_lock_and_reacquires_after_loss_within_256_pixels() {
        let mut enemies = [homing_enemy(1,[0.0,200.0]),homing_enemy(2,[100.0,300.0])];
        enemies[1].data.flags = flag::HIDDEN;
        let mut b = amulet(-std::f32::consts::FRAC_PI_2,8.0,0);
        update_homing(&mut b, &enemies);
        assert_eq!(b.homing_target,Some(1));
        enemies[1].data.flags = 0;
        enemies[1].data.pos = [20.0,300.0];
        enemies[0].data.pos = [0.0,0.0];
        update_homing(&mut b, &enemies);
        assert_eq!(b.homing_target,Some(1),"retained target may leave acquisition radius");
        enemies[0].data.delete = true;
        let speed = b.speed;
        update_homing(&mut b, &enemies);
        assert_eq!(b.homing_target,None,"target loss waits until next tick to reacquire");
        assert!((b.speed-speed-0.1).abs() < 0.001);
        update_homing(&mut b, &enemies);
        assert_eq!(b.homing_target,Some(2));
        enemies[1].data.pos = [0.0,44.0]; b.homing_target = None;
        update_homing(&mut b, &enemies);
        assert_eq!(b.homing_target,None,"exactly 256 pixels is excluded");
    }

    #[test]
    fn amulet_stops_steering_at_tick_60_and_unlocked_acceleration_is_gradual() {
        let mut b = amulet(-1.0,2.0,0);
        for _ in 0..10 { update_homing(&mut b, &[]); }
        assert!((b.speed-3.0).abs() < 0.001);
        assert_eq!(b.angle,-1.0);
        b.speed = 16.0;
        update_homing(&mut b, &[]);
        assert_eq!(b.speed,16.0);
        b.time = 60;
        update_homing(&mut b, &[homing_enemy(1,[100.0,300.0])]);
        assert_eq!(b.angle,-1.0);
        assert!((b.speed-16.2).abs() < 0.001,"locked late acceleration has no 16 cap");
    }
#[test]
fn combined_player_colliders_obey_original_damage_cap() {
    let path=std::path::PathBuf::from(std::env::var_os("TH19_DAT").unwrap_or_else(||
        std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("Downloads/th19/th19/th19.dat").into_os_string()));
    if !path.is_file() {return;}
    let mut dat=th_formats::dat::Archive::open(path).unwrap();
    let mut lib=AnmLibrary::default();
    let file=lib.load("pl00.anm",&dat.read("pl00.anm").unwrap()).unwrap();
    let mut world=World::default();
    let sht=ShtFile::parse(&dat.read("pl00.sht").unwrap()).unwrap();
    let mut p=Player::new(sht,file);p.state=State::Normal;
    assert_eq!(i32::from_le_bytes(p.sht.params_raw[8..12].try_into().unwrap()),90);
    // Three skill colliders total 120 this frame; the original caps their sum at 90.
    p.damage_circles=(0..3).map(|_|crate::skill::DamageCircle {
        pos:p.pos,radius:10.0,growth:0.0,damage:40,remaining:1,rect:None
    }).collect();
    let mut enemies=[crate::Enemy {
        threads:th_ecl::Threads::new(th_ecl::Context::new(th_ecl::SubRef{file:0,sub:0})),
        data:crate::EnemyData {id:100,pos:p.pos,hurtbox:[10.0;2],hp:5000,..Default::default()}
    }];
    p.update(0,&mut enemies,&mut world,&lib,crate::FIELDS[0]);
    assert_eq!(enemies[0].data.hp,4910);
    enemies[0].data.hp=1500; enemies[0].data.spell_hp7=Some(10500);
    p.damage_circles=(0..3).map(|_|crate::skill::DamageCircle {
        pos:p.pos,radius:10.0,growth:0.0,damage:40,remaining:1,rect:None
    }).collect();
    p.update(0,&mut enemies,&mut world,&lib,crate::FIELDS[0]);
    assert_eq!(enemies[0].data.spell_hp7,Some(10410));
    assert_eq!(enemies[0].data.hp,1487);
    p.state=State::Entering;
    enemies[0].data.hp=5000;enemies[0].data.spell_hp7=None;
    p.damage_circles=(0..3).map(|_|crate::skill::DamageCircle {
        pos:p.pos,radius:10.0,growth:0.0,damage:40,remaining:1,rect:None
    }).collect();
    p.update(0,&mut enemies,&mut world,&lib,crate::FIELDS[0]);
    assert_eq!(enemies[0].data.hp,4982,"Entering divides the capped 90 damage by five");
}

}
