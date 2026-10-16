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
    /// Seiran's orbit angle (option +0xd4, index * pi at creation) and whether the last
    /// target wrapped across the field (+0x104 bit 1).
    orbit: f32,
    wrapped: bool,
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
    /// SHT update callback index (+0x4c of the record, table 0x5a0400).
    update_callback: i32,
    /// Shot +0x114: the default hit callback (`FUN_0054fba0`) leaves the shot alive.
    pub pierce: bool,
    /// SHT +0x34: at this age the VM gets interrupt 1 and the age jumps to -999
    /// (`FUN_0054df10`); 999 for most records.
    lifetime: i32,
    /// Kinds 2 and 8 skip `FUN_0054df10`'s off-field check.
    stays_off_field: bool,
    /// Orin's wander (+0xb8 angle step, +0xbc speed step; `FUN_0054b0c0`).
    drift: [f32; 2],
    /// Orin's charge column: the collider trails the shot as a (length, 28) box
    /// (`FUN_0054adc0`).
    column: Option<f32>,
    /// The motion's velocity from the last movement step (+0x8c), the record's at spawn.
    velocity: [f32; 2],
}

/// A shot's impact collider (`FUN_005200f0`): a circle that moves with the shot's motion,
/// grows each frame and hits every `interval` frames until `life`.
struct SnakeExplosion {pos:[f32;2],angle:f32,speed:f32,radius:f32,growth:f32,damage:i32,time:i32,interval:i32,life:i32}

/// Nazrin's skill (`FUN_0041b420`): pl05.anm script 9 at the cast position; its X scale is the
/// radius of a collider (flags 0x2c0) that kills disabled spirits until the VM ends.
struct MarkCircle { pos: [f32; 2], vm: VmId }

/// Yachie's charge areas (EX class +0x8c timer, +0x110 skill flag; `FUN_004222d0`): a hexagon
/// (a vertex at angle 0) where she released, radius 1 -> 48 over 10 frames (the basic one,
/// `FUN_00421f80`, script 8) or 1 -> 128 over 60 (the skill, `FUN_00422150`, script 10), mode 4.
/// While its timer is below 40 (110) it cancels bullets and lasers and hits for 1 every frame;
/// it ends once the timer passes 40 (110).
struct HexArea { pos: [f32; 2], skill: bool, time: i32, growth: th_anm::interp::Interp<1>, radius: f32 }

/// Tsukasa's skill (`FUN_0041e5e0`, EX [11] `FUN_0041e750`): scripts 13 and 12 where she cast it;
/// for 170 frames a radius 20 -> 420 (mode 4) scales script 12 and is a one-frame explosion
/// (`FUN_0051d4e0(.., 1, 0, 2, 1)`: no damage, no kill credit) that cancels small bullets into
/// attacks for the opponent; then both VMs are deleted.
struct SweepCircle { pos: [f32; 2], frames: i32, growth: th_anm::interp::Interp<1>, vms: [VmId; 2] }
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
    /// APPROX (sequencing): the player's draws use their own copy of the replay RNG (`rng.rs`).
    skill_rng: crate::rng::Rng,
    charge_timer: Option<i32>,
    /// Marisa: a skill release also fires the two side charge beams (SHT slot 11).
    wide_charge: bool,
    charge_origin: [f32;2],
    skill_volley: bool,
    boost_frames: i32,
    /// Orin's skill arms her next charge shot with SHT slot 11 (EX class +0xf0, `FUN_0041d020`).
    orin_armed: bool,
    nazrin_skill: Option<MarkCircle>,
    yachie_area: Option<HexArea>,
    tsukasa_skill: Option<SweepCircle>,
    /// Mamizou's skill burst this frame (cancel radius 40, collider radius 40 hitting for 20).
    mamizou_burst: Option<[f32; 2]>,
    /// Bomb fragments (status +0x6c): three make a bomb (`FUN_00537b90`).
    pub bomb_fragments: i32,
    /// One-frame explosions (centre, radius) for the side's explosion list (Tsukasa's skill).
    pub explosion_requests: Vec<([f32; 2], f32)>,
    /// Yachie's hexagons (centre, radius, angle, sides) that cancel bullets (`FUN_004ee640`) and
    /// lasers (`FUN_00530320`).
    pub cancel_polygons: Vec<([f32; 2], f32, f32, u32)>,
    /// Set the frame a bomb's duration ran out; the side turns it into a screen shake.
    pub bomb_shake: bool,
    /// Positions of the enemies a shot's init callback may aim at (refreshed every update;
    /// `FUN_00508530` skips flags 0x21).
    targets: Vec<[f32; 2]>,
    pub cancel_rects: Vec<([f32;2],[f32;2])>,
    /// Aunn's bomb: ellipses (centre, axis along the angle, axis across, angle) that cancel
    /// bullets (`FUN_004ee500`).
    pub cancel_ellipses: Vec<([f32;2], f32, f32, f32)>,
    /// Orin's bomb: oriented boxes (centre, size along / across, angle) that cancel bullets on
    /// the field (`FUN_004ee0d0`) and lasers (`FUN_005303f0`).
    pub cancel_orects: Vec<([f32;2], [f32;2], f32)>,
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
    /// Bomb stock (status +0x68, at most +0x74 = 7): three at the start of every round
    /// (`FUN_005177a0`), one spent per bomb (`FUN_004ed7d0`).
    pub bombs: i32,
    pub max_bombs: i32,
    /// Hearts lost this round (status +0x64, counted by `FUN_00549830`).
    pub deaths: u32,
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
/// Bombs at the start of each round and the stock cap (status +0x68 = 3, +0x74 = 7 in
/// `FUN_005177a0`).
pub const START_BOMBS: i32 = 3;
pub const MAX_BOMBS: i32 = 7;

impl Player {
    pub fn new(sht: ShtFile, anm_file: usize) -> Self {
        // Normal power is 200 (two options); the game raises it to 400 only for a timed
        // boost and drops back to 200 afterwards (FUN_00419ae0).
        let power = 200.min(sht.power_levels * 100).clamp(0, 400);
        // Charge speed is status +0x88: 100 for both sides at game init (FUN_005177a0 sets
        // +0x84..+0x98 to 100); only ability cards change it (FUN_004085a0: 125), and VS has none.
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
            skills: Vec::new(), bomb: None, bomb_prev: false, cancel_circles: Vec::new(), cancel_columns: Vec::new(), damage_circles: Vec::new(), snake_explosions: Vec::new(), skill_rng: crate::rng::Rng::new(0x1cafe777),
            charge_timer: None,
            wide_charge: false, charge_origin: [0.0;2], skill_volley: false, boost_frames: 0, cancel_rects: Vec::new(),
            orin_armed: false, nazrin_skill: None, yachie_area: None, tsukasa_skill: None, mamizou_burst: None, bomb_fragments: 0,
            explosion_requests: Vec::new(), cancel_polygons: Vec::new(), bomb_shake: false, targets: Vec::new(), cancel_ellipses: Vec::new(), cancel_orects: Vec::new(),
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
            bombs: START_BOMBS,
            max_bombs: MAX_BOMBS,
            deaths: 0,
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
        self.deaths += 1;
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
        self.targets = enemies.iter().filter(|e| e.data.flags & (flag::NO_HURTBOX | flag::HIDDEN) == 0
            && !e.data.delete && !e.data.killed).map(|e| e.data.pos).collect();
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
            && self.state == State::Normal, self.character, self.pos, self.prev_vx as f32, &*enemies,
            self.anm_file, world, lib, field);
        self.cancel_circles.clear();
        self.cancel_columns.clear();
        self.cancel_rects.clear();
        self.cancel_ellipses.clear();
        self.cancel_orects.clear();
        self.cancel_polygons.clear();
        self.update_later_skills(world);
        self.skills.retain_mut(|s| {
            let alive = s.tick(world, field, &mut self.cancel_circles, &mut self.damage_circles);
            if std::mem::take(&mut s.ended) {
                // FUN_0053b3a0: stop 0x31, then 0x2c panned to the side.
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x31, Some(crate::sfx::STOP))); self.sounds.push((0x2c, None)); }
            }
            alive
        });
        // The bomb button (FUN_00546320 states 1 and 4): pressing it starts the bomb, and during
        // the 8-frame hit window it also cancels the death (a deathbomb). FUN_004ed6d0 needs a
        // bomb in stock and none running; FUN_004ed7d0 spends one and the bomb reads the meter.
        // Full-charge practice keeps the stock and counts the meter as full.
        let bomb_pressed = keys & input::BOMB != 0 && !self.bomb_prev;
        self.bomb_prev = keys & input::BOMB != 0;
        if bomb_pressed && self.bomb.is_none() && matches!(self.state, State::Normal | State::Hit)
            && (self.free_charge || self.bombs > 0) {
            if !self.free_charge {
                self.bombs = (self.bombs - 1).max(0).min(self.max_bombs.clamp(0, 7));
            }
            if self.state == State::Hit {
                self.state = State::Normal;
                self.state_time = 0;
            }
            let meter = if self.free_charge { crate::bomb::MAX_METER } else { self.meter };
            self.bomb = Some(if self.character == Character::Marisa {
                crate::bomb::Bomb::new_marisa(self.pos, meter, self.anm_file, world, lib, field)
            } else if matches!(self.character, Character::Sanae | Character::Ran | Character::Seiran) {
                crate::bomb::Bomb::new_character(self.character,self.pos,meter,self.anm_file,world,lib,field)
            } else if matches!(self.character, Character::Aunn | Character::Nazrin | Character::Orin) {
                crate::bomb::Bomb::new_moving(self.character,self.pos,self.prev_vx as f32,meter,self.anm_file,world,lib,field)
            } else if matches!(self.character, Character::Tsukasa | Character::Mamizou | Character::Yachie | Character::Saki) {
                crate::bomb::Bomb::new_later(self.character,self.pos,self.prev_vx as f32,meter,self.anm_file,world,lib,field)
            } else {
                crate::bomb::Bomb::new(self.pos, meter, self.anm_file, world, lib, field)
            });
            // The bomb starts with sound 0x31 (all four characters' start functions).
            if crate::sfx::on(crate::sfx::Cat::Bomb) { self.sounds.push((0x31, Some(0))); }
        }
        if let Some(b) = &mut self.bomb {
            b.vel_x = self.prev_vx as f32;
            let protecting = b.protecting();
            let alive = b.tick_character(self.pos, &*enemies, world, field, &mut crate::bomb::Areas {
                circles: &mut self.cancel_circles, columns: &mut self.cancel_columns, rects: &mut self.cancel_rects,
                ellipses: &mut self.cancel_ellipses, orects: &mut self.cancel_orects, polygons: &mut self.cancel_polygons }, &mut self.damage_circles);
            // Every bomb update writes its draining meter (FUN_00403250 at bomb +0x88: the meter
            // at the start -> 0, linear, over the duration) and, 10 frames after the end, resets
            // the meter to the attack threshold (status +0x28).
            if !self.free_charge {
                self.meter = if alive { b.meter() } else { self.charge_settings.attack.clamp(0, crate::bomb::MAX_METER) };
            }
            if b.ended_now() {
                // When the duration runs out the update functions shake the field
                // (FUN_004b9f00(1, 15, 6, 6, side)), stop 0x31 and play 0x2c (centre pan).
                self.bomb_shake = true;
                if crate::sfx::on(crate::sfx::Cat::Bomb) { self.sounds.push((0x31, Some(crate::sfx::STOP))); self.sounds.push((0x2c, Some(0))); }
            }
            if !alive {
                self.bomb = None;
            }
            if protecting {
                self.invuln = self.invuln.max(crate::bomb::PROTECTION);
            }
        }
        let mut damage = vec![(0i32, self.pos); enemies.len()];
        // 0054c2f0 copies shot motion with speed 2, hit interval +a4 4, life 20; 0054c5f0
        // (Seiran's) moves up at 0.3, hit interval 2, life 16.
        for x in &mut self.snake_explosions {
            x.time+=1;x.pos[0]+=x.angle.cos()*x.speed;x.pos[1]+=x.angle.sin()*x.speed;x.radius+=x.growth;
            if x.time%x.interval==0 {self.damage_circles.push(crate::skill::DamageCircle {pos:x.pos,radius:x.radius,
                damage:x.damage,remaining:1,growth:0.0,rect:None,ellipse:None,orect:None,polygon:None});}
        }
        self.snake_explosions.retain(|x|x.time<x.life);
        // Nazrin's skill collider (FUN_005402e0 keeps its radius at the VM's X scale; flags
        // 0x2c0 in FUN_00520450: only spirits that are disabled, marked +0x6250 |= 2 instead of
        // damaged, which FUN_00509980 turns into the death routine).
        if let Some(sk) = &self.nazrin_skill {
            match world.get(sk.vm) {
                None => self.nazrin_skill = None,
                Some(vm) => {
                    let r = vm.scale[0];
                    for e in enemies.iter_mut() {
                        let d = &mut e.data;
                        if d.delete || d.killed || d.spirit_kind == 0 || !d.spirit_disabled
                            || d.flags & flag::NO_HURTBOX != 0 { continue; }
                        if dist2(sk.pos, d.pos) <= (r + d.hurtbox[0] / 2.0).powi(2) { d.killed = true; }
                    }
                }
            }
        }
        for circle in &mut self.damage_circles {
            for (index, enemy) in enemies.iter_mut().enumerate() {
                let d = &mut enemy.data;
                if d.delete || d.flags & flag::NO_HURTBOX != 0 || d.hurtbox[0] <= 0.0 { continue; }
                let hit = if let Some((r, angle, sides)) = circle.polygon {
                    // Collider type 7 (FUN_0051e670): FUN_004d22d0 for a box body (flag 0x1000),
                    // FUN_004d26a0 for a round one. APPROX: the box body is not turned.
                    if d.flags & 0x1000 != 0 {
                        crate::scope::polygon_rect(d.pos, d.hurtbox, 0.0, circle.pos, r, angle, sides)
                    } else {
                        crate::scope::polygon_circle(d.pos, d.hurtbox[0] / 2.0, circle.pos, r, angle, sides)
                    }
                } else if let Some((size, angle)) = circle.orect {
                    // FUN_0051f320's oriented box, tested like a shot's collider.
                    shot_hits(circle.pos, size, angle, d)
                } else if let Some((a, b, theta)) = circle.ellipse {
                    // Collider type 5 (FUN_0051e670 -> FUN_004d4020): the body as a circle.
                    crate::scope::ellipse_circle(d.pos, d.hurtbox[0] / 2.0, circle.pos, a, b, theta)
                } else if let Some(size) = circle.rect {
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
            // A skill that refuses (Mamizou's at full bomb stock, FUN_00420380; Saki's while her
            // volley runs, FUN_00423f80) returns non-zero to FUN_004e79e0, which then neither
            // spends the meter nor plays the skill: only the basic shot follows, as for an attack.
            let level = if level == Level::Skill && self.skill_refused() { Level::Attack } else { level };
            self.last_charge = Some(level);
            self.charge_event = Some(level);
            if !self.skill_volley || level==Level::Skill {self.charge_origin = self.pos;}
            let threshold = match level {
                Level::Attack => self.charge_settings.attack,
                Level::Skill => self.charge_settings.skill,
                Level::Extra => self.charge_settings.extra,
                Level::Boss => self.charge_settings.boss,
            };
            if !self.free_charge { self.meter = (self.meter-threshold+self.charge_settings.attack).clamp(0,2500); }
            // FUN_004e79e0 calls the EX class's skill ([4]) before the charge shot ([3]), so a
            // skill that sets the class's busy flag (Sanae, Aunn and Seiran: their volley;
            // Nazrin: her skill collider) keeps the basic shot from restarting.
            if level == Level::Skill && self.character == Character::Marisa {
                // FUN_00417b90: sound 0x33, panned to the player's side.
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x33, None)); }
                self.wide_charge = true;
            }
            if level == Level::Skill && self.character == Character::Reimu {
                let phase = self.skill_rng.angle();
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
            if level == Level::Skill && self.character == Character::Aunn {
                // FUN_0041a9b0: SHT slot 11 volley and 90 frames of invulnerability (+0x2078).
                self.skill_volley = true; self.charge_timer = Some(0);
                self.invuln = 90;
            }
            if level == Level::Skill && self.character == Character::Nazrin {
                // FUN_0041b420: sound 0x1d panned by her x, pl05.anm script 9 where she is,
                // 40 frames of invulnerability, and the marking collider.
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x1d, Some((self.pos[0] * 1000.0 / 192.0) as i32))); }
                if let Some(old) = self.nazrin_skill.take() { world.delete(old.vm); }
                let vm = world.spawn_clipped(lib, self.anm_file, 9, field_to_canvas(field, self.pos), field);
                self.nazrin_skill = Some(MarkCircle { pos: self.pos, vm });
                self.invuln = 40;
            }
            if level == Level::Skill && self.character == Character::Seiran {
                // FUN_0041bec0: SHT slot 11 volley, sound 0x31 panned to her side.
                self.skill_volley = true; self.charge_timer = Some(0);
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x31, None)); }
            }
            if level == Level::Skill && self.character == Character::Orin {
                // FUN_0041d020: sound 0x1e panned to her side, the aura's skill circle at her
                // position, and the next charge shot uses slot 11 (it starts right below).
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x1e, None)); }
                self.scope.start_circle(self.pos, self.anm_file, world, lib, field);
                self.orin_armed = true;
            }
            if level == Level::Skill && self.character == Character::Tsukasa {
                // FUN_0041e5e0: scripts 13 and 12 at her, radius 20 -> 420 over 170 frames.
                // APPROX: a second cast deletes the first one's VMs (the original overwrites the
                // handles).
                if let Some(old) = self.tsukasa_skill.take() { for id in old.vms { world.delete(id); } }
                let at = field_to_canvas(field, self.pos);
                let vms = [world.spawn_clipped(lib, self.anm_file, 13, at, field), world.spawn_clipped(lib, self.anm_file, 12, at, field)];
                let mut growth = th_anm::interp::Interp::default();
                growth.start([20.0], [420.0], 170, 4);
                self.tsukasa_skill = Some(SweepCircle { pos: self.pos, frames: 170, growth, vms });
            }
            if level == Level::Skill && self.character == Character::Mamizou {
                // FUN_00420380: a bomb fragment, script 9 where she is, and a radius 40 burst.
                self.add_bomb_fragment();
                world.spawn_clipped(lib, self.anm_file, 9, field_to_canvas(field, self.pos), field);
                self.mamizou_burst = Some(self.pos);
            }
            if level == Level::Skill && self.character == Character::Yachie {
                // FUN_00422150: sound 0x29 panned to her side; the skill area replaces the basic.
                if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x29, None)); }
                self.start_hex_area(true, world, lib, field);
            }
            if level == Level::Skill && self.character == Character::Saki {
                // FUN_00423f80: SHT slot 11 for 60 frames.
                self.skill_volley = true; self.charge_timer = Some(0);
            }
            // The basic local shot accompanies every release ([3] after the switch). Yachie's
            // (FUN_00421f80) is her small hexagon instead, not while the skill one runs.
            if self.character == Character::Yachie {
                if !self.yachie_area.as_ref().is_some_and(|a| a.skill) {
                    if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x29, None)); }
                    self.start_hex_area(false, world, lib, field);
                }
            } else if !self.skill_volley && self.nazrin_skill.is_none() { self.charge_timer = Some(0); }
            if matches!(level, Level::Extra | Level::Boss) { self.barrier = Barrier::Up; }
        }
        if let Some(t) = self.charge_timer {
            // (slot, fires while t < n, stops once the advanced timer passes `end`, i.e. at
            // end + 1): FUN_0053df80 Sanae 30/40 and 60/70 for the volley; FUN_0053eb60 Aunn
            // 30/40 and 90/100; FUN_005402e0 Nazrin 120/120; FUN_00541330 Seiran 30/40 and
            // 200/210; FUN_0041d1b0 Orin 10/10, slot 11 while armed.
            let (slot, fire_frames, stop) = match self.character {
                Character::Sanae => if self.skill_volley {(11,60,71)} else {(10,30,41)},
                Character::Ran => (if self.boost_frames>0 {11} else {10},50,61),
                Character::Aunn => if self.skill_volley {(11,90,101)} else {(10,30,41)},
                Character::Nazrin => (10,120,121),
                Character::Seiran => if self.skill_volley {(11,200,211)} else {(10,30,41)},
                Character::Orin => (if self.orin_armed {11} else {10},10,11),
                // FUN_0041e750 (Tsukasa), FUN_004204f0 (Mamizou): slot 10, 50 frames, stop past
                // 60; FUN_00424000 (Saki): slot 10 the same, slot 11 60 / 70 for the skill.
                Character::Tsukasa | Character::Mamizou => (10,50,61),
                Character::Saki => if self.skill_volley {(11,60,71)} else {(10,50,61)},
                _ => (10,21,21),
            };
            // Saki's update passes the timer as the long timer too (FUN_0054f990(t, t, slot)),
            // which her records' alternate interval uses.
            let long = if self.character == Character::Saki { t } else { 0 };
            if t < fire_frames { self.fire_slot(slot, t, long, world, lib, field); }
            if t == 0 && std::mem::take(&mut self.wide_charge) { self.fire_slot(11,t,0,world,lib,field); }
            self.charge_timer = (t+1 < stop).then_some(t+1);
            if self.charge_timer.is_none() {
                self.skill_volley=false;
                if slot == 11 && self.character == Character::Orin { self.orin_armed = false; }
            }
        }
    }

    /// Whether the EX class's skill callback refuses: Mamizou's when the bomb stock (clamped
    /// to 0..10) has reached the maximum (clamped to 0..7), Saki's while her volley runs.
    fn skill_refused(&self) -> bool {
        match self.character {
            Character::Mamizou => self.max_bombs.clamp(0, 7) <= self.bombs.clamp(0, 10),
            Character::Saki => self.skill_volley,
            _ => false,
        }
    }

    /// `FUN_00537b90`: below the stock maximum a fragment plays 0x2e; the third one is a bomb
    /// (`FUN_00537c30(1, 1)`: 0x2e again unless that went over the maximum).
    fn add_bomb_fragment(&mut self) {
        let max = self.max_bombs.clamp(0, 7);
        if max <= self.bombs { self.bomb_fragments = 0; return; }
        if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x2e, Some(0))); }
        self.bomb_fragments += 1;
        if self.bomb_fragments > 2 {
            self.bomb_fragments = 0;
            self.bombs += 1;
            if max < self.bombs { self.bombs = max; }
            else if crate::sfx::on(crate::sfx::Cat::Skill) { self.sounds.push((0x2e, Some(0))); }
        }
    }

    /// Yachie's area starts where she is (EX +0x9c) with a fresh timer.
    fn start_hex_area(&mut self, skill: bool, world: &mut World, lib: &AnmLibrary, field: [f32; 4]) {
        world.spawn_clipped(lib, self.anm_file, if skill { 10 } else { 8 }, field_to_canvas(field, self.pos), field);
        let mut growth = th_anm::interp::Interp::default();
        if skill { growth.start([1.0], [128.0], 60, 4); } else { growth.start([1.0], [48.0], 10, 4); }
        self.yachie_area = Some(HexArea { pos: self.pos, skill, time: 0, growth, radius: 1.0 });
    }

    /// The later characters' EX class updates that run every frame: Mamizou's burst, Tsukasa's
    /// sweep and Yachie's hexagons.
    fn update_later_skills(&mut self, world: &mut World) {
        use crate::skill::DamageCircle;
        if let Some(pos) = self.mamizou_burst.take() {
            // FUN_004ee770 / FUN_00530c60 radius 40; FUN_005200f0(radius 40, life 1, damage 20).
            self.cancel_circles.push((pos, 40.0));
            self.damage_circles.push(DamageCircle { pos, radius: 40.0, damage: 20, remaining: 1, growth: 0.0,
                rect: None, ellipse: None, orect: None, polygon: None });
        }
        if let Some(s) = &mut self.tsukasa_skill {
            if s.frames > 0 {
                let r = s.growth.step().map_or(20.0, |[r]| r);
                self.explosion_requests.push((s.pos, r));
                if let Some(vm) = world.get_mut(s.vms[1]) { vm.scale = [r, r]; }
                s.frames -= 1;
                if s.frames < 1 {
                    for id in s.vms { world.delete(id); }
                    self.tsukasa_skill = None;
                }
            }
        }
        if let Some(a) = &mut self.yachie_area {
            let (areas, end) = if a.skill { (110, 110) } else { (40, 40) };
            if a.time < areas {
                if let Some([r]) = a.growth.step() { a.radius = r; }
                self.cancel_polygons.push((a.pos, a.radius, 0.0, 6));
                // FUN_0051f860(.., angle 0, 6 sides, damage 1): life 1, flag 0x10.
                self.damage_circles.push(DamageCircle { pos: a.pos, radius: 0.0, damage: 1, remaining: 1, growth: 0.0,
                    rect: None, ellipse: None, orect: None, polygon: Some((a.radius, 0.0, 6)) });
            }
            a.time += 1;
            if a.time > end { self.yachie_area = None; }
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
            for (i, off) in offsets.iter().enumerate() {
                let at = [self.fixed[0] + (off[0] * 128.0) as i32, self.fixed[1] + (off[1] * 128.0) as i32];
                let script = self.sht.option_script;
                let canvas = field_to_canvas(field, [at[0] as f32 / 128.0, at[1] as f32 / 128.0]);
                let vm = (script >= 0).then(|| world.spawn_clipped(lib, self.anm_file, script as usize, canvas, field));
                // Seiran's orbit starts at index * pi (FUN_005483d0, option +0xd4).
                let orbit = crate::motion::wrap_angle(i as f32 * std::f32::consts::PI);
                self.options.push(PlayerOption { pos: at, vm, extra: None, hold: if latching { 10 } else { 0 }, orbit, wrapped: false });
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
        let orbiting = self.character == Character::Seiran;
        let (fixed, focused) = (self.fixed, self.focused);
        for (o, off) in self.options.iter_mut().zip(&offsets) {
            let mut target = [anchor[0] + (off[0] * 128.0) as i32, anchor[1] + (off[1] * 128.0) as i32];
            if orbiting {
                // Seiran's callback FUN_00545b70: the option circles her at radius 40 (24
                // focused), a sixtieth of a half turn a frame, wrapping across the field's
                // sides (+-160) with a one-frame snap.
                let r = if focused { 24.0 } else { 40.0 };
                let (s, c) = o.orbit.sin_cos();
                target = [fixed[0] - (c * r * -128.0) as i32, fixed[1] - (s * r * -128.0) as i32];
                o.orbit = crate::motion::wrap_angle(o.orbit + std::f32::consts::PI / 60.0);
                if target[0] < -0x5000 {
                    o.hold = 1; o.wrapped = true; target[0] += 0xa000;
                } else if target[0] <= 0x5000 {
                    if o.wrapped { o.hold = 1; }
                    o.wrapped = false;
                } else {
                    o.hold = 1; o.wrapped = true; target[0] -= 0xa000;
                }
            }
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
        let sound = |pos_x: f32| (r.sound >= 0 && crate::sfx::on(crate::sfx::Cat::PlayerShot))
            .then(|| (r.sound as u32, Some((pos_x * 1000.0 / 192.0) as i32)));
        if r.kind == 2 || r.funcs[0] == 12 {
            let back = if matches!(r.kind, 4..=6) { 0.0 } else { r.angle.cos() * r.speed };
            self.sounds.extend(sound(origin[0] + r.offset[0] - back));
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
        let mut pos = [origin[0] + r.offset[0] - back[0], origin[1] + r.offset[1] - back[1]];
        let mut angle = r.angle;
        let mut pierce = false;
        let mut scale = None;
        // Init callbacks (table 0x5a0440) that change the shot.
        match r.funcs[0] {
            // FUN_0054b5c0 / FUN_0054b3f0 (Nazrin): aim at the nearest enemy within 640 of the
            // shot (FUN_00508530); 13 adds up to 10 degrees either way.
            10 | 13 => {
                let mut best = 640.0f32 * 640.0;
                let mut target = None;
                for t in &self.targets {
                    let d = dist2(*t, pos);
                    if d < best { best = d; target = Some(*t); }
                }
                if let Some(t) = target {
                    angle = (t[1] - pos[1]).atan2(t[0] - pos[0]);
                    if r.funcs[0] == 13 { angle += self.rand_signed() * std::f32::consts::PI * 5.0 / 90.0; }
                    angle = crate::motion::wrap_angle(angle);
                }
            }
            // FUN_0054ba40 (Seiran's options): within 36 degrees of straight up.
            14 => angle = crate::motion::wrap_angle(self.rand_signed() * std::f32::consts::PI * 0.2 - std::f32::consts::FRAC_PI_2),
            // FUN_0054b9a0 (Seiran's skill): x moved by up to 32, the sprite at scale 2.
            15 => { pos[0] += self.rand_signed() * 32.0; scale = Some([2.0, 2.0]); }
            // FUN_0054b270 (Orin): piercing, within 10 degrees of straight up.
            16 => { pierce = true; angle = crate::motion::wrap_angle(self.rand_signed() * 0.17453294 - std::f32::consts::FRAC_PI_2); }
            // FUN_0054afc0 (Orin's charge): piercing, up or 6 degrees towards her movement.
            17 => { pierce = true; angle = crate::scope::orin_angle(self.prev_vx as f32); }
            _ => {}
        }
        // The record's sound plays after the init callback, panned by the shot's x.
        self.sounds.extend(sound(pos[0]));
        let vm = (r.anm_script >= 0).then(|| {
            let id = world.spawn_clipped(lib, self.anm_file, r.anm_script as usize, field_to_canvas(field, pos), field);
            if let Some(v) = world.get_mut(id) {
                // Charge orbs rotate through script 10's angular velocity. Their ANM
                // does not request direction-following rotation (0054df10, +0x4cc).
                if v.auto_rotate { v.rot[2] = angle; }
                if let Some(sc) = scale { v.scale = sc; }
            }
            id
        });
        self.bullets.push(PlayerBullet {
            pos,
            angle,
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
            update_callback: r.funcs[1],
            pierce,
            lifetime: i32::from_le_bytes(r.raw[0x34..0x38].try_into().unwrap()),
            stays_off_field: matches!(r.kind, 2 | 8),
            drift: [0.0; 2],
            column: None,
            velocity: vel,
        });
    }

    /// `FUN_00404b70` on the replay RNG: [-1, 1].
    fn rand_signed(&mut self) -> f32 {
        self.skill_rng.signed()
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
            // FUN_0054d370: a record with a life (+0x3c) only needs the player up and no bomb.
            let alive = if b.is_charge() { !matches!(self.state, State::GameOver) }
                else if b.life() > 0 { !matches!(self.state, State::Dead | State::Hit | State::GameOver) && self.bomb.is_none() }
                else { keep };
            let was_ended = b.ended;
            b.update(origin, alive, field, world);
            // 0054d370 / 0054c830: when a beam ends (state 2, VM interrupt 1) they stop sound 0x14,
            // which is a looped sound (table row +0xc = 1) started by the beam init callbacks.
            if !was_ended && b.ended && crate::sfx::on(crate::sfx::Cat::BeamStart) {
                self.sounds.push((0x14, Some(crate::sfx::STOP)));
            }
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
            let wander = (self.bullets[i].update_callback == 10 && self.bullets[i].time % 30 == 0)
                .then(|| [self.rand_signed() * 0.017453292, self.rand_signed() * 0.2]);
            let b = &mut self.bullets[i];
            // FUN_0054df10 first: a shot at its SHT +0x34 age gets interrupt 1 and restarts its
            // age at -999.
            if b.lifetime > 0 && b.lifetime <= b.time {
                if let Some(id) = b.vm { world.interrupt(id, 1); }
                b.time = -999;
            }
            let mut expired = false;
            match b.update_callback {
                // 0054c7a0 (Seiran): +0.3 speed a frame while flying.
                3 if !b.impacting => b.speed += 0.3,
                // 0054b0c0 (Orin): every 30 frames a new turn (up to 1 degree a frame) and speed
                // step (up to 0.2, not below speed 2 when negative).
                10 => {
                    if let Some(w) = wander { b.drift = w; }
                    b.angle = crate::motion::wrap_angle(b.angle + b.drift[0]);
                    if b.speed >= 2.0 || b.drift[1] >= 0.0 { b.speed += b.drift[1]; }
                }
                // 0054adc0 (Orin's charge): +0.1 speed; the script-9 child's X scale grows by the
                // speed and is the length of a (length, 28) box trailing the shot along its last
                // velocity; gone after 129 frames.
                11 => {
                    b.speed += 0.1;
                    let child = b.vm.and_then(|id| world.find_descendant(id, 9));
                    let length = match child.and_then(|id| world.get_mut(id)) {
                        Some(v) => { v.scale[0] += b.speed; v.scale[0] }
                        None => 16.0,
                    };
                    b.column = Some(length);
                    expired = b.time > 0x81;
                }
                _ => {}
            }
            if expired {
                if let Some(id) = b.vm { world.delete(id); }
                self.bullets.swap_remove(i);
                continue;
            }
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
            // Orin's column box is placed from the position and velocity before this move.
            let column = b.column.map(|length| {
                let v = b.velocity;
                let len = (v[0]*v[0] + v[1]*v[1]).sqrt();
                let dir = if len.abs() >= 0.01 { [v[0]/len, v[1]/len] } else { v };
                ([b.pos[0] - dir[0]*length/2.0, b.pos[1] - dir[1]*length/2.0], [length, 28.0])
            });
            let (s, c) = b.angle.sin_cos();
            b.velocity = [c * b.speed, s * b.speed];
            b.pos = [b.pos[0] + b.velocity[0], b.pos[1] + b.velocity[1]];

            // Kinds 2 and 8 skip the off-field check.
            let mut dead = !b.stays_off_field && (b.pos[0].abs() > 148.0 + 32.0 || b.pos[1] < -32.0 || b.pos[1] > 448.0 + 32.0);
            dead |= b.vm.is_some_and(|id| world.get(id).is_none());
            if !dead && !b.impacting {
                for (index, e) in enemies.iter_mut().enumerate() {
                    let d = &mut e.data;
                    if d.flags & flag::NO_HURTBOX != 0 || d.hurtbox[0] <= 0.0 || d.delete {
                        continue;
                    }
                    let overlap = match column {
                        Some((centre, size)) => shot_hits(centre, size, b.angle, d),
                        None => shot_hits(b.pos, b.hitbox, b.angle, d),
                    };
                    if overlap {
                        // A collider remembers the last enemy it hit (+0xa8) only to avoid a
                        // second hit within one frame: the per-frame collider tick (0051fa30)
                        // clears it, and the hit interval (+0xa4) is 1. A piercing charge
                        // orb (callback 7 leaves it alive, 0054fba0) therefore hits every
                        // overlapping enemy on every frame.
                        damage[index].0 += b.damage;
                        damage[index].1 = b.pos;
                        if !b.charge_shot && !b.pierce {
                            // Hit callbacks 3, 4 and 9 (0054c5f0, 0054c2f0, 0054a7b0) end with sound 0x41,
                            // panned by the shot's x.
                            if matches!(b.hit_callback, 3 | 4 | 9) {
                                if crate::sfx::on(crate::sfx::Cat::ChargeHit) { self.sounds.push((0x41, Some((b.pos[0] * 1000.0 / 192.0) as i32))); }
                            }
                            // Callback 1 adds effect 142 before the shared impact transition.
                            if b.hit_callback == 1 || b.hit_callback == 5 {
                                if let Some(file) = self.effect_anm {
                                    // FUN_0054da60: the signed helper times 20 degrees, then four
                                    // raw draws for the colour and alpha.
                                    let spread = self.skill_rng.signed();
                                    let mut random = || self.skill_rng.next();
                                    let angle = crate::motion::wrap_angle(b.angle + spread * 0.34906584 + std::f32::consts::PI);
                                    let id = world.spawn_rotated(lib, file, 142, field_to_canvas(field, b.pos), field, angle);
                                    if let Some(vm) = world.get_mut(id) {
                                        vm.color = [(random() & 127) as f32 + 127.0, (random() & 63) as f32 + 64.0, (random() & 63) as f32 + 64.0];
                                        vm.alpha = (random() & 63) as f32 + 96.0;
                                    }
                                }
                            }
                            if b.hit_callback == 4 {
                                //0054c2f0: replace the shot collider with radius24,+SHT68/frame,life20.
                                self.snake_explosions.push(SnakeExplosion {pos:b.pos,angle:b.angle,speed:2.0,radius:24.0,
                                    damage:b.damage,time:0,growth:b.explosion_growth,interval:4,life:20});
                            }
                            if b.hit_callback == 3 {
                                // 0054c5f0 (Seiran): a radius 24 collider growing 1 a frame, life 16,
                                // hit interval 2, drifting up at 0.3; the shot slows to 0.3.
                                self.snake_explosions.push(SnakeExplosion {pos:b.pos,angle:-std::f32::consts::FRAC_PI_2,speed:0.3,
                                    radius:24.0,damage:b.damage,time:0,growth:1.0,interval:2,life:16});
                            }
                            b.impacting = true;
                            b.speed = match b.hit_callback { 4 => 2.0, 3 => 0.3, _ => b.speed/8.0 };
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

    #[test]
    fn marisa_shot_sounds_stop_with_the_shot() {
        let path=std::path::PathBuf::from(std::env::var_os("TH19_DAT").unwrap_or_else(||
            std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("Downloads/th19/th19/th19.dat").into_os_string()));
        if !path.is_file() {return;}
        let mut dat=th_formats::dat::Archive::open(path).unwrap();
        let (mut world,lib,field)=(World::default(),AnmLibrary::default(),[0.0,0.0,384.0,448.0]);
        let mut sht=th_formats::sht::ShtFile::parse(&dat.read("pl01.sht").unwrap()).unwrap();
        sht.option_script=-1;
        let mut p=Player::new(sht,0);p.character=Character::Marisa;p.state=State::Normal;p.power=200;
        let mut log=Vec::new();
        for t in 0..200 {
            let keys=if t<100 {input::SHOT} else {0};
            p.update(keys,&mut [],&mut world,&lib,field);
            for (id,pan) in p.sounds.drain(..) {log.push((t,id,pan));}
        }
        eprintln!("{log:?}");
        // The looped beam sound 0x14 starts with the beam and is stopped when the shot ends.
        let stop=Some(crate::sfx::STOP);
        assert!(log.iter().any(|&(t,id,pan)| id==0x14&&pan!=stop&&t<5));
        assert!(log.iter().any(|&(t,id,pan)| id==0x14&&pan==stop&&t>=100),"no stop for the beam sound: {log:?}");
        assert!(log.iter().all(|&(t,id,pan)| t<100+15||(id==0x14&&pan==stop)),"sound after the shot stopped: {log:?}");
    }

    fn amulet(angle: f32, speed: f32, time: i32) -> PlayerBullet {
        PlayerBullet { pos: [0.0, 300.0], angle, speed, hitbox: [4.0; 2], damage: 1,
            homing: true, homing_target: None, angular_velocity: 0.0, acceleration: 0.0,
            vm: None, time, charge_shot: false, impacting: false, hit_callback: 0, snake_turn: None, snake: false, explosion_growth: 0.0,
            update_callback: 0, pierce: false, lifetime: 999, stays_off_field: false, drift: [0.0; 2], column: None, velocity: [0.0; 2] }
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
        pos:p.pos,radius:10.0,growth:0.0,damage:40,remaining:1,rect: None, ellipse: None, orect: None, polygon: None
    }).collect();
    let mut enemies=[crate::Enemy {
        threads:th_ecl::Threads::new(th_ecl::Context::new(th_ecl::SubRef{file:0,sub:0})),
        data:crate::EnemyData {id:100,pos:p.pos,hurtbox:[10.0;2],hp:5000,..Default::default()}
    }];
    p.update(0,&mut enemies,&mut world,&lib,crate::FIELDS[0]);
    assert_eq!(enemies[0].data.hp,4910);
    enemies[0].data.hp=1500; enemies[0].data.spell_hp7=Some(10500);
    p.damage_circles=(0..3).map(|_|crate::skill::DamageCircle {
        pos:p.pos,radius:10.0,growth:0.0,damage:40,remaining:1,rect: None, ellipse: None, orect: None, polygon: None
    }).collect();
    p.update(0,&mut enemies,&mut world,&lib,crate::FIELDS[0]);
    assert_eq!(enemies[0].data.spell_hp7,Some(10410));
    assert_eq!(enemies[0].data.hp,1487);
    p.state=State::Entering;
    enemies[0].data.hp=5000;enemies[0].data.spell_hp7=None;
    p.damage_circles=(0..3).map(|_|crate::skill::DamageCircle {
        pos:p.pos,radius:10.0,growth:0.0,damage:40,remaining:1,rect: None, ellipse: None, orect: None, polygon: None
    }).collect();
    p.update(0,&mut enemies,&mut world,&lib,crate::FIELDS[0]);
    assert_eq!(enemies[0].data.hp,4982,"Entering divides the capped 90 damage by five");
}

}
