//! One player's side of the VS screen: its field, enemies, and the wave system.
//!
//! TH19 stages are an endless stream of waves. The stage script (`common.ecl` `main`, via
//! `WorldWave01`) fills a weighted pool of wave ids (`ins_1010`/`ins_1011`), then repeatedly
//! spawns one (`ins_1012`) and waits for it to finish. The wave picked is
//! `pool[sequence[counter] % pool_len]`, where `sequence` is 256 values rolled once from the
//! game's synchronised RNG (`FUN_005378c0`) — the same for both players.

use std::collections::HashMap;
use std::rc::Rc;

use th_anm::{AnmLibrary, World};
use th_ecl::Program;

use crate::enemy::Enemy;
use crate::bullet::{self, Bullet, Et};
use crate::player::Player;
use th_formats::exe::BulletType;

/// Sub names for wave ids (table at 0x5a0f50 in th19.exe; id 0 is unused).
pub fn wave_sub(id: i32) -> Option<String> {
    if !(1..=26).contains(&id) {
        return None;
    }
    let n = (id - 1) / 2 + 1;
    Some(if id % 2 == 1 { format!("Wave{n:02}") } else { format!("Wave{n:02}f") })
}

/// Roll the 256-entry wave sequence with the game's Lehmer generator (x * 48271 mod 2^31-1).
pub fn roll_wave_sequence(seed: u32) -> [u8; 256] {
    let mut x = (seed % 0x7fff_ffff).max(1) as u64;
    let mut out = [0u8; 256];
    for v in out.iter_mut() {
        x = x * 48271 % 0x7fff_ffff;
        *v = (x & 0xff) as u8;
    }
    out
}

pub struct SpawnRequest {
    /// Assigned by [`SideState::request`].
    pub id: i32,
    /// Difficulty selecting rank-masked instructions; set by [`SideState::request`].
    pub difficulty: u8,
    pub sub: String,
    pub pos: [f32; 2],
    pub hp: i32,
    pub score: i32,
    pub item: i32,
    pub mirrored: bool,
    pub ivars: [i32; 4],
    pub fvars: [f32; 8],
}

/// Everything enemies can read or change besides themselves.
pub struct SideState {
    /// Field rectangle on the canvas `[x, y, w, h]`.
    pub field: [f32; 4],
    /// Difficulty (`DAT_0062b250`): 0 Easy, 1 Normal, 2 Hard, 3 Lunatic.
    pub difficulty: u8,
    pub player_pos: [f32; 2],
    /// TH19's escalating level (0..7), selecting columns of the level tables.
    pub level: usize,
    pub extra_level: u8,
    pub boss_level: u8,
    pub incoming_extra_level: u8,
    pub incoming_boss_level: u8,
    pub boss_ids: Vec<i32>,
    pub boss_globals: HashMap<i32, f32>,
    pub level_table: [[i32; 8]; 32],
    pub wave_pool: Vec<i32>,
    pub wave_sequence: [u8; 256],
    pub wave_counter: usize,
    pub wave_param: i32,
    /// ECL anm slot -> loaded file.
    pub anm_files: [Option<usize>; 8],
    pub spawn: Vec<SpawnRequest>,
    /// (id, wave member) of every live enemy, refreshed as enemies come and go.
    pub alive: Vec<(i32, bool)>,
    pub next_enemy_id: i32,
    pub last_enemy_id: i32,
    rng: u32,
    pub unknown_ops: HashMap<u16, u32>,
    pub unknown_vars: HashMap<i32, u32>,
    /// Kill chain (`FUN_00549780`): a timer topped up by each kill (capped at 60 frames),
    /// the current chain length and the best chain. APPROX: the timer counting down by one
    /// per frame and the chain ending at 0 is assumed; that code hasn't been located yet.
    pub chain_timer: i32,
    pub chain: i32,
    pub chain_max: i32,
    /// Enemy bullets on this field.
    pub bullets: Vec<Bullet>,
    /// Bullet sprite types from th19.exe.
    pub bullet_types: Rc<Vec<BulletType>>,
    /// `ins_613`: cancel every bullet this frame.
    pub cancel_bullets: bool,
    /// Spell card background VM (ECL 522 / 523).
    pub spell_background: Option<th_anm::VmId>,
    pub spell_start_effect: Option<th_anm::VmId>,
    /// ECL 525 requested destroying this field's enemies.
    pub clear_enemies: bool,
    /// `ghost.anm` (kill explosions).
    pub ghost_anm: Option<usize>,
    /// Kill explosions in progress.
    pub explosions: Vec<Explosion>,
    pub scope_circle: Option<([f32; 2], f32)>,
    pub spirit_credit: i32,
    pub spirit_origin: [f32;2],
    pub spirit_pressure: i32,
    pub spirit_frames: i32,
    pub outgoing_spirits: Vec<crate::spirit::Request>,
    pub pending_spirits: Vec<crate::spirit::Pending>,
    /// Bullets cancelled by this side's explosions, waiting to be sent across.
    pub outgoing_reflects: Vec<crate::reflect::Request>,
    /// Converted bullets on their way to this side.
    pub pending_reflects: Vec<crate::reflect::Pending>,
    /// Converted bullets that arrived this tick and fire on this side.
    pub arrived_reflects: Vec<crate::reflect::Arrival>,
    pub sent_spirits_pending: usize,
}

/// The explosion a destroyed enemy leaves (GhostCtrl, th19.exe `FUN_0051d4e0` /
/// `FUN_0051cc70` / `FUN_0051c8c0`): a circle growing from radius 1 to `size` (interpolation
/// mode 5) over `duration` frames that damages enemies it touches (so kills chain) and
/// cancels small bullets. Its parameters are the enemy's kill rewards (`ins_1028`: size,
/// `ins_1031`: duration, `ins_1029`: damage).
pub struct Explosion {
    pub pos: [f32; 2],
    pub time: i32,
    pub duration: i32,
    pub radius: f32,
    pub damage: i32,
    radius_interp: th_anm::interp::Interp<1>,
    /// The collider skips the enemy it hit most recently (`+0xa8`).
    vm: Option<th_anm::VmId>,
}

impl SideState {
    pub fn scope_contains(&self, pos: [f32;2], radius: f32) -> bool {
        self.scope_circle.is_some_and(|(centre,r)|
            (pos[0]-centre[0]).powi(2)+(pos[1]-centre[1]).powi(2) < r*r+radius*radius)
    }
    pub fn rand(&mut self) -> u32 {
        // APPROX: the game uses its own replay RNG; any uniform generator plays the same.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }
    /// Uniform in [0, 1).
    pub fn randf(&mut self) -> f32 {
        (self.rand() >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Uniform in [-1, 1).
    pub fn randf2(&mut self) -> f32 {
        self.randf() * 2.0 - 1.0
    }

    /// Queue an enemy for creation. Its id is allocated now, as the game creates enemies
    /// synchronously and scripts read the new id right away (variable -9931).
    pub fn request(&mut self, mut req: SpawnRequest) {
        req.difficulty = self.difficulty;
        req.id = self.next_enemy_id;
        self.next_enemy_id = (self.next_enemy_id + 1) & 0xffff;
        if self.next_enemy_id == 0 {
            self.next_enemy_id = 1;
        }
        self.last_enemy_id = req.id;
        self.alive.push((req.id, false));
        self.spawn.push(req);
    }

    /// Fire a shooter (`ins_601`).
    pub fn fire(&mut self, et: &Et, origin: [f32; 2], world: &mut World, lib: &AnmLibrary) {
        let Some(anm) = self.anm_files[0] else { return };
        let types = self.bullet_types.clone();
        let mut out = Vec::new();
        let mut rng = self.rng;
        let mut rand = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut ctx = bullet::Ctx {
            world,
            lib,
            types: &types,
            anm,
            field: self.field,
            player: self.player_pos,
            rand: &mut rand,
        };
        bullet::fire(et, origin, &mut ctx, &mut out);
        self.rand();
        self.bullets.extend(out);
    }

    pub fn wave_members_alive(&self) -> i32 {
        self.alive.iter().filter(|a| a.1).count() as i32
    }

    /// `ins_1012`: create the next wave's controller enemy at the field origin.
    pub fn spawn_wave(&mut self) {
        if self.wave_pool.is_empty() {
            return;
        }
        let k = self.wave_sequence[self.wave_counter] as usize % self.wave_pool.len();
        self.wave_counter = (self.wave_counter + 1) & 0xff;
        let Some(sub) = wave_sub(self.wave_pool[k]) else { return };
        self.request(SpawnRequest {
            id: 0,
            difficulty: 0,
            sub,
            pos: [0.0, 0.0],
            hp: 10,
            score: 0,
            item: -1,
            mirrored: false,
            ivars: [0; 4],
            fvars: [0.0; 8],
        });
    }
}

pub struct Side {
    pub st: SideState,
    pub enemies: Vec<Enemy>,
    /// The stage script thread group (`main`), run like an enemy with no body.
    pub stage: Option<Enemy>,
    pub player: Option<Player>,
}

impl Side {
    pub fn new(field: [f32; 4], seed: u32, wave_sequence: [u8; 256], anm_files: [Option<usize>; 8]) -> Self {
        Self {
            st: SideState {
                field,
                difficulty: 1,
                player_pos: [0.0, 400.0],
                level: 0,
                extra_level: 0, boss_level: 0, incoming_extra_level: 0, incoming_boss_level: 0,
                boss_ids: Vec::new(), boss_globals: HashMap::new(),
                level_table: [[0; 8]; 32],
                wave_pool: Vec::new(),
                wave_sequence,
                wave_counter: 0,
                wave_param: 0,
                anm_files,
                spawn: Vec::new(),
                alive: Vec::new(),
                next_enemy_id: 1,
                last_enemy_id: 0,
                rng: seed | 1,
                unknown_ops: HashMap::new(),
                unknown_vars: HashMap::new(),
                chain_timer: 0,
                chain: 0,
                chain_max: 0,
                bullets: Vec::new(),
                bullet_types: Rc::new(Vec::new()),
                cancel_bullets: false,
                spell_background: None, spell_start_effect: None,
                clear_enemies: false,
                ghost_anm: None,
                explosions: Vec::new(),
                scope_circle: None,
                spirit_credit: 0, spirit_origin: [0.0;2], spirit_pressure: 0, spirit_frames: 0,
                outgoing_spirits: Vec::new(), pending_spirits: Vec::new(), outgoing_reflects: Vec::new(), pending_reflects: Vec::new(), arrived_reflects: Vec::new(),
                sent_spirits_pending: 0,
            },
            enemies: Vec::new(),
            stage: None,
            player: None,
        }
    }

    /// Explosions grow, hit enemies (`FUN_00520450`'s collider rules) and cancel bullets.
    fn update_explosions(&mut self, world: &mut World) {
        let st = &mut self.st;
        let mut cancelled: Vec<(i32, i32, f32, [f32; 2])> = Vec::new();
        for x in st.explosions.iter_mut() {
            if x.time == x.duration - 10 {
                if let Some(id) = x.vm {
                    world.interrupt(id, 1);
                }
            }
            if let Some([r]) = x.radius_interp.step() {
                x.radius = r;
            }
            // Damage: circle against each enemy's hurtbox rectangle.
            if x.damage != 0 {
                for e in self.enemies.iter_mut() {
                    let d = &mut e.data;
                    if d.delete || d.flags & crate::enemy::flag::NO_HURTBOX != 0 || d.hurtbox[0] <= 0.0 {
                        continue;
                    }
                    // Like every collider, the explosion hits each overlapping enemy once per
                    // frame for its whole lifetime (FUN_0051fa30 clears the last-hit id).
                    let (hw, hh) = (d.hurtbox[0] / 2.0, d.hurtbox[1] / 2.0);
                    let cx = x.pos[0].clamp(d.pos[0] - hw, d.pos[0] + hw);
                    let cy = x.pos[1].clamp(d.pos[1] - hh, d.pos[1] + hh);
                    if (x.pos[0] - cx).powi(2) + (x.pos[1] - cy).powi(2) <= x.radius * x.radius {
                        d.damage(x.damage, x.pos);
                    }
                }
            }
            // FUN_0051c8c0: live bullets of types 0..3 whose centre is inside the circle
            // (radius + 1) are cancelled, invulnerable ones too. Each is converted into an
            // attack for the opponent and counts as a chain hit.
            for b in st.bullets.iter_mut() {
                let (dx, dy) = (b.pos[0] - x.pos[0], b.pos[1] - x.pos[1]);
                if b.state == 1 && !b.delete && (0..4).contains(&b.kind) && dx * dx + dy * dy <= (x.radius + 1.0).powi(2) {
                    b.cancel();
                    cancelled.push((b.kind, b.color, b.speed, b.pos));
                }
            }
            x.time += 1;
        }
        st.explosions.retain(|x| x.time < x.duration);
        for (kind, color, speed, origin) in cancelled {
            let roll = st.rand();
            let kind = crate::reflect::converted_kind(kind, st.spirit_frames, roll);
            st.outgoing_reflects.push(crate::reflect::Request { kind, color, speed, origin });
            // FUN_00549780(0): one more chain hit, no extra chain time.
            st.chain += 1;
            st.chain_max = st.chain_max.max(st.chain);
        }
    }

    fn update_bullets(&mut self, world: &mut World, lib: &AnmLibrary) {
        let st = &mut self.st;
        // ECL 613 (FUN_004edf90): every bullet that is not already gone is cancelled,
        // whatever its invulnerability.
        if std::mem::take(&mut st.cancel_bullets) {
            for b in st.bullets.iter_mut() {
                b.cancel();
            }
        }
        let Some(anm) = st.anm_files[0] else { return };
        let types = st.bullet_types.clone();
        let mut rng = st.rand();
        let mut rand = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut ctx =
            bullet::Ctx { world, lib, types: &types, anm, field: st.field, player: st.player_pos, rand: &mut rand };
        let mut hit: Option<[f32; 2]> = None;
        let player_r = self.player.as_ref().map_or(0.0, |p| p.hitbox_radius());
        let cancel = self.player.as_ref().and_then(|p| p.cancel_radius);
        for b in st.bullets.iter_mut() {
            // Original contact callback applies its factor to the following movement tick.
            let dt = b.scope_time_scale;
            b.update_scaled(&mut ctx, dt);
            b.scope_time_scale = if b.state == 1 && b.hitbox > 0.0 && st.scope_circle.is_some_and(|(pos,r)|
                (b.pos[0]-pos[0]).powi(2)+(b.pos[1]-pos[1]).powi(2) < r*r+b.hitbox*b.hitbox) { 0.5 } else { 1.0 };
            let (dx, dy) = (b.pos[0] - st.player_pos[0], b.pos[1] - st.player_pos[1]);
            let d2 = dx * dx + dy * dy;
            if let Some(r) = cancel {
                if d2 <= r * r && b.invuln == 0 {
                    b.cancel();
                    continue;
                }
            }
            if self.player.as_ref().is_some_and(|p| p.cancel_circles.iter().any(|(pos,r)|
                b.intersects_cancel(*pos,*r,ctx.world,ctx.lib))) { b.cancel(); continue; }
            // FUN_00549e00: hit when d^2 < player_r^2 + bullet_r^2.
            if b.state == 1 && !b.delete && player_r > 0.0 && d2 < player_r * player_r + b.hitbox * b.hitbox {
                hit = Some(b.pos);
            }
        }
        let field = st.field;
        st.bullets.retain(|b| {
            if b.delete {
                if let Some(id) = b.vm {
                    ctx.world.delete(id);
                }
                // The cancel effect (bullet.anm script chosen by the type's cancel mode).
                if b.cancelled {
                    let script = ctx.types.get(b.kind.max(0) as usize).and_then(|ty| b.cancel_script(ty));
                    if let Some(script) = script.filter(|&s| s >= 0) {
                        let at = crate::enemy::field_to_canvas(field, b.pos);
                        ctx.world.spawn_clipped(ctx.lib, ctx.anm, script as usize, at, field);
                    }
                }
            }
            !b.delete
        });
        // Converted bullets that arrived fire on this field (FUN_0051bd50).
        for a in std::mem::take(&mut st.arrived_reflects) {
            let et = bullet::Et { sprite: a.kind, color: a.color, absolute: true, angle: [a.angle, 0.0],
                speed: [a.speed, 0.0], count: [1, 1], aim: 1, ..bullet::Et::default() };
            bullet::fire(&et, a.pos, &mut ctx, &mut st.bullets);
        }
        if let Some(from) = hit {
            if let Some(p) = &mut self.player {
                p.hit(from, ctx.world, lib, st.field);
            }
        }
    }

    /// The enemy death routine (`FUN_0050c1f0`): death effect, chain, removal.
    fn kill(st: &mut SideState, e: &mut crate::enemy::EnemyData, world: &mut World, lib: &AnmLibrary) {
        // FUN_0051d4e0 case0 earns spirit credit at chains>9; case1 dispatches an
        // animal selected by distance, and earns5 further credit (default modifiers).
        st.spirit_origin = e.pos;
        if (1..=4).contains(&e.spirit_kind) {
            let distance = (e.pos[0]-st.player_pos[0]).powi(2)+(e.pos[1]-st.player_pos[1]).powi(2);
            st.outgoing_spirits.push(crate::spirit::Request {
                kind:crate::spirit::Kind::from_distance(distance), origin:e.pos,level:e.ivars[2] });
            st.spirit_credit = (st.spirit_credit+5).min(700);
        } else if st.chain > 9 {
            st.spirit_credit = (st.spirit_credit+30).min(700);
        }
        // The effect is turned to face away from the shot that landed the kill.
        let angle = match e.last_hit {
            Some(h) if (e.pos[0] - h[0]).powi(2) + (e.pos[1] - h[1]).powi(2) >= 0.04 => {
                (e.pos[1] - h[1]).atan2(e.pos[0] - h[0])
            }
            _ => -std::f32::consts::FRAC_PI_2,
        };
        if let Some(file) = st.anm_files[1] {
            let at = crate::enemy::field_to_canvas(st.field, e.pos);
            // The angle is the effect's rotation from its very first instruction.
            world.spawn_rotated(lib, file, e.death_effect(), at, st.field, angle);
        }
        // The kill explosion.
        let mut interp = th_anm::interp::Interp::<1>::default();
        interp.start([1.0], [e.rewards.a], e.rewards.d, 5);
        let vm = st.ghost_anm.map(|file| {
            let at = crate::enemy::field_to_canvas(st.field, e.pos);
            let id = world.spawn_clipped(lib, file, 16, at, st.field);
            if let Some(v) = world.get_mut(id) {
                v.interp_scale([e.rewards.a, e.rewards.a], e.rewards.d, 5);
            }
            id
        });
        st.explosions.push(Explosion {
            pos: e.pos,
            time: 0,
            duration: e.rewards.d,
            radius: 1.0,
            damage: e.rewards.b,
            radius_interp: interp,
            vm,
        });
        // Chain: each kill adds its reward (ins_1032) to a timer capped at 60 frames.
        st.chain_timer = (st.chain_timer + e.rewards.e).min(60);
        st.chain += 1;
        st.chain_max = st.chain_max.max(st.chain);
        e.delete = true;
    }

    /// Start the stage script (`main`).
    pub fn start(&mut self, prog: &Program) {
        let req = SpawnRequest {
            id: 0,
            difficulty: 0,
            sub: "main".into(),
            pos: [0.0, 0.0],
            hp: 0,
            score: 0,
            item: -1,
            mirrored: false,
            ivars: [0; 4],
            fvars: [0.0; 8],
        };
        let req = SpawnRequest { difficulty: self.st.difficulty, ..req };
        self.stage = Enemy::new(prog, &req, 0);
    }

    fn create_pending(&mut self, prog: &Program) {
        for req in std::mem::take(&mut self.st.spawn) {
            match Enemy::new(prog, &req, req.id) {
                Some(e) => self.enemies.push(e),
                None => self.st.alive.retain(|a| a.0 != req.id),
            }
        }
    }

    /// One frame. `keys` are the player's input bits ([`crate::player::input`]).
    pub fn tick(&mut self, prog: &Program, keys: u32, world: &mut World, lib: &AnmLibrary) {
        self.st.scope_circle = self.player.as_ref().filter(|p| p.scope.active()).map(|p| (p.scope.pos,p.scope.radius));
        let mut requests = Vec::new();
        self.st.pending_spirits.retain_mut(|p| {
            if let Some(req) = p.tick(self.st.ghost_anm,world,lib) { requests.push(req); false } else { true }
        });
        for req in requests { self.st.request(req); }
        let mut pending = std::mem::take(&mut self.st.pending_reflects);
        let types = self.st.bullet_types.clone();
        let anm = self.st.anm_files[0];
        pending.retain_mut(|p| match p.tick(anm, &types, &mut self.st, world, lib) {
            Some(arrival) => { self.st.arrived_reflects.push(arrival); false }
            None => true,
        });
        self.st.pending_reflects = pending;
        if let Some(stage) = &mut self.stage {
            stage.update(prog, &mut self.st, world, lib);
        }
        self.create_pending(prog);
        // Enemies created during the frame run in that same frame, like the game's list walk.
        let mut i = 0;
        while i < self.enemies.len() {
            self.enemies[i].update(prog, &mut self.st, world, lib);
            if std::mem::take(&mut self.st.clear_enemies) {
                self.clear_enemies();
            }
            let (id, member) = (self.enemies[i].data.id, self.enemies[i].data.wave_member);
            if let Some(a) = self.st.alive.iter_mut().find(|a| a.0 == id) {
                a.1 = member;
            }
            self.create_pending(prog);
            i += 1;
        }
        if let Some(p) = &mut self.player {
            p.update(keys, &mut self.enemies, world, lib, self.st.field);
            self.st.player_pos = p.pos;
            // Losing a heart raises this side's EX and boss levels by one while below 3
            // (FUN_00549830, status +0x38/+0x3c).
            if std::mem::take(&mut p.just_died) {
                for level in [&mut self.st.extra_level, &mut self.st.boss_level] {
                    if *level < 3 {
                        *level += 1;
                    }
                }
            }
            if let Some(level) = p.charge_event {
                use crate::charge::Level;
                let sub = match level { Level::Attack => None, Level::Skill => Some("Ecl_EtBreakLv2"),
                    Level::Extra => Some("Ecl_EtBreakLv3"), Level::Boss => Some("Ecl_EtBreakLv4") };
                if let Some(sub) = sub { self.st.request(script_request(sub,p.pos)); }
            }
        }
        self.st.scope_circle = self.player.as_ref().filter(|p| p.scope.active()).map(|p| (p.scope.pos,p.scope.radius));
        self.update_bullets(world, lib);
        self.update_explosions(world);
        for e in &mut self.enemies {
            if std::mem::take(&mut e.data.killed) && !e.data.delete {
                Self::kill(&mut self.st, &mut e.data, world, lib);
            }
        }
        if self.st.chain_timer > 0 {
            self.st.chain_timer -= 1;
            if self.st.chain_timer == 0 {
                self.st.chain = 0;
            }
        }
        crate::spirit::send_earned(&mut self.st);
        let mut k = 0;
        while k < self.enemies.len() {
            if self.enemies[k].data.delete {
                let mut e = self.enemies.remove(k);
                e.remove_sprites(world);
                self.st.alive.retain(|a| a.0 != e.data.id);
                self.st.boss_ids.retain(|&id| id != e.data.id);
            } else {
                k += 1;
            }
        }
    }
}

/// Match-level dispatch happens after both fields tick, so side ordering cannot grant
/// one attack an extra simulation tick. Higher attacks are sent to the opponent only.
pub fn prepare_charge_targets(sides: &mut [Side]) {
    if sides.len()!=2 { return; }
    let active = [sides[0].has_boss(),sides[1].has_boss()];
    for i in 0..2 { if let Some(p)=&mut sides[i].player { p.opponent_boss_active=active[1-i]; } }
}
pub fn dispatch_charges(sides: &mut [Side]) {
    if sides.len()!=2 { return; }
    // GhostCtrl's attacks cross to the opponent independently of charge releases.
    for sender in 0..2 {
        let field = sides[sender].st.field;
        let requests = std::mem::take(&mut sides[sender].st.outgoing_spirits);
        for req in requests {
            let receiver = &mut sides[1-sender].st;
            let limit = match receiver.spirit_frames {0..3600=>40,3600..7200=>60,7200..10800=>80,_=>999};
            if receiver.pending_spirits.len() < limit {
                let pending = crate::spirit::Pending::new(req,field,receiver);
                receiver.pending_spirits.push(pending);
            }
        }
    }
    // Bullets cancelled by a kill explosion fly to the opponent's field and fire there.
    for sender in 0..2 {
        let field = sides[sender].st.field;
        for req in std::mem::take(&mut sides[sender].st.outgoing_reflects) {
            let receiver = &mut sides[1 - sender].st;
            let pending = crate::reflect::Pending::new(req, field, receiver);
            receiver.pending_reflects.push(pending);
        }
    }
    sides[0].st.sent_spirits_pending = sides[1].st.pending_spirits.len();
    sides[1].st.sent_spirits_pending = sides[0].st.pending_spirits.len();
    let events: [Option<crate::charge::Level>;2] = std::array::from_fn(|i| sides[i].player.as_mut().and_then(|p| p.charge_event.take()));
    for (sender,event) in events.into_iter().enumerate() {
        use crate::charge::Level;
        match event {
            // The release controller (FUN_004e79e0) dispatches, then raises the sender's
            // level by one (capped at 7). The attack plays at the level shown on the gauge
            // HUD before the release ("Lv N" = rank N-1): the wiki's per-level Reimu tables
            // (EX Lv1 = 1 wave of 6, Lv2 = 1 of 8; boss Lv1-2 non-spell, Lv3-4 spell, Lv5-8
            // non-spell + spell) match ExtraAttack's ins_570 tables and Boss00Boss's rank
            // masks only at rank = pre-raise level.
            Some(Level::Extra) => {
                let rank=sides[sender].st.extra_level.min(7);
                sides[sender].st.extra_level=(rank+1).min(7);
                sides[1-sender].st.incoming_extra_level=rank;
                sides[1-sender].st.request(script_request("ExtraAttack",[0.0,0.0]));
            }
            Some(Level::Boss) => {
                let rank=sides[sender].st.boss_level.min(7);
                sides[sender].st.boss_level=(rank+1).min(7);
                sides[1-sender].st.incoming_boss_level=rank;
                sides[1-sender].st.request(script_request("BossAttack",[0.0,0.0]));
            }
            _ => {}
        }
    }
}
impl Side {
    /// ECL 525 (FUN_0050ca70): every enemy without flags 0x20/0x80/0x400 (or with 0x100)
    /// is destroyed through the normal death path; its item drops are cleared.
    fn clear_enemies(&mut self) {
        for e in &mut self.enemies {
            let f = e.data.flags;
            if !e.data.delete && (f & 0xa0 == 0 && f & 0x400 == 0 || f & 0x100 != 0) {
                e.data.item = -1;
                e.data.killed = true;
            }
        }
    }
    /// Development aid (not a game mechanic): remove every bullet, enemy, boss and
    /// EX/boss attack script on this field at once, without death effects or rewards.
    /// The stage script keeps running.
    pub fn dev_clear(&mut self, world: &mut World) {
        for mut e in self.enemies.drain(..) {
            e.remove_sprites(world);
        }
        self.st.spawn.clear();
        self.st.outgoing_reflects.clear();
        self.st.arrived_reflects.clear();
        for r in self.st.pending_reflects.drain(..) { r.delete(world); }
        self.st.outgoing_spirits.clear();
        for spirit in self.st.pending_spirits.drain(..) { spirit.delete(world); }
        self.st.sent_spirits_pending = 0;
        self.st.spirit_credit = 0;
        self.st.alive.clear();
        if let Some(id) = self.st.spell_background.take() {
            world.delete(id);
        }
        if let Some(id)=self.st.spell_start_effect.take() {world.delete(id);}
        self.st.boss_ids.clear();
        for b in self.st.bullets.drain(..) {
            if let Some(id) = b.vm {
                world.delete(id);
            }
        }
        for x in self.st.explosions.drain(..) {
            if let Some(id) = x.vm {
                world.delete(id);
            }
        }
    }
    pub fn has_boss(&self) -> bool {
        !self.st.boss_ids.is_empty() || self.st.spawn.iter().any(|s| s.sub=="BossAttack" || s.sub=="Boss00")
            || self.enemies.iter().any(|e| !e.data.delete && (e.data.sub=="BossAttack" || e.data.sub=="Boss00"))
    }
}
fn script_request(sub: &str,pos: [f32;2]) -> SpawnRequest {
    SpawnRequest { id:0,difficulty:0,sub:sub.into(),pos,hp:200,score:0,item:-1,mirrored:false,ivars:[0;4],fvars:[0.0;8] }
}

#[cfg(test)]
mod spirit_tests {
    use super::*;
    #[test]
    fn chain_deaths_earn_pure_spirits_and_spirit_deaths_send_distance_selected_animals() {
        let mut side=Side::new(crate::FIELDS[0],77,[0;256],[None;8]);
        let mut world=World::default(); let lib=AnmLibrary::default();
        side.st.chain=9;
        let mut enemy=crate::EnemyData::default();
        Side::kill(&mut side.st,&mut enemy,&mut world,&lib);
        assert_eq!(side.st.spirit_credit,0,"chain9 does not earn spirit credit");
        for _ in 0..4 {
            let mut enemy=crate::EnemyData::default();
            Side::kill(&mut side.st,&mut enemy,&mut world,&lib);
        }
        assert_eq!(side.st.spirit_credit,120);
        crate::spirit::send_earned(&mut side.st);
        assert_eq!(side.st.outgoing_spirits[0].kind,crate::spirit::Kind::Pure);
        side.st.outgoing_spirits.clear();
        for (distance,kind) in [(60.0,crate::spirit::Kind::Wolf),(100.0,crate::spirit::Kind::Otter),(140.0,crate::spirit::Kind::Eagle)] {
            let mut enemy=crate::EnemyData {pos:[distance,400.0],spirit_kind:1,ivars:[2,0,2,0],..Default::default()};
            Side::kill(&mut side.st,&mut enemy,&mut world,&lib);
            let req=side.st.outgoing_spirits.last().unwrap();
            assert_eq!(req.kind,kind); assert_eq!(req.level,2);
        }
    }
}
