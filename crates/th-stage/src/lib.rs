//! STD stage background player: runs the camera/fog script and spawns the
//! stage's ANM quads as 3D VMs.
//!
//! Instruction meanings follow the community STD reference (see README credits);
//! opcodes whose effect is not modelled yet are counted in [`Stage::unknown_ops`].

use std::collections::HashMap;

use th_anm::interp::Interp;
use th_anm::{AnmLibrary, Camera, VmId, World};
use th_formats::stage::{Instr, StdFile};

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

    i_pos: Interp<3>,
    i_facing: Interp<3>,
    i_up: Interp<3>,
    i_fog: Interp<6>,

    vms: Vec<VmId>,
    /// STD sprite slots (`ins_14`): 2D backdrop sprites, at most 8 at a time.
    slots: [Option<VmId>; 8],
    /// Canvas position backdrop sprites are placed relative to (centre of the stage view).
    pub backdrop_origin: [f32; 2],
    pub unknown_ops: HashMap<u16, u32>,
}

/// `0xAARRGGBB` -> `[r, g, b, a]`.
fn argb(c: i32) -> [f32; 4] {
    let c = c as u32;
    [((c >> 16) & 0xff) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32, (c >> 24) as f32]
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
        // APPROX: matches the intended layering; the exact order (or a depth buffer) in
        // th19.exe still needs confirming.
        let mut vms = Vec::new();
        for obj in &file.objects {
            for inst in file.instances.iter().filter(|i| i.object == obj.id) {
                for q in &obj.quads {
                if only.as_ref().is_some_and(|o| !o.contains(&q.script)) {
                    continue;
                }
                    let p = [inst.pos[0] + q.pos[0], inst.pos[1] + q.pos[1], inst.pos[2] + q.pos[2]];
                    // The quad's size overrides the sprite's (e.g. a 512x256 mist sprite tiled at 1024x512).
                    let size = (q.size[0] > 0.0 && q.size[1] > 0.0).then_some(q.size);
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
            pos: [0.0; 3],
            facing: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, -1.0],
            fov: std::f32::consts::FRAC_PI_4,
            fog_color: [0.0, 0.0, 0.0, 255.0],
            fog_range: [0.0, 0.0],
            clear_color: [0.0; 3],
            i_pos: Interp::default(),
            i_facing: Interp::default(),
            i_up: Interp::default(),
            i_fog: Interp::default(),
            vms,
            slots: [None; 8],
            // APPROX: the real anchor is the playfield's stage viewport; confirm with th19.exe.
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
        if let Some(v) = self.i_pos.step() {
            self.pos = v;
        }
        if let Some(v) = self.i_facing.step() {
            self.facing = v;
        }
        if let Some(v) = self.i_up.step() {
            self.up = v;
        }
        if let Some([r, g, b, a, near, far]) = self.i_fog.step() {
            self.fog_color = [r, g, b, a];
            self.fog_range = [near, far];
        }
        self.time += 1;
        self.run(world, lib);
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
            13 => {
                let c = argb(i(0));
                self.clear_color = [c[0], c[1], c[2]];
            }
            14 => {
                // sprite(slot, script, layer): replace the 2D backdrop sprite in `slot`.
                // APPROX: every slot is drawn behind the 3D scene; how the layer argument
                // interleaves with stage geometry is not modelled yet.
                if let Some(cell) = usize::try_from(i(0)).ok().and_then(|s| self.slots.get_mut(s)) {
                    if let Some(old) = cell.take() {
                        world.delete(old);
                    }
                    if i(1) >= 0 {
                        *cell = Some(world.spawn_backdrop(lib, self.anm_file, i(1) as usize, self.backdrop_origin));
                    }
                }
            }
            16 => {} // interrupt label: no-op when reached normally
            18 => self.i_up.start(self.up, v3(2), i(0), i(1)),
            19 => world.interrupt_3d(i(0) + 7),
            // 12 rock (camera shake), 17 distortion, 20 draw distance, 21 TH19-only.
            op => *self.unknown_ops.entry(op).or_default() += 1,
        }
    }

    /// Camera for drawing the stage into `viewport` (canvas rectangle `[x, y, w, h]`).
    pub fn camera(&self, viewport: [f32; 4]) -> Camera {
        Camera::new(self.pos, self.facing, self.up, self.fov, viewport, (self.fog_range[0], self.fog_range[1]))
    }

    /// Remove this stage's VMs from the world.
    pub fn despawn(&mut self, world: &mut World) {
        for id in self.vms.drain(..).chain(self.slots.iter_mut().filter_map(Option::take)) {
            world.delete(id);
        }
    }
}
