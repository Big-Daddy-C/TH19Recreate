//! The VS HUD, driven like th19.exe's: the field frame and per-side hearts, bomb stars and
//! round-win stars from `front.anm` (FrontInf: `FUN_005140b0`, `FUN_005142f0`, `FUN_005145f0`,
//! `FUN_005141c0`, `FUN_00515200`, `FUN_00512fd0`), and each player's charge gauge from
//! `aura.anm` (AuraCtrl: `FUN_004e9740` creates it, `FUN_004e8a20` updates it every frame).
//!
//! Positions are on the 640x480 canvas. The original computes screen pixels with the window
//! scale (`DAT_0062eeb0`); scripts with a resolution mode (ANM313) are converted by `th_anm`.
use crate::player::Player;
use crate::text;
use th_anm::{AnmLibrary, VmId, World};

/// The ANM files the HUD draws from.
#[derive(Clone, Copy, Debug)]
pub struct Files {
    pub front: usize,
    pub aura: usize,
    pub ascii: usize,
}

/// Horizontal centre of each field (`164.0` / `476.0` throughout the HUD code).
pub const CENTRE: [f32; 2] = [164.0, 476.0];

/// The gauge's canvas y (`FUN_004e9740`: root at y 456).
pub const GAUGE_Y: f32 = 456.0;

/// Port-only: in 2 vs 2 the teammate's gauge sits this far above the first player's.
pub const PARTNER_GAUGE_DY: f32 = -24.0;
/// Port-only: in 2 vs 2 the teammate's hearts sit this far below the first player's.
pub const PARTNER_HEARTS_DY: f32 = 20.0;

/// What the gauge shows for one player (status fields read by `FUN_004e8a20`).
#[derive(Clone, Copy, Debug)]
pub struct GaugeInput {
    /// Status +0x24, clamped 0..2500.
    pub meter: i32,
    /// AuraCtrl +0x94: the held charge.
    pub charge: i32,
    pub settings: crate::charge::Settings,
    /// Status +0x38 / +0x3c (0..7).
    pub extra_level: u8,
    pub boss_level: u8,
    /// A bomb is running (`status+0x1c` object's +0x18 == 1).
    pub bombing: bool,
}

impl GaugeInput {
    pub fn of(p: &Player, extra_level: u8, boss_level: u8) -> Self {
        Self {
            meter: if p.free_charge { 2500 } else { p.meter.clamp(0, 2500) },
            charge: p.charge.value,
            settings: p.charge_settings,
            extra_level,
            boss_level,
            bombing: p.bomb.is_some(),
        }
    }
}

/// One player's charge gauge (AuraCtrl).
pub struct Gauge {
    pub root: VmId,
    /// `FUN_004e95c0` marker states for the four thresholds (0 = not yet set).
    markers: [u8; 4],
    /// AuraCtrl +0xc4: the meter bar flashes while the meter reaches the EX threshold.
    flashing: bool,
    /// AuraCtrl +0xbc / +0xc0: the charge cursor (aura script 13) and the bomb cursor (15).
    cursor: Option<VmId>,
    bomb_cursor: Option<VmId>,
    /// This frame's text (the text object redraws every frame).
    texts: Vec<VmId>,
    centre: f32,
    y: f32,
}

/// Bar positions are `threshold * 224 / 2500 - 112` from the gauge centre.
fn bar_x(value: i32) -> f32 {
    value as f32 * 224.0 / 2500.0 - 112.0
}

impl Gauge {
    /// `FUN_004e9740`: aura.anm script 0 at the field's centre, one update, then interrupt 4
    /// (shown) run at once on it and its children.
    pub fn new(world: &mut World, lib: &AnmLibrary, files: Files, centre: f32, y: f32, input: GaugeInput, flash_enabled: bool) -> Self {
        let root = world.spawn(lib, files.aura, 0, [centre, y]);
        let mut g = Self { root, markers: [0; 4], flashing: false, cursor: None, bomb_cursor: None, texts: Vec::new(), centre, y };
        g.update(world, lib, files, input, flash_enabled);
        world.interrupt_now(lib, root, 4);
        g
    }

    pub fn remove(&mut self, world: &mut World) {
        for id in self.texts.drain(..).chain(self.cursor.take()).chain(self.bomb_cursor.take()) {
            world.delete(id);
        }
        world.delete(self.root);
    }

    /// `FUN_004e8a20`, every frame.
    pub fn update(&mut self, world: &mut World, lib: &AnmLibrary, files: Files, input: GaugeInput, flash_enabled: bool) {
        for id in self.texts.drain(..) {
            world.delete(id);
        }
        let s = input.settings;
        let (meter, charge) = (input.meter.clamp(0, 2500), input.charge);
        // FUN_004e95c0 for the four threshold markers (aura scripts 5/6 .. 11/12).
        for (k, threshold) in [s.attack, s.skill, s.extra, s.boss].into_iter().enumerate() {
            let script = 5 + 2 * k;
            let pair = [world.find_descendant(self.root, script), world.find_descendant(self.root, script + 1)];
            for id in pair.into_iter().flatten() {
                if let Some(vm) = world.get_mut(id) {
                    vm.origin[0] = bar_x(threshold);
                }
            }
            let state = if charge < threshold { if meter < threshold { 1 } else { 2 } } else { 3 };
            if self.markers[k] != state {
                self.markers[k] = state;
                let n = match state { 1 => 5, 2 => 3, _ => 2 };
                for id in pair.into_iter().flatten() {
                    world.interrupt_one(id, n);
                }
            }
        }
        // The meter bar (script 3) and the charge bar (script 4) scale by value / 2500.
        let meter_bar = world.find_descendant(self.root, 3);
        let charge_bar = world.find_descendant(self.root, 4);
        if let Some(vm) = meter_bar.and_then(|id| world.get_mut(id)) {
            vm.scale = [meter as f32 / 2500.0, 1.0];
        }
        if let Some(vm) = charge_bar.and_then(|id| world.get_mut(id)) {
            vm.scale = [charge as f32 / 2500.0, 1.0];
        }
        // Both cursors sit on the charge bar's left end (its total position) plus the value,
        // 4 screen pixels lower.
        let bar_left = [self.centre - 113.0, self.y + 4.0 / th_anm::vm::RESOLUTION_SCALE];
        let colour = |argb: u32| argb;
        if charge >= 1 {
            let id = *self.cursor.get_or_insert_with(|| world.spawn(lib, files.aura, 13, [0.0; 2]));
            if let Some(vm) = world.get_mut(id) {
                vm.origin = [bar_left[0] + charge as f32 * 224.0 / 2500.0, bar_left[1], 0.0];
            }
            let (label, at, argb) = if charge < s.boss {
                if s.extra <= charge {
                    (Some(format!("Ex.Attack Lv{}", input.extra_level as i32 + 1)), s.extra, 0xffffff80)
                } else if s.skill <= charge {
                    (Some("Skill".to_string()), s.skill, 0xff80d0ff)
                } else {
                    (None, 0, 0)
                }
            } else {
                (Some(format!("Boss Lv{}", input.boss_level as i32 + 1)), s.boss, 0xffff8080)
            };
            if let Some(label) = label {
                self.texts.extend(text::small_text(world, lib, files.ascii, &label, [self.centre + bar_x(at), 448.0 + (self.y - GAUGE_Y)], true, colour(argb)));
            }
        } else if let Some(id) = self.cursor.take() {
            world.delete(id);
        }
        // The grey next-level labels under the boss and EX markers.
        if charge < s.boss {
            let t = format!("Lv{}", input.boss_level as i32 + 1);
            self.texts.extend(text::small_text(world, lib, files.ascii, &t, [self.centre + bar_x(s.boss), 461.0 + (self.y - GAUGE_Y)], true, 0xffc0c0c0));
        }
        if charge < s.extra {
            let t = format!("Lv{}", input.extra_level as i32 + 1);
            self.texts.extend(text::small_text(world, lib, files.ascii, &t, [self.centre + bar_x(s.extra), 461.0 + (self.y - GAUGE_Y)], true, 0xffc0c0c0));
        }
        if input.bombing {
            let id = *self.bomb_cursor.get_or_insert_with(|| world.spawn(lib, files.aura, 15, [0.0; 2]));
            if let Some(vm) = world.get_mut(id) {
                vm.origin = [bar_left[0] + meter as f32 * 224.0 / 2500.0, bar_left[1], 0.0];
            }
        } else if let Some(id) = self.bomb_cursor.take() {
            world.delete(id);
        }
        // With the match option bits 0x38 set, the meter bar flashes (interrupt 2) once the
        // meter reaches the EX threshold and returns to normal (3) when it drops or a bomb runs.
        if flash_enabled {
            if !input.bombing && s.extra <= meter {
                if !self.flashing {
                    if let Some(id) = meter_bar { world.interrupt_one(id, 2); }
                    self.flashing = true;
                }
            } else if self.flashing {
                if let Some(id) = meter_bar { world.interrupt_one(id, 3); }
                self.flashing = false;
            }
        }
    }
}

/// The hearts, bomb stars and gauge of one player.
pub struct PlayerHud {
    hearts: Vec<VmId>,
    bombs: Vec<VmId>,
    pub gauge: Gauge,
    shown_lives: Option<(i32, i32)>,
    shown_bombs: Option<(i32, bool)>,
    /// FrontInf +0x54: the bomb stars are faded while the player is under them.
    faded: bool,
    /// The 2 vs 2 teammate (port-only placement).
    partner: bool,
}

impl PlayerHud {
    /// `FUN_005145f0` (hearts and bomb stars) and `FUN_005142f0`'s first part (all shown with
    /// interrupt 4), then the heart and star refreshes.
    fn new(world: &mut World, lib: &AnmLibrary, files: Files, side: usize, partner: bool, p: &Player, input: GaugeInput, flash: bool) -> Self {
        let centre = CENTRE[side];
        let hearts_y = 16.0 + if partner { PARTNER_HEARTS_DY } else { 0.0 };
        let max = p.max_lives.clamp(0, 7);
        let hearts: Vec<VmId> = (0..max)
            .map(|i| world.spawn(lib, files.front, 16, [centre - max as f32 * 8.0 + 16.0 * i as f32, hearts_y]))
            .collect();
        // Port-only: the teammate's bomb column mirrors the first player's on the left.
        let star_x = if partner { centre - 140.0 } else { centre + 140.0 };
        let bombs: Vec<VmId> = (0..7).map(|k| world.spawn(lib, files.front, 17, [star_x, 464.0 - 8.0 * k as f32])).collect();
        for &id in hearts.iter().chain(&bombs) {
            world.interrupt_now(lib, id, 4);
        }
        let gauge_y = GAUGE_Y + if partner { PARTNER_GAUGE_DY } else { 0.0 };
        let gauge = Gauge::new(world, lib, files, centre, gauge_y, input, flash);
        let mut hud = Self { hearts, bombs, gauge, shown_lives: None, shown_bombs: None, faded: false, partner };
        hud.refresh(world, lib, p);
        hud
    }

    fn remove(&mut self, world: &mut World) {
        for id in self.hearts.drain(..).chain(self.bombs.drain(..)) {
            world.delete(id);
        }
        self.gauge.remove(world);
    }

    /// `FUN_005141c0` when the hearts change and `FUN_00515200` when the bombs change.
    fn refresh(&mut self, world: &mut World, lib: &AnmLibrary, p: &Player) {
        let lives = (p.lives.clamp(-1, 7), p.max_lives.clamp(0, 7));
        if self.shown_lives != Some(lives) {
            self.shown_lives = Some(lives);
            for (i, &id) in self.hearts.iter().enumerate() {
                world.interrupt_now(lib, id, if (i as i32) < lives.0 { 2 } else { 3 });
            }
            if lives.0 == 1 {
                if let Some(&id) = self.hearts.first() {
                    world.interrupt_now(lib, id, 7);
                }
            }
        }
        let bombs = (p.bombs.clamp(0, 10), p.bomb.is_some());
        if self.shown_bombs != Some(bombs) {
            self.shown_bombs = Some(bombs);
            let mut k = 0usize;
            while (k as i32) < bombs.0 && k < self.bombs.len() {
                world.interrupt_now(lib, self.bombs[k], 0x14);
                world.interrupt_now(lib, self.bombs[k], 2);
                k += 1;
            }
            if bombs.1 && k < 7 {
                world.interrupt_now(lib, self.bombs[k], 7);
                k += 1;
            }
            for &id in &self.bombs[k.min(self.bombs.len())..] {
                world.interrupt_now(lib, id, 3);
            }
        }
    }

    fn update(&mut self, world: &mut World, lib: &AnmLibrary, files: Files, p: &Player, input: GaugeInput, flash: bool) {
        self.refresh(world, lib, p);
        // FUN_00512fd0 (+0x54): the bomb stars fade (interrupt 9) while the player is at
        // x >= 112, y >= 416 and come back (8) once it is at x < 112 and y < 416. Port-only:
        // the teammate's column is on the left, so its test is mirrored.
        let x = if self.partner { -p.pos[0] } else { p.pos[0] };
        let count = p.bombs.clamp(0, 10) as usize;
        if !self.faded && x >= 112.0 && p.pos[1] >= 416.0 {
            self.faded = true;
            for &id in self.bombs.iter().take(count) {
                world.interrupt(id, 9);
            }
        } else if self.faded && x < 112.0 && p.pos[1] < 416.0 {
            self.faded = false;
            for &id in self.bombs.iter().take(count) {
                world.interrupt(id, 8);
            }
        }
        self.gauge.update(world, lib, files, input, flash);
    }
}

/// One field's HUD.
pub struct SideHud {
    pub players: Vec<PlayerHud>,
    wins: Vec<VmId>,
    side: usize,
}

/// Round-win star position (`FUN_005142f0` / `FUN_00512fd0`): (16 + 16 i, 8) for the left
/// player and (624 - 16 i, 8) for the right, from screen pixels (x * 2, 16) at the 2x window.
pub fn win_star_pos(side: usize, i: usize) -> [f32; 2] {
    let x = if side == 0 { 16.0 + 16.0 * i as f32 } else { 624.0 - 16.0 * i as f32 };
    [x * 2.0 / th_anm::vm::RESOLUTION_SCALE, 16.0 / th_anm::vm::RESOLUTION_SCALE]
}

/// The whole HUD: the field frame (front.anm script 3) and both sides.
pub struct Hud {
    frame: VmId,
    pub sides: Vec<SideHud>,
    pub files: Files,
    /// The match option bits 0x38 of each side's handicap byte (`DAT_0062bb14`) enable the
    /// meter-bar flash.
    pub flash: [bool; 2],
    time_text: Vec<VmId>,
}

impl Hud {
    /// `FUN_005140b0`: the frame, then each side's HUD (`FUN_005142f0`) with its round-win stars.
    pub fn new(world: &mut World, lib: &AnmLibrary, files: Files, sides: &[crate::Side], wins: [u32; 2], flash: [bool; 2]) -> Self {
        let frame = world.spawn(lib, files.front, 3, [0.0, 0.0]);
        let mut out = Vec::new();
        for (i, s) in sides.iter().enumerate().take(2) {
            let mut players = Vec::new();
            for (slot, p) in s.players().enumerate() {
                let levels = s.levels(slot);
                players.push(PlayerHud::new(world, lib, files, i, slot == 1, p, GaugeInput::of(p, levels[0], levels[1]), flash[i]));
            }
            let wins = (0..wins[i].min(9) as usize).map(|k| world.spawn(lib, files.front, 18, win_star_pos(i, k))).collect();
            out.push(SideHud { players, wins, side: i });
        }
        Self { frame, sides: out, files, flash, time_text: Vec::new() }
    }

    pub fn remove(&mut self, world: &mut World) {
        for s in &mut self.sides {
            for p in &mut s.players {
                p.remove(world);
            }
            for id in s.wins.drain(..) {
                world.delete(id);
            }
        }
        for id in self.time_text.drain(..) {
            world.delete(id);
        }
        world.delete(self.frame);
    }

    /// One frame. `time_left` is the round's remaining frames when a time limit is set
    /// (`FUN_00512d20` draws `(frames + 59) / 60` at (318, 0) in style 4).
    pub fn update(&mut self, world: &mut World, lib: &AnmLibrary, sides: &[crate::Side], time_left: Option<i32>) {
        let files = self.files;
        for (hud, s) in self.sides.iter_mut().zip(sides) {
            let flash = self.flash[hud.side];
            for (slot, (ph, p)) in hud.players.iter_mut().zip(s.players()).enumerate() {
                let levels = s.levels(slot);
                ph.update(world, lib, files, p, GaugeInput::of(p, levels[0], levels[1]), flash);
            }
        }
        for id in self.time_text.drain(..) {
            world.delete(id);
        }
        if let Some(frames) = time_left.filter(|&t| t > 0) {
            self.time_text = text::digit_text(world, files.ascii, &((frames + 59) / 60).to_string(), [318.0, 0.0]);
        }
    }

    /// `FUN_00512fd0` at the round result: the winner gets a new round-win star.
    pub fn add_win_star(&mut self, world: &mut World, lib: &AnmLibrary, side: usize, wins: u32) {
        if let Some(s) = self.sides.get_mut(side) {
            let i = (wins.max(1) - 1) as usize;
            s.wins.push(world.spawn(lib, self.files.front, 18, win_star_pos(side, i)));
        }
    }
}
