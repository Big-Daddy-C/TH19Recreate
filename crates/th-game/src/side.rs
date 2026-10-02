//! One player's side of the VS screen: its field, enemies, and the wave system.
//!
//! TH19 stages are an endless stream of waves. The stage script (`common.ecl` `main`, via
//! `WorldWave01`) fills a weighted pool of wave ids (`ins_1010`/`ins_1011`), then repeatedly
//! spawns one (`ins_1012`) and waits for it to finish. The wave picked is
//! `pool[sequence[counter] % pool_len]`, where `sequence` is 256 values rolled once from the
//! game's synchronised RNG (`FUN_005378c0`) — the same for both players.

use std::collections::HashMap;

use th_anm::{AnmLibrary, World};
use th_ecl::Program;

use crate::enemy::Enemy;
use crate::player::Player;

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
    pub player_pos: [f32; 2],
    /// TH19's escalating level (0..7), selecting columns of the level tables.
    pub level: usize,
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
}

impl SideState {
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
        req.id = self.next_enemy_id;
        self.next_enemy_id = (self.next_enemy_id + 1) & 0xffff;
        if self.next_enemy_id == 0 {
            self.next_enemy_id = 1;
        }
        self.last_enemy_id = req.id;
        self.alive.push((req.id, false));
        self.spawn.push(req);
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
                player_pos: [0.0, 400.0],
                level: 0,
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
            },
            enemies: Vec::new(),
            stage: None,
            player: None,
        }
    }

    /// The enemy death routine (`FUN_0050c1f0`): death effect, chain, removal.
    fn kill(st: &mut SideState, e: &mut crate::enemy::EnemyData, world: &mut World, lib: &AnmLibrary) {
        // The effect is turned to face away from the shot that landed the kill.
        let angle = match e.last_hit {
            Some(h) if (e.pos[0] - h[0]).powi(2) + (e.pos[1] - h[1]).powi(2) >= 0.04 => {
                (e.pos[1] - h[1]).atan2(e.pos[0] - h[0])
            }
            _ => -std::f32::consts::FRAC_PI_2,
        };
        let (slot, script) = e.death_effect();
        if let Some(file) = st.anm_files.get(slot).copied().flatten() {
            let at = crate::enemy::field_to_canvas(st.field, e.pos);
            let id = world.spawn_clipped(lib, file, script, at, st.field);
            if let Some(vm) = world.get_mut(id) {
                vm.rot[2] = angle;
            }
        }
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
            sub: "main".into(),
            pos: [0.0, 0.0],
            hp: 0,
            score: 0,
            item: -1,
            mirrored: false,
            ivars: [0; 4],
            fvars: [0.0; 8],
        };
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
        if let Some(stage) = &mut self.stage {
            stage.update(prog, &mut self.st, world, lib);
        }
        self.create_pending(prog);
        // Enemies created during the frame run in that same frame, like the game's list walk.
        let mut i = 0;
        while i < self.enemies.len() {
            self.enemies[i].update(prog, &mut self.st, world, lib);
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
        }
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
        let mut k = 0;
        while k < self.enemies.len() {
            if self.enemies[k].data.delete {
                let mut e = self.enemies.remove(k);
                e.remove_sprites(world);
                self.st.alive.retain(|a| a.0 != e.data.id);
            } else {
                k += 1;
            }
        }
    }
}
