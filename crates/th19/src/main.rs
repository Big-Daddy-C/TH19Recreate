//! th19-rs: a Rust reimplementation of Touhou 19 (Unfinished Dream of All Living Ghost).
//!
//! All assets are loaded at runtime from the user's own copy of the game.
//! Currently this binary is the ANM viewer used to validate the formats, VM and renderer.

mod render;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use th_anm::{AnmLibrary, World};
use th_stage::Stage;
use th_formats::dat::Archive;
mod audio;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use render::Renderer;

const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Locate `th19.dat`: `--dat <path>`, `$TH19_DAT`, `$TH19_DIR`, or a few conventional places.
fn find_dat() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--dat" {
            return args.next().map(PathBuf::from);
        }
    }
    if let Ok(p) = std::env::var("TH19_DAT") {
        return Some(PathBuf::from(p));
    }
    let mut candidates = vec![PathBuf::from("th19.dat"), PathBuf::from("game/th19.dat")];
    if let Ok(d) = std::env::var("TH19_DIR") {
        candidates.insert(0, Path::new(&d).join("th19.dat"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        candidates.push(Path::new(&home).join("Downloads/th19/th19/th19.dat"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Anm,
    Stage,
    /// Stage background plus both fields running the wave scripts.
    Game,
}

use th_game::FIELDS;

/// Gameplay state for game mode: the ECL program and the two sides.
struct Game {
    progs: Vec<th_ecl::Program>,
    sides: Vec<th_game::Side>,
    round: th_game::round::Round,
}

/// Draw priority of the pause menu's text; the dimming layer sits just below it.
const MENU_PRIORITY: i32 = 200;

/// The pause menu: the highlighted entry and its text sprites.
struct Menu {
    sel: usize,
    vms: Vec<th_anm::VmId>,
}

/// A row of the pause menu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Resume,
    /// 1 vs 1 or 2 vs 2.
    Mode,
    /// The character of player slot `n` (0 you, 1 rival, 2 ally, 3 second rival).
    Char(usize),
    Song,
    Se,
    Music,
    Quit,
}

struct Viewer {
    audio: Option<audio::Audio>,
    /// The pause menu (Esc in a game), with its text sprites.
    menu: Option<Menu>,
    /// Set by the menu's Quit entry; the event loop exits.
    quit: bool,
    /// Where the volume settings are saved (not the game's own th19.cfg).
    settings_path: std::path::PathBuf,
    /// Each character's stage (`th_formats::music::character_stages`) and the stage whose music
    /// is playing.
    character_stages: Vec<u32>,
    music_stage: Option<usize>,
    mode: Mode,
    /// Overlay the playfield divider on the stage background.
    show_fields: bool,
    stage: Option<Stage>,
    stage_files: Vec<String>,
    stage_idx: usize,
    fast: bool,
    dat: Archive,
    lib: AnmLibrary,
    world: World,
    files: Vec<String>,
    file_idx: usize,
    script: usize,
    paused: bool,
    centered: bool,
    show_all: bool,
    game: Option<Game>,
    /// Keys currently held (gameplay input).
    held: std::collections::HashSet<KeyCode>,
    /// Development godmode (G): neither player can be hit.
    godmode: bool,
    /// Player 2 is run by the game's bot (K) instead of just holding the shot button.
    bot_ai: bool,
    /// L: the opponent does nothing at all (no movement, shooting or bombs), for testing.
    opponent_idle: bool,
    /// Shift+K gives player 1 an independent copy of the same AI controller.
    player_bot_ai: bool,
    player_bot: th_game::bot::Bot,
    /// Controllers for the teammates in 2v2 (yours, then the opposing team's).
    ally_bot: th_game::bot::Bot,
    rival_bot: th_game::bot::Bot,
    /// Each side's character (`]` / `[` change the bot's, with Shift the player's).
    chars: [th_game::character::Character; 4],
    /// Players per team: 1 (1 vs 1) or 2 (2 vs 2).
    team_size: usize,
    /// The song chosen in the menu (an index into thbgm.fmt), overriding the stage and boss music.
    music_override: Option<usize>,
    /// The theme of the stage's characters, chosen when the stage loads.
    stage_theme: Option<&'static str>,
    /// Development full-charge mode (M): every tier is always available and costs nothing.
    full_charge: bool,
    bot: th_game::bot::Bot,
    /// Development: player 1's starting boss level (`--boss-level N`, 1-8 as on the HUD).
    start_boss_level: u8,
    /// Bullet sprite table read from th19.exe.
    bullet_types: std::rc::Rc<Vec<th_formats::exe::BulletType>>,
    /// Frames simulated since start (debug triggers).
    frames: u64,
}

impl Viewer {
    fn new(dat: Archive) -> Self {
        let mut files: Vec<String> =
            dat.entries().iter().map(|e| e.name.clone()).filter(|n| n.ends_with(".anm")).collect();
        files.sort();
        let file_idx = files.iter().position(|f| f == "title.anm").unwrap_or(0);
        let mut stage_files: Vec<String> =
            dat.entries().iter().map(|e| e.name.clone()).filter(|n| n.ends_with(".std")).collect();
        stage_files.sort();
        let mut v = Self {
            audio: None,
            menu: None,
            quit: false,
            settings_path: std::path::PathBuf::from("th19-rs-settings.txt"),
            character_stages: Vec::new(),
            music_stage: None,
            mode: Mode::Anm,
            show_fields: true,
            stage: None,
            stage_files,
            stage_idx: 0,
            fast: false,
            dat,
            lib: AnmLibrary::default(),
            world: World::default(),
            files,
            file_idx,
            script: 0,
            paused: false,
            centered: false,
            show_all: true,
            game: None,
            held: Default::default(),
            godmode: false,
            bot_ai: true,
            opponent_idle: false,
            player_bot_ai: false,
            player_bot: th_game::bot::Bot::new(),
            ally_bot: th_game::bot::Bot::new(),
            rival_bot: th_game::bot::Bot::new(),
            chars: [th_game::character::Character::Reimu; 4],
            team_size: 1,
            music_override: None,
            stage_theme: None,
            full_charge: false,
            bot: th_game::bot::Bot::new(),
            start_boss_level: 0,
            bullet_types: Default::default(),
            frames: 0,
        };
        v.restart();
        v
    }

    fn file_id(&mut self) -> Option<usize> {
        let name = self.files.get(self.file_idx)?.clone();
        if let Some(id) = self.lib.file_id(&name) {
            return Some(id);
        }
        let bytes = self.dat.read(&name).map_err(|e| log::error!("{name}: {e}")).ok()?;
        self.lib.load(&name, &bytes).map_err(|e| log::error!("{name}: {e}")).ok()
    }

    fn script_count(&mut self) -> usize {
        self.file_id().map_or(0, |f| self.lib.files[f].script_count())
    }

    fn load_stage(&mut self) {
        self.world.clear();
        self.stage = None;
        self.bot = th_game::bot::Bot::new();
        self.player_bot = th_game::bot::Bot::new();
        self.ally_bot = th_game::bot::Bot::new();
        self.rival_bot = th_game::bot::Bot::new();
        let Some(name) = self.stage_files.get(self.stage_idx).cloned() else { return };
        let std = match self.dat.read(&name).map(|b| th_formats::stage::StdFile::parse(&b)) {
            Ok(Ok(s)) => s,
            Ok(Err(e)) | Err(e) => return log::error!("{name}: {e}"),
        };
        let anm_name = std.anm_name.clone();
        let anm = match self.dat.read(&anm_name) {
            Ok(b) => match self.lib.load(&anm_name, &b) {
                Ok(id) => id,
                Err(e) => return log::error!("{anm_name}: {e}"),
            },
            Err(e) => return log::error!("{anm_name}: {e}"),
        };
        let stage = Stage::new(std, anm, &self.lib, &mut self.world);
        self.stage = Some(stage);
        self.game = None;
        if self.mode == Mode::Game {
            self.game = self.load_game();
            self.start_music();
        }
    }

    /// The stage's music: the theme of a character who uses this stage, chosen at random when
    /// several do (`th_formats::music`). A stage nobody uses stays silent. It keeps playing across
    /// rounds of the same stage.
    fn start_music(&mut self) {
        if self.audio.is_none() || self.music_stage == Some(self.stage_idx) {
            return;
        }
        self.music_stage = Some(self.stage_idx);
        let themes = th_formats::music::stage_themes(&self.character_stages, self.stage_idx as u32);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        self.stage_theme = (!themes.is_empty()).then(|| themes[nanos as usize % themes.len()]);
        self.update_music();
    }

    /// Keep the right track playing: the song picked in the menu if any, else the theme of the
    /// character whose boss is on either field, else the stage's theme (silence for a stage no
    /// character uses). Boss music is an assumption: the original's trigger was not found.
    fn update_music(&mut self) {
        if self.mode != Mode::Game {
            return;
        }
        let boss = self.game.as_ref().and_then(|g| {
            g.sides.iter().find_map(|s| s.boss_owner()).and_then(|owner| self.chars.get(owner).map(|c| *c as usize))
        });
        let Some(audio) = &mut self.audio else { return };
        let picked = self.music_override.and_then(|i| audio.track_names().get(i).cloned());
        let theme = picked
            .or_else(|| boss.and_then(|c| th_formats::music::CHARACTER_THEMES.get(c)).map(|t| t.to_string()))
            .or_else(|| self.stage_theme.map(str::to_string));
        match theme {
            Some(name) => audio.play_music(&name),
            None => audio.stop_music(),
        }
    }

    /// Load the ECL scripts and their sprites and start both sides' stage scripts.
    fn load_game(&mut self) -> Option<Game> {
        let chars = self.chars;
        let team = self.team_size.clamp(1, 2);
        // One ECL program and sprite-slot set per player's character: attacks a player sends
        // run the sender's script on the other field. Index = side for the first players,
        // side + 2 for their partners.
        let mut progs = Vec::new();
        let mut sets = [[None; 8]; 4];
        for (i, c) in chars.iter().enumerate().take(2 * team) {
            let ecl = format!("{}.ecl", c.prefix());
            let dat = &mut self.dat;
            let prog = th_ecl::Program::load(&ecl, |n| dat.read(n).ok()).map_err(|e| log::error!("{ecl}: {e}")).ok()?;
            // Enemy sprite slots: 0 = bullet.anm, 1 = effect.anm (FUN_0050d380), then the ECL
            // file's anim list from 2 upwards.
            let names: Vec<String> = ["bullet.anm".to_string(), "effect.anm".to_string()]
                .into_iter()
                .chain(prog.files[0].1.anim.clone())
                .collect();
            for (k, name) in names.iter().enumerate().take(8) {
                sets[i][k] = Some(self.load_anm(name)?);
            }
            progs.push(prog);
        }
        // Both players face the same wave order.
        let seq = th_game::side::roll_wave_sequence(0x1234_5678);
        let mut sides: Vec<th_game::Side> = FIELDS
            .iter()
            .enumerate()
            .map(|(i, &f)| {
                let mut s = th_game::Side::new(f, 0x9e37_79b9 ^ i as u32, seq, sets[i]);
                s.st.anm_sets = sets;
                s.st.index = i;
                s
            })
            .collect();
        let ghost = self.load_anm("ghost.anm");
        let aura = self.load_anm("aura.anm");
        let ascii = self.load_anm("ascii.anm");
        for (i, s) in sides.iter_mut().enumerate() {
            s.st.bullet_types = self.bullet_types.clone();
            s.st.ascii_anm = ascii;
            s.st.ghost_anm = ghost;
            s.start(&progs[i]);
            for slot in 0..team {
                let c = chars[i + 2 * slot];
                let prefix = c.prefix();
                let sht = self.dat.read(&format!("{prefix}.sht")).ok().and_then(|b| th_formats::sht::ShtFile::parse(&b).ok());
                let pl_anm = self.load_anm(&format!("{prefix}.anm"));
                if let (Some(sht), Some(anm)) = (sht, pl_anm) {
                    let mut p = th_game::player::Player::new(sht, anm);
                    p.character = c;
                    p.free_charge = self.full_charge;
                    p.aura_anm = aura;
                    p.effect_anm = s.st.anm_files[1];
                    if slot == 0 { s.player = Some(p) } else { s.partner = Some(p) }
                }
            }
        }
        sides[0].st.boss_level = self.start_boss_level;
        Some(Game { progs, sides, round: Default::default() })
    }

    /// Load an `.anm` from the archive once and return its library id.
    fn load_anm(&mut self, name: &str) -> Option<usize> {
        if let Some(id) = self.lib.file_id(name) {
            return Some(id);
        }
        let bytes = self.dat.read(name).map_err(|e| log::error!("{name}: {e}")).ok()?;
        self.lib.load(name, &bytes).map_err(|e| log::error!("{name}: {e}")).ok()
    }

    fn tick(&mut self) {
        for _ in 0..if self.fast { 8 } else { 1 } {
            if let Some(st) = &mut self.stage {
                st.tick(&mut self.world, &self.lib);
            }
            self.frames += 1;
            if std::env::var("TH19_DEBUG_EXTRA").ok().and_then(|v| v.parse::<u64>().ok()) == Some(self.frames) {
                self.debug_extra_attack();
            }
            self.debug_spawn();
            // Development: `TH19_DEBUG_MENU=frame` opens the pause menu (for screenshots).
            if std::env::var("TH19_DEBUG_MENU").ok().and_then(|v| v.parse::<u64>().ok()) == Some(self.frames) {
                self.open_menu();
            }
            // Development: `TH19_DEBUG_SPECIAL=frame` loads player 2's dot and special counters.
            if std::env::var("TH19_DEBUG_SPECIAL").ok().and_then(|v| v.parse::<u64>().ok()) == Some(self.frames) {
                if let Some(g) = &mut self.game {
                    g.sides[1].st.dot_counter = 750;
                    g.sides[1].st.special_counter = 360;
                    g.sides[1].st.last_credit_pos = [0.0, 120.0];
                    g.sides[1].st.last_special_pos = [0.0, 120.0];
                }
            }
            // Development: `TH19_DEBUG_CHAIN=frame` gives both sides a 14-chain.
            if std::env::var("TH19_DEBUG_CHAIN").ok().and_then(|v| v.parse::<u64>().ok()) == Some(self.frames) {
                if let Some(g) = &mut self.game {
                    for s in &mut g.sides { s.st.chain = 14; s.st.chain_timer = 30; }
                }
            }
            if std::env::var_os("TH19_DEBUG_FOCUS").is_some() { self.held.insert(KeyCode::ShiftLeft); }
            if std::env::var("TH19_DEBUG_RAIN").ok().and_then(|v| v.parse::<u64>().ok()) == Some(self.frames) {
                if let Some(g) = &mut self.game { g.sides[1].start_rain(); }
            }
            // Development: `TH19_DEBUG_BOMB=frame` presses the bomb button for one frame.
            if let Some(f) = std::env::var("TH19_DEBUG_BOMB").ok().and_then(|v| v.parse::<u64>().ok()) {
                if f == self.frames { self.held.insert(KeyCode::KeyC); }
                else if f + 1 == self.frames { self.held.remove(&KeyCode::KeyC); }
            }
            let keys = self.player_keys();
            if let Some(g) = &mut self.game {
                th_game::side::prepare_charge_targets(&mut g.sides);
                for (i, s) in g.sides.iter_mut().enumerate().filter(|_| g.round.outcome.is_none()) {
                    // Each AI owns its targeting/charge state; player 1 returns to held
                    // keyboard input immediately when Shift+K disables its controller.
                    let k = if i == 0 {
                        if self.player_bot_ai { self.player_bot.think(s) } else { keys }
                    } else if self.opponent_idle { 0 } else if self.bot_ai { self.bot.think(s) } else { th_game::player::input::SHOT };
                    // Teammates are always bots for now (the menu will configure this later).
                    if s.partner.is_some() {
                        std::mem::swap(&mut s.player, &mut s.partner);
                        s.partner_keys = if i == 0 { self.ally_bot.think(s) }
                            else if self.opponent_idle { 0 } else if self.bot_ai { self.rival_bot.think(s) } else { th_game::player::input::SHOT };
                        std::mem::swap(&mut s.player, &mut s.partner);
                    }
                    for p in s.player.iter_mut().chain(s.partner.iter_mut()) {
                        p.godmode = self.godmode;
                        p.free_charge = self.full_charge;
                    }
                    s.tick(&g.progs, k, &mut self.world, &self.lib);
                    if let Some(a) = &mut self.audio { a.play_all(&s.st.sounds); }
                    s.st.sounds.clear();
                }
                th_game::side::dispatch_charges(&mut g.sides);
                // A team is out when both teammates are out of hearts.
                let defeated = std::array::from_fn(|i| g.sides[i].defeated());
                g.round.resolve(defeated);
                // APPROX: after a KO, hold briefly, then start the next round on fresh
                // fields with the score kept (the original transition is not ported yet).
                if g.round.hold() {
                    let round = g.round.next();
                    let fast = self.fast;
                    self.load_stage();
                    self.fast = fast;
                    if let Some(g) = &mut self.game {
                        g.round = round;
                    }
                    log::info!("next round");
                    continue;
                }
            }
            self.world.tick(&self.lib);
            self.update_music();
        }
    }

    fn restart(&mut self) {
        if self.mode != Mode::Anm {
            return self.load_stage();
        }
        self.world.clear();
        let Some(f) = self.file_id() else { return };
        let n = self.lib.files[f].script_count();
        if n == 0 {
            return;
        }
        let origin = if self.centered { [320.0, 240.0] } else { [0.0, 0.0] };
        if self.show_all {
            // Scripts that are only meant to be spawned as children will simply also appear here.
            for s in 0..n {
                self.world.spawn(&self.lib, f, s, origin);
            }
        } else {
            self.script = self.script.min(n - 1);
            self.world.spawn(&self.lib, f, self.script, origin);
        }
    }

    /// Everything needed to draw the current frame: triangles, background colour, fog colour.
    /// Everything needed to draw the current frame: triangles, how many of them form the stage
    /// scene (which later batches may sample), background colour and fog colour.
    fn frame(&self) -> (th_anm::DrawList, usize, [f32; 3], [f32; 3]) {
        let (mut list, bg, fog) = match &self.stage {
            Some(st) => {
                let c = st.clear_color.map(|x| x / 255.0);
                let f = [st.fog_color[0] / 255.0, st.fog_color[1] / 255.0, st.fog_color[2] / 255.0];
                // Distant geometry fades into the fog, so fill the background with it too.
                let sky = if st.fog_range[1] > 0.0 { f } else { c };
                // TH19 draws the stage once across the whole screen; the two playfields are
                // windows laid over that single shared background.
                let rect = [0.0, 0.0, render::CANVAS_W, render::CANVAS_H];
                let mut list = vec![th_anm::DrawBatch::solid_rect(rect, [sky[0], sky[1], sky[2], 1.0])];
                // The stage, layer by layer as the game draws it (sprite slots interleaved).
                let cam = st.camera(rect);
                list.extend(self.world.draw_stage(&self.lib, &cam));
                if self.show_fields {
                    // Stand-in for the real frame from front.anm: darken the strip between fields.
                    let gap_x = FIELDS[0][0] + FIELDS[0][2];
                    let mut divider = th_anm::DrawBatch::solid_rect(
                        [gap_x, 0.0, FIELDS[1][0] - gap_x, render::CANVAS_H],
                        [0.0, 0.0, 0.0, 0.5],
                    );
                    divider.blend = th_anm::BlendMode::Normal;
                    list.push(divider);
                }
                (list, [0.0; 3], f)
            }
            None => (Vec::new(), [0.0; 3], [0.0; 3]),
        };
        let scene_end = if self.stage.is_some() { list.len() } else { 0 };
        if let Some(st) = &self.stage {
            list.extend(st.distortion_batches(&FIELDS));
        }
        if let Some(g) = &self.game {
            for side in &g.sides {
                for enemy in &side.enemies {
                    if let Some(mesh) = &enemy.data.boss_mesh {
                        list.push(mesh.batch(th_game::enemy::field_to_canvas(side.st.field, enemy.data.pos), side.st.field));
                    }
                }
            }
        }
        if let Some(g)=&self.game {
            let priority=th_game::laser::DRAW_PRIORITY;
            list.extend(self.world.draw_priority_range(&self.lib,th_anm::Pass::Ui,i32::MIN,priority+1));
            for side in &g.sides { for laser in &side.st.lasers {
                if let Some(batch)=laser.curve_batch(&self.world,&self.lib,side.st.field) {list.push(batch);}
            }}
            list.extend(self.world.draw_priority_range(&self.lib,th_anm::Pass::Ui,priority+1,MENU_PRIORITY-1));
            if self.menu.is_some() {
                let mut dim = th_anm::DrawBatch::solid_rect([0.0, 0.0, 640.0, 480.0], [0.0, 0.0, 0.0, 0.65]);
                dim.blend = th_anm::BlendMode::Normal;
                list.push(dim);
            }
            list.extend(self.world.draw_priority_range(&self.lib,th_anm::Pass::Ui,MENU_PRIORITY-1,i32::MAX));
            for side in &g.sides { for spark in &side.st.sparks {
                if let Some(batch)=spark.batch(side.st.field) {list.push(batch);}
            }}
        } else {list.extend(self.world.draw_list(&self.lib,th_anm::Pass::Ui));}
        if let Some(g) = &self.game {
            for (side, field) in g.sides.iter().zip(FIELDS) {
                // Provisional: hearts of each player in the top margin (partner on the right).
                for (k, p) in side.players().enumerate() {
                    for h in 0..p.lives.max(0) {
                        let hx = if k == 0 { field[0] + 8.0 * h as f32 } else { field[0] + field[2] - 8.0 * (h + 1) as f32 };
                        list.push(th_anm::DrawBatch::solid_rect([hx, 1.0, 6.0, 4.0], [0.95, 0.2, 0.3, 1.0]));
                    }
                }
                if let Some(p) = &side.partner {
                    let [x, _, w, _] = field;
                    list.push(th_anm::DrawBatch::solid_rect([x, 6.0, w*(if p.free_charge {2500} else {p.meter}) as f32/2500.0, 3.0], [0.15, 0.45, 0.95, 1.0]));
                    let ready = p.charge.value >= p.charge_settings.attack;
                    let color = if ready { [1.0, 0.8, 0.2, 1.0] } else { [0.7, 0.7, 0.8, 1.0] };
                    list.push(th_anm::DrawBatch::solid_rect([x, 10.0, w*p.charge.value as f32/2500.0, 4.0], color));
                }
                if let Some(p) = &side.player {
                    // Provisional HUD, outside the playfield. Top row = available meter;
                    // bottom row = held charge. Original front.anm HUD remains to be ported.
                    let [x, _, w, _] = field;
                    for e in side.enemies.iter().filter(|e|e.data.boss_slot.is_some() && e.data.max_hp>0) {
                        let ratio=(e.data.hp as f32/e.data.max_hp as f32).clamp(0.0,1.0);
                        list.push(th_anm::DrawBatch::solid_rect([x,field[1],w*ratio,3.0],[0.95,0.15,0.35,1.0]));
                    }
                    list.push(th_anm::DrawBatch::solid_rect([x, 465.0, w, 14.0], [0.04, 0.04, 0.08, 1.0]));
                    list.push(th_anm::DrawBatch::solid_rect([x, 466.0, w*(if p.free_charge {2500} else {p.meter}) as f32/2500.0, 4.0], [0.15, 0.45, 0.95, 1.0]));
                    let ready = p.charge.value >= p.charge_settings.attack;
                    let color = if ready { [1.0, 0.8, 0.2, 1.0] } else { [0.7, 0.7, 0.8, 1.0] };
                    list.push(th_anm::DrawBatch::solid_rect([x, 472.0, w*p.charge.value as f32/2500.0, 5.0], color));
                    for threshold in [p.charge_settings.attack,p.charge_settings.skill,p.charge_settings.extra,p.charge_settings.boss] {
                        let marker=(x+w*threshold as f32/2500.0).min(x+w-1.0);
                        list.push(th_anm::DrawBatch::solid_rect([marker,466.0,1.0,12.0],[1.0,1.0,1.0,1.0]));
                    }
                }
            }
        }
        (list, scene_end, bg, fog)
    }

    fn title(&mut self) -> String {
        if self.mode != Mode::Anm {
            let name = self.stage_files.get(self.stage_idx).cloned().unwrap_or_default();
            if let Some(g) = &self.game {
                let n: Vec<String> = g
                    .sides
                    .iter()
                    .map(|s| {
                        let lives = s.player.as_ref().map_or(0, |p| p.lives);
                        // Levels as the HUD labels them (stored level + 1, FUN_004e8a20).
                        format!("{} enemies, {} hearts, EX Lv.{} Boss Lv.{}, charge {} (free), last {:?}",s.enemies.len(),lives,
                            s.st.extra_level+1, s.st.boss_level+1,
                            s.player.as_ref().map_or("none",|p| {
                                let v=p.charge.value;let c=p.charge_settings;
                                if v>=c.boss {"4 BOSS"} else if v>=c.extra {"3 EX"} else if v>=c.skill {"2 SKILL"}
                                else if v>=c.attack {"1 SHOT"} else if v>0 {"charging"} else {"idle"}
                            }), s.player.as_ref().and_then(|p|p.last_charge))
                    })
                    .collect();
                return format!(
                    "th19-rs - {name}{}{} - {} - {} VMs{}{} - points {}:{} {:?}",
                    {
                        let muted = th_game::sfx::muted();
                        if muted.is_empty() { String::new() } else { format!(" (muted: {})", muted.join(", ")) }
                    },
                    format!("{}{}{}{}", if self.godmode { " - GODMODE" } else { "" }, if self.full_charge { " - FULLCHARGE" } else { "" }, if self.opponent_idle { " - IDLE" } else if self.bot_ai { " - BOT" } else { " - DUMMY" }, if self.player_bot_ai { " - P1 AI" } else { "" }),
                    n.join(" | "),
                    self.world.len(),
                    if self.fast { " - x8" } else { "" },
                    if self.paused { " - PAUSED" } else { "" },
                    g.round.points[0], g.round.points[1], g.round.outcome
                );
            }
            let (t, cam) = self.stage.as_ref().map_or((0, [0.0; 3]), |s| (s.time(), s.pos));
            return format!(
                "th19-rs stage viewer - {name} ({}/{}) - frame {t} - camera ({:.0}, {:.0}, {:.0}) - {} VMs{}{}",
                self.stage_idx + 1,
                self.stage_files.len(),
                cam[0],
                cam[1],
                cam[2],
                self.world.len(),
                if self.fast { " - x8" } else { "" },
                if self.paused { " - PAUSED" } else { "" }
            );
        }
        let n = self.script_count();
        let name = self.files.get(self.file_idx).cloned().unwrap_or_default();
        let what = if self.show_all { "all scripts".to_string() } else { format!("script {}/{}", self.script, n) };
        format!(
            "th19-rs ANM viewer - {name} ({}/{}) - {what} - {} VMs{}",
            self.file_idx + 1,
            self.files.len(),
            self.world.len(),
            if self.paused { " - PAUSED" } else { "" }
        )
    }

    /// Player 1 input bits from the held keys: arrows, Z = shot, Shift = focus.
    fn player_keys(&self) -> u32 {
        use th_game::player::input::*;
        let h = |k: KeyCode| self.held.contains(&k);
        let mut bits = 0;
        for (key, bit) in [
            (KeyCode::ArrowUp, UP),
            (KeyCode::ArrowDown, DOWN),
            (KeyCode::ArrowLeft, LEFT),
            (KeyCode::ArrowRight, RIGHT),
            (KeyCode::KeyZ, SHOT),
            (KeyCode::KeyX, CHARGE),
            (KeyCode::KeyC, BOMB),
        ] {
            if h(key) {
                bits |= bit;
            }
        }
        if h(KeyCode::ShiftLeft) || h(KeyCode::ShiftRight) {
            bits |= FOCUS;
        }
        bits
    }

    /// Esc in a game: pause and show the menu. FUN_00539470 (opening the pause menu) plays 0xe.
    fn open_menu(&mut self) {
        self.paused = true;
        self.menu = Some(Menu { sel: 0, vms: Vec::new() });
        if let Some(a) = &mut self.audio {
            a.set_duck(true);
            if th_game::sfx::on(th_game::sfx::Cat::Pause) {
                a.play(0xe, 0);
            }
        }
        self.draw_menu();
    }

    fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            for id in m.vms {
                self.world.delete(id);
            }
        }
        if let Some(a) = &mut self.audio {
            a.set_duck(false);
        }
        self.paused = false;
    }

    /// The menu's rows for the current mode.
    fn menu_rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Resume, Row::Mode, Row::Char(0)];
        if self.team_size == 2 {
            rows.extend([Row::Char(2), Row::Char(1), Row::Char(3)]);
        } else {
            rows.push(Row::Char(1));
        }
        rows.extend([Row::Song, Row::Se, Row::Music, Row::Quit]);
        rows
    }

    /// Rebuild the menu's text sprites.
    fn draw_menu(&mut self) {
        let Some(ascii) = self.load_anm("ascii.anm") else { return };
        let (se, bgm) = self.audio.as_ref().map_or((0, 0), |a| (a.se_setting(), a.bgm_setting()));
        let rows = self.menu_rows();
        let team = self.team_size;
        let song = match self.music_override {
            None => "Auto".to_string(),
            Some(i) => self.audio.as_ref().and_then(|a| a.track_names().get(i).map(|n| th_formats::music::track_label(n))).unwrap_or_default(),
        };
        // (label, value shown right-aligned)
        let lines: Vec<(String, String)> = rows
            .iter()
            .map(|r| match *r {
                Row::Resume => ("Resume".into(), String::new()),
                Row::Mode => ("Players".into(), if team == 2 { "2 vs 2".into() } else { "1 vs 1".into() }),
                Row::Char(i) => {
                    let label = match (team, i) {
                        (1, 0) => "Player 1",
                        (1, _) => "Player 2",
                        (_, 0) => "You",
                        (_, 2) => "Ally",
                        (_, 1) => "Rival 1",
                        _ => "Rival 2",
                    };
                    (label.into(), self.chars[i].name().to_string())
                }
                Row::Song => ("Song".into(), song.clone()),
                Row::Se => ("Sound effects".into(), format!("{se}%")),
                Row::Music => ("Music".into(), format!("{bgm}%")),
                Row::Quit => ("Quit".into(), String::new()),
            })
            .collect();
        let Some(menu) = &mut self.menu else { return };
        for id in menu.vms.drain(..) {
            self.world.delete(id);
        }
        let sel = menu.sel.min(lines.len() - 1);
        let mut vms = Vec::new();
        vms.extend(th_game::text::big_text_at(&mut self.world, &self.lib, ascii, "- Menu -", [150.0, 60.0], 1.0,
            th_game::text::Align::Left, [180.0, 205.0, 255.0], MENU_PRIORITY));
        for (i, (label, value)) in lines.iter().enumerate() {
            let (mark, color) = if i == sel { ("> ", [255.0, 230.0, 100.0]) } else { ("  ", [255.0, 255.0, 255.0]) };
            let y = 110.0 + i as f32 * 34.0;
            vms.extend(th_game::text::big_text_at(&mut self.world, &self.lib, ascii, &format!("{mark}{label}"),
                [150.0, y], 1.0, th_game::text::Align::Left, color, MENU_PRIORITY));
            if !value.is_empty() {
                let shown = if matches!(rows[i], Row::Mode | Row::Char(_) | Row::Song | Row::Se | Row::Music) { format!("< {value} >") } else { value.clone() };
                vms.extend(th_game::text::big_text_at(&mut self.world, &self.lib, ascii, &shown, [500.0, y], 1.0,
                    th_game::text::Align::Right, color, MENU_PRIORITY));
            }
        }
        if let Some(menu) = &mut self.menu {
            menu.vms = vms;
            menu.sel = sel;
        }
    }

    /// Menu input: Up / Down choose a row; Left / Right or `,` / `.` (the `<` and `>` keys) change
    /// it (volumes by 10%); Z or Enter activates; Esc resumes.
    fn menu_key(&mut self, code: KeyCode) {
        let Some(sel) = self.menu.as_ref().map(|m| m.sel) else { return };
        let rows = self.menu_rows();
        let n = rows.len();
        let row = rows[sel.min(n - 1)];
        // The cursor and value sounds use the menu ids 10 (select00) and 9 (cancel00) seen in the
        // menu code (FUN_004e0f60); which one the original plays for which action is not traced.
        let beep = |v: &mut Viewer, id: u32| {
            if let Some(a) = &mut v.audio {
                a.play(id, 0);
            }
        };
        let delta: i32 = match code {
            KeyCode::ArrowLeft | KeyCode::Comma => -1,
            KeyCode::ArrowRight | KeyCode::Period => 1,
            _ => 0,
        };
        match code {
            KeyCode::Escape => {
                beep(self, 9);
                return self.close_menu();
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown => {
                let next = if code == KeyCode::ArrowUp { (sel + n - 1) % n } else { (sel + 1) % n };
                if let Some(m) = &mut self.menu {
                    m.sel = next;
                }
                beep(self, 10);
            }
            _ if delta != 0 => match row {
                Row::Se | Row::Music => {
                    if let Some(a) = &mut self.audio {
                        let cur = if row == Row::Se { a.se_setting() } else { a.bgm_setting() } as i32;
                        let new = (cur + delta * 10).clamp(0, 100) as u8;
                        if row == Row::Se {
                            a.set_se_setting(new);
                        } else {
                            a.set_bgm_setting(new);
                        }
                    }
                    self.save_settings();
                    beep(self, 10);
                }
                Row::Song => {
                    let count = self.audio.as_ref().map_or(0, |a| a.track_names().len());
                    if count > 0 {
                        // Auto, then every track in thbgm.fmt order.
                        let cur = self.music_override.map_or(0, |i| i + 1) as i32;
                        let next = (cur + delta).rem_euclid(count as i32 + 1) as usize;
                        self.music_override = next.checked_sub(1);
                        self.update_music();
                    }
                    beep(self, 10);
                }
                Row::Char(i) => {
                    self.chars[i] = self.chars[i].shifted(delta);
                    beep(self, 10);
                    self.load_stage();
                }
                Row::Mode => {
                    self.team_size = if self.team_size == 1 { 2 } else { 1 };
                    beep(self, 10);
                    self.load_stage();
                }
                _ => return,
            },
            KeyCode::KeyZ | KeyCode::Enter => match row {
                Row::Resume => {
                    beep(self, 9);
                    return self.close_menu();
                }
                Row::Quit => self.quit = true,
                _ => {}
            },
            _ => return,
        }
        // Changing characters or the mode reloads the match, which clears the world (and the menu's
        // sprites); the menu stays open on top of it.
        if let Some(m) = &mut self.menu {
            m.vms.clear();
        }
        self.draw_menu();
    }

    /// Volume settings are kept in `th19-rs-settings.txt` (`se=N`, `bgm=N`), never in the game's
    /// own th19.cfg.
    fn load_settings(&mut self) {
        let Some(a) = &mut self.audio else { return };
        let Ok(text) = std::fs::read_to_string(&self.settings_path) else { return };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let Ok(v) = value.trim().parse::<u8>() else { continue };
            let v = (v.min(100) + 5) / 10 * 10;
            match key.trim() {
                "se" => a.set_se_setting(v),
                "bgm" => a.set_bgm_setting(v),
                _ => {}
            }
        }
    }

    fn save_settings(&self) {
        let Some(a) = &self.audio else { return };
        let text = format!("se={}\nbgm={}\n", a.se_setting(), a.bgm_setting());
        if let Err(e) = std::fs::write(&self.settings_path, text) {
            log::warn!("{}: {e}", self.settings_path.display());
        }
    }


    fn key(&mut self, code: KeyCode) {
        if self.menu.is_some() {
            return self.menu_key(code);
        }
        // In game mode the arrows steer the player; PageUp/PageDown change stage.
        let code = match (self.mode, code) {
            (Mode::Game, KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown) => return,
            (Mode::Game, KeyCode::PageUp) => KeyCode::ArrowLeft,
            (Mode::Game, KeyCode::PageDown) => KeyCode::ArrowRight,
            (_, c) => c,
        };
        match code {
            KeyCode::Tab => {
                self.mode = match self.mode {
                    Mode::Anm => Mode::Stage,
                    Mode::Stage => Mode::Game,
                    Mode::Game => Mode::Anm,
                };
                self.stage = None;
                if self.mode != Mode::Game {
                    if let Some(a) = &mut self.audio {
                        a.stop_music();
                    }
                    self.music_stage = None;
                }
                self.restart();
            }
            KeyCode::KeyF => self.fast = !self.fast,
            // Development godmode: neither player can be hit.
            KeyCode::KeyG if self.mode == Mode::Game => {
                self.godmode = !self.godmode;
                log::info!("godmode {}", if self.godmode { "on" } else { "off" });
            }
            // K toggles the opponent; Shift+K toggles AI/manual control of player 1.
            KeyCode::KeyK if self.mode == Mode::Game => {
                if self.held.contains(&KeyCode::ShiftLeft) || self.held.contains(&KeyCode::ShiftRight) {
                    self.player_bot_ai = !self.player_bot_ai;
                    self.player_bot = th_game::bot::Bot::new();
                    log::info!("player 1 control {}", if self.player_bot_ai { "AI" } else { "manual" });
                } else {
                    self.bot_ai = !self.bot_ai;
                    self.bot = th_game::bot::Bot::new();
                    log::info!("bot ai {}", if self.bot_ai { "on" } else { "off (holds shot)" });
                }
            }
            // F1..F12 mute / unmute sound groups (Shift: the next twelve); see th_game::sfx::Cat.
            KeyCode::F1 | KeyCode::F2 | KeyCode::F3 | KeyCode::F4 | KeyCode::F5 | KeyCode::F6
            | KeyCode::F7 | KeyCode::F8 | KeyCode::F9 | KeyCode::F10 | KeyCode::F11 | KeyCode::F12 => {
                let n = code as usize - KeyCode::F1 as usize;
                let shift = self.held.contains(&KeyCode::ShiftLeft) || self.held.contains(&KeyCode::ShiftRight);
                if th_game::sfx::toggle(n + if shift { 12 } else { 0 }) {
                    log::info!("muted sound groups: {:?}", th_game::sfx::muted());
                }
            }
            // L: the opponent stands still and does nothing.
            KeyCode::KeyL if self.mode == Mode::Game => {
                self.opponent_idle = !self.opponent_idle;
                log::info!("opponent idle {}", self.opponent_idle);
            }
            // Development full-charge mode: all tiers always available, no meter cost.
            KeyCode::KeyM if self.mode == Mode::Game => {
                self.full_charge = !self.full_charge;
                log::info!("full charge mode {}", if self.full_charge { "on" } else { "off (meter from orbs)" });
            }
            // Character select: ] / [ move the bot's character up / down the list; with Shift
            // they change the player's. The match restarts with the new characters.
            KeyCode::BracketRight | KeyCode::BracketLeft if self.mode == Mode::Game => {
                let side = if self.held.contains(&KeyCode::ShiftLeft) || self.held.contains(&KeyCode::ShiftRight) { 0 } else { 1 };
                let delta = if code == KeyCode::BracketRight { 1 } else { -1 };
                self.chars[side] = self.chars[side].shifted(delta);
                log::info!("player {} is {} ({})", side + 1, self.chars[side].name(), self.chars[side].number());
                self.load_stage();
            }
            KeyCode::KeyV => self.show_fields = !self.show_fields,
            KeyCode::ArrowRight | KeyCode::ArrowLeft if self.mode != Mode::Anm => {
                let n = self.stage_files.len().max(1);
                self.stage_idx =
                    if code == KeyCode::ArrowRight { (self.stage_idx + 1) % n } else { (self.stage_idx + n - 1) % n };
                self.load_stage();
            }
            KeyCode::ArrowRight | KeyCode::ArrowLeft => {
                let n = self.files.len();
                self.file_idx = if code == KeyCode::ArrowRight { (self.file_idx + 1) % n } else { (self.file_idx + n - 1) % n };
                self.script = 0;
                self.restart();
            }
            KeyCode::ArrowDown | KeyCode::ArrowUp => {
                let n = self.script_count().max(1);
                self.show_all = false;
                self.script = if code == KeyCode::ArrowDown { (self.script + 1) % n } else { (self.script + n - 1) % n };
                self.restart();
            }
            KeyCode::KeyA => {
                self.show_all = !self.show_all;
                self.restart();
            }
            // Development rain of bullets and spirits: T on the opponent, Shift+T on yourself.
            KeyCode::KeyT if self.mode == Mode::Game => {
                let own = self.held.contains(&KeyCode::ShiftLeft) || self.held.contains(&KeyCode::ShiftRight);
                if let Some(g) = &mut self.game {
                    g.sides[if own { 0 } else { 1 }].start_rain();
                    log::info!("rain on {}", if own { "player 1" } else { "player 2" });
                }
            }
            // Development: clear both fields (bullets, enemies, bosses, EX attacks).
            KeyCode::KeyB if self.mode == Mode::Game => {
                if let Some(g) = &mut self.game {
                    for s in &mut g.sides {
                        s.dev_clear(&mut self.world);
                    }
                    log::info!("cleared both fields");
                }
            }
            // In game mode C is the bomb (a held input bit, see `player_keys`).
            KeyCode::KeyC if self.mode == Mode::Game => {}
            KeyCode::KeyC => {
                self.centered = !self.centered;
                self.restart();
            }
            KeyCode::KeyR => self.restart(),
            KeyCode::Space => self.paused = !self.paused,
            KeyCode::Escape if self.mode == Mode::Game => self.open_menu(),
            KeyCode::Period if self.paused => {
                let fast = std::mem::replace(&mut self.fast, false);
                self.tick();
                self.fast = fast;
            }
            // Development: release player 1's charge at a tier instantly
            // (1 charge shot, 2 skill, 3 EX attack, 4 boss attack).
            KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 if self.mode == Mode::Game => {
                use th_game::charge::Level;
                let level = [Level::Attack, Level::Skill, Level::Extra, Level::Boss]
                    [code as usize - KeyCode::Digit1 as usize];
                // With Shift, the opponent (player 2) releases it instead, so EX and boss
                // attacks land on player 1's field.
                let side = (self.held.contains(&KeyCode::ShiftLeft) || self.held.contains(&KeyCode::ShiftRight)) as usize;
                if let Some(p) = self.game.as_mut().and_then(|g| g.sides[side].player.as_mut()) {
                    p.instant_charge(level);
                }
            }
            KeyCode::Digit0 | KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4
            | KeyCode::Digit5 | KeyCode::Digit6 | KeyCode::Digit7 | KeyCode::Digit8 | KeyCode::Digit9 => {
                let n = code as i32 - KeyCode::Digit0 as i32;
                log::info!("interrupt {n} -> all VMs");
                self.world.interrupt_all(n);
            }
            _ => {}
        }
    }

    /// Development: `TH19_DEBUG_SPAWN=frame:Sub:x:y:side[:level]` (several, separated by `;`)
    /// creates an ECL sub on a field at that frame, for screenshots and comparisons.
    fn debug_spawn(&mut self) {
        let Ok(spec) = std::env::var("TH19_DEBUG_SPAWN") else { return };
        let Some(g) = &mut self.game else { return };
        for one in spec.split(';') {
            let f: Vec<&str> = one.split(':').collect();
            if f.len() < 5 || f[0].parse::<u64>().ok() != Some(self.frames) {
                continue;
            }
            let num = |i: usize| f.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
            let level = num(5) as i32;
            if let Some(side) = g.sides.get_mut(num(4) as usize) {
                side.st.request(th_game::side::SpawnRequest {
                    id: 0, difficulty: 0, sub: f[1].into(), pos: [num(2), num(3)], hp: 200, score: 0,
                    item: -1, mirrored: false, ivars: [level, 0, level, 0], fvars: [0.0; 8], owner: None,
                });
            }
        }
    }

    /// Debug: send an extra attack into player 1's field.
    fn debug_extra_attack(&mut self) {
        if let Some(g) = &mut self.game {
            g.sides[0].st.request(th_game::side::SpawnRequest {
                id: 0,
                difficulty: 0,
                sub: "ExtraAttack".into(),
                pos: [0.0, 0.0],
                hp: 1000,
                score: 0,
                item: -1,
                mirrored: false,
                ivars: [0; 4],
                fvars: [0.0; 8],
                // Sent by player 2, so it runs that character's program.
                owner: Some(1),
            });
        }
    }
}

/// `--screenshot <file.png> [--frames N]`: render frame N offscreen, save it and exit.
struct Screenshot {
    /// Deterministic input exercise, used only for an offscreen capture.
    charge_tier: Option<usize>,
    path: PathBuf,
    frames: u32,
}

struct App {
    screenshot: Option<Screenshot>,
    viewer: Viewer,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    next_frame: Instant,
    last_title: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("th19-rs")
            .with_visible(self.screenshot.is_none())
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 960.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        self.renderer = Some(Renderer::new(window.clone()));
        self.window = Some(window);
        self.next_frame = Instant::now();

        if let Some(shot) = self.screenshot.take() {
            let charge_frames=shot.charge_tier.and_then(|tier| self.viewer.game.as_ref()
                .and_then(|g|g.sides[0].player.as_ref()).map(|p| {
                    let s=p.charge_settings; [s.attack,s.skill,s.extra,s.boss][tier-1] as u32/10
                }));
            for frame in 0..shot.frames {
                if let Some(n)=charge_frames {
                    if (60..60+n).contains(&frame) {self.viewer.held.insert(KeyCode::KeyX);}
                    else {self.viewer.held.remove(&KeyCode::KeyX);}
                }
                self.viewer.tick();
            }
            let (list, scene_end, bg, fog) = self.viewer.frame();
            let r = self.renderer.as_mut().unwrap();
            let (w, h) = (1280, 960);
            let rgba = r.screenshot(&self.viewer.lib, &list, scene_end, bg, fog, w, h);
            match image::save_buffer(&shot.path, &rgba, w, h, image::ExtendedColorType::Rgba8) {
                Ok(()) => println!("saved {} (frame {})", shot.path.display(), shot.frames),
                Err(e) => eprintln!("failed to save {}: {e}", shot.path.display()),
            }
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Released, .. },
                ..
            } => {
                self.viewer.held.remove(&code);
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Pressed, repeat, .. },
                ..
            } => {
                self.viewer.held.insert(code);
                if repeat {
                    return;
                }
                // In a game Esc pauses (as in the original) and Shift+Esc quits; elsewhere Esc quits.
                if code == KeyCode::Escape {
                    let shift = self.viewer.held.contains(&KeyCode::ShiftLeft) || self.viewer.held.contains(&KeyCode::ShiftRight);
                    if self.viewer.mode != Mode::Game || shift {
                        event_loop.exit();
                    }
                }
                let before = (self.viewer.file_idx, self.viewer.stage_idx, self.viewer.mode);
                self.viewer.key(code);
                if self.viewer.quit {
                    event_loop.exit();
                }
                if before != (self.viewer.file_idx, self.viewer.stage_idx, self.viewer.mode) {
                    if let Some(r) = &mut self.renderer {
                        r.forget_textures();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                // Fixed 60 Hz simulation, like the original game.
                let mut steps = 0;
                while self.next_frame <= now && steps < 4 {
                    if !self.viewer.paused {
                        self.viewer.tick();
                    }
                    self.next_frame += FRAME;
                    steps += 1;
                }
                if self.next_frame < now {
                    self.next_frame = now;
                }
                let (list, scene_end, bg, fog) = self.viewer.frame();
                if let Some(r) = &mut self.renderer {
                    r.render(&self.viewer.lib, &list, scene_end, bg, fog);
                }
                if now.duration_since(self.last_title) > Duration::from_millis(250) {
                    self.last_title = now;
                    let t = self.viewer.title();
                    if let Some(w) = &self.window {
                        w.set_title(&t);
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let Some(dat_path) = find_dat() else {
        eprintln!(
            "Could not find th19.dat.\n\
             th19-rs needs your own copy of Touhou 19. Pass --dat <path/to/th19.dat>,\n\
             or set TH19_DAT / TH19_DIR."
        );
        std::process::exit(1);
    };
    let dat = match Archive::open(&dat_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to open {}: {e}", dat_path.display());
            std::process::exit(1);
        }
    };
    log::info!("using {} ({} files)", dat_path.display(), dat.entries().len());
    println!(
        "Controls: Tab = switch ANM viewer / stage viewer / game, Left/Right = file or stage, Up/Down = single script,\n\
         A = all scripts, C = toggle centre origin, F = fast-forward x8, V = toggle field divider,\n\
         R = restart, Space = pause. Game: arrows = move, Z = shot, X = hold/release charge (1 shot / 2 skill / 3 EX / 4 boss), M = toggle full-charge practice, C = bomb, B = clear fields, T = rain on the opponent (Shift+T on you), G = godmode, 1-4 = instant charge tier (4 = boss; Shift+1-4 = opponent's), Shift = focus, [ / ] = bot character (Shift for player), K = opponent bot/dummy, L = opponent does nothing, Shift+K = player AI/manual, Esc = menu (sound / music volume), F1-F12 (Shift for more) = mute a sound group, PgUp/PgDn = stage,\n\
         . = step frame, 0-9 = send interrupt, Space = pause (Shift+Esc quits a game; elsewhere Esc quits)"
    );

    let mut viewer = Viewer::new(dat);
    // Bullet sprite types live in th19.exe, next to th19.dat.
    let exe_path = dat_path.with_file_name("th19.exe");
    match std::fs::read(&exe_path).map_err(|e| e.to_string()).and_then(|b| th_formats::exe::Exe::parse(b).map_err(|e| e.to_string())) {
        Ok(exe) => {
            viewer.bullet_types = std::rc::Rc::new(th_formats::exe::bullet_types(&exe));
            if !std::env::args().any(|a| a == "--no-audio") {
                viewer.audio = audio::Audio::new(
                    &exe,
                    &mut viewer.dat,
                    &exe_path.with_file_name("th19.cfg"),
                    &exe_path.with_file_name("thbgm.dat"),
                );
                viewer.character_stages = th_formats::music::character_stages(&exe);
                viewer.load_settings();
            }
        }
        Err(e) => log::warn!("{}: {e} (enemy bullets disabled)", exe_path.display()),
    }
    // `--stage N` starts in the stage viewer on worldNN.std; `--game N` also runs the waves.
    let args: Vec<String> = std::env::args().collect();
    let start = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)?.parse::<usize>().ok());
    if let Some((n, mode)) = start("--game").map(|n| (n, Mode::Game)).or(start("--stage").map(|n| (n, Mode::Stage))) {
        if let Some(n) = args.iter().position(|a| a == "--boss-level").and_then(|i| args.get(i + 1)?.parse::<u8>().ok()) {
            viewer.start_boss_level = n.clamp(1, 8) - 1;
        }
        viewer.godmode = args.iter().any(|a| a == "--godmode");
        viewer.full_charge = args.iter().any(|a| a == "--full-charge");
        viewer.bot_ai = !args.iter().any(|a| a == "--no-bot");
        // `--p1 N` / `--p2 N`: character select numbers (1 Reimu, 2 Marisa, 3 Sanae, 4 Ran).
        // `--2v2` plays two per team; `--p3` is your ally, `--p4` the second rival.
        if args.iter().any(|a| a == "--2v2") { viewer.team_size = 2; }
        for (flag, side) in [("--p1", 0), ("--p2", 1), ("--p3", 2), ("--p4", 3)] {
            if let Some(n) = args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)?.parse::<usize>().ok()) {
                viewer.chars[side] = th_game::character::Character::ALL[(n.max(1) - 1).min(th_game::character::Character::ALL.len() - 1)];
            }
        }
        viewer.mode = mode;
        viewer.stage_idx = n.saturating_sub(1).min(viewer.stage_files.len().saturating_sub(1));
        viewer.restart();
    }

    // Development: `--anm FILE.anm [--script N]` shows one ANM script (with --screenshot).
    if let Some(name) = args.iter().position(|a| a == "--anm").and_then(|i| args.get(i + 1)) {
        if let Some(i) = viewer.files.iter().position(|f| f == name) {
            viewer.file_idx = i;
            viewer.script = args.iter().position(|a| a == "--script").and_then(|i| args.get(i + 1)?.parse().ok()).unwrap_or(0);
            viewer.centered = true;
            viewer.mode = Mode::Anm;
            viewer.restart();
        }
    }

    let screenshot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1)).map(|p| Screenshot {
        path: PathBuf::from(p),
        charge_tier: args.iter().position(|a|a=="--preview-charge")
            .and_then(|i|args.get(i+1)?.parse::<usize>().ok()).filter(|v|(1..=4).contains(v)),
        frames: args
            .iter()
            .position(|a| a == "--frames")
            .and_then(|i| args.get(i + 1)?.parse().ok())
            .unwrap_or(0),
    });

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        screenshot,
        viewer,
        window: None,
        renderer: None,
        next_frame: Instant::now(),
        last_title: Instant::now(),
    };
    event_loop.run_app(&mut app).expect("run app");
}

#[cfg(test)]
mod charge_input_tests {
    use super::*;
    #[test]
    fn two_vs_two_teammates_have_own_hearts_and_one_boss_per_field() {
        let Some(home) = std::env::var_os("USERPROFILE") else { return };
        let path = std::env::var_os("TH19_DAT").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join("Downloads/th19/th19/th19.dat"));
        if !path.is_file() { return; }
        use th_game::character::Character::*;
        use th_game::charge::Level;
        let mut viewer = Viewer::new(Archive::open(path).unwrap());
        viewer.mode = Mode::Game;
        viewer.team_size = 2;
        viewer.chars = [Reimu, Marisa, Sanae, Ran];
        viewer.game = viewer.load_game();
        let g = viewer.game.as_mut().unwrap();
        assert_eq!(g.progs.len(), 4);
        assert_eq!(g.sides[1].partner.as_ref().unwrap().character, Ran);
        // Separate hearts: one teammate out does not end the team.
        g.sides[0].partner.as_mut().unwrap().state = th_game::player::State::GameOver;
        assert!(!g.sides[0].defeated());
        g.sides[0].player.as_mut().unwrap().state = th_game::player::State::GameOver;
        assert!(g.sides[0].defeated());
        // Both teammates release a boss attack in the same frame: one boss, one EX.
        for side in &mut g.sides { for p in side.player.iter_mut().chain(side.partner.iter_mut()) { p.state = th_game::player::State::Normal; } }
        g.sides[0].player.as_mut().unwrap().charge_event = Some(Level::Boss);
        g.sides[0].partner.as_mut().unwrap().charge_event = Some(Level::Boss);
        th_game::side::dispatch_charges(&mut g.sides);
        let subs: Vec<(&str, Option<usize>)> = g.sides[1].st.spawn.iter().map(|r| (r.sub.as_str(), r.owner)).collect();
        assert_eq!(subs, [("BossAttack", Some(0)), ("ExtraAttack", Some(2))], "{subs:?}");
    }

    #[test]
    fn shift_k_toggles_independent_player_ai_and_returns_to_manual_input() {
        let Some(home) = std::env::var_os("USERPROFILE") else { return };
        let path = std::env::var_os("TH19_DAT").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join("Downloads/th19/th19/th19.dat"));
        if !path.is_file() { return; }
        let mut viewer = Viewer::new(Archive::open(path).unwrap());
        viewer.mode = Mode::Game;
        viewer.game = viewer.load_game();
        for side in &mut viewer.game.as_mut().unwrap().sides {
            side.stage = None;
            side.player.as_mut().unwrap().state = th_game::player::State::Normal;
        }
        viewer.game.as_mut().unwrap().sides[0].player.as_mut().unwrap().pos = [80.0,300.0];
        viewer.held.insert(KeyCode::ArrowRight);
        viewer.held.insert(KeyCode::ShiftLeft);
        viewer.key(KeyCode::KeyK);
        viewer.held.remove(&KeyCode::ShiftLeft);
        assert!(viewer.player_bot_ai && viewer.bot_ai);
        for _ in 0..10 { viewer.tick(); }
        let ai_x = viewer.game.as_ref().unwrap().sides[0].player.as_ref().unwrap().pos[0];
        assert!(ai_x < 80.0, "AI steers toward home, ignoring held right: {ai_x}");
        assert!(viewer.title().contains("P1 AI"));
        viewer.key(KeyCode::KeyK);
        assert!(!viewer.bot_ai && viewer.player_bot_ai, "plain K only changes opponent");
        viewer.held.insert(KeyCode::ShiftRight);
        viewer.key(KeyCode::KeyK);
        viewer.held.remove(&KeyCode::ShiftRight);
        assert!(!viewer.player_bot_ai && !viewer.bot_ai);
        viewer.tick();
        assert!(viewer.game.as_ref().unwrap().sides[0].player.as_ref().unwrap().pos[0] > ai_x,
            "manual movement resumes immediately");
        assert!(!viewer.title().contains("P1 AI"));
    }

    #[test]
    fn x_drives_player_charge_without_debug_enemy_injection() {
        let Some(home) = std::env::var_os("USERPROFILE") else { return };
        let path = std::env::var_os("TH19_DAT").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join("Downloads/th19/th19/th19.dat"));
        if !path.is_file() { eprintln!("Original assets absent: skipping viewer input test"); return; }
        let mut viewer = Viewer::new(Archive::open(path).unwrap());
        viewer.mode = Mode::Game;
        viewer.game = viewer.load_game();
        for side in &mut viewer.game.as_mut().unwrap().sides {
            side.stage = None;
            side.player.as_mut().unwrap().state = th_game::player::State::Normal;
        }
        viewer.held.insert(KeyCode::KeyX);
        viewer.key(KeyCode::KeyX);
        for _ in 0..50 { viewer.tick(); }
        let game = viewer.game.as_ref().unwrap();
        assert_eq!(game.sides[0].player.as_ref().unwrap().charge.value, 500);
        assert!(game.sides[0].enemies.is_empty(), "X must no longer inject debug ExtraAttack");
        let (list, _, _, _) = viewer.frame();
        assert!(list.iter().any(|batch| batch.vertices.iter().any(|v| v.color == [1.0, 0.8, 0.2, 1.0])),
            "ready charge must produce a gold HUD bar");
        viewer.held.remove(&KeyCode::KeyX);
        viewer.tick();
        let game = viewer.game.as_ref().unwrap();
        assert_eq!(game.sides[0].player.as_ref().unwrap().bullets.iter().filter(|b| b.charge_shot).count(), 4);
        assert_eq!(game.sides[1].player.as_ref().unwrap().bullets.iter().filter(|b| b.charge_shot).count(), 0);
    }
}
