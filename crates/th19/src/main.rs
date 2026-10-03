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
    prog: th_ecl::Program,
    sides: Vec<th_game::Side>,
    round: th_game::round::Round,
}

struct Viewer {
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
        }
    }

    /// Load the ECL scripts and their sprites and start both sides' stage scripts.
    fn load_game(&mut self) -> Option<Game> {
        let dat = &mut self.dat;
        let prog = th_ecl::Program::load("pl00.ecl", |n| dat.read(n).ok())
            .map_err(|e| log::error!("pl00.ecl: {e}"))
            .ok()?;
        // Enemy sprite slots: 0 = bullet.anm, 1 = effect.anm (FUN_0050d380), then the ECL
        // file's anim list from 2 upwards.
        let mut anm_files = [None; 8];
        let names: Vec<String> =
            ["bullet.anm".to_string(), "effect.anm".to_string()].into_iter().chain(prog.files[0].1.anim.clone()).collect();
        for (i, name) in names.iter().enumerate() {
            let id = match self.lib.file_id(name) {
                Some(id) => id,
                None => {
                    let bytes = self.dat.read(name).map_err(|e| log::error!("{name}: {e}")).ok()?;
                    self.lib.load(name, &bytes).map_err(|e| log::error!("{name}: {e}")).ok()?
                }
            };
            anm_files[i] = Some(id);
        }
        // Both players face the same wave order.
        let seq = th_game::side::roll_wave_sequence(0x1234_5678);
        let mut sides: Vec<th_game::Side> = FIELDS
            .iter()
            .enumerate()
            .map(|(i, &f)| th_game::Side::new(f, 0x9e37_79b9 ^ i as u32, seq, anm_files))
            .collect();
        // APPROX: both sides play Reimu until character select exists.
        let sht = self.dat.read("pl00.sht").ok().and_then(|b| th_formats::sht::ShtFile::parse(&b).ok());
        let pl_anm = match self.lib.file_id("pl00.anm") {
            Some(id) => Some(id),
            None => self.dat.read("pl00.anm").ok().and_then(|b| self.lib.load("pl00.anm", &b).ok()),
        };
        let ghost = match self.lib.file_id("ghost.anm") {
            Some(id) => Some(id),
            None => self.dat.read("ghost.anm").ok().and_then(|b| self.lib.load("ghost.anm", &b).ok()),
        };
        let aura = match self.lib.file_id("aura.anm") {
            Some(id) => Some(id),
            None => self.dat.read("aura.anm").ok().and_then(|b| self.lib.load("aura.anm", &b).ok()),
        };
        for s in &mut sides {
            s.st.bullet_types = self.bullet_types.clone();
            s.st.ghost_anm = ghost;
            s.start(&prog);
            if let (Some(sht), Some(anm)) = (&sht, pl_anm) {
                let mut p = th_game::player::Player::new(sht.clone(), anm);
                p.free_charge = true;
                p.aura_anm = aura;
                p.effect_anm = s.st.anm_files[1];
                s.player = Some(p);
            }
        }
        Some(Game { prog, sides, round: Default::default() })
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
            let keys = self.player_keys();
            if let Some(g) = &mut self.game {
                th_game::side::prepare_charge_targets(&mut g.sides);
                for (i, s) in g.sides.iter_mut().enumerate().filter(|_| g.round.outcome.is_none()) {
                    // Side 2 has no controller yet: it just holds the shot button.
                    let k = if i == 0 { keys } else { th_game::player::input::SHOT };
                    if let Some(p) = &mut s.player {
                        p.godmode = self.godmode;
                    }
                    s.tick(&g.prog, k, &mut self.world, &self.lib);
                }
                th_game::side::dispatch_charges(&mut g.sides);
                let defeated = std::array::from_fn(|i| g.sides[i].player.as_ref()
                    .is_some_and(|p| p.state == th_game::player::State::GameOver));
                g.round.resolve(defeated);
            }
            self.world.tick(&self.lib);
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
        list.extend(self.world.draw_list(&self.lib, th_anm::Pass::Ui));
        if let Some(g) = &self.game {
            for (side, field) in g.sides.iter().zip(FIELDS) {
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
                    "th19-rs - {name}{} - {} - {} VMs{}{} - points {}:{} {:?}",
                    if self.godmode { " - GODMODE" } else { "" },
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

    fn key(&mut self, code: KeyCode) {
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
                self.restart();
            }
            KeyCode::KeyF => self.fast = !self.fast,
            // Development godmode: neither player can be hit.
            KeyCode::KeyG if self.mode == Mode::Game => {
                self.godmode = !self.godmode;
                log::info!("godmode {}", if self.godmode { "on" } else { "off" });
            }
            // Debug: send an extra attack into player 1's field.
            KeyCode::KeyB if self.mode == Mode::Game => self.debug_extra_attack(),
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
            // Development: clear both fields (bullets, enemies, bosses, EX attacks).
            KeyCode::KeyC if self.mode == Mode::Game => {
                if let Some(g) = &mut self.game {
                    for s in &mut g.sides {
                        s.dev_clear(&mut self.world);
                    }
                    log::info!("cleared both fields");
                }
            }
            KeyCode::KeyC => {
                self.centered = !self.centered;
                self.restart();
            }
            KeyCode::KeyR => self.restart(),
            KeyCode::Space => self.paused = !self.paused,
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
                if let Some(p) = self.game.as_mut().and_then(|g| g.sides[0].player.as_mut()) {
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
                if code == KeyCode::Escape {
                    event_loop.exit();
                }
                let before = (self.viewer.file_idx, self.viewer.stage_idx, self.viewer.mode);
                self.viewer.key(code);
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
         R = restart, Space = pause. Game: arrows = move, Z = shot, X = hold/release charge (1 shot / 2 skill / 3 EX / 4 boss; free meter), B = debug EX, G = godmode, 1-4 = instant charge tier (4 = boss), C = clear fields, Shift = focus, PgUp/PgDn = stage,\n\
         . = step frame, 0-9 = send interrupt, Esc = quit"
    );

    let mut viewer = Viewer::new(dat);
    // Bullet sprite types live in th19.exe, next to th19.dat.
    let exe_path = dat_path.with_file_name("th19.exe");
    match std::fs::read(&exe_path).map_err(|e| e.to_string()).and_then(|b| th_formats::exe::Exe::parse(b).map_err(|e| e.to_string())) {
        Ok(exe) => viewer.bullet_types = std::rc::Rc::new(th_formats::exe::bullet_types(&exe)),
        Err(e) => log::warn!("{}: {e} (enemy bullets disabled)", exe_path.display()),
    }
    // `--stage N` starts in the stage viewer on worldNN.std; `--game N` also runs the waves.
    let args: Vec<String> = std::env::args().collect();
    let start = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)?.parse::<usize>().ok());
    if let Some((n, mode)) = start("--game").map(|n| (n, Mode::Game)).or(start("--stage").map(|n| (n, Mode::Stage))) {
        viewer.mode = mode;
        viewer.stage_idx = n.saturating_sub(1).min(viewer.stage_files.len().saturating_sub(1));
        viewer.restart();
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
