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
    /// Side whose character's ECL program runs this enemy (`None`: this field's own).
    pub owner: Option<usize>,
}

/// Everything enemies can read or change besides themselves.
pub struct SideState {
    /// Field rectangle on the canvas `[x, y, w, h]`.
    pub field: [f32; 4],
    /// Difficulty (`DAT_0062b250`): 0 Easy, 1 Normal, 2 Hard, 3 Lunatic.
    pub difficulty: u8,
    pub player_pos: [f32; 2],
    pub opponent_pos: [f32;2],
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
    /// The sprite slots of each side's character program (slot 3 is its `plXXb.anm`); enemies
    /// run their owner's slots.
    pub anm_sets: [[Option<usize>; 8]; 4],
    /// This field's side index (0 left, 1 right).
    pub index: usize,
    pub spawn: Vec<SpawnRequest>,
    /// (id, wave member) of every live enemy, refreshed as enemies come and go.
    pub alive: Vec<(i32, bool)>,
    pub next_enemy_id: i32,
    pub last_enemy_id: i32,
    rng: u32,
    pub unknown_ops: HashMap<u16, u32>,
    pub unknown_vars: HashMap<i32, u32>,
    /// Kill chain (`FUN_00549780`): a timer topped up by each kill (capped at 60 frames),
    /// the current chain length and the best chain. FUN_005450e0 counts down and clears
    /// the current chain on expiry; the lifetime best is only a statistic.
    pub chain_timer: i32,
    pub chain: i32,
    pub chain_max: i32,
    /// Automatic EX / boss send counters of the EX attack manager (`FUN_0050df40`).
    pub auto: AutoSend,
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
    /// Remove the enemies flagged 0x100 (see ECL 512 with slot -1).
    pub clear_overrides: bool,
    /// `ghost.anm` (kill explosions).
    pub ghost_anm: Option<usize>,
    /// `ascii.anm`, for the combo counter.
    pub ascii_anm: Option<usize>,
    /// Displayed chain (player +0x22b8), remaining display frames (+0x2290), and glyph VMs.
    pub combo_shown: i32,
    pub combo_timer: i32,
    pub combo_vms: Vec<th_anm::VmId>,
    /// Lasers fired at this field's player.
    pub lasers: Vec<crate::laser::Laser>,
    /// Meter orbs and the other items (`ItemInf`), and the graze count (status +0x80).
    pub items: crate::item::Items,
    pub graze: i32,
    /// Sound requests this tick as `(sound id, DirectSound pan)`, like `FUN_004bb8d0`; the viewer
    /// drains and plays them. Use [`SideState::play_sound`].
    pub sounds: Vec<(u32, i32)>,
    /// Graze sparks in flight (bullet-manager pool type 4).
    pub sparks: Vec<crate::graze::Spark>,
    /// Kill explosions in progress.
    pub explosions: Vec<Explosion>,
    /// The charging aura of this field's player: Reimu's circle (centre, radius) or Marisa's
    /// full-height column (centre x in `pos[0]`, half width).
    pub scope_circle: Option<([f32; 2], f32)>,
    pub scope_column: bool,
    pub scope_character: crate::character::Character,
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
    /// White dot counter (FUN_00548bc0, max 1400) and special bullet counter (FUN_00548d00).
    pub dot_counter: i32,
    pub special_counter: i32,
    special_sequence: usize,
    /// Where the last kill or cancel happened (the origin of what they send).
    pub last_credit_pos: [f32; 2],
    pub last_special_pos: [f32; 2],
    /// Development rain (frames left): bullets and spirits fall on this field.
    pub rain: i32,
    pub sent_spirits_pending: usize,
    /// EX and boss attacks this field sent this round (status +0x4c / +0x50).
    pub ex_sent: u32,
    pub boss_sent: u32,
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

/// The counters behind the automatic EX ("C3") and boss ("C4") attacks that chains send to the
/// opponent (ExtraAttackInf, `FUN_0050df40`, fields +0x20/+0x24/+0x28/+0x30/+0x34).
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoSend {
    /// EX counter: sends at more than 99, then restarts at minus the penalty.
    pub ex: i32,
    /// Boss counter: sends at more than 199.
    pub boss: i32,
    /// Grows by 100 per automatic EX and drains by 1 per frame.
    pub ex_penalty: i32,
    /// After a send the counter cannot gain for this many frames (60 EX, 240 boss).
    pub ex_cooldown: i32,
    pub boss_cooldown: i32,
}

/// What one frame of [`AutoSend`] asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutoSends {
    pub ex: bool,
    /// A boss attack; an EX attack instead while the opponent already has a boss.
    pub boss: bool,
}

impl AutoSend {
    /// Both counters gain together, each only while its own cooldown is over
    /// (`FUN_0051d4e0`: +15 for a spirit kill at chain > 4; `FUN_0051c8c0`: +1 per
    /// cancelled bullet at chain > 9).
    pub fn gain(&mut self, amount: i32) {
        if self.ex_cooldown == 0 { self.ex += amount; }
        if self.boss_cooldown == 0 { self.boss += amount; }
    }

    /// One frame (`FUN_0050df40`); `frame` is the round's frame counter, which makes the
    /// counters drain by one every fourth frame.
    pub fn tick(&mut self, frame: i32) -> AutoSends {
        let mut out = AutoSends::default();
        if self.ex < 0 {
            self.ex += 1;
        } else if self.ex > 0 {
            if self.ex > 99 {
                out.ex = true;
                self.ex = -self.ex_penalty;
                self.ex_cooldown = 60;
                self.ex_penalty += 100;
            }
            if frame & 3 == 0 { self.ex -= 1; }
        }
        if self.ex_penalty > 0 { self.ex_penalty -= 1; }
        if self.ex_cooldown != 0 { self.ex_cooldown -= 1; }
        if self.boss > 0 {
            if self.boss > 199 {
                out.boss = true;
                self.boss = 0;
                self.boss_cooldown = 240;
            }
            if frame & 3 == 0 { self.boss -= 1; }
            if self.boss < 0 { self.boss = 0; }
        }
        if self.boss_cooldown != 0 { self.boss_cooldown -= 1; }
        out
    }
}

impl SideState {
    /// Whether the aura touches a body of `radius` at `pos`. The circle adds squared radii
    /// (`FUN_00416f10`); the column (`FUN_00417a60`) is a full-height band of half width `w`,
    /// tested here as the body's extent overlapping it.
    pub fn scope_contains(&self, pos: [f32;2], radius: f32) -> bool {
        if matches!(self.scope_character,crate::character::Character::Sanae|crate::character::Character::Ran) {
            return self.scope_circle.is_some_and(|(centre,r)| crate::scope::bullet_hit(self.scope_character,centre,r,pos,radius));
        }
        self.scope_circle.is_some_and(|(centre,r)| if self.scope_column {
            (pos[0]-centre[0]).abs() < r + radius
        } else {
            (pos[0]-centre[0]).powi(2)+(pos[1]-centre[1]).powi(2) < r*r+radius*radius
        })
    }
    /// Ran's body callback 00419620 tests the oriented body against both cross rectangles.
    pub fn scope_body(&self,body:&crate::enemy::EnemyData)->bool {
        if self.scope_character==crate::character::Character::Ran {
            return self.scope_circle.is_some_and(|(c,r)| {
                let angle=body.delta[1].atan2(body.delta[0]);
                [[r,7.0*r],[7.0*r,r]].into_iter().any(|size|
                    crate::laser::rect_rect(body.pos,body.hitbox,angle,c,size))
            });
        }
        self.scope_contains(body.pos,body.hitbox[0]*0.5)
    }
    /// The bullet callback: Reimu's circle again, Marisa's column by x alone (`FUN_00417a10`).
    pub fn scope_bullet(&self, pos: [f32;2], radius: f32) -> bool {
        self.scope_circle.is_some_and(|(c,r)| crate::scope::bullet_hit(self.scope_character,c,r,pos,radius))
    }
    /// `FUN_00548bc0`: feed the white dot counter (cap 1400) and remember where it happened.
    pub fn add_dots(&mut self, n: i32, pos: [f32; 2]) {
        self.dot_counter = (self.dot_counter + n).clamp(0, 1400);
        self.last_credit_pos = pos;
    }

    /// `FUN_00548d00`: feed the special bullet counter.
    pub fn add_special(&mut self, n: i32, pos: [f32; 2]) {
        self.special_counter += n;
        self.last_special_pos = pos;
    }

    pub fn cancel_lasers(&mut self, area: crate::laser::CancelArea, respect_invuln: bool, world: &mut World, lib: &AnmLibrary) {
        let Some(anm) = self.anm_files[0] else { return };
        let mut ctx = crate::laser::CancelContext { world, lib, types: &self.bullet_types, anm, field: self.field };
        crate::laser::cancel_lasers(&mut self.lasers, area, respect_invuln, &mut ctx);
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
    /// `FUN_004bb8d0(id, x * 1000 / 192)`: queue a sound effect panned by the field x of its
    /// source (`id` indexes the exe's sound table).
    pub fn play_sound(&mut self, id: u32, x: f32) {
        self.sounds.push((id, (x * 1000.0 / 192.0) as i32));
    }

    pub fn randf(&mut self) -> f32 {
        (self.rand() >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Uniform in [-1, 1).
    pub fn randf2(&mut self) -> f32 {
        self.randf() * 2.0 - 1.0
    }

    /// Queue an enemy for creation. Its id is allocated now, as the game creates enemies
    /// synchronously and scripts read the new id right away (variable -9931).
    /// Sprite slots of `owner`'s character program.
    pub fn anm_set(&self, owner: usize) -> &[Option<usize>; 8] {
        &self.anm_sets[owner.min(3)]
    }

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
        // FUN_004f1340 ends by queueing the ET's bullet sound (flag +0x474 & 0x20, set by ins_600),
        // panned by the origin x.
        if et.laser.sound[0] >= 0 {
            if crate::sfx::on(crate::sfx::Cat::EnemyFire) { self.play_sound(et.laser.sound[0] as u32, origin[0]); }
        }
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
            owner: None,
        });
    }
}

pub struct Side {
    pub st: SideState,
    pub enemies: Vec<Enemy>,
    /// The stage script thread group (`main`), run like an enemy with no body.
    pub stage: Option<Enemy>,
    pub player: Option<Player>,
    /// 2v2: the teammate sharing this field. It has its own HP and charge pool.
    pub partner: Option<Player>,
    /// Input bits for the partner (the viewer or a bot writes them each frame).
    pub partner_keys: u32,
    /// The partner's EX and boss levels (`SideState` holds the first player's).
    pub partner_levels: [u8; 2],
}

impl Side {
    /// Both players of this field (the first player, then the partner).
    pub fn players(&self) -> impl Iterator<Item = &Player> {
        self.player.iter().chain(self.partner.iter())
    }

    /// Player `slot` (0 = first player, 1 = partner).
    pub fn slot_mut(&mut self, slot: usize) -> Option<&mut Player> {
        if slot == 0 { self.player.as_mut() } else { self.partner.as_mut() }
    }

    /// Where bullets and enemies aim: the first player, or the partner once it is out.
    pub fn target_pos(&self) -> [f32; 2] {
        let out = |p: &Player| matches!(p.state, crate::player::State::Dead | crate::player::State::GameOver);
        self.players().find(|p| !out(p)).or(self.player.as_ref()).map_or([0.0, 400.0], |p| p.pos)
    }

    /// Player `slot`'s EX and boss levels.
    pub fn levels(&self, slot: usize) -> [u8; 2] {
        if slot == 0 { [self.st.extra_level, self.st.boss_level] } else { self.partner_levels }
    }

    /// Every player on this field has lost all hearts.
    pub fn defeated(&self) -> bool {
        self.player.is_some() && self.players().all(|p| p.state == crate::player::State::GameOver)
    }

    pub fn new(field: [f32; 4], seed: u32, wave_sequence: [u8; 256], anm_files: [Option<usize>; 8]) -> Self {
        Self {
            st: SideState {
                field,
                difficulty: 1,
                player_pos: [0.0, 400.0],
                opponent_pos: [0.0,400.0],
                level: 0,
                extra_level: 0, boss_level: 0, incoming_extra_level: 0, incoming_boss_level: 0,
                boss_ids: Vec::new(), boss_globals: HashMap::new(),
                level_table: [[0; 8]; 32],
                wave_pool: Vec::new(),
                wave_sequence,
                wave_counter: 0,
                wave_param: 0,
                anm_files,
                anm_sets: [anm_files; 4],
                index: 0,
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
                auto: AutoSend::default(),
                bullets: Vec::new(),
                bullet_types: Rc::new(Vec::new()),
                cancel_bullets: false,
                spell_background: None, spell_start_effect: None,
                clear_enemies: false,
                clear_overrides: false,
                ghost_anm: None,
                ascii_anm: None,
                combo_shown: 0,
                combo_timer: 0,
                combo_vms: Vec::new(),
                lasers: Vec::new(),
                items: crate::item::Items::new(),
                graze: 0,
                sparks: Vec::new(),
                sounds: Vec::new(),
                explosions: Vec::new(),
                scope_circle: None,
                scope_column: false, scope_character: Default::default(),
                spirit_credit: 0, spirit_origin: [0.0;2], spirit_pressure: 0, spirit_frames: 0,
                outgoing_spirits: Vec::new(), pending_spirits: Vec::new(), outgoing_reflects: Vec::new(), pending_reflects: Vec::new(), arrived_reflects: Vec::new(), rain: 0, dot_counter: 0, special_counter: 0, special_sequence: 0, last_credit_pos: [0.0; 2], last_special_pos: [0.0; 2],
                sent_spirits_pending: 0,
                ex_sent: 0,
                boss_sent: 0,
            },
            enemies: Vec::new(),
            stage: None,
            player: None,
            partner: None,
            partner_keys: 0,
            partner_levels: [0; 2],
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
            st.outgoing_reflects.push(crate::reflect::Request { kind, color, speed, origin, target: None, special_rank: None, owner: 0, script: None, tangents: None, aim_player: false });
            // FUN_00548bc0(30) and FUN_00548d00(1) per cancelled bullet.
            st.add_dots(30, origin);
            st.add_special(1, origin);
            // FUN_00549780(0): one more chain hit, no extra chain time.
            if st.chain > 9 { st.auto.gain(1); }
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
        let mut hit: Option<(usize, [f32; 2])> = None;
        let mut grazes: Vec<([f32; 2], u32)> = Vec::new();
        let ps: Vec<&Player> = self.player.iter().chain(self.partner.iter()).collect();
        let radii: Vec<f32> = ps.iter().map(|p| p.hitbox_radius()).collect();
        let (scope, scope_character) = (st.scope_circle, st.scope_character);
        'bullets: for b in st.bullets.iter_mut() {
            // Original contact callback applies its factor to the following movement tick.
            let dt = b.scope_time_scale;
            b.update_scaled(&mut ctx, dt);
            st.sounds.extend(b.sounds.drain(..));
            let slowed = b.state == 1 && b.hitbox > 0.0 && scope.is_some_and(|(c,r)| crate::scope::bullet_hit(scope_character,c,r,b.pos,b.hitbox));
            // The graze ring counts as contact too (iVar6 == 2 from FUN_00549e00); a hit does not.
            let d2_of = |b: &bullet::Bullet, p: &Player| (b.pos[0] - p.pos[0]).powi(2) + (b.pos[1] - p.pos[1]).powi(2);
            let reach2 = |b: &bullet::Bullet| (3.0 + 40.0f32.max(b.hitbox / 2.5)).powi(2) + b.hitbox * b.hitbox;
            let ring = b.state == 1 && !b.delete && b.hitbox > 0.0
                && ps.iter().zip(&radii).any(|(p, &r)| {
                    let d2 = d2_of(b, p);
                    !(r > 0.0 && d2 < r * r + b.hitbox * b.hitbox) && d2 < reach2(b)
                });
            b.scope_contact(slowed, slowed || ring, ctx.world, &mut *ctx.rand);
            for p in &ps {
                if let Some(r) = p.cancel_radius {
                    if d2_of(b, p) <= r * r && b.invuln == 0 {
                        b.cancel();
                        continue 'bullets;
                    }
                }
            }
            // Marisa's bomb band (FUN_004ee0d0): a live or appearing bullet touching the column.
            if ps.iter().any(|p| p.cancel_columns.iter().any(|&(x, w)|
                b.invuln == 0 && matches!(b.state, 1 | 2) && (b.pos[0] - x).abs() <= w + b.hitbox)) { b.cancel(); continue; }
            if ps.iter().any(|p| p.cancel_rects.iter().any(|&(pos,size)|
                b.invuln==0 && matches!(b.state,1|2) && crate::scope::rect_circle(pos,size,b.pos,b.hitbox))) { b.cancel(); continue; }
            if ps.iter().any(|p| p.cancel_circles.iter().any(|(pos,r)|
                b.intersects_cancel(*pos,*r,ctx.world,ctx.lib))) { b.cancel(); continue; }
            // FUN_00549e00: hit when d^2 < player_r^2 + bullet_r^2.
            let hit_by = if b.state == 1 && !b.delete {
                ps.iter().zip(&radii).position(|(p, &r)| r > 0.0 && d2_of(b, p) < r * r + b.hitbox * b.hitbox)
            } else { None };
            if let Some(i) = hit_by {
                hit = Some((i, b.pos));
            } else if b.state == 1 && !b.delete && b.hitbox > 0.0 && !ps.is_empty() {
                // Otherwise inside the graze ring (radius 3 + max(40, r / 2.5), plus r): the
                // bullet's gate decides whether this tick grazes (FUN_004f5130 / FUN_00549fb0).
                let near = ps.iter().any(|p| d2_of(b, p) < reach2(b));
                if b.graze.tick(near) {
                    // Colour +0x694: the type's table colour for mode 6, else 0xffd08080.
                    let rgb = types.get(b.kind.max(0) as usize)
                        .filter(|t| t.cancel_mode == 6)
                        .map_or(crate::graze::DEFAULT_RGB, |t| t.colours[b.color.clamp(0, 15) as usize][4] as u32 & 0xff_ffff);
                    grazes.push((b.pos, rgb));
                }
            }
        }
        for (at, rgb) in grazes {
            st.graze += 1;
            let delay = (st.rand() & 7) as i32;
            if crate::sfx::on(crate::sfx::Cat::Graze) { st.play_sound(0x2a, at[0]); }
            st.sparks.push(crate::graze::Spark::new(at, rgb, delay));
            let mut seed = st.rand() | 1;
            st.items.drop_orbs(at, || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            });
        }
        let mut sparks = std::mem::take(&mut st.sparks);
        sparks.retain_mut(|sp| sp.tick(&mut || st.randf()));
        st.sparks = sparks;
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
            if let Some(rank) = a.special {
                // EXOnmyoInf: the flying yin-yang becomes ExtraAttack00 (FUN_0053bc50).
                st.incoming_extra_level = rank.clamp(0, 7) as u8;
                st.request(SpawnRequest { id: 0, difficulty: 0, sub: if matches!(a.character,crate::character::Character::Sanae|crate::character::Character::Ran) {"ExAttack"} else {"ExtraAttack00"}.into(), pos: a.pos, hp: 200, score: 0,
                    item: -1, mirrored: false, ivars: [0; 4], fvars: [0.0; 8], owner: Some(a.owner) });
                continue;
            }
            let et = bullet::Et { sprite: a.kind, color: a.color, absolute: true, angle: [a.angle, 0.0],
                speed: [a.speed, 0.0], count: [1, 1], aim: 1,
                // 0051bd50 zeroes its local ET (flags +0x474 = 0: no fire sound) and sets +0x47c to -1.
                laser: crate::laser::LaserEt { sound: [0, -1], ..Default::default() }, ..bullet::Et::default() };
            bullet::fire(&et, a.pos, &mut ctx, &mut st.bullets);
        }
        if let Some((i, from)) = hit {
            if let Some(p) = if i == 0 { self.player.as_mut() } else { self.partner.as_mut() } {
                p.hit(from, ctx.world, lib, st.field);
            }
        }
    }

    /// The enemy death routine (`FUN_0050c1f0`): death effect, chain, removal.
    fn kill(st: &mut SideState, e: &mut crate::enemy::EnemyData, world: &mut World, lib: &AnmLibrary) {
        // FUN_0051d4e0 case0 earns spirit credit at chains>9; case1 dispatches an
        // animal selected by distance, and earns5 further credit (default modifiers).
        // FUN_0050c1f0 opens with the death sound (enemy +0x6328 = (id & 1) + 3 from creation).
        if crate::sfx::on(crate::sfx::Cat::EnemyDeath) { st.play_sound(3 + (e.id & 1) as u32, e.pos[0]); }
        st.spirit_origin = e.pos;
        if !e.suppress_kill_rewards {
        if (1..=4).contains(&e.spirit_kind) {
            let distance = (e.pos[0]-st.player_pos[0]).powi(2)+(e.pos[1]-st.player_pos[1]).powi(2);
            st.outgoing_spirits.push(crate::spirit::Request {
                kind:crate::spirit::Kind::from_distance(distance), origin:e.pos,level:e.ivars[2] });
            // FUN_0051d4e0 case 1: dots min(3*chain,60) + time bonus + 20; special bullets
            // time bonus + (N*5+5)*2 for a spirit reflected N times; spirit credit bonus + 5.
            let frames = st.spirit_frames;
            let n = (e.ivars[2] - 1).max(0);
            st.add_dots((3 * st.chain).min(60) + crate::reflect::dot_time_bonus(frames) + 20, e.pos);
            st.add_special(crate::reflect::special_time_bonus(frames) + (n * 5 + 5) * 2, e.pos);
            st.spirit_credit = (st.spirit_credit + crate::reflect::special_time_bonus(frames) + 5).min(700);
            if st.chain > 4 { st.auto.gain(15); }
        } else {
            // Case 0: dots min(3*chain,60) + time bonus + 40.
            st.add_dots((3 * st.chain).min(60) + crate::reflect::dot_time_bonus(st.spirit_frames) + 40, e.pos);
            if st.chain > 9 {
                st.spirit_credit = (st.spirit_credit+30).min(700);
            }
        }
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
        // ECL 1041 (enemy flag 0x20000000) skips this and the credit above as one block in
        // FUN_0050c1f0 (FUN_0051d4e0 + FUN_00549780); the death effect, explosion and orbs stay.
        if !e.suppress_kill_rewards {
            st.chain_timer = (st.chain_timer + e.rewards.e).min(60);
            st.chain += 1;
            st.chain_max = st.chain_max.max(st.chain);
        }
        // FUN_0050c1f0: the enemy's own item, then orbs for a chain beyond 5: 1 for 6..11,
        // 2 for 12..23 and 3 from 24.
        if (9..=14).contains(&e.item) {
            st.items.spawn(e.item, e.pos, -std::f32::consts::FRAC_PI_2, 2.2);
        }
        if st.chain > 5 {
            let orbs = (st.chain / 12 + 1).min(3);
            for _ in 0..orbs {
                let mut seed = st.rand() | 1;
                st.items.drop_orbs(e.pos, || {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
                });
            }
        }
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
            owner: None,
        };
        let req = SpawnRequest { difficulty: self.st.difficulty, ..req };
        self.stage = Enemy::new(prog, &req, 0, self.st.index);
    }

    fn create_pending(&mut self, progs: &[Program]) {
        for req in std::mem::take(&mut self.st.spawn) {
            match Enemy::new(&progs[req.owner.unwrap_or(self.st.index)], &req, req.id, self.st.index) {
                Some(e) => self.enemies.push(e),
                None => self.st.alive.retain(|a| a.0 != req.id),
            }
        }
    }

    /// The scope of the first player whose scope is active (else the first player's character).
    fn update_scope(&mut self) {
        let scoped = self.player.iter().chain(self.partner.iter()).find(|p| p.scope.active()).or(self.player.as_ref());
        let st = &mut self.st;
        st.scope_circle = scoped.filter(|p| p.scope.active()).map(|p| (p.scope.pos, p.scope.radius));
        st.scope_column = scoped.is_some_and(|p| p.character == crate::character::Character::Marisa);
        st.scope_character = scoped.map_or(Default::default(), |p| p.character);
    }

    /// One frame. `keys` are the player's input bits ([`crate::player::input`]).
    pub fn tick(&mut self, progs: &[Program], keys: u32, world: &mut World, lib: &AnmLibrary) {
        self.update_scope();
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
        self.rain_tick();
        if let Some(stage) = &mut self.stage {
            stage.update(&progs[self.st.index], &mut self.st, world, lib);
        }
        self.create_pending(progs);
        // Enemies created during the frame run in that same frame, like the game's list walk.
        let mut i = 0;
        while i < self.enemies.len() {
            let owner = self.enemies[i].data.owner;
            self.enemies[i].update(&progs[owner], &mut self.st, world, lib);
            if std::mem::take(&mut self.st.clear_enemies) {
                self.clear_enemies();
            }
            if std::mem::take(&mut self.st.clear_overrides) {
                for e in &mut self.enemies {
                    if !e.data.delete && e.data.flags & 0x100 != 0 && e.data.boss_slot.is_none() {
                        e.data.item = -1;
                        e.data.killed = true;
                    }
                }
            }
            let (id, member) = (self.enemies[i].data.id, self.enemies[i].data.wave_member);
            if let Some(a) = self.st.alive.iter_mut().find(|a| a.0 == id) {
                a.1 = member;
            }
            self.create_pending(progs);
            i += 1;
        }
        for slot in 0..2 {
            let k = if slot == 0 { keys } else { self.partner_keys };
            let field = self.st.field;
            let Some(p) = (if slot == 0 { self.player.as_mut() } else { self.partner.as_mut() }) else { continue };
            p.update(k, &mut self.enemies, world, lib, field);
            // The player has no side index; a sound without its own pan is panned to its side.
            let side = if field[0] < 320.0 { 0 } else { 1 };
            let side_pan = if side == 0 { -100 } else { 100 } * 1000 / 192;
            let charge = p.charge.sounds.drain(..).map(|(id, pan)| (id, (pan == crate::sfx::STOP).then_some(pan)));
            for (id, pan) in p.sounds.drain(..).chain(charge).collect::<Vec<_>>() {
                let id = if id & crate::sfx::PER_SIDE != 0 { (id & !crate::sfx::PER_SIDE) + side } else { id };
                self.st.sounds.push((id, pan.unwrap_or(side_pan)));
            }
            // Losing a heart raises this player's EX and boss levels by one while below 3
            // (FUN_00549830, status +0x38/+0x3c).
            if std::mem::take(&mut p.just_died) {
                let levels = if slot == 0 { [&mut self.st.extra_level, &mut self.st.boss_level] }
                    else { let [e, b] = &mut self.partner_levels; [e, b] };
                for level in levels {
                    if *level < 3 {
                        *level += 1;
                    }
                }
            }
            if let Some(level) = p.charge_event {
                use crate::charge::Level;
                let sub = match level { Level::Attack => None, Level::Skill => Some("Ecl_EtBreakLv2"),
                    Level::Extra => Some("Ecl_EtBreakLv3"), Level::Boss => Some("Ecl_EtBreakLv4") };
                let pos = p.pos;
                if let Some(sub) = sub { self.st.request(script_request(sub,pos,None)); }
            }
        }
        self.st.player_pos = self.target_pos();
        self.update_scope();
        self.update_bullets(world, lib);
        self.update_lasers(world, lib);
        self.update_explosions(world);
        for e in &mut self.enemies {
            if std::mem::take(&mut e.data.killed) && !e.data.delete {
                Self::kill(&mut self.st, &mut e.data, world, lib);
            }
        }
        self.tick_chain();
        crate::spirit::send_earned(&mut self.st);
        self.update_combo(world, lib);
        // FUN_00509980: a body with flag 0x200 that is not touching the player but is inside
        // its graze ring throws an orb at the player every 6th frame of its own time.
        let mut at = None;
        for p in self.players().filter(|p| !matches!(p.state, crate::player::State::Dead)) {
            for e in &self.enemies {
                let d = &e.data;
                if d.flags & 0x200 == 0 || d.flags & (crate::enemy::flag::NO_HITBOX | crate::enemy::flag::HIDDEN) != 0 || d.time % 6 != 0 {
                    continue;
                }
                let r = d.hitbox[0] / 2.0;
                let d2 = (p.pos[0] - d.pos[0]).powi(2) + (p.pos[1] - d.pos[1]).powi(2);
                let reach = 3.0 + 40.0f32.max(r / 2.5);
                if d2 >= 9.0 + r * r && d2 < reach * reach + r * r {
                    at = Some(p.pos);
                }
            }
        }
        {
            if let Some(at) = at {
                // FUN_00549fb0(player pos, colour 0): the same award as a bullet graze, with a
                // black spark and sound 0x2a at the player.
                self.st.graze += 1;
                let delay = (self.st.rand() & 7) as i32;
                if crate::sfx::on(crate::sfx::Cat::Graze) { self.st.play_sound(0x2a, at[0]); }
                self.st.sparks.push(crate::graze::Spark::new(at, 0, delay));
                let mut seed = self.st.rand() | 1;
                self.st.items.drop_orbs(at, || {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
                });
            }
        }
        let mut items = std::mem::take(&mut self.st.items);
        let orbs = {
            let ps: Vec<&Player> = self.player.iter().chain(self.partner.iter()).collect();
            items.update(&ps, &self.st, world, lib)
        };
        self.st.sounds.extend(items.sounds.drain(..));
        self.st.items = items;
        for (slot, &n) in orbs.iter().enumerate() {
            if n > 0 {
                if let Some(p) = if slot == 0 { self.player.as_mut() } else { self.partner.as_mut() } {
                    p.meter = (p.meter + crate::item::meter_for(n)).clamp(0, 2500);
                }
            }
        }
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

impl Side {
    /// The lasers on this field: widen, hurt the player or get grazed (`FUN_0052b550`,
    /// `FUN_0052b250`).
    fn update_lasers(&mut self, world: &mut World, lib: &AnmLibrary) {
        let field = self.st.field;
        // The player's circle/column clearing also visits the laser manager. Script clears
        // use the same geometry with a separate immunity policy (00530c60/005303f0).
        let mut areas = Vec::new();
        for p in self.players() {
            areas.extend(p.cancel_circles.iter().map(|&(pos,radius)| crate::laser::CancelArea::Circle {pos,radius}));
            areas.extend(p.cancel_rects.iter().map(|&(pos,size)| crate::laser::CancelArea::Rect {pos,size}));
            areas.extend(p.cancel_columns.iter().map(|&(x,half_width)| crate::laser::CancelArea::Column {x,half_width}));
        }
        for area in areas { self.st.cancel_lasers(area, true, world, lib); }
        let mut lasers = std::mem::take(&mut self.st.lasers);
        let mut hit: Option<usize> = None;
        let mut grazed = None;
        let mut spark_at = None;
        let players: Vec<([f32; 2], f32, bool)> =
            self.players().map(|p| (p.pos, p.hitbox_radius().max(3.0), p.hitbox_radius() > 0.0)).collect();
        lasers.retain_mut(|l| {
            let alive = l.update(world, field);
            if alive {
                for (i, &(pos, radius, vulnerable)) in players.iter().enumerate() {
                    match l.contact(pos, radius) {
                        crate::laser::Contact::Hit if vulnerable => hit = Some(i),
                        crate::laser::Contact::Hit | crate::laser::Contact::Graze if l.graze_tick() => {
                            grazed = Some(pos);
                            spark_at = Some(l.graze_point(pos));
                        }
                        _ => {}
                    }
                }
            }
            if !alive {
                l.delete(world);
            }
            alive
        });
        self.st.lasers = lasers;
        if let Some(at) = grazed {
            self.st.graze += 1;
            let delay = (self.st.rand() & 7) as i32;
            // 00549fb0 pans by the effect position (the closest point of the laser).
            let spark = spark_at.unwrap_or(at);
            if crate::sfx::on(crate::sfx::Cat::Graze) { self.st.play_sound(0x2a, spark[0]); }
            self.st.sparks.push(crate::graze::Spark::new(spark, crate::graze::DEFAULT_RGB, delay));
            let mut seed = self.st.rand() | 1;
            self.st.items.drop_orbs(at, || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            });
        }
        if let Some(slot) = hit {
            if let Some(p) = if slot == 0 { self.player.as_mut() } else { self.partner.as_mut() } {
                let from = p.pos;
                p.hit(from, world, lib, field);
            }
        }
    }

    /// Native chain expiry is in FUN_005450e0. Zero-time cancellation hits can occur
    /// after expiry, so also clear a chain whose timer was already zero this frame.
    fn tick_chain(&mut self) {
        self.st.chain_timer = self.st.chain_timer.saturating_sub(1).max(0);
        if self.st.chain_timer == 0 {
            self.st.chain = 0;
        }
    }

    /// FUN_00546080 draws the counter; FUN_005450e0 counts its display timer DOWN.
    /// Once reward-chain time expires, the latched number flashes yellow/gray until
    /// display time expires. The displayed count never contributes to gameplay rewards.
    fn update_combo(&mut self, world: &mut World, lib: &AnmLibrary) {
        use crate::text::{big_text, Align};
        let st = &mut self.st;
        for id in st.combo_vms.drain(..) {
            world.delete(id);
        }
        if st.combo_timer > 0 {
            st.combo_timer -= 1;
            if st.combo_timer == 0 { st.combo_shown = 0; }
        }
        if st.chain > 1 && st.combo_shown < st.chain && st.chain_timer > 0 {
            st.combo_shown = st.chain;
            st.combo_timer = 120;
        }
        let (Some(file), true) = (st.ascii_anm, st.combo_shown != 0) else { return };
        let color = if st.chain_timer <= 0 && st.combo_timer % 6 >= 3 {
            [128.0; 3]
        } else {
            [255.0, 255.0, 0.0]
        };
        let x = if st.field[0] < 320.0 { 48.0 } else { 360.0 };
        let pos = [x, 32.0];
        st.combo_vms = big_text(world, lib, file, &st.combo_shown.to_string(), pos, 1.0, Align::Right, color);
        st.combo_vms.extend(big_text(world, lib, file, "hits", pos, 0.6, Align::Left, color));
    }
}

/// Match-level dispatch happens after both fields tick, so side ordering cannot grant
/// one attack an extra simulation tick. Higher attacks are sent to the opponent only.
pub fn prepare_charge_targets(sides: &mut [Side]) {
    if sides.len()!=2 { return; }
    let positions=[sides[0].target_pos(),sides[1].target_pos()];
    for i in 0..2 {sides[i].st.opponent_pos=positions[1-i];}
    let active = [sides[0].has_boss(),sides[1].has_boss()];
    for i in 0..2 {
        for slot in 0..2 { if let Some(p)=sides[i].slot_mut(slot) { p.opponent_boss_active=active[1-i]; } }
    }
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
    // Chains build the automatic EX / boss counters (FUN_0050df40); crossing a threshold
    // sends the attack without raising the sender's level (only the release controller does).
    for sender in 0..2 {
        let frame = sides[sender].st.spirit_frames;
        let sends = sides[sender].st.auto.tick(frame);
        // The boss counter falls back to an EX attack while the opponent has a boss (as a
        // held charge does).
        let boss_as_ex = sends.boss && sides[1 - sender].has_boss();
        if sends.ex || boss_as_ex {
            let rank = sides[sender].st.extra_level.min(7);
            sides[1 - sender].st.incoming_extra_level = rank;
            sides[1 - sender].st.request(script_request("ExtraAttack", [0.0, 0.0], Some(sender)));
            sides[sender].st.ex_sent += 1;
            // FUN_0050d740 family -> FUN_0050db70: the launch cut-in plays 0x4d at centre pan.
            if crate::sfx::on(crate::sfx::Cat::Release) { sides[sender].st.sounds.push((0x4d, 0)); }
        }
        if sends.boss && !boss_as_ex {
            let rank = sides[sender].st.boss_level.min(7);
            sides[1 - sender].st.incoming_boss_level = rank;
            sides[1 - sender].st.request(script_request("BossAttack", [0.0, 0.0], Some(sender)));
            sides[sender].st.boss_sent += 1;
            // FUN_0050d740 family -> FUN_0050db70: the launch cut-in plays 0x4d at centre pan.
            if crate::sfx::on(crate::sfx::Cat::Release) { sides[sender].st.sounds.push((0x4d, 0)); }
        }
    }
    // The dot and special-bullet counters send attacks (FUN_005450e0 and Reimu's FUN_0053ac80).
    for sender in 0..2 {
        let frames = sides[sender].st.spirit_frames;
        let mut active = sides[1 - sender].enemies.iter().filter(|e| matches!(e.data.sub.as_str(),"ExtraAttack00"|"ExAttack")).count()
            + sides[1 - sender].st.pending_reflects.iter().filter(|p| p.is_special()).count();
        let rank = sides[sender].st.extra_level as i32;
        let character = sides[sender].player.as_ref().map_or(Default::default(),|p|p.character);
        let st = &mut sides[sender].st;
        // White dots: threshold 150 minus the pressure term, one bullet each.
        let threshold = (150 - (st.spirit_pressure as f32 / 200.0 * 100.0) as i32).max(1);
        while st.dot_counter >= threshold {
            st.dot_counter -= threshold;
            let roll = st.rand();
            let (a, r) = (st.randf() * std::f32::consts::TAU, st.randf() * 24.0);
            let o = st.last_credit_pos;
            let origin = [o[0] + a.cos() * r, o[1] + a.sin() * r];
            let target = [st.randf2() * 148.0, st.randf() * 16.0];
            let color = (st.rand() & 0xf) as i32;
            st.outgoing_reflects.push(crate::reflect::Request { kind: crate::reflect::dot_kind(frames, roll), color,
                speed: 0.5, origin, target: Some(target), special_rank: None, owner: 0, script: None, tangents: None, aim_player: false });
        }
        // Special bullets: threshold 100/90/90/80 by difficulty (Normal assumed), limited.
        // Reimu's special bullet is a yin-yang (EXOnmyoInf); Marisa's a laser orb (EXLaserInf,
        // bullet.anm script 63). Both land as ExtraAttack00 of the sender's program.
        let threshold=if character==crate::character::Character::Ran {65} else {90};
        while st.special_counter >= threshold {
            st.special_counter -= threshold;
            if active >= crate::reflect::special_limit(frames) { break; }
            active += 1;
            let target = match character {
                crate::character::Character::Sanae => {
                    let k=st.special_sequence;st.special_sequence+=1;
                    let x=if k%2==0 {-148.0+st.randf()*32.0} else {148.0-st.randf()*32.0};
                    [x,st.randf()*20.0]
                }
                crate::character::Character::Ran => {
                    let k=st.special_sequence%11;st.special_sequence+=1;
                    match k {
                        0|3|7 => [st.randf2()*132.0,0.0],
                        4|8 => [st.randf2()*132.0,448.0],
                        _ => [if matches!(k,1|6|9) {-148.0} else {148.0},st.randf()*416.0+16.0],
                    }
                }
                _ => [st.randf2()*160.0,st.randf()*60.0],
            };
            st.outgoing_reflects.push(crate::reflect::Request { kind: 44, color: 1, speed: 0.0,
                origin: st.last_special_pos, target: Some(target), special_rank: Some(rank), owner: sender,
                script: match character {crate::character::Character::Reimu=>None,crate::character::Character::Marisa=>Some(63),crate::character::Character::Sanae=>Some(61),crate::character::Character::Ran=>Some(14)}, tangents: None, aim_player: false });
        }
    }
    // Bullets cancelled by a kill explosion fly to the opponent's field and fire there.
    for sender in 0..2 {
        let field = sides[sender].st.field;
        let character=sides[sender].player.as_ref().map_or(Default::default(),|p|p.character);
        for mut req in std::mem::take(&mut sides[sender].st.outgoing_reflects) {
            if req.aim_player {
                let x = sides[1 - sender].target_pos()[0];
                req.target = Some([x, 448.0]);
            }
            let receiver = &mut sides[1 - sender].st;
            let pending = crate::reflect::Pending::new_character(req, field, receiver, character);
            receiver.pending_reflects.push(pending);
        }
    }
    sides[0].st.sent_spirits_pending = sides[1].st.pending_spirits.len();
    sides[1].st.sent_spirits_pending = sides[0].st.pending_spirits.len();
    // Program owners: the first players are 0 and 1, their partners 2 and 3 (see `anm_set`).
    for (sender, slot) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let Some(mut event) = sides[sender].slot_mut(slot).and_then(|p| p.charge_event.take()) else { continue };
        use crate::charge::Level;
        let owner = sender + 2 * slot;
        // A partner has its own EX and boss levels; swap them in while it sends.
        if slot == 1 {
            let s = &mut sides[sender];
            std::mem::swap(&mut s.st.extra_level, &mut s.partner_levels[0]);
            std::mem::swap(&mut s.st.boss_level, &mut s.partner_levels[1]);
        }
        // Only one boss attack can be on a field: a second one (the teammate's, sent in the
        // same frame) is that player's EX attack instead.
        if event == Level::Boss && sides[1 - sender].has_boss() { event = Level::Extra; }
        let character = sides[sender].slot_mut(slot).map_or(Default::default(), |p| p.character);
        let origin_pos = sides[sender].slot_mut(slot).map_or([0.0; 2], |p| p.pos);
        match event {
            // The release controller (FUN_004e79e0) dispatches, then raises the sender's
            // level by one (capped at 7). The attack plays at the level shown on the gauge
            // HUD before the release ("Lv N" = rank N-1): the wiki's per-level Reimu tables
            // (EX Lv1 = 1 wave of 6, Lv2 = 1 of 8; boss Lv1-2 non-spell, Lv3-4 spell, Lv5-8
            // non-spell + spell) match ExtraAttack's ins_570 tables and Boss00Boss's rank
            // masks only at rank = pre-raise level.
            Level::Extra => {
                let rank=sides[sender].st.extra_level.min(7);
                sides[sender].st.extra_level=(rank+1).min(7);
                sides[1-sender].st.incoming_extra_level=rank;
                sides[1-sender].st.request(script_request("ExtraAttack",[0.0,0.0],Some(owner)));
            sides[sender].st.ex_sent += 1;
            // FUN_0050d740 family -> FUN_0050db70: the launch cut-in plays 0x4d at centre pan.
            if crate::sfx::on(crate::sfx::Cat::Release) { sides[sender].st.sounds.push((0x4d, 0)); }
            }
            Level::Boss => {
                let rank=sides[sender].st.boss_level.min(7);
                sides[sender].st.boss_level=(rank+1).min(7);
                sides[1-sender].st.incoming_boss_level=rank;
                sides[1-sender].st.request(script_request("BossAttack",[0.0,0.0],Some(owner)));
            sides[sender].st.boss_sent += 1;
            // FUN_0050d740 family -> FUN_0050db70: the launch cut-in plays 0x4d at centre pan.
            if crate::sfx::on(crate::sfx::Cat::Release) { sides[sender].st.sounds.push((0x4d, 0)); }
            }
            // Marisa's skill sends six laser orbs that curve down to the opponent player's column
            // (FUN_00417b90): tangents of length 600 turned a sixth of a turn apart from straight up.
            Level::Skill if character == crate::character::Character::Marisa => {
                let rank = sides[sender].st.extra_level as i32;
                let origin = origin_pos;
                for k in 0..6 {
                    let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::FRAC_PI_3;
                    let t = [a.cos() * 600.0, a.sin() * 600.0];
                    sides[sender].st.outgoing_reflects.push(crate::reflect::Request { kind: 44, color: 1, speed: 0.0, origin,
                        target: None, special_rank: Some(rank), owner, script: Some(63), tangents: Some((t, t)), aim_player: true });
                }
            }
            _ => {}
        }
        if slot == 1 {
            let s = &mut sides[sender];
            std::mem::swap(&mut s.st.extra_level, &mut s.partner_levels[0]);
            std::mem::swap(&mut s.st.boss_level, &mut s.partner_levels[1]);
        }
    }
    // Requests the events above made go out on the next frame's pass.
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
    /// `FUN_00515540` at a round's result: every enemy is destroyed through the death path
    /// with its drops cleared (`FUN_0050ca70`), every bullet is cancelled (`FUN_004edf90(0)`)
    /// and the lasers are removed (`FUN_00530bd0(1)`). The fields stop updating afterwards,
    /// so this finishes at once.
    pub fn round_end_clear(&mut self, world: &mut World, lib: &AnmLibrary) {
        // FUN_0050ca70 spares flags 0x20/0x80/0x400 unless 0x100 is set.
        let mut kept = Vec::new();
        for mut e in std::mem::take(&mut self.enemies) {
            let f = e.data.flags;
            if e.data.delete || !(f & 0xa0 == 0 && f & 0x400 == 0 || f & 0x100 != 0) {
                kept.push(e);
                continue;
            }
            e.data.item = -1;
            Self::kill(&mut self.st, &mut e.data, world, lib);
            e.remove_sprites(world);
            self.st.alive.retain(|a| a.0 != e.data.id);
            self.st.boss_ids.retain(|&id| id != e.data.id);
        }
        self.enemies = kept;
        let field = self.st.field;
        let types = self.st.bullet_types.clone();
        let anm = self.st.anm_files[0];
        for mut b in std::mem::take(&mut self.st.bullets) {
            if let Some(id) = b.vm { world.delete(id); }
            b.cancel();
            let script = types.get(b.kind.max(0) as usize).and_then(|ty| b.cancel_script(ty));
            if let (Some(anm), Some(script)) = (anm, script.filter(|&s| s >= 0)) {
                let at = crate::enemy::field_to_canvas(field, b.pos);
                world.spawn_clipped(lib, anm, script as usize, at, field);
            }
        }
        for mut l in std::mem::take(&mut self.st.lasers) {
            l.delete(world);
        }
    }

    /// Frames a development rain lasts.
    pub const RAIN_FRAMES: i32 = 600;

    /// Development aid (not a game mechanic): rain bullets and spirits onto this field. Every
    /// other frame a small bullet falls from the top at a random place; every 40th frame a
    /// spirit of a random kind appears in the upper part.
    pub fn start_rain(&mut self) {
        self.st.rain = Self::RAIN_FRAMES;
    }

    fn rain_tick(&mut self) {
        if self.st.rain <= 0 {
            return;
        }
        self.st.rain -= 1;
        let left = self.st.rain;
        let st = &mut self.st;
        if left % 2 == 0 {
            let x = st.randf2() * 140.0;
            let a = crate::reflect::Arrival {
                kind: (st.rand() % 4) as i32,
                color: (st.rand() % 16) as i32,
                speed: 2.0 + st.randf() * 1.5,
                angle: std::f32::consts::FRAC_PI_2 + st.randf2() * 0.15,
                pos: [x, 0.0],
                special: None,
                owner: 0, character:Default::default(),
            };
            st.arrived_reflects.push(a);
        }
        if left % 40 == 0 {
            let sub = ["GhostPure", "GhostWolf", "GhostOtterL", "GhostOtterR", "GhostEagle"][(st.rand() % 5) as usize];
            let pos = [st.randf2() * 120.0, 40.0 + st.randf() * 120.0];
            st.request(SpawnRequest { id: 0, difficulty: 0, sub: sub.into(), pos, hp: 200, score: 0, item: -1,
                mirrored: false, ivars: [1, 0, 1, 0], fvars: [0.0; 8], owner: None });
        }
    }

    /// Development aid (not a game mechanic): remove every bullet, enemy, boss and
    /// EX/boss attack script on this field at once, without death effects or rewards.
    /// The stage script keeps running.
    pub fn dev_clear(&mut self, world: &mut World) {
        self.st.chain_timer = 0;
        self.tick_chain();
        self.st.combo_shown = 0;
        self.st.combo_timer = 0;
        for id in self.st.combo_vms.drain(..) { world.delete(id); }
        for mut e in self.enemies.drain(..) {
            e.remove_sprites(world);
        }
        self.st.spawn.clear();
        self.st.rain = 0;
        self.st.dot_counter = 0;
        self.st.special_counter = 0;
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
        for mut laser in self.st.lasers.drain(..) { laser.delete(world); }
        for x in self.st.explosions.drain(..) {
            if let Some(id) = x.vm {
                world.delete(id);
            }
        }
    }
    /// The program owner (the sender's player index) of the boss on this field, if there is one.
    pub fn boss_owner(&self) -> Option<usize> {
        let boss = |sub: &str| matches!(sub, "BossAttack" | "Boss00" | "Boss00Boss");
        self.enemies.iter().find(|e| !e.data.delete && (e.data.boss_slot.is_some() || boss(&e.data.sub))).map(|e| e.data.owner)
            .or_else(|| self.st.spawn.iter().find(|s| boss(&s.sub)).and_then(|s| s.owner))
    }

    pub fn has_boss(&self) -> bool {
        !self.st.boss_ids.is_empty() || self.st.spawn.iter().any(|s| s.sub=="BossAttack" || s.sub=="Boss00")
            || self.enemies.iter().any(|e| !e.data.delete && (e.data.sub=="BossAttack" || e.data.sub=="Boss00"))
    }
}
fn script_request(sub: &str,pos: [f32;2],owner: Option<usize>) -> SpawnRequest {
    SpawnRequest { id:0,difficulty:0,sub:sub.into(),pos,hp:200,score:0,item:-1,mirrored:false,ivars:[0;4],fvars:[0.0;8],owner }
}

#[cfg(test)]
mod spirit_tests {
    #[test]
    fn expired_chain_does_not_boost_next_kill_rewards() {
        for timer in [1, 0, -1] {
            let mut side = Side::new(crate::FIELDS[0], 77, [0; 256], [None; 8]);
            let mut world = World::default();
            let lib = AnmLibrary::default();
            side.st.chain = 24;
            side.st.chain_max = 24;
            side.st.chain_timer = timer;
            side.tick_chain();
            assert_eq!((side.st.chain, side.st.chain_timer), (0, 0));
            let mut enemy = crate::EnemyData::default();
            Side::kill(&mut side.st, &mut enemy, &mut world, &lib);
            assert_eq!(side.st.chain, 1);
            assert_eq!(side.st.dot_counter, 40, "no old-chain dot bonus");
            assert_eq!(side.st.spirit_credit, 0);
            assert!(side.st.items.list.is_empty(), "no old-chain meter orbs");
            assert_eq!(side.st.chain_max, 24, "best is only a statistic");
            // Hits awarding no time must not accumulate across simulation frames.
            side.st.chain_timer = 0;
            side.tick_chain();
            assert_eq!(side.st.chain, 0);
            let mut spirit = crate::EnemyData { spirit_kind: 1, ..Default::default() };
            Side::kill(&mut side.st, &mut spirit, &mut world, &lib);
            assert_eq!(side.st.auto.ex, 0, "no old-chain automatic attack credit");
            assert_eq!(side.st.auto.boss, 0);
        }
    }

    #[test]
    fn automatic_ex_and_boss_counters_follow_the_native_rules() {
        use super::AutoSend;
        // Twelve chained spirit kills: the first five only build the chain, the next seven
        // add 15 each (105) and the EX counter passes 99 on the twelfth.
        let mut a = AutoSend::default();
        let mut chain = 0;
        let mut sent = None;
        for kill in 1..=12 {
            if chain > 4 { a.gain(15); }
            chain += 1;
            if a.ex > 99 && sent.is_none() { sent = Some(kill); }
        }
        assert_eq!(sent, Some(12));
        assert!(a.tick(1).ex);
        // The send restarts the counter at minus the (still zero) penalty, grows the penalty by
        // 100 (draining one that frame), and blocks gains for 60 frames.
        assert_eq!((a.ex, a.ex_penalty, a.ex_cooldown), (0, 99, 59));
        a.gain(15);
        assert_eq!(a.ex, 0, "no gain during the cooldown");
        // The boss counter needs more than 199.
        let mut b = AutoSend::default();
        b.boss = 200;
        assert!(b.tick(1).boss);
        assert_eq!((b.boss, b.boss_cooldown), (0, 239));
        // Counters drain by one every fourth frame.
        let mut c = AutoSend::default();
        c.ex = 10;
        for frame in 0..8 { c.tick(frame); }
        assert_eq!(c.ex, 8);
    }

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
