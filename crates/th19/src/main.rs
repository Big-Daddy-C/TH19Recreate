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
}

struct Viewer {
    mode: Mode,
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
        self.stage = Some(Stage::new(std, anm, &self.lib, &mut self.world));
    }

    fn tick(&mut self) {
        for _ in 0..if self.fast { 8 } else { 1 } {
            if let Some(st) = &mut self.stage {
                st.tick(&mut self.world);
            }
            self.world.tick(&self.lib);
        }
    }

    fn restart(&mut self) {
        if self.mode == Mode::Stage {
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

    fn title(&mut self) -> String {
        if self.mode == Mode::Stage {
            let name = self.stage_files.get(self.stage_idx).cloned().unwrap_or_default();
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

    fn key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Tab => {
                self.mode = if self.mode == Mode::Anm { Mode::Stage } else { Mode::Anm };
                self.stage = None;
                self.restart();
            }
            KeyCode::KeyF => self.fast = !self.fast,
            KeyCode::ArrowRight | KeyCode::ArrowLeft if self.mode == Mode::Stage => {
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
            KeyCode::Digit0 | KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4
            | KeyCode::Digit5 | KeyCode::Digit6 | KeyCode::Digit7 | KeyCode::Digit8 | KeyCode::Digit9 => {
                let n = code as i32 - KeyCode::Digit0 as i32;
                log::info!("interrupt {n} -> all VMs");
                self.world.interrupt_all(n);
            }
            _ => {}
        }
    }
}

struct App {
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
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 960.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        self.renderer = Some(Renderer::new(window.clone()));
        self.window = Some(window);
        self.next_frame = Instant::now();
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
                event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Pressed, .. },
                ..
            } => {
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
                let v = &self.viewer;
                let (mut list, bg, fog) = match &v.stage {
                    Some(st) => {
                        let cam = st.camera([0.0, 0.0, render::CANVAS_W, render::CANVAS_H]);
                        let c = st.clear_color.map(|x| x / 255.0);
                        let f = [st.fog_color[0] / 255.0, st.fog_color[1] / 255.0, st.fog_color[2] / 255.0];
                        // Distant geometry fades into the fog, so fill the background with it too.
                        (v.world.draw_list(&v.lib, Some(&cam)), if st.fog_range[1] > 0.0 { f } else { c }, f)
                    }
                    None => (Vec::new(), [0.0; 3], [0.0; 3]),
                };
                list.extend(v.world.draw_list(&v.lib, None));
                if let Some(r) = &mut self.renderer {
                    r.render(&v.lib, &list, bg, fog);
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
        "Controls: Left/Right = file, Up/Down = single script, A = all scripts, C = toggle centre origin,\n\
         R = restart, Space = pause, . = step frame, 0-9 = send interrupt, Esc = quit"
    );

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        viewer: Viewer::new(dat),
        window: None,
        renderer: None,
        next_frame: Instant::now(),
        last_title: Instant::now(),
    };
    event_loop.run_app(&mut app).expect("run app");
}
