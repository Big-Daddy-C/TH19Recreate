//! Enemies: an ECL thread group plus position, movement, sprites and collision data.
//!
//! The opcode and variable semantics follow th19.exe's enemy instruction handler
//! (`FUN_004fecc0`, opcodes 300+) and its variable getters (`FUN_004fd720` int,
//! `FUN_004fc530` float), reimplemented.

use std::f32::consts::PI;

use th_anm::{AnmLibrary, VmId, World};
use th_ecl::{Args, Context, Flow, Host, Program, Threads};
use th_formats::ecl::Instr;

use crate::motion::{wrap_angle, Mode, Motion, KEEP};
use crate::side::{SideState, SpawnRequest};

/// Enemy flags set by `ins_502` / cleared by `ins_503`.
pub mod flag {
    /// Not hit by player shots.
    pub const NO_HURTBOX: u32 = 1;
    /// Does not hurt the player on contact.
    pub const NO_HITBOX: u32 = 2;
    /// Survives leaving the field horizontally.
    pub const OFFSCREEN_LR: u32 = 4;
    /// Survives leaving the field vertically.
    pub const OFFSCREEN_UD: u32 = 8;
    /// Takes no damage.
    pub const INVINCIBLE: u32 = 0x10;
    /// Sprites hidden.
    pub const HIDDEN: u32 = 0x20;
    /// Reaching 0 HP does not kill.
    pub const NO_DEATH: u32 = 0x80;
}

/// Reward parameters set by TH19's `ins_1028`..`ins_1033`, used when the enemy is destroyed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rewards {
    pub a: f32,
    pub b: i32,
    pub c: i32,
    pub d: i32,
    pub e: i32,
    pub f: i32,
}

/// The directional "main" sprite set up by `ins_306`.
#[derive(Clone, Copy, Debug)]
pub struct MainAnm {
    /// ECL anm slot the sprite came from (`ins_302`) and the loaded file.
    pub anm_slot: usize,
    pub file: usize,
    pub base_script: usize,
    pub slot: usize,
    /// -1 moving left, 0 still, 1 moving right.
    pub dir: i32,
}

#[derive(Clone, Debug, Default)]
pub struct EnemyData {
    pub id: i32,
    /// Name of the sub the enemy was created with.
    pub sub: String,
    /// Final position (absolute + relative layers) in field coordinates.
    pub pos: [f32; 2],
    pub prev: [f32; 2],
    pub delta: [f32; 2],
    pub abs: Motion,
    pub rel: Motion,
    pub flags: u32,
    pub suppress_kill_rewards: bool,
    pub mirrored: bool,
    /// Has been inside the field (only then does leaving it delete the enemy).
    pub entered: bool,
    /// Counted by the wave's "enemies remaining" variable (`ins_1039`).
    pub wave_member: bool,
    pub hurtbox: [f32; 2],
    pub hitbox: [f32; 2],
    pub hp: i32,
    pub boss_slot: Option<i32>,
    pub max_hp: i32,
    pub script_rank: Option<u8>,
    pub phase_time: i32,
    pub phase_interrupt: Option<(i32,i32,String)>,
    pub invuln_time: i32,
    /// After a spell card is declared (ECL 522) damage is taken from HP x 7 and HP is
    /// that / 7 (FUN_004fecc0 case 0x43, FUN_00509980 +0x5014/+0x5020): cards take 1/7.
    pub spell_hp7: Option<i32>,
    pub movement_limits: Option<[f32;4]>,
    pub boss_mesh: Option<crate::boss_mesh::BossMesh>,
    pub score: i32,
    pub item: i32,
    pub ivars: [i32; 4],
    pub fvars: [f32; 8],
    pub anm_slot: usize,
    pub vms: [Option<VmId>; 16],
    pub main: Option<MainAnm>,
    pub time: i32,
    pub rewards: Rewards,
    /// Size of the main sprite, used for the off-field test.
    pub size: [f32; 2],
    pub delete: bool,
    /// HP reached 0 this frame; the side runs the death routine.
    pub killed: bool,
    /// Where the last damaging shot was (the death effect faces away from it).
    pub last_hit: Option<[f32; 2]>,
    /// Bullet shooters (`ins_600`..).
    pub ets: Vec<crate::bullet::Et>,
    /// Took damage since the last update (flag 0x20 of the enemy's state).
    pub damaged: bool,
    /// Frames the hit flash stays off after flashing.
    pub flash_hold: i32,
    /// Internal flags 0x700000, separate from ECL's public flags.
    pub spirit_kind: u8,
    pub spirit_disabled: bool,
    /// Side whose character's program (and sprite slots) this enemy runs.
    pub owner: usize,
    /// Frames the bot skips this enemy as a target (+0x63ac, set to 10 when it reverses).
    pub bot_cooldown: i32,
    pub scope_disable_sub: Option<String>,
}

pub struct Enemy {
    pub threads: Threads,
    pub data: EnemyData,
}

/// Field coordinates -> canvas coordinates for a field rectangle `[x, y, w, h]`.
pub fn field_to_canvas(field: [f32; 4], p: [f32; 2]) -> [f32; 2] {
    [field[0] + field[2] / 2.0 + p[0], field[1] + p[1]]
}

impl Enemy {
    pub fn new(prog: &Program, req: &SpawnRequest, id: i32, own: usize) -> Option<Self> {
        let sub = prog.sub(&req.sub)?;
        let mut data = EnemyData { id, sub: req.sub.clone(), hp: req.hp, score: req.score, item: req.item, ..Default::default() };
        data.abs.pv.pos = req.pos;
        data.pos = req.pos;
        data.prev = req.pos;
        data.mirrored = req.mirrored;
        data.ivars = req.ivars;
        data.fvars = req.fvars;
        data.anm_slot = 2;
        data.owner = req.owner.unwrap_or(own);
        data.ets = vec![crate::bullet::Et::default(); 16];
        let mut ctx = Context::new(sub);
        ctx.rank_bit = 1 << req.difficulty.min(7);
        Some(Self { threads: Threads::new(ctx), data })
    }

    /// One frame: script, movement, sprites, off-field check.
    pub fn update(&mut self, prog: &Program, st: &mut SideState, world: &mut World, lib: &AnmLibrary) {
        // FUN_00509030: ordinary enemies get movement drag; spirits switch once to
        // the disable sub registered by ECL 514. Their ECL handles HP and movement.
        if self.data.spirit_kind == 0 {
            let drag = if self.data.flags & flag::HIDDEN == 0 && st.scope_body(&self.data) { 0.5 } else { 0.0 };
            self.data.abs.pv.drag = drag;
            self.data.rel.pv.drag = drag;
        } else if !self.data.spirit_disabled && st.scope_body(&self.data) {
            if let Some(name) = self.data.scope_disable_sub.clone() {
                if let Some(sub) = prog.sub(&name) {
                    let mut ctx = Context::new(sub);
                    ctx.rank_bit = 1 << self.data.script_rank.unwrap_or(st.difficulty).min(7);
                    self.threads = Threads::new(ctx);
                    self.data.spirit_disabled = true;
                    self.data.phase_time = 0;
                }
            }
        }
        if self.data.phase_interrupt.as_ref().is_some_and(|(hp,time,_)|
            self.data.hp<=*hp || (*time>=0 && self.data.phase_time>=*time)) {
            let (_,_,name)=self.data.phase_interrupt.take().unwrap();
            if let Some(sub)=prog.sub(&name) {
                let mut ctx=Context::new(sub); ctx.rank_bit=1<<self.data.script_rank.unwrap_or(st.difficulty).min(7);
                self.threads=Threads::new(ctx); self.data.killed=false;
            }
        }
        self.data.phase_time+=1;
        self.data.invuln_time=(self.data.invuln_time-1).max(0);
        let mut host = EnemyHost { e: &mut self.data, st, world, lib };
        if self.threads.run(prog, &mut host, 1.0) == Flow::End {
            self.data.delete = true;
        }
        let e = &mut self.data;
        e.time += 1;
        if let Some(mesh) = &mut e.boss_mesh { mesh.tick(); }

        e.abs.step();
        e.rel.step();
        e.prev = e.pos;
        e.pos = [e.abs.pv.pos[0] + e.rel.pv.pos[0], e.abs.pv.pos[1] + e.rel.pv.pos[1]];
        if let Some([x,y,w,h])=e.movement_limits {
            e.pos[0]=e.pos[0].clamp(x-w/2.0,x+w/2.0); e.pos[1]=e.pos[1].clamp(y-h/2.0,y+h/2.0);
            e.abs.pv.pos=[e.pos[0]-e.rel.pv.pos[0],e.pos[1]-e.rel.pv.pos[1]];
        }
        e.delta = [e.pos[0] - e.prev[0], e.pos[1] - e.prev[1]];

        // Directional sprite: switch scripts when the horizontal direction changes.
        if let Some(m) = e.main.as_mut().filter(|m| m.dir != i32::MIN) {
            let dir = if e.delta[0] < -0.03 {
                -1
            } else {
                (e.delta[0] > 0.03) as i32
            };
            if dir != m.dir {
                let offset = match (m.dir, dir) {
                    (-1, 0) => 3,
                    (-1, _) => 2,
                    (0, 1) => 2,
                    (0, _) => 1,
                    (_, 0) => 4,
                    _ => 1,
                };
                m.dir = dir;
                let (file, script, slot) = (m.file, m.base_script + offset, m.slot);
                set_vm(e, world, lib, st.field, slot, file, script);
            }
        }
        e.size = main_size(e, world, lib);

        // FUN_00509980's tail: a damaged enemy (flag 0x20) whose main VM exists and whose hold
        // counter (+0x5104) is zero is tinted 0xff0000ff, sets the hold to 4 and plays a damage
        // sound panned by its x; while the hold counts down (4 more ticks) neither happens, so a
        // continuously hit enemy flashes and sounds once every 5 ticks. The sound's override
        // +0x510c is -1 at creation and never written. 0x22 normally; 0x23 for a boss (script
        // flags 0x4080: ECL 512 with a slot sets 0x80, 511 then 0x4000) whose health above its HP
        // interrupt (+0x6240 = HP - the first set interrupt threshold, set by 0050d080 / ECL phase
        // interrupt; plain HP without one) is below 900, or below 200 while a spell card is active
        // (spell object +0x7c bit 1, set by 004f67c0 and cleared by 004f6730). APPROX: the spell
        // object's bit 8 (which suppresses 0x23 together with bit 1) has no setter found and is
        // taken as clear. NOT PORTED: 0x24 for a damage rate (+0x5164) of zero. Not on the killing
        // blow: FUN_00509980 returns into the death routine before it sets the damaged flag.
        let mut hurt_sound = None;
        if let Some(vm) = e.main.and_then(|m| e.vms[m.slot]).and_then(|id| world.get_mut(id)) {
            if e.flash_hold > 0 {
                vm.color_mode = 0;
                e.flash_hold -= 1;
            } else if std::mem::take(&mut e.damaged) {
                vm.color2 = [0.0, 0.0, 255.0];
                vm.alpha2 = 255.0;
                vm.color_mode = 1;
                e.flash_hold = 4;
                if !e.killed {
                    let boss = st.boss_ids.contains(&e.id);
                    let to_interrupt = e.hp - e.phase_interrupt.as_ref().map_or(0, |(hp, _, _)| (*hp).max(0));
                    let low = if e.spell_hp7.is_some() { 200 } else { 900 };
                    hurt_sound = Some(if boss && to_interrupt < low { 0x23 } else { 0x22 });
                }
            } else {
                vm.color_mode = 0;
            }
        }
        if let Some(id) = hurt_sound.filter(|_| crate::sfx::on(crate::sfx::Cat::EnemyHit)) {
            st.play_sound(id, e.pos[0]);
        }
        e.damaged = false;

        // Leaving the field deletes an enemy that has been inside it.
        let (hw, hh) = (e.size[0] / 2.0, e.size[1] / 2.0);
        let out_x = e.pos[0] + hw < -148.0 || e.pos[0] - hw > 148.0;
        let out_y = e.pos[1] + hh < 0.0 || e.pos[1] - hh > 448.0;
        if out_x {
            if e.entered && e.flags & flag::OFFSCREEN_LR == 0 {
                e.delete = true;
            }
        } else if out_y {
            if e.entered && e.flags & flag::OFFSCREEN_UD == 0 {
                e.delete = true;
            }
        } else {
            e.entered = true;
        }

        let origin = field_to_canvas(st.field, e.pos);
        let hidden = e.flags & flag::HIDDEN != 0;
        for id in e.vms.iter().flatten() {
            if let Some(vm) = world.get_mut(*id) {
                vm.origin = [origin[0], origin[1], 0.0];
                vm.visible = !hidden;
                face_travel(vm, e.delta);
            }
        }
    }

    pub fn remove_sprites(&mut self, world: &mut World) {
        for id in self.data.vms.iter_mut() {
            if let Some(id) = id.take() {
                world.delete(id);
            }
        }
    }
}

impl EnemyData {
    /// Apply shot damage (the hit handling in th19.exe `FUN_00509980`).
    pub fn damage(&mut self, amount: i32, from: [f32; 2]) {
        if self.flags & flag::INVINCIBLE != 0 || self.invuln_time>0 {
            return;
        }
        match &mut self.spell_hp7 {
            Some(h7) => {
                *h7 -= amount;
                self.hp = *h7 / 7;
            }
            None => self.hp -= amount,
        }
        self.damaged = amount > 0;
        self.last_hit = Some(from);
        if self.hp < 1 && self.flags & flag::NO_DEATH == 0 {
            self.killed = true;
        }
    }

    /// Death effect script in `effect.anm` (ANM slot 1), chosen when the enemy is set up
    /// (`FUN_0050be30`'s tail): colour variants for some `enemy.anm` fairies, otherwise 0x25.
    pub fn death_effect(&self) -> usize {
        match self.main.filter(|m| m.anm_slot == 2).map(|m| m.base_script) {
            Some(5 | 25 | 53 | 91) => 0x21,
            Some(10 | 56 | 96) => 0x29,
            Some(15 | 106) => 0x2d,
            Some(30) => 0x33,
            Some(35) => 0x32,
            Some(40) => 0x31,
            _ => 0x25,
        }
    }
}

/// The owner-to-ANM rotation path of `FUN_005089d0` (switch on the VM's ANM `424` mode, read
/// at +0x4cc): the sprite is turned by the enemy's travel direction `atan2(dy, dx)` (+0x80,
/// +0x84). Mode 1 faces the direction; 2 keeps the sprite within +-pi/2 and flips it
/// horizontally when heading left; 3, 4 and 5 add pi, pi/2 and -pi/2.
fn face_travel(vm: &mut th_anm::vm::Vm, delta: [f32; 2]) {
    use std::f32::consts::{FRAC_PI_2, PI};
    if !(1..=5).contains(&vm.rotate_mode) {
        return;
    }
    let a = delta[1].atan2(delta[0]);
    match vm.rotate_mode {
        1 => vm.rot[2] = a,
        2 => {
            let flip = !(-FRAC_PI_2..=FRAC_PI_2).contains(&a);
            vm.rot[2] = if flip { wrap_angle(a + PI) } else { a };
            vm.scale[0] = if flip { -vm.scale[0].abs() } else { vm.scale[0].abs() };
        }
        3 => vm.rot[2] = wrap_angle(a + PI),
        4 => vm.rot[2] = wrap_angle(a + FRAC_PI_2),
        _ => vm.rot[2] = wrap_angle(a - FRAC_PI_2),
    }
}

fn set_vm(e: &mut EnemyData, world: &mut World, lib: &AnmLibrary, field: [f32; 4], slot: usize, file: usize, script: usize) {
    let Some(entry) = e.vms.get_mut(slot) else { return };
    if let Some(old) = entry.take() {
        world.delete(old);
    }
    let origin = field_to_canvas(field, e.pos);
    let clip = field;
    *entry = Some(world.spawn_clipped(lib, file, script, origin, clip));
}

fn main_size(e: &EnemyData, world: &World, lib: &AnmLibrary) -> [f32; 2] {
    let Some(m) = e.main else { return [0.0; 2] };
    let Some(vm) = e.vms.get(m.slot).copied().flatten().and_then(|id| world.get(id)) else { return [0.0; 2] };
    let Some(s) = lib.files.get(vm.file).and_then(|f| f.sprites.get(vm.sprite.max(0) as usize)) else {
        return [0.0; 2];
    };
    [(s.size[0] * vm.scale[0]).abs(), (s.size[1] * vm.scale[1]).abs()]
}

/// Variable ids (the ECL "globals" an enemy can read).
pub mod var {
    pub const RAND: i32 = -10000;
    pub const RANDF: i32 = -9999;
    pub const RAND_ANGLE: i32 = -9998;
    pub const X: i32 = -9997;
    pub const Y: i32 = -9996;
    pub const ABS_X: i32 = -9995;
    pub const ABS_Y: i32 = -9994;
    pub const REL_X: i32 = -9993;
    pub const REL_Y: i32 = -9992;
    pub const PLAYER_X: i32 = -9991;
    pub const PLAYER_Y: i32 = -9990;
    pub const ANGLE_TO_PLAYER: i32 = -9989;
    pub const TIME: i32 = -9988;
    pub const RANDF2: i32 = -9987;
    pub const I0: i32 = -9985;
    pub const F0: i32 = -9981;
    pub const DX: i32 = -9977;
    pub const DY: i32 = -9976;
    pub const ABS_ANGLE: i32 = -9971;
    pub const REL_ANGLE: i32 = -9970;
    pub const ABS_SPEED: i32 = -9969;
    pub const REL_SPEED: i32 = -9968;
    pub const ABS_RADIUS: i32 = -9967;
    pub const REL_RADIUS: i32 = -9966;
    pub const PLAYER_X2: i32 = -9965;
    pub const PLAYER_Y2: i32 = -9964;
    pub const SPEED: i32 = -9958;
    pub const ONE: i32 = -9957;
    pub const LEVEL_IS_0: i32 = -9953;
    pub const DIST_TO_PLAYER: i32 = -9944;
    pub const F4: i32 = -9935;
    pub const LAST_ENEMY_ID: i32 = -9931;
    pub const WAVE_REMAINING: i32 = -9889;
    pub const LEVEL: i32 = -9885;
}

/// The ECL host for one enemy.
pub struct EnemyHost<'a> {
    pub e: &'a mut EnemyData,
    pub st: &'a mut SideState,
    pub world: &'a mut World,
    pub lib: &'a AnmLibrary,
}

impl EnemyHost<'_> {
    fn angle_to_player(&self) -> f32 {
        let p = self.st.player_pos;
        (p[1] - self.e.pos[1]).atan2(p[0] - self.e.pos[0])
    }

    fn var(&mut self, id: i32) -> f32 {
        use var::*;
        let e = &*self.e;
        match id {
            RAND => (self.st.rand() & 0x7fff_ffff) as f32,
            RANDF => self.st.randf(),
            RAND_ANGLE => self.st.randf2() * PI,
            X => e.pos[0],
            Y => e.pos[1],
            ABS_X => e.abs.pv.pos[0],
            ABS_Y => e.abs.pv.pos[1],
            REL_X => e.rel.pv.pos[0],
            REL_Y => e.rel.pv.pos[1],
            PLAYER_X | PLAYER_X2 => self.st.player_pos[0],
            PLAYER_Y | PLAYER_Y2 => self.st.player_pos[1],
            ANGLE_TO_PLAYER => self.angle_to_player(),
            TIME => e.time as f32,
            RANDF2 => self.st.randf2(),
            -9985..=-9982 => e.ivars[(id - I0) as usize] as f32,
            -9981..=-9978 => e.fvars[(id - F0) as usize],
            // FUN_004fc530 case d918: status escalation /200, consumed by Ghost*At.
            -9960 => self.st.spirit_pressure as f32 / 200.0,
            DX => e.delta[0],
            DY => e.delta[1],
            ABS_ANGLE => e.abs.pv.angle,
            REL_ANGLE => e.rel.pv.angle,
            ABS_SPEED => e.abs.pv.speed,
            REL_SPEED => e.rel.pv.speed,
            ABS_RADIUS => e.abs.pv.radius,
            REL_RADIUS => e.rel.pv.radius,
            SPEED => (e.delta[0] * e.delta[0] + e.delta[1] * e.delta[1]).sqrt(),
            ONE => 1.0,
            -9953..=-9950 => (self.st.level == (id - LEVEL_IS_0) as usize) as i32 as f32,
            DIST_TO_PLAYER => {
                let p = self.st.player_pos;
                ((p[0] - e.pos[0]).powi(2) + (p[1] - e.pos[1]).powi(2)).sqrt()
            }
            -9935..=-9932 => e.fvars[4 + (id - F4) as usize],
            LAST_ENEMY_ID => self.st.last_enemy_id as f32,
            -9986 => ((e.flags >> 8)&1) as f32,
            -9949..=-9947 | -9923 | -9887 => *self.st.boss_globals.get(&id).unwrap_or(&0.0),
            // EnemyManager +0x44. Only the scripted senders FUN_0050e740/FUN_0050d6f0 set it
            // to 1 (Boss00 then calls ins_1036, a player death); a plain VS send leaves it 0.
            -9886 => 0.0,
            WAVE_REMAINING => self.st.wave_members_alive() as f32,
            LEVEL => self.st.level as f32,
            _ => {
                *self.st.unknown_vars.entry(id).or_default() += 1;
                0.0
            }
        }
    }

    fn int_var(&mut self, id: i32) -> i32 {
        use var::*;
        match id {
            RAND => (self.st.rand() & 0x7fff_ffff) as i32,
            -9985..=-9982 => self.e.ivars[(id - I0) as usize],
            _ => self.var(id) as i32,
        }
    }

    fn set_var(&mut self, id: i32, v: f32) {
        use var::*;
        let e = &mut *self.e;
        match id {
            -9985..=-9982 => e.ivars[(id - I0) as usize] = v as i32,
            -9981..=-9978 => e.fvars[(id - F0) as usize] = v,
            -9935..=-9932 => e.fvars[4 + (id - F4) as usize] = v,
            -9949..=-9947 | -9923 | -9887 => { self.st.boss_globals.insert(id,v); }
            ABS_X => e.abs.pv.pos[0] = v,
            ABS_Y => e.abs.pv.pos[1] = v,
            REL_X => e.rel.pv.pos[0] = v,
            REL_Y => e.rel.pv.pos[1] = v,
            ABS_ANGLE => e.abs.pv.angle = wrap_angle(v),
            REL_ANGLE => e.rel.pv.angle = wrap_angle(v),
            ABS_SPEED => e.abs.pv.speed = v,
            REL_SPEED => e.rel.pv.speed = v,
            ABS_RADIUS => e.abs.pv.radius = v,
            REL_RADIUS => e.rel.pv.radius = v,
            _ => *self.st.unknown_vars.entry(id).or_default() += 1,
        }
    }

    fn motion(&mut self, abs: bool) -> &mut Motion {
        if abs {
            &mut self.e.abs
        } else {
            &mut self.e.rel
        }
    }

    /// Mirror an angle for mirrored enemies: `pi - a` (or `-a` for per-frame increments).
    fn mirror_angle(&self, a: f32, additive: bool) -> f32 {
        if !self.e.mirrored || a <= KEEP {
            a
        } else if additive {
            -a
        } else {
            PI - a
        }
    }

    fn create_enemy(&mut self, ctx: &Context, ins: &Instr) {
        let op = ins.opcode;
        let mut a = Args::new(ins);
        let sub = a.string();
        let mut x = a.float(ctx, self);
        let mut y = a.float(ctx, self);
        let hp = a.int(ctx, self);
        let score = a.int(ctx, self);
        let item = a.int(ctx, self);
        let relative = matches!(op, 300 | 304 | 309 | 311 | 321);
        if relative {
            x += self.e.pos[0];
            y += self.e.pos[1];
        }
        let mut mirrored = matches!(op, 304 | 305 | 312);
        if self.e.mirrored {
            x = -x;
            mirrored = !mirrored;
        }
        self.st.request(SpawnRequest {
            id: 0,
            difficulty: 0,
            sub,
            pos: [x, y],
            hp,
            score,
            item,
            mirrored,
            ivars: self.e.ivars,
            fvars: self.e.fvars,
            owner: Some(self.e.owner),
        });
    }

    fn e_difficulty(&self) -> usize {
        self.e.script_rank.unwrap_or(self.st.difficulty).min(7) as usize
    }

    /// Bullet shooter instructions (handler cases 112..128 of `FUN_004fecc0`).
    fn et_op(&mut self, ctx: &mut Context, ins: &Instr) {
        use crate::bullet::{Et, Ex};
        let op = ins.opcode;
        let i = |s: &mut Self, ctx: &Context, n: usize| ctx.int_arg(ins, n, s);
        let f = |s: &mut Self, ctx: &Context, n: usize| ctx.float_arg(ins, n, s);
        if op == 613 {
            self.st.cancel_bullets = true;
            self.st.cancel_lasers(crate::laser::CancelArea::All, false, self.world, self.lib);
            return;
        }
        if matches!(op,615|616) {
            let r=f(self,ctx,0).max(0.0);
            // FUN_004ee880: live or appearing bullets in the circle, invulnerable ones included.
            for b in &mut self.st.bullets {
                if b.within_cancel(self.e.pos,r,self.world,self.lib) { b.cancel(); }
            }
            self.st.cancel_lasers(crate::laser::CancelArea::Circle { pos: self.e.pos, radius: r }, false, self.world, self.lib);
            return;
        }
        let et = i(self, ctx, 0);
        let Some(slot) = usize::try_from(et).ok().filter(|&e| e < 16) else { return };
        match op {
            600 => self.e.ets[slot] = Et::default(),
            601 => {
                let e = &self.e.ets[slot];
                let origin =
                    if e.absolute { e.offset } else { [self.e.pos[0] + e.offset[0], self.e.pos[1] + e.offset[1]] };
                let et = e.clone();
                self.st.fire(&et, origin, self.world, self.lib);
            }
            602 => {
                let (sp, col) = (i(self, ctx, 1), i(self, ctx, 2));
                let e = &mut self.e.ets[slot];
                e.sprite = sp;
                e.color = col;
            }
            603 => {
                let (x, y) = (f(self, ctx, 1), f(self, ctx, 2));
                let e = &mut self.e.ets[slot];
                e.offset = [x, y];
                e.absolute = false;
            }
            604 => {
                let (a, b) = (f(self, ctx, 1), f(self, ctx, 2));
                self.e.ets[slot].angle = [a, b];
            }
            605 => {
                let (a, b) = (f(self, ctx, 1), f(self, ctx, 2));
                self.e.ets[slot].speed = [a, b];
            }
            606 => {
                let (a, b) = (i(self, ctx, 1), i(self, ctx, 2));
                self.e.ets[slot].count = [a, b];
            }
            607 => {
                let m = i(self, ctx, 1);
                self.e.ets[slot].aim = m;
            }
            // etEx: 609/610 name the slot, 611/612 append; 610/612 add c, d, m, n.
            609..=612 => {
                let (mut n, idx) = if op <= 610 { (2, i(self, ctx, 1)) } else { (1, -1) };
                let asynchronous = i(self, ctx, n) != 0;
                n += 1;
                let kind = i(self, ctx, n) as u32;
                let mut ex = Ex { kind, asynchronous, ..Default::default() };
                ex.a = i(self, ctx, n + 1);
                ex.b = i(self, ctx, n + 2);
                n += 3;
                if op == 610 || op == 612 {
                    ex.c = i(self, ctx, n);
                    ex.d = i(self, ctx, n + 1);
                    n += 2;
                }
                ex.r = f(self, ctx, n);
                ex.s = f(self, ctx, n + 1);
                if op == 610 || op == 612 {
                    ex.m = f(self, ctx, n + 2);
                    ex.n = f(self, ctx, n + 3);
                }
                let e = &mut self.e.ets[slot];
                let k = if idx >= 0 { idx as usize } else { e.ex_next };
                if k < 24 {
                    e.ex[k] = ex;
                    e.ex_next = k + 1;
                }
            }
            // Laser parameters (cases 0x8d, 0x8e, 0x78): lengths and width, the four phase times
            // and flags, sound ids.
            700 => {
                let (a, b, z, w) = (f(self, ctx, 1), f(self, ctx, 2), f(self, ctx, 3), f(self, ctx, 4));
                let l = &mut self.e.ets[slot].laser;
                l.len = [a, b];
                l.f3 = z;
                l.width = w;
            }
            701 => {
                let t = [i(self, ctx, 1), i(self, ctx, 2), i(self, ctx, 3), i(self, ctx, 4)];
                let flags = i(self, ctx, 5);
                let l = &mut self.e.ets[slot].laser;
                l.times = t;
                l.flags = flags;
            }
            608 => {
                let (a, b) = (i(self, ctx, 1), i(self, ctx, 2));
                self.e.ets[slot].laser.sound = [a, b];
            }
            // Line laser (case 0x8f, type 0) or infinite laser (case 0x90, type 1) at the ET's
            // origin along its first angle.
            711 => {
                let e=&self.e.ets[slot];
                let origin=if e.absolute {e.offset} else {[self.e.pos[0]+e.offset[0],self.e.pos[1]+e.offset[1]]};
                if let Some(anm)=self.st.anm_files[0] {
                    let sound=e.laser.sound[0];
                    let laser=crate::laser::Laser::new_curve(origin,e,&self.st.bullet_types,anm,self.world,self.lib,self.st.field);
                    self.st.lasers.push(laser);
                    // The curved laser's init FUN_005286c0 plays +0xcd4 = ET +0x478 at centre pan.
                    if sound>=0 && crate::sfx::on(crate::sfx::Cat::Laser) { self.st.sounds.push((sound as u32, 0)); }
                }
            }
            702 | 703 => {
                let e = &self.e.ets[slot];
                let mut origin =
                    if e.absolute { e.offset } else { [self.e.pos[0] + e.offset[0], self.e.pos[1] + e.offset[1]] };
                if let Some(anm) = self.st.anm_files[0] {
                    origin[0] += e.angle[0].cos() * e.distance;
                    origin[1] += e.angle[0].sin() * e.distance;
                    let kind = if op == 702 { crate::laser::Kind::Line } else { crate::laser::Kind::Infinite };
                    let sound = e.laser.sound[0];
                    let laser = crate::laser::Laser::new(kind, origin, e.angle[0], e.speed[0], e.sprite, e.color, &e.laser,
                        &self.st.bullet_types, anm, self.world, self.lib, self.st.field);
                    self.st.lasers.push(laser);
                    // The laser inits play the ET's first sound id (ET +0x478) at centre pan: the line
                    // laser FUN_0052fca0 (+0xce0) and the infinite laser FUN_0052b8e0 (+0x8c8).
                    if sound >= 0 && crate::sfx::on(crate::sfx::Cat::Laser) { self.st.sounds.push((sound as u32, 0)); }
                }
            }
            614 => {
                let src = i(self, ctx, 1);
                if let Some(s) = self.e.ets.get(src as usize).cloned() {
                    self.e.ets[slot] = s;
                }
            }
            // Offset given as (angle, distance) (case 126).
            618 => {
                let (a, r) = (f(self, ctx, 1), f(self, ctx, 2));
                let e = &mut self.e.ets[slot];
                e.offset = [a.cos() * r, a.sin() * r];
                e.absolute = false;
            }
            619 => {
                let d = f(self, ctx, 1);
                self.e.ets[slot].distance = d;
            }
            // Absolute origin (case 128): x below -990 switches back to relative.
            620 => {
                let (x, y) = (f(self, ctx, 1), f(self, ctx, 2));
                let e = &mut self.e.ets[slot];
                e.offset = [x, y];
                e.absolute = x >= -990.0;
            }
            _ => *self.st.unknown_ops.entry(op).or_default() += 1,
        }
    }

    fn exec_inner(&mut self, ctx: &mut Context, ins: &Instr) -> Flow {
        let op = ins.opcode;
        macro_rules! i {
            ($n:expr) => {
                ctx.int_arg(ins, $n, self)
            };
        }
        macro_rules! f {
            ($n:expr) => {
                ctx.float_arg(ins, $n, self)
            };
        }
        match op {
            300 | 301 | 304 | 305 | 309 | 311 | 312 | 321 => self.create_enemy(ctx, ins),
            // anmSelect(slot)
            302 => self.e.anm_slot = i!(0).max(0) as usize,
            // anmSet(vm_slot, script) / anmSetMain(vm_slot, script)
            303 | 306 => {
                let (slot, script) = (i!(0).max(0) as usize, i!(1));
                let Some(file) = self.st.anm_set(self.e.owner).get(self.e.anm_slot).copied().flatten() else {
                    return Flow::Next;
                };
                if script < 0 {
                    if let Some(id) = self.e.vms.get_mut(slot).and_then(Option::take) {
                        self.world.delete(id);
                    }
                    return Flow::Next;
                }
                set_vm(self.e, self.world, self.lib, self.st.field, slot, file, script as usize);
                if op == 306 || slot == 0 {
                    let anm_slot = self.e.anm_slot;
                    self.e.main = Some(MainAnm { anm_slot, file, base_script: script as usize, slot, dir: if op == 306 {0} else {i32::MIN} });
                }
            }
            // Detached ANM effect, using the selected file (307).
            307 => {
                let slot=i!(0).max(0) as usize; let script=i!(1);
                if let Some(file)=self.st.anm_set(self.e.owner).get(slot).copied().flatten() {
                    if script>=0 { self.world.spawn_clipped(self.lib,file,script as usize,field_to_canvas(self.st.field,self.e.pos),self.st.field); }
                }
            }
            // 316 selects a casting animation relative to anmSetMain's base, not an interrupt.
            // FUN004fecc0 at 004ff6c5: script = base + 5 + variant; negative variant = base.
            // 313 replaces the selected VM by main base+5 (004ff5e5).
            313 => {
                let slot=i!(0).max(0) as usize;
                if let Some(main)=self.e.main {if slot<self.e.vms.len() {
                    set_vm(self.e,self.world,self.lib,self.st.field,slot,main.file,main.base_script+5);
                }}
            }
            316 => {
                let slot=i!(0).max(0) as usize; let variant=i!(1);
                if let Some(main)=self.e.main {
                    let script=if variant<0 {main.base_script} else {main.base_script+5+variant as usize};
                    let file=self.st.anm_set(self.e.owner).get(self.e.anm_slot).copied().flatten().unwrap_or(main.file);
                    if slot<self.e.vms.len() {set_vm(self.e,self.world,self.lib,self.st.field,slot,file,script);}
                }
            }
            336 => {let slot=i!(0).max(0) as usize;let mode=i!(1);if let Some(v)=self.e.vms.get(slot).copied().flatten().and_then(|id|self.world.get_mut(id)) {v.render_mode=mode;} }
            1041 => self.e.suppress_kill_rewards=i!(0)!=0,
            317 => { let slot=i!(0).max(0) as usize; let label=i!(1); if let Some(id)=self.e.vms.get(slot).copied().flatten() { self.world.interrupt(id,label); } }
            319 | 329 | 330 => {
                let slot=i!(0).max(0) as usize;
                let id=self.e.vms.get(slot).copied().flatten();
                if op==319 { let angle=f!(1); if let Some(vm)=id.and_then(|id| self.world.get_mut(id)) {vm.rot[2]=angle;} }
                else if op==329 { let scale=[f!(1),f!(2)]; if let Some(vm)=id.and_then(|id| self.world.get_mut(id)) {vm.scale=scale;} }
                else { let (t,mode)=(i!(1),i!(2));let scale=[f!(3),f!(4)]; if let Some(vm)=id.and_then(|id|self.world.get_mut(id)){vm.interp_scale(scale,t,mode);} }
            }
            // movePos(x, y)
            400 | 402 => {
                let (x, y) = (f!(0), f!(1));
                let m = self.motion(op == 400);
                m.i_pos.active = false;
                if x > KEEP {
                    m.pv.pos[0] = x;
                }
                if y > KEEP {
                    m.pv.pos[1] = y;
                }
                // The game recomputes the final position right away (case 22 of
                // FUN_004fecc0), so instructions later this frame see the new position.
                let e = &mut *self.e;
                e.pos = [e.abs.pv.pos[0] + e.rel.pv.pos[0], e.abs.pv.pos[1] + e.rel.pv.pos[1]];
            }
            // movePosTime(t, mode, x, y)
            401 | 403 | 436 | 437 => {
                let (t, mode) = (i!(0), i!(1));
                let (mut x, y) = (f!(2), f!(3));
                if matches!(op, 436 | 437) && self.e.mirrored && x > KEEP {
                    x = -x;
                }
                let m = self.motion(matches!(op, 401 | 436));
                if t < 1 {
                    m.i_pos.active = false;
                    return Flow::Next;
                }
                let cur = m.pv.pos;
                let goal = [if x > KEEP { x } else { cur[0] }, if y > KEEP { y } else { cur[1] }];
                m.pv.speed = 0.0;
                m.i_pos.start(cur, goal, t, mode);
            }
            // moveVel(angle, speed); 428/430 never mirror
            404 | 406 | 428 | 430 => {
                let ang = f!(0);
                let ang = if op < 428 { self.mirror_angle(ang, false) } else { ang };
                let speed = f!(1);
                let m = self.motion(matches!(op, 404 | 428));
                m.i_angle.active = false;
                m.i_speed.active = false;
                if ang > KEEP {
                    m.pv.angle = wrap_angle(ang);
                }
                if speed > KEEP {
                    m.pv.speed = speed;
                }
            }
            // moveVelTime(t, mode, angle, speed); 429/431 never mirror
            405 | 407 | 429 | 431 => {
                let (t, mode) = (i!(0), i!(1));
                let ang = f!(2);
                let ang = if op < 429 { self.mirror_angle(ang, mode == 7) } else { ang };
                let speed = f!(3);
                let m = self.motion(matches!(op, 405 | 429));
                start_angle(m, t, mode, ang);
                start_speed(m, t, mode, speed);
            }
            // moveCircle(angle, angular speed, radius, radius delta)
            408 | 410 => {
                let v = [f!(0), f!(1), f!(2), f!(3)];
                let m = self.motion(op == 408);
                m.enter_circle(Mode::Circle);
                set_if(&mut m.pv.angle, v[0], true);
                set_if(&mut m.pv.speed, v[1], false);
                set_if(&mut m.pv.radius, v[2], false);
                set_if(&mut m.pv.radius_delta, v[3], false);
            }
            // moveCircleTime(t, mode, angular speed, radius, radius delta)
            409 | 411 => {
                let (t, mode) = (i!(0), i!(1));
                let (speed, radius, delta) = (f!(2), f!(3), f!(4));
                let m = self.motion(op == 409);
                m.enter_circle(Mode::Circle);
                start_speed(m, t, mode, speed);
                if t >= 1 {
                    let cur = [m.pv.radius, m.pv.radius_delta];
                    let goal = [keep_or(radius, cur[0]), keep_or(delta, cur[1])];
                    m.i_radius.start(cur, goal, t, mode);
                }
            }
            // moveEllipse(angle, angular speed, radius, radius delta, ellipse angle, ratio)
            420 | 422 => {
                let v = [f!(0), f!(1), f!(2), f!(3), f!(4), f!(5)];
                let m = self.motion(op == 420);
                m.enter_circle(Mode::Ellipse);
                set_if(&mut m.pv.angle, v[0], true);
                set_if(&mut m.pv.speed, v[1], false);
                set_if(&mut m.pv.radius, v[2], false);
                set_if(&mut m.pv.radius_delta, v[3], false);
                set_if(&mut m.pv.ell_angle, v[4], true);
                set_if(&mut m.pv.ell_ratio, v[5], false);
            }
            // setMirror(flag)
            424 => self.e.mirrored = i!(0) != 0,
            // moveStop: cancel all movement on both layers
            427 => {
                self.e.abs.stop();
                self.e.rel.stop();
            }
            // moveAngleTime(t, mode, angle); 440/442 never mirror
            440 | 441 | 442 | 443 => {
                let (t, mode) = (i!(0), i!(1));
                let ang = f!(2);
                let ang = if matches!(op, 441 | 443) { self.mirror_angle(ang, mode == 7) } else { ang };
                start_angle(self.motion(matches!(op, 440 | 441)), t, mode, ang);
            }
            // moveSpeedTime(t, mode, speed)
            444 | 445 | 446 | 447 => {
                let (t, mode, speed) = (i!(0), i!(1), f!(2));
                start_speed(self.motion(matches!(op, 444 | 445)), t, mode, speed);
            }
            // Random movement within the boss region. APPROX: choose a bounded target;
            // original 412 uses position-biased random direction before interpolation.
            412 => {
                let (time,mode,speed)=(i!(0),i!(1),f!(2));
                let [x,y,w,h]=self.e.movement_limits.unwrap_or([0.0,128.0,280.0,96.0]);
                let angle=self.st.randf()*std::f32::consts::TAU;
                let goal=[(self.e.pos[0]+angle.cos()*speed*time as f32).clamp(x-w/2.0,x+w/2.0),
                    (self.e.pos[1]+angle.sin()*speed*time as f32).clamp(y-h/2.0,y+h/2.0)];
                self.e.abs.i_pos.start(self.e.abs.pv.pos,goal,time,mode);
            }
            504 => self.e.movement_limits=Some([f!(0),f!(1),f!(2),f!(3)]),
            505 => self.e.movement_limits=None,
            // setLife: HP, max HP and the x7 spell counter (FUN_0040f870/0040f860/0040f840).
            511 => {
                self.e.hp=i!(0); self.e.max_hp=self.e.hp;
                if self.e.spell_hp7.is_some() { self.e.spell_hp7=Some(self.e.hp*7); }
            }
            // Spell card declaration (FUN_004f67c0): damage now goes through HP x 7, and the
            // boss file's script 0 (ANM slot 3, enemy manager +0x4adc) draws the spell
            // background over the field.
            522 => {
                // FUN_004f67c0 plays 0x21 at centre pan.
                if crate::sfx::on(crate::sfx::Cat::Spell) { self.st.sounds.push((0x21, 0)); }
                self.e.spell_hp7=Some(self.e.hp*7);
                if let Some(old)=self.st.spell_start_effect.take() {self.world.delete(old);}
                if let Some(file)=self.st.anm_files[1] {
                    // FUN004f67c0: persistent declaration flash at the boss, transient field beams.
                    self.st.spell_start_effect=Some(self.world.spawn_clipped(self.lib,file,6,
                        field_to_canvas(self.st.field,self.e.pos),self.st.field));
                    // APPROX: field-space anchor while ANM draw type600 remains unsupported.
                    self.world.spawn_clipped(self.lib,file,13,[self.st.field[0],self.st.field[1]],self.st.field);
                }
                if let Some(file)=self.st.anm_set(self.e.owner).get(3).copied().flatten() {
                    if let Some(old)=self.st.spell_background.take() { self.world.delete(old); }
                    let at=field_to_canvas(self.st.field,[0.0,0.0]);
                    self.st.spell_background=Some(self.world.spawn_clipped(self.lib,file,0,at,self.st.field));
                }
            }
            // Spell card end (FUN_004f6730): the background goes away.
            523 => {
                // 005030f6 calls 0040f7a0(enemy+5008, 0): clear spell damage mode.
                self.e.spell_hp7=None;
                if let Some(id)=self.st.spell_background.take() {self.world.delete(id);}
                if let Some(id)=self.st.spell_start_effect.take() {self.world.delete(id);}
            },
            512 => {
                let slot=i!(0); self.st.boss_ids.retain(|&id| id!=self.e.id);
                self.e.boss_slot=(slot>=0).then_some(slot);
                if slot>=0 { self.st.boss_ids.push(self.e.id); self.e.flags|=flag::NO_DEATH; }
                else { self.e.flags&=!flag::NO_DEATH; }
            }
            513 => self.e.phase_time=0,
            // Wait while one of this field's boss slots is occupied (FUN_0040b4b0 per slot;
            // the handler returns -1 so the instruction runs again next frame).
            520 => if !self.st.boss_ids.is_empty() { return Flow::Retry; },
            // Destroy every enemy on this field except those flagged 0x20/0x80/0x400 (0x100
            // overrides), with their item drops cleared (FUN_0050ca70).
            525 => self.st.clear_enemies=true,
            514 => {
                let mut args=Args::new(ins); let _slot=args.int(ctx,self);
                let hp=args.int(ctx,self); let time=args.int(ctx,self); let sub=args.string();
                self.e.phase_interrupt=Some((hp,time,sub));
            }
            515 => self.e.invuln_time=i!(0),
            // 00502d5d /0050d0f0: scope interrupt sub, separate from HP/time interrupt.
            521 => {
                let slot = i!(0);
                if slot == 0 { self.e.scope_disable_sub = ins.arg_str(4).map(|(name,_)| name); }
                else { *self.st.unknown_ops.entry(op).or_default() += 1; }
            },
            // Audio, score/spell presentation and item-drop metadata. Gameplay mechanics
            // are handled separately; these unsupported presentation ops stay counted.
            // setHurtbox(w, h) / setHitbox(w, h)
            500 => self.e.hurtbox = [f!(0), f!(1)],
            501 => self.e.hitbox = [f!(0), f!(1)],
            502 => self.e.flags |= i!(0) as u32,
            503 => self.e.flags &= !(i!(0) as u32),
            // isEnemyAlive(&out, id)
            549 => {
                let id = i!(1);
                let alive = self.st.alive.iter().any(|a| a.0 == id) as i32;
                ctx.set_int_var(ins, 0, alive, self);
            }
            // Wave control (TH19): reset pool, add wave, spawn next wave, set level table.
            1010 => self.st.wave_pool.clear(),
            1011 => {
                let (id, weight) = (i!(0), i!(1));
                for _ in 0..weight.max(0) {
                    if self.st.wave_pool.len() < 256 {
                        self.st.wave_pool.push(id);
                    }
                }
            }
            1012 => self.st.spawn_wave(),
            1013 => {
                let slot = i!(0);
                if (0..32).contains(&slot) {
                    for k in 0..8 {
                        self.st.level_table[slot as usize][k] = i!(1 + k);
                    }
                }
            }
            // levelValue(&out, slot)
            1015 => {
                let slot = i!(1).clamp(0, 31) as usize;
                let v = self.st.level_table[slot][self.st.level.min(7)];
                ctx.set_int_var(ins, 0, v, self);
            }
            // rankI / rankF: pick the value for the current difficulty.
            570 => {
                let d = self.e_difficulty();
                let v = i!(1 + d);
                ctx.set_int_var(ins, 0, v, self);
            }
            571 => {
                let d = self.e_difficulty();
                let v = f!(1 + d);
                ctx.set_float_var(ins, 0, v, self);
            }
            600..=620 | 700 | 701 | 702 | 703 | 711 => self.et_op(ctx, ins),
            // Marisa's spell clears the optional per-frame enemy callback. Table 005994f8
            // entry zero is null; nonzero callback IDs remain explicitly unsupported.
            624 if i!(0) == 0 => {},
            621 => {
                let radius = f!(0);
                let color = i!(1) as u32;
                self.e.boss_mesh = (radius > 0.0).then(|| crate::boss_mesh::BossMesh::new(radius, color));
            }
            1007 | 1034 | 1035 => {
                let rank=match op {1034=>self.st.incoming_extra_level,1035=>self.st.incoming_boss_level,_=>i!(0).clamp(0,7) as u8};
                self.e.script_rank=Some(rank); ctx.rank_bit=1<<rank;
            }
            // Ray to the field's edge (case 0x6d): from (arg 2, arg 3) along the angle in arg 4,
            // where it meets the field rectangle (x +-148, y 0..448); the point goes to the
            // variables named by args 0 and 1 (FUN_004d3140).
            569 => {
                let (x, y, a) = (f!(2), f!(3), f!(4));
                let p = crate::laser::ray_to_field_edge([x, y], a);
                ctx.set_float_var(ins, 0, p[0], self);
                ctx.set_float_var(ins, 1, p[1], self);
            }
            // 00505116: the request is built on a stack struct; the stores after `push` sit one
            // dword lower than their displacement suggests, so arg0 lands at request+0xc and arg1
            // at +0x10 (the x/y of the target, as in Ran's own 00419c00 edge cycle, which writes
            // the same two fields). pl03.ecl passes (+-150, y) for the side edges and (x, 0/448)
            // for top/bottom.
            1037 => {
                let (x,y)=(f!(0),f!(1));
                let character=crate::character::Character::Ran;
                let req=crate::reflect::Request {kind:44,color:1,speed:0.0,origin:self.st.opponent_pos,
                    target:Some([x,y]),special_rank:Some(self.e.script_rank.unwrap_or(self.st.incoming_extra_level) as i32),
                    owner:self.e.owner,script:Some(14),tangents:None,aim_player:false};
                let field=self.st.field;let source=[640.0-field[0]-field[2],field[1],field[2],field[3]];
                let pending=crate::reflect::Pending::new_character(req,source,self.st,character);
                self.st.pending_reflects.push(pending);
            }
            // ECL 516 (case 0x3d, 00502df4): sound arg0 panned by the enemy's x (FUN_004bb8a0).
            516 => if crate::sfx::on(crate::sfx::Cat::Script) { let id=i!(0); if id>=0 { self.st.play_sound(id as u32, self.e.pos[0]); } },
            1023 => self.st.wave_param = i!(0),
            // FUN_004fecc0 case b4: set internal type, clear disabled bit 0x80000.
            1027 => { self.e.spirit_kind = (i!(0) & 7) as u8; self.e.spirit_disabled = false; },
            1028 => self.e.rewards.a = f!(0),
            1029 => self.e.rewards.b = i!(0),
            1030 => self.e.rewards.c = i!(0),
            1031 => self.e.rewards.d = i!(0),
            1032 => self.e.rewards.e = i!(0),
            1033 => self.e.rewards.f = i!(0),
            1039 => self.e.wave_member = i!(0) != 0,
            _ => *self.st.unknown_ops.entry(op).or_default() += 1,
        }
        Flow::Next
    }
}

fn keep_or(v: f32, cur: f32) -> f32 {
    if v > KEEP {
        v
    } else {
        cur
    }
}

fn set_if(dst: &mut f32, v: f32, angle: bool) {
    if v > KEEP {
        *dst = if angle { wrap_angle(v) } else { v };
    }
}

/// Interpolate the movement angle; mode 7 adds `angle` every frame. Takes the short way round.
fn start_angle(m: &mut Motion, t: i32, mode: i32, ang: f32) {
    if t < 1 {
        m.i_angle.active = false;
        return;
    }
    let cur = m.pv.angle;
    let goal = if mode == 7 {
        if ang > KEEP { ang } else { 0.0 }
    } else if ang > KEEP {
        cur + wrap_angle(ang - cur)
    } else {
        cur
    };
    m.i_angle.start([cur], [goal], t, mode);
}

fn start_speed(m: &mut Motion, t: i32, mode: i32, speed: f32) {
    if t < 1 {
        m.i_speed.active = false;
        return;
    }
    let cur = m.pv.speed;
    let goal = if mode == 7 {
        if speed > KEEP { speed } else { 0.0 }
    } else {
        keep_or(speed, cur)
    };
    m.i_speed.start([cur], [goal], t, mode);
}

impl Host for EnemyHost<'_> {
    fn get_int(&mut self, id: i32) -> i32 {
        self.int_var(id)
    }
    fn get_float(&mut self, id: i32) -> f32 {
        self.var(id)
    }
    fn set_int(&mut self, id: i32, value: i32) {
        self.set_var(id, value as f32)
    }
    fn set_float(&mut self, id: i32, value: f32) {
        self.set_var(id, value)
    }
    fn rand(&mut self) -> u32 {
        self.st.rand()
    }
    fn exec(&mut self, ctx: &mut Context, _prog: &Program, ins: &Instr) -> Flow {
        self.exec_inner(ctx, ins)
    }
}
