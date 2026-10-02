//! STD stage background player: runs the camera/fog script and spawns the
//! stage's ANM quads as 3D VMs.
//!
//! Behaviour follows the community STD reference plus observations of th19.exe
//! (see `docs/stage.md`). Opcodes whose effect is not modelled yet are counted in
//! [`Stage::unknown_ops`].

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use th_anm::interp::Interp;
use th_anm::{AnmLibrary, Camera, VmId, World};
use th_formats::stage::{Instr, StdFile};

/// Camera "rock" (sway) state set by `ins_12`.
#[derive(Clone, Copy, Debug, Default)]
struct Rock {
    mode: u8,
    /// Main sway timer in frames.
    t: f32,
    /// Secondary timer used by mode 6 to fade the sway out.
    t2: f32,
    /// Added to the camera position.
    eye: [f32; 3],
    /// Added to the facing vector before it is normalised.
    facing: [f32; 3],
}

pub struct Stage {
    file: StdFile,
    anm_file: usize,
    ip: usize,
    time: i32,
    stopped: bool,
    pending_interrupt: Option<i32>,

    pub pos: [f32; 3],
    pub facing: [f32; 3],
    pub up: [f32; 3],
    pub fov: f32,
    /// Fog colour `[r, g, b, a]` in 0..255.
    pub fog_color: [f32; 4],
    /// Fog near/far distances.
    pub fog_range: [f32; 2],
    /// Background clear colour `[r, g, b]` in 0..255.
    pub clear_color: [f32; 3],
    /// Squared draw distance (`ins_20`); not used for culling yet.
    pub draw_distance_sq: f32,

    i_pos: Interp<3>,
    i_facing: Interp<3>,
    i_up: Interp<3>,
    i_fog: Interp<6>,
    i_fov: Interp<1>,
    rock: Rock,

    vms: Vec<VmId>,
    /// STD sprite slots (`ins_14`): 2D sprites, at most 8 at a time, with their layer.
    slots: [Option<(VmId, i32)>; 8],
    /// Canvas position backdrop sprites are placed relative to.
    pub backdrop_origin: [f32; 2],
    pub unknown_ops: HashMap<u16, u32>,
}

/// `0xAARRGGBB` -> `[r, g, b, a]`.
fn argb(c: i32) -> [f32; 4] {
    let c = c as u32;
    [((c >> 16) & 0xff) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32, (c >> 24) as f32]
}

/// Wrap an angle into `[-pi, pi]`.
fn wrap(mut a: f32) -> f32 {
    while a > PI {
        a -= TAU;
    }
    while a < -PI {
        a += TAU;
    }
    a
}

impl Stage {
    /// `anm_file` is the id of the stage's `.anm` (named by `file.anm_name`) in `lib`.
    pub fn new(file: StdFile, anm_file: usize, lib: &AnmLibrary, world: &mut World) -> Self {
        // Debug aid: TH19_STD_SCRIPTS=0,3 spawns only quads using those ANM scripts.
        let only: Option<Vec<u16>> = std::env::var("TH19_STD_SCRIPTS")
            .ok()
            .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect());
        // Draw order: each object (objects are listed in layer order: ground, trees, canopy...)
        // across all of its instances, rather than instance by instance. Going instance by
        // instance lets a nearer row's lower layer paint over a farther row's upper layer,
        // which shows up as seams between repeating tiles.
        // APPROX: the exact order in th19.exe still needs confirming.
        let mut vms = Vec::new();
        for obj in &file.objects {
            for inst in file.instances.iter().filter(|i| i.object == obj.id) {
                for q in &obj.quads {
                    if only.as_ref().is_some_and(|o| !o.contains(&q.script)) {
                        continue;
                    }
                    let p = [inst.pos[0] + q.pos[0], inst.pos[1] + q.pos[1], inst.pos[2] + q.pos[2]];
                    // A non-zero quad width/height stretches the sprite on that axis (negative
                    // mirrors it), e.g. a 512x256 mist sprite tiled at 1024x512, or stage 13's
                    // blossoms at -640x640.
                    let size = (q.size[0] != 0.0 || q.size[1] != 0.0).then_some(q.size);
                    vms.push(world.spawn_3d(lib, anm_file, q.script as usize, p, size));
                }
            }
        }
        let mut stage = Self {
            file,
            anm_file,
            ip: 0,
            time: 0,
            stopped: false,
            pending_interrupt: None,
            // Defaults used by the game when a stage script does not set them.
            pos: [0.0, 0.0, -600.0],
            facing: [0.0, 300.0, 600.0],
            up: [0.0, 1.0, 0.0],
            fov: std::f32::consts::FRAC_PI_4,
            fog_color: [0xa0 as f32, 0xa0 as f32, 0xa0 as f32, 255.0],
            fog_range: [1000.0, 5000.0],
            clear_color: [0.0; 3],
            draw_distance_sq: 3100.0 * 3100.0,
            i_pos: Interp::default(),
            i_facing: Interp::default(),
            i_up: Interp::default(),
            i_fog: Interp::default(),
            i_fov: Interp::default(),
            rock: Rock::default(),
            vms,
            slots: [None; 8],
            // APPROX: anchor of sprite-slot sprites; confirm against th19.exe.
            backdrop_origin: [320.0, 240.0],
            unknown_ops: HashMap::new(),
        };
        stage.run(world, lib);
        stage
    }

    pub fn anm_file(&self) -> usize {
        self.anm_file
    }

    pub fn time(&self) -> i32 {
        self.time
    }

    /// Trigger an STD interrupt (`case n`).
    pub fn interrupt(&mut self, n: i32) {
        self.pending_interrupt = Some(n);
    }

    /// Advance the stage one frame. The caller ticks the ANM `world` separately.
    pub fn tick(&mut self, world: &mut World, lib: &AnmLibrary) {
        self.time += 1;
        self.run(world, lib);
        // Interpolations advance after the frame's instructions, as in the game.
        if let Some(v) = self.i_facing.step() {
            self.facing = v;
        }
        if let Some(v) = self.i_pos.step() {
            self.pos = v;
        }
        if let Some([r, g, b, a, near, far]) = self.i_fog.step() {
            self.fog_color = [r, g, b, a];
            self.fog_range = [near, far];
        }
        if let Some(v) = self.i_up.step() {
            self.up = v;
        }
        if let Some([f]) = self.i_fov.step() {
            self.fov = f;
        }
        self.update_rock();
    }

    fn run(&mut self, world: &mut World, lib: &AnmLibrary) {
        if let Some(n) = self.pending_interrupt.take() {
            if let Some(at) = self.file.script.iter().position(|i| i.opcode == 16 && i.arg_i32(0) == n) {
                self.ip = at + 1;
                self.time = self.file.script[at].time;
                self.stopped = false;
            }
        }
        let mut budget = 10_000;
        while !self.stopped && budget > 0 {
            budget -= 1;
            let Some(ins) = self.file.script.get(self.ip).cloned() else {
                self.stopped = true;
                break;
            };
            if ins.time > self.time {
                break;
            }
            self.ip += 1;
            self.exec(&ins, world, lib);
        }
    }

    fn exec(&mut self, ins: &Instr, world: &mut World, lib: &AnmLibrary) {
        let i = |n| ins.arg_i32(n);
        let f = |n| ins.arg_f32(n);
        let v3 = |k: usize| [ins.arg_f32(k), ins.arg_f32(k + 1), ins.arg_f32(k + 2)];
        match ins.opcode {
            0 => self.stopped = true,
            1 => {
                if let Some(at) = self.file.script.iter().position(|x| x.offset == i(0) as u32) {
                    self.ip = at;
                    self.time = i(1);
                }
            }
            2 => self.pos = v3(0),
            3 => self.i_pos.start(self.pos, v3(2), i(0), i(1)),
            4 => self.facing = v3(0),
            5 => self.i_facing.start(self.facing, v3(2), i(0), i(1)),
            6 => self.up = v3(0),
            7 => self.fov = f(0),
            8 => {
                self.fog_color = argb(i(0));
                self.fog_range = [f(1), f(2)];
            }
            9 => {
                let [r, g, b, a] = argb(i(2));
                let [cr, cg, cb, ca] = self.fog_color;
                let cur = [cr, cg, cb, ca, self.fog_range[0], self.fog_range[1]];
                self.i_fog.start(cur, [r, g, b, a, f(3), f(4)], i(0), i(1));
            }
            10 => self.i_pos.start_bezier(self.pos, v3(2), v3(5), v3(8), i(0)),
            11 => self.i_facing.start_bezier(self.facing, v3(2), v3(5), v3(8), i(0)),
            12 => self.set_rock(i(0) as u8),
            13 => {
                let c = argb(i(0));
                self.clear_color = [c[0], c[1], c[2]];
            }
            14 => {
                // sprite(slot, script, layer). Script -1 removes the slot's sprite, -2 hides it.
                // APPROX: every slot is drawn over the 3D scene; how `layer` interleaves with
                // stage geometry is not modelled yet.
                let Some(cell) = usize::try_from(i(0)).ok().and_then(|s| self.slots.get_mut(s)) else {
                    return;
                };
                match i(1) {
                    -1 => {
                        if let Some((old, _)) = cell.take() {
                            world.delete(old);
                        }
                    }
                    -2 => {
                        if let Some(vm) = cell.and_then(|(id, _)| world.get_mut(id)) {
                            vm.visible = false;
                        }
                    }
                    s if s >= 0 => {
                        if let Some((old, _)) = cell.take() {
                            world.delete(old);
                        }
                        let id = world.spawn_backdrop(lib, self.anm_file, s as usize, self.backdrop_origin);
                        *cell = Some((id, i(2)));
                    }
                    _ => {}
                }
            }
            16 => {} // interrupt label: no-op when reached normally
            18 => self.i_up.start(self.up, v3(2), i(0), i(1)),
            // ANM interrupt n+7 on every stage sprite, sprite slots included.
            19 => {
                world.interrupt_3d(i(0) + 7);
                for (id, _) in self.slots.iter().flatten() {
                    world.interrupt(*id, i(0) + 7);
                }
            }
            20 => self.draw_distance_sq = f(0) * f(0),
            // fovTime(t, mode, fov)
            21 => self.i_fov.start([self.fov], [f(2)], i(0), i(1)),
            // 17: screen-edge distortion.
            op => *self.unknown_ops.entry(op).or_default() += 1,
        }
    }

    fn set_rock(&mut self, mode: u8) {
        self.rock.mode = mode;
        match mode {
            0 => {
                self.rock.eye = [0.0; 3];
                self.rock.t = 0.0;
            }
            // Mode 6 fades an existing sway out: only its secondary timer restarts.
            6 => self.rock.t2 = 0.0,
            // Mode 2 starts half-way through its ramp.
            2 => self.rock.t = 512.0,
            _ => self.rock.t = 0.0,
        }
    }

    /// Camera sway, evaluated once per frame after the script runs.
    fn update_rock(&mut self) {
        let r = &mut self.rock;
        let t = r.t;
        let ti = t as i32;
        let cyc = |period: i32| (ti.rem_euclid(period)) as f32;
        // (frames at which the timer loops, how far back it jumps)
        // APPROX: the loop point is taken from the game; the value it resets to is chosen
        // so the motion stays continuous.
        let (limit, rewind) = match r.mode {
            1 => {
                let amp = if ti < 512 { t / 512.0 } else { 1.0 };
                let a = wrap(t * TAU / 1024.0);
                r.eye[0] = a.sin() * -10.0 * amp;
                r.eye[2] = wrap(a * 2.0).sin() * -10.0 * amp;
                self.up[0] = -a.sin() * 0.01 * amp;
                r.facing[0] = -r.eye[0] / 2.0;
                r.facing[2] = -r.eye[2] / 2.0;
                (2048, 1024)
            }
            2 | 4 => {
                let a = wrap(t * TAU / 3072.0);
                self.up[0] = -a.sin();
                self.up[2] = if r.mode == 2 { a.cos() } else { -a.cos() };
                (3072, 3072)
            }
            3 => {
                let a = wrap(cyc(1024) * TAU / 1024.0);
                let amp = if ti < 1024 { t * 80.0 / 1024.0 } else { 80.0 };
                r.eye = [a.sin() * amp, 0.0, 0.0];
                r.facing[0] = -(a.sin() * amp) / 10.0;
                (2048, 1024)
            }
            5 => {
                let a = wrap(cyc(1024) * TAU / 1024.0);
                let amp = if ti < 512 { t / 512.0 } else { 1.0 };
                r.eye[2] = a.sin() * -50.0 * amp;
                (2048, 1024)
            }
            6 => {
                let a = wrap(cyc(1024) * TAU / 1024.0);
                let amp = if r.t2 < 512.0 { (1.0 - r.t2 / 512.0) * 80.0 } else { 0.0 };
                r.eye = [a.sin() * amp, 0.0, 0.0];
                r.facing[0] = -(a.sin() * amp) / 10.0;
                r.t2 += 1.0;
                (2048, 1024)
            }
            9 => {
                let a = t * TAU / 2048.0 - PI;
                r.eye[0] = a.sin() * 70.0;
                r.eye[2] = wrap(a * 2.0).sin() * 200.0;
                self.up[0] = -a.sin() * 0.1;
                (2048, 2048)
            }
            10 => {
                let a = t * TAU / 1024.0 - PI;
                r.eye[0] = a.sin() * -50.0;
                self.up[0] = -a.sin() * 0.1;
                (1024, 1024)
            }
            11 => {
                let a = t * TAU / 512.0 - PI;
                r.eye[0] = a.sin() * -15.0;
                self.up[0] = -a.sin() * 0.01;
                (512, 512)
            }
            _ => return,
        };
        r.t += 1.0;
        if r.t as i32 >= limit {
            r.t -= rewind as f32;
        }
    }

    /// Camera for drawing the stage into `viewport` (canvas rectangle `[x, y, w, h]`).
    pub fn camera(&self, viewport: [f32; 4]) -> Camera {
        let eye = [self.pos[0] + self.rock.eye[0], self.pos[1] + self.rock.eye[1], self.pos[2] + self.rock.eye[2]];
        let mut dir = [
            self.facing[0] + self.rock.facing[0],
            self.facing[1] + self.rock.facing[1],
            self.facing[2] + self.rock.facing[2],
        ];
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if len >= 0.01 {
            dir = dir.map(|v| v / len);
        }
        let mut cam = Camera::new(eye, dir, self.up, self.fov, viewport, (self.fog_range[0], self.fog_range[1]));
        cam.fog_color = [self.fog_color[0] / 255.0, self.fog_color[1] / 255.0, self.fog_color[2] / 255.0];
        cam
    }

    /// Remove this stage's VMs from the world.
    pub fn despawn(&mut self, world: &mut World) {
        let slots = self.slots.iter_mut().filter_map(Option::take).map(|(id, _)| id);
        for id in self.vms.drain(..).chain(slots) {
            world.delete(id);
        }
    }
}
