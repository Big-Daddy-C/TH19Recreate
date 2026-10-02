//! ANM VM instances and the world that owns them.
//!
//! Opcode numbering is the v8 (TH13+) instruction set as documented by the
//! community (thtk's format tables and ExpHP's thpages). Behaviour that is not yet
//! confirmed against th19.exe is marked `APPROX` so it can be audited later.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use th_formats::anm::Instr;

use crate::interp::Interp;
use crate::AnmLibrary;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VmId {
    index: u32,
    generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Subtract,
    Replace,
    Screen,
    Multiply,
    Weird6,
    Behind,
    Min,
    Max,
}

impl BlendMode {
    fn from_id(id: i32) -> Self {
        use BlendMode::*;
        [Normal, Add, Subtract, Replace, Screen, Multiply, Weird6, Behind, Min, Max]
            .get(id as usize)
            .copied()
            .unwrap_or(Normal)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// Consecutive triangles sharing a texture and blend mode.
#[derive(Debug)]
pub struct DrawBatch {
    /// `None` = untextured (solid colour primitives).
    pub texture: Option<u32>,
    pub blend: BlendMode,
    pub layer: i32,
    pub vertices: Vec<Vertex>,
}

pub type DrawList = Vec<DrawBatch>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunState {
    Running,
    /// `stop`: script halted, interpolation and interrupts still work.
    Stopped,
    /// `static`: frozen entirely.
    Static,
}

#[derive(Clone, Copy, Debug)]
enum Primitive {
    Rect { w: f32, h: f32, rotated: bool, gradient: bool },
    RectBorder { w: f32, h: f32 },
    Poly { radius: f32, segments: i32, border: bool },
    Ring { radius: f32, thickness: f32, segments: i32 },
}

#[derive(Clone, Debug)]
pub struct Vm {
    pub file: usize,
    pub script: usize,
    ip: usize,
    pub time: i32,
    wait: i32,
    state: RunState,
    pending_interrupt: Option<i32>,
    return_point: Option<(usize, i32)>,
    order: u64,

    pub ivars: [i32; 6],
    pub fvars: [f32; 4],

    pub pos: [f32; 3],
    pub rot: [f32; 3],
    angle_vel: [f32; 3],
    pub scale: [f32; 2],
    scale2: [f32; 2],
    scale_growth: [f32; 2],
    pub color: [f32; 3],
    color2: [f32; 3],
    pub alpha: f32,
    alpha2: f32,
    color_mode: i32,

    pub sprite: i32,
    pub blend: BlendMode,
    pub render_mode: i32,
    pub layer: i32,
    pub visible: bool,
    anchor: [i16; 2],
    anchor_offset: [f32; 2],
    uv_scroll: [f32; 2],
    uv_vel: [f32; 2],
    uv_scale: [f32; 2],
    primitive: Option<Primitive>,

    i_pos: Interp<3>,
    i_rot: Interp<3>,
    i_scale: Interp<2>,
    i_scale2: Interp<2>,
    i_color: Interp<3>,
    i_color2: Interp<3>,
    i_alpha: Interp<1>,
    i_alpha2: Interp<1>,
    i_uv_vel_x: Interp<1>,
    i_uv_vel_y: Interp<1>,
    i_uv_scale: Interp<2>,

    pub parent: Option<VmId>,
    /// Extra offset applied on top of `pos` (spawn position for roots, offset for scriptNewPos).
    pub origin: [f32; 2],
}

impl Vm {
    fn new(file: usize, script: usize, order: u64) -> Self {
        Self {
            file,
            script,
            ip: 0,
            time: 0,
            wait: 0,
            state: RunState::Running,
            pending_interrupt: None,
            return_point: None,
            order,
            ivars: [0; 6],
            fvars: [0.0; 4],
            pos: [0.0; 3],
            rot: [0.0; 3],
            angle_vel: [0.0; 3],
            scale: [1.0; 2],
            scale2: [1.0; 2],
            scale_growth: [0.0; 2],
            color: [255.0; 3],
            color2: [255.0; 3],
            alpha: 255.0,
            alpha2: 255.0,
            color_mode: 0,
            sprite: -1,
            blend: BlendMode::Normal,
            render_mode: 0,
            layer: 0,
            visible: true,
            anchor: [0, 0],
            anchor_offset: [0.0; 2],
            uv_scroll: [0.0; 2],
            uv_vel: [0.0; 2],
            uv_scale: [1.0; 2],
            primitive: None,
            i_pos: Interp::default(),
            i_rot: Interp::default(),
            i_scale: Interp::default(),
            i_scale2: Interp::default(),
            i_color: Interp::default(),
            i_color2: Interp::default(),
            i_alpha: Interp::default(),
            i_alpha2: Interp::default(),
            i_uv_vel_x: Interp::default(),
            i_uv_vel_y: Interp::default(),
            i_uv_scale: Interp::default(),
            parent: None,
            origin: [0.0; 2],
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.state != RunState::Running
    }
}

struct Slot {
    generation: u32,
    vm: Option<Vm>,
}

enum Flow {
    Next,
    Jump { offset: u32, time: i32 },
    Yield,
    Delete,
}

enum Spawn {
    Child { script: i32, offset: [f32; 2] },
    Root { script: i32, offset: [f32; 2] },
}

/// Owns every live ANM VM.
pub struct World {
    slots: Vec<Slot>,
    free: Vec<u32>,
    next_order: u64,
    rng: u32,
    /// Opcodes encountered that this implementation does not handle yet, with hit counts.
    pub unknown_ops: HashMap<u16, u32>,
}

impl Default for World {
    fn default() -> Self {
        Self::new(0x1234_5678)
    }
}

impl World {
    pub fn new(seed: u32) -> Self {
        Self { slots: Vec::new(), free: Vec::new(), next_order: 0, rng: seed.max(1), unknown_ops: HashMap::new() }
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.vm.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: VmId) -> Option<&Vm> {
        let s = self.slots.get(id.index as usize)?;
        (s.generation == id.generation).then_some(s.vm.as_ref()).flatten()
    }

    pub fn get_mut(&mut self, id: VmId) -> Option<&mut Vm> {
        let s = self.slots.get_mut(id.index as usize)?;
        (s.generation == id.generation).then_some(s.vm.as_mut()).flatten()
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.free.clear();
    }

    pub fn delete(&mut self, id: VmId) {
        if let Some(s) = self.slots.get_mut(id.index as usize) {
            if s.generation == id.generation && s.vm.take().is_some() {
                s.generation = s.generation.wrapping_add(1);
                self.free.push(id.index);
            }
        }
    }

    pub fn interrupt(&mut self, id: VmId, n: i32) {
        if let Some(vm) = self.get_mut(id) {
            vm.pending_interrupt = Some(n);
        }
    }

    pub fn interrupt_all(&mut self, n: i32) {
        for vm in self.slots.iter_mut().filter_map(|s| s.vm.as_mut()) {
            vm.pending_interrupt = Some(n);
        }
    }

    fn insert(&mut self, vm: Vm) -> VmId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.vm = Some(vm);
            VmId { index, generation: slot.generation }
        } else {
            self.slots.push(Slot { generation: 0, vm: Some(vm) });
            VmId { index: self.slots.len() as u32 - 1, generation: 0 }
        }
    }

    /// Create a root VM running `script` of `file` at `pos`, executing its first instructions immediately.
    pub fn spawn(&mut self, lib: &AnmLibrary, file: usize, script: usize, pos: [f32; 2]) -> VmId {
        self.spawn_inner(lib, file, script, None, pos)
    }

    fn spawn_inner(
        &mut self,
        lib: &AnmLibrary,
        file: usize,
        script: usize,
        parent: Option<VmId>,
        origin: [f32; 2],
    ) -> VmId {
        let mut vm = Vm::new(file, script, self.next_order);
        self.next_order += 1;
        vm.parent = parent;
        vm.origin = origin;
        let id = self.insert(vm);
        self.run(lib, id);
        id
    }

    /// Advance every VM by one frame (1/60 s).
    pub fn tick(&mut self, lib: &AnmLibrary) {
        let ids: Vec<VmId> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.vm.is_some())
            .map(|(i, s)| VmId { index: i as u32, generation: s.generation })
            .collect();
        for id in ids {
            let Some(vm) = self.get_mut(id) else { continue };
            if vm.state != RunState::Static {
                update_motion(vm);
                vm.time += 1;
                if vm.wait > 0 {
                    vm.wait -= 1;
                }
            }
            self.run(lib, id);
        }
        // Children of deleted parents go with them.
        let dead: Vec<VmId> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let p = s.vm.as_ref()?.parent?;
                self.get(p).is_none().then_some(VmId { index: i as u32, generation: s.generation })
            })
            .collect();
        for id in dead {
            self.delete(id);
        }
    }

    fn run(&mut self, lib: &AnmLibrary, id: VmId) {
        let mut spawns = Vec::new();
        let delete = {
            let rng = &mut self.rng;
            let unknown = &mut self.unknown_ops;
            let Some(vm) = self.slots[id.index as usize].vm.as_mut() else { return };
            let Some(script) = lib.files.get(vm.file).and_then(|f| f.script(vm.script)) else {
                return;
            };

            if let Some(n) = vm.pending_interrupt.take() {
                if vm.state != RunState::Static {
                    let label = |n: i32| script.instrs.iter().position(|i| i.opcode == 5 && i.arg_i32(0) == n);
                    if let Some(at) = label(n).or_else(|| label(-1)) {
                        vm.return_point = Some((vm.ip, vm.time));
                        vm.ip = at + 1;
                        vm.time = script.instrs[at].time as i32;
                        vm.wait = 0;
                        vm.state = RunState::Running;
                    }
                }
            }

            let mut delete = false;
            let mut budget = 10_000;
            while vm.state == RunState::Running && vm.wait == 0 && budget > 0 {
                budget -= 1;
                let Some(ins) = script.instrs.get(vm.ip) else {
                    vm.state = RunState::Stopped;
                    break;
                };
                if ins.time as i32 > vm.time {
                    break;
                }
                vm.ip += 1;
                match exec(vm, ins, rng, unknown, &mut spawns) {
                    Flow::Next => {}
                    Flow::Yield => break,
                    Flow::Delete => {
                        delete = true;
                        break;
                    }
                    Flow::Jump { offset, time } => match script.index_of_offset(offset) {
                        Some(i) => {
                            vm.ip = i;
                            vm.time = time;
                        }
                        None => vm.state = RunState::Stopped,
                    },
                }
            }
            delete
        };

        let (file, base) = {
            let vm = self.slots[id.index as usize].vm.as_ref().unwrap();
            (vm.file, world_pos(self, vm))
        };
        for s in spawns {
            match s {
                Spawn::Child { script, offset } if script >= 0 => {
                    self.spawn_inner(lib, file, script as usize, Some(id), offset);
                }
                Spawn::Root { script, offset } if script >= 0 => {
                    let o = [base[0] + offset[0], base[1] + offset[1]];
                    self.spawn_inner(lib, file, script as usize, None, o);
                }
                _ => {}
            }
        }
        if delete {
            self.delete(id);
        }
    }

    /// Build triangles for every visible VM, sorted by layer then creation order.
    pub fn draw_list(&self, lib: &AnmLibrary) -> DrawList {
        let mut vms: Vec<&Vm> = self.slots.iter().filter_map(|s| s.vm.as_ref()).collect();
        vms.sort_by_key(|v| (v.layer, v.order));

        let mut out: DrawList = Vec::new();
        let mut verts = Vec::new();
        for vm in vms {
            if !vm.visible || vm.alpha <= 0.0 && vm.color_mode == 0 {
                continue;
            }
            verts.clear();
            let texture = emit(self, lib, vm, &mut verts);
            if verts.is_empty() {
                continue;
            }
            match out.last_mut() {
                Some(b) if b.texture == texture && b.blend == vm.blend && b.layer == vm.layer => {
                    b.vertices.extend_from_slice(&verts)
                }
                _ => out.push(DrawBatch { texture, blend: vm.blend, layer: vm.layer, vertices: verts.clone() }),
            }
        }
        out
    }
}

fn next_rand(rng: &mut u32) -> u32 {
    // APPROX: xorshift32; the real game uses its own replay-deterministic generator.
    let mut x = *rng;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *rng = x;
    x
}

fn randf01(rng: &mut u32) -> f32 {
    (next_rand(rng) >> 8) as f32 / (1u32 << 24) as f32
}

fn world_pos(world: &World, vm: &Vm) -> [f32; 2] {
    let mut p = [vm.pos[0] + vm.origin[0], vm.pos[1] + vm.origin[1]];
    let mut parent = vm.parent;
    let mut depth = 0;
    while let (Some(pid), true) = (parent, depth < 32) {
        let Some(pv) = world.get(pid) else { break };
        p[0] += pv.pos[0] + pv.origin[0];
        p[1] += pv.pos[1] + pv.origin[1];
        parent = pv.parent;
        depth += 1;
    }
    p
}

fn update_motion(vm: &mut Vm) {
    if let Some(v) = vm.i_pos.step() {
        vm.pos = v;
    }
    if let Some(v) = vm.i_rot.step() {
        vm.rot = v;
    }
    if let Some(v) = vm.i_scale.step() {
        vm.scale = v;
    }
    if let Some(v) = vm.i_scale2.step() {
        vm.scale2 = v;
    }
    if let Some(v) = vm.i_color.step() {
        vm.color = v;
    }
    if let Some(v) = vm.i_color2.step() {
        vm.color2 = v;
    }
    if let Some([a]) = vm.i_alpha.step() {
        vm.alpha = a;
    }
    if let Some([a]) = vm.i_alpha2.step() {
        vm.alpha2 = a;
    }
    if let Some([u]) = vm.i_uv_vel_x.step() {
        vm.uv_vel[0] = u;
    }
    if let Some([v]) = vm.i_uv_vel_y.step() {
        vm.uv_vel[1] = v;
    }
    if let Some(v) = vm.i_uv_scale.step() {
        vm.uv_scale = v;
    }
    for i in 0..3 {
        vm.rot[i] += vm.angle_vel[i];
    }
    vm.scale[0] += vm.scale_growth[0];
    vm.scale[1] += vm.scale_growth[1];
    vm.uv_scroll[0] += vm.uv_vel[0];
    vm.uv_scroll[1] += vm.uv_vel[1];
}

// ---------------------------------------------------------------------------
// Variables

fn read_var(vm: &Vm, id: i32, rng: &mut u32) -> f32 {
    use std::f32::consts::PI;
    match id {
        10000..=10003 => vm.ivars[(id - 10000) as usize] as f32,
        10004..=10007 => vm.fvars[(id - 10004) as usize],
        10008 | 10009 => vm.ivars[(id - 10004) as usize] as f32,
        10010 | 10030 => randf01(rng) * 2.0 * PI - PI,
        10011 | 10031 => randf01(rng),
        10012 | 10032 => randf01(rng) * 2.0 - 1.0,
        10013..=10015 => vm.pos[(id - 10013) as usize],
        10022 => next_rand(rng) as i32 as f32,
        10023..=10025 => vm.rot[(id - 10023) as usize],
        10026 => vm.rot[2],
        10027 => 1.0,
        10028 => PI,
        10029 => 1.0,
        _ => 0.0,
    }
}

fn read_ivar(vm: &Vm, id: i32, rng: &mut u32) -> i32 {
    match id {
        10000..=10003 => vm.ivars[(id - 10000) as usize],
        10008 | 10009 => vm.ivars[(id - 10004) as usize],
        10022 => next_rand(rng) as i32,
        _ => read_var(vm, id, rng) as i32,
    }
}

fn write_ivar(vm: &mut Vm, id: i32, v: i32) {
    match id {
        10000..=10003 => vm.ivars[(id - 10000) as usize] = v,
        10008 | 10009 => vm.ivars[(id - 10004) as usize] = v,
        _ => write_fvar(vm, id, v as f32),
    }
}

fn write_fvar(vm: &mut Vm, id: i32, v: f32) {
    match id {
        10004..=10007 => vm.fvars[(id - 10004) as usize] = v,
        10000..=10003 | 10008 | 10009 => write_ivar(vm, id, v as i32),
        10013..=10015 => vm.pos[(id - 10013) as usize] = v,
        10023..=10025 => vm.rot[(id - 10023) as usize] = v,
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Execution

struct Args<'a> {
    ins: &'a Instr,
}

impl Args<'_> {
    fn i(&self, vm: &Vm, n: usize, rng: &mut u32) -> i32 {
        let raw = self.ins.arg_i32(n);
        if self.ins.is_var(n) {
            read_ivar(vm, raw, rng)
        } else {
            raw
        }
    }
    fn f(&self, vm: &Vm, n: usize, rng: &mut u32) -> f32 {
        let raw = self.ins.arg_f32(n);
        if self.ins.is_var(n) {
            read_var(vm, raw as i32, rng)
        } else {
            raw
        }
    }
    /// Destination variable id for integer writes.
    fn dst_i(&self, n: usize) -> i32 {
        self.ins.arg_i32(n)
    }
    /// Destination variable id for float writes (stored as a float, e.g. `10004.0`).
    fn dst_f(&self, n: usize) -> i32 {
        self.ins.arg_f32(n) as i32
    }
}

fn exec(
    vm: &mut Vm,
    ins: &Instr,
    rng: &mut u32,
    unknown: &mut HashMap<u16, u32>,
    spawns: &mut Vec<Spawn>,
) -> Flow {
    let a = Args { ins };
    macro_rules! i {
        ($n:expr) => {
            a.i(vm, $n, rng)
        };
    }
    macro_rules! f {
        ($n:expr) => {
            a.f(vm, $n, rng)
        };
    }

    match ins.opcode {
        // --- control ---
        0 => {}
        1 => return Flow::Delete,
        2 => {
            vm.state = RunState::Static;
            return Flow::Yield;
        }
        3 => {
            vm.state = RunState::Stopped;
            return Flow::Yield;
        }
        4 => {
            vm.state = RunState::Stopped;
            vm.visible = false;
            return Flow::Yield;
        }
        5 => {} // interrupt label: no-op when reached normally
        6 => {
            vm.wait = i!(0).max(0);
            if vm.wait > 0 {
                return Flow::Yield;
            }
        }
        7 => {
            if let Some((ip, time)) = vm.return_point.take() {
                vm.ip = ip;
                vm.time = time;
            }
        }

        // --- arithmetic: 100..=121, even = int, odd = float ---
        100..=111 => {
            let op = (ins.opcode - 100) / 2;
            if ins.opcode % 2 == 0 {
                let (d, b) = (a.dst_i(0), i!(1));
                let cur = read_ivar(vm, d, rng);
                let v = match op {
                    0 => b,
                    1 => cur.wrapping_add(b),
                    2 => cur.wrapping_sub(b),
                    3 => cur.wrapping_mul(b),
                    4 => cur.checked_div(b).unwrap_or(0),
                    _ => cur.checked_rem(b).unwrap_or(0),
                };
                write_ivar(vm, d, v);
            } else {
                let (d, b) = (a.dst_f(0), f!(1));
                let cur = read_var(vm, d, rng);
                let v = match op {
                    0 => b,
                    1 => cur + b,
                    2 => cur - b,
                    3 => cur * b,
                    4 => cur / b,
                    _ => cur % b,
                };
                write_fvar(vm, d, v);
            }
        }
        112..=121 => {
            let op = (ins.opcode - 112) / 2;
            if ins.opcode % 2 == 0 {
                let (d, b, c) = (a.dst_i(0), i!(1), i!(2));
                let v = match op {
                    0 => b.wrapping_add(c),
                    1 => b.wrapping_sub(c),
                    2 => b.wrapping_mul(c),
                    3 => b.checked_div(c).unwrap_or(0),
                    _ => b.checked_rem(c).unwrap_or(0),
                };
                write_ivar(vm, d, v);
            } else {
                let (d, b, c) = (a.dst_f(0), f!(1), f!(2));
                let v = match op {
                    0 => b + c,
                    1 => b - c,
                    2 => b * c,
                    3 => b / c,
                    _ => b % c,
                };
                write_fvar(vm, d, v);
            }
        }
        122 => {
            let n = i!(1);
            let v = if n != 0 { (next_rand(rng) % n.unsigned_abs()) as i32 } else { 0 };
            write_ivar(vm, a.dst_i(0), v);
        }
        123 => {
            let r = f!(1);
            write_fvar(vm, a.dst_f(0), randf01(rng) * r);
        }
        124..=128 => {
            let x = f!(1);
            let v = match ins.opcode {
                124 => x.sin(),
                125 => x.cos(),
                126 => x.tan(),
                127 => x.acos(),
                _ => x.atan(),
            };
            write_fvar(vm, a.dst_f(0), v);
        }
        129 => {
            use std::f32::consts::{PI, TAU};
            let d = a.dst_f(0);
            let mut v = read_var(vm, d, rng);
            while v > PI {
                v -= TAU;
            }
            while v < -PI {
                v += TAU;
            }
            write_fvar(vm, d, v);
        }
        130 => {
            let (ang, dist) = (f!(2), f!(3));
            write_fvar(vm, a.dst_f(0), ang.cos() * dist);
            write_fvar(vm, a.dst_f(1), ang.sin() * dist);
        }
        131 => {
            use std::f32::consts::PI;
            let (min, max) = (f!(2), f!(3));
            let r = randf01(rng) * (max - min) + min;
            let ang = randf01(rng) * 2.0 * PI - PI;
            write_fvar(vm, a.dst_f(0), ang.cos() * r);
            write_fvar(vm, a.dst_f(1), ang.sin() * r);
        }

        // --- jumps ---
        200 => return Flow::Jump { offset: ins.arg_i32(0) as u32, time: ins.arg_i32(1) },
        201 => {
            let d = a.dst_i(0);
            let v = read_ivar(vm, d, rng) - 1;
            write_ivar(vm, d, v);
            if v > 0 {
                return Flow::Jump { offset: ins.arg_i32(1) as u32, time: ins.arg_i32(2) };
            }
        }
        202..=213 => {
            let take = if ins.opcode % 2 == 0 {
                let (x, y) = (i!(0), i!(1));
                cmp(ins.opcode, x.cmp(&y))
            } else {
                let (x, y) = (f!(0), f!(1));
                x.partial_cmp(&y).is_some_and(|o| cmp(ins.opcode, o))
            };
            if take {
                return Flow::Jump { offset: ins.arg_i32(2) as u32, time: ins.arg_i32(3) };
            }
        }

        // --- render state ---
        300 => {
            vm.sprite = i!(0);
            vm.primitive = None;
        }
        301 => {
            let (base, n) = (i!(0), i!(1));
            let r = if n > 0 { (next_rand(rng) % n as u32) as i32 } else { 0 };
            vm.sprite = base + r;
            vm.primitive = None;
        }
        302 => vm.render_mode = i!(0),
        303 => vm.blend = BlendMode::from_id(i!(0)),
        304 => vm.layer = i!(0),
        305 | 306 | 307 | 311 | 312 | 313 | 314 | 315 | 316 | 317 | 318 | 319 => {} // no visual effect modelled yet
        308 => vm.scale[0] = -vm.scale[0],
        309 => vm.scale[1] = -vm.scale[1],
        310 => vm.visible = i!(0) != 0,

        // --- transform / colour ---
        400 => vm.pos = [f!(0), f!(1), f!(2)],
        401 => vm.rot = [f!(0), f!(1), f!(2)],
        402 => vm.scale = [f!(0), f!(1)],
        403 => vm.alpha = i!(0) as f32,
        404 => vm.color = [i!(0) as f32, i!(1) as f32, i!(2) as f32],
        405 => vm.alpha2 = i!(0) as f32,
        406 => vm.color2 = [i!(0) as f32, i!(1) as f32, i!(2) as f32],
        407 => {
            let g = [f!(2), f!(3), f!(4)];
            vm.i_pos.start(vm.pos, g, i!(0), i!(1));
        }
        408 => {
            let g = [i!(2) as f32, i!(3) as f32, i!(4) as f32];
            vm.i_color.start(vm.color, g, i!(0), i!(1));
        }
        409 => {
            let g = [i!(2) as f32];
            vm.i_alpha.start([vm.alpha], g, i!(0), i!(1));
        }
        410 => {
            let g = [f!(2), f!(3), f!(4)];
            vm.i_rot.start(vm.rot, g, i!(0), i!(1));
        }
        411 => {
            let g = [vm.rot[0], vm.rot[1], f!(2)];
            vm.i_rot.start(vm.rot, g, i!(0), i!(1));
        }
        412 => {
            let g = [f!(2), f!(3)];
            vm.i_scale.start(vm.scale, g, i!(0), i!(1));
        }
        413 => {
            let g = [i!(2) as f32, i!(3) as f32, i!(4) as f32];
            vm.i_color2.start(vm.color2, g, i!(0), i!(1));
            vm.color_mode = 1;
        }
        414 => {
            let g = [i!(2) as f32];
            vm.i_alpha2.start([vm.alpha2], g, i!(0), i!(1));
            vm.color_mode = 1;
        }
        415 => vm.angle_vel = [f!(0), f!(1), f!(2)],
        416 => vm.scale_growth = [f!(0), f!(1)],
        417 => {
            // alphaTimeLinear(alpha, t)
            let g = [i!(0) as f32];
            vm.i_alpha.start([vm.alpha], g, i!(1), 0);
        }
        420 => {
            let t = i!(0);
            let mut p = |k: usize| [a.f(vm, k, rng), a.f(vm, k + 1, rng), a.f(vm, k + 2, rng)];
            let (p1, p2, p3) = (p(1), p(4), p(7));
            vm.i_pos.start_bezier(vm.pos, p1, p2, p3, t);
        }
        421 => {
            let raw = ins.arg_i32(0);
            vm.anchor = [raw as i16, (raw >> 16) as i16];
        }
        423 => vm.color_mode = i!(0),
        424 => {} // rotateAuto: only meaningful for bullets; handled by the owner
        425 => vm.uv_vel[0] = f!(0),
        426 => vm.uv_vel[1] = f!(0),
        427 => {
            let g = [f!(2)];
            vm.i_uv_vel_x.start([vm.uv_vel[0]], g, i!(0), i!(1));
        }
        428 => {
            let g = [f!(2)];
            vm.i_uv_vel_y.start([vm.uv_vel[1]], g, i!(0), i!(1));
        }
        429 => vm.uv_scale = [f!(0), f!(1)],
        430 => {
            let g = [f!(2), f!(3)];
            vm.i_uv_scale.start(vm.uv_scale, g, i!(0), i!(1));
        }
        431 | 432 | 437 | 438 | 439 | 440 => {}
        434 => vm.scale2 = [f!(0), f!(1)],
        435 => {
            let g = [f!(2), f!(3)];
            vm.i_scale2.start(vm.scale2, g, i!(0), i!(1));
        }
        436 => vm.anchor_offset = [f!(0), f!(1)],

        // --- child VMs ---
        500..=503 => spawns.push(Spawn::Child { script: i!(0), offset: [0.0; 2] }),
        504 => spawns.push(Spawn::Root { script: i!(0), offset: [0.0; 2] }),
        505 | 510 => spawns.push(Spawn::Child { script: i!(0), offset: [f!(1), f!(2)] }),
        506 => spawns.push(Spawn::Root { script: i!(0), offset: [f!(1), f!(2)] }),
        507 | 509 => {}

        // --- special drawing ---
        603 => vm.primitive = Some(Primitive::Rect { w: f!(0), h: f!(1), rotated: false, gradient: false }),
        606 => vm.primitive = Some(Primitive::Rect { w: f!(0), h: f!(1), rotated: false, gradient: true }),
        607 => vm.primitive = Some(Primitive::Rect { w: f!(0), h: f!(1), rotated: true, gradient: false }),
        608 => vm.primitive = Some(Primitive::Rect { w: f!(0), h: f!(1), rotated: true, gradient: true }),
        604 => vm.primitive = Some(Primitive::Poly { radius: f!(0), segments: i!(1), border: false }),
        605 => vm.primitive = Some(Primitive::Poly { radius: f!(0), segments: i!(1), border: true }),
        611 => vm.primitive = Some(Primitive::Ring { radius: f!(0), thickness: f!(1), segments: i!(2) }),
        612 => vm.primitive = Some(Primitive::RectBorder { w: f!(0), h: f!(1) }),

        op => *unknown.entry(op).or_default() += 1,
    }
    Flow::Next
}

fn cmp(opcode: u16, o: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match (opcode - 202) / 2 {
        0 => o == Equal,
        1 => o != Equal,
        2 => o == Less,
        3 => o != Greater,
        4 => o == Greater,
        _ => o != Less,
    }
}

// ---------------------------------------------------------------------------
// Geometry

fn rgba(c: [f32; 3], a: f32) -> [f32; 4] {
    [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, (a / 255.0).clamp(0.0, 1.0)]
}

/// Emit triangles for one VM; returns the texture it needs.
fn emit(world: &World, lib: &AnmLibrary, vm: &Vm, out: &mut Vec<Vertex>) -> Option<u32> {
    let [cx, cy] = world_pos(world, vm);
    let c1 = rgba(vm.color, vm.alpha);
    let c2 = rgba(vm.color2, vm.alpha2);
    let rot_z = if matches!(vm.render_mode, 0 | 2) { 0.0 } else { vm.rot[2] };
    let (s, c) = rot_z.sin_cos();
    let xf = |x: f32, y: f32| [cx + x * c - y * s, cy + x * s + y * c];
    let solid = |out: &mut Vec<Vertex>, p: [f32; 2], col: [f32; 4]| out.push(Vertex { pos: p, uv: [0.0; 2], color: col });

    if let Some(prim) = vm.primitive {
        let sx = vm.scale[0] * vm.scale2[0];
        let sy = vm.scale[1] * vm.scale2[1];
        match prim {
            Primitive::Rect { w, h, rotated, gradient } => {
                let (hw, hh) = (w * sx * 0.5, h * sy * 0.5);
                let (l, r) = if gradient { (c1, c2) } else { (c1, c1) };
                let f = |x, y| if rotated { xf(x, y) } else { [cx + x, cy + y] };
                for (p, col) in [((-hw, -hh), l), ((hw, -hh), r), ((hw, hh), r), ((-hw, -hh), l), ((hw, hh), r), ((-hw, hh), l)] {
                    solid(out, f(p.0, p.1), col);
                }
            }
            Primitive::RectBorder { w, h } => {
                let (hw, hh, t) = (w * sx * 0.5, h * sy * 0.5, 1.0);
                for (x0, y0, x1, y1) in [(-hw, -hh, hw, -hh + t), (-hw, hh - t, hw, hh), (-hw, -hh, -hw + t, hh), (hw - t, -hh, hw, hh)] {
                    for p in [(x0, y0), (x1, y0), (x1, y1), (x0, y0), (x1, y1), (x0, y1)] {
                        solid(out, xf(p.0, p.1), c1);
                    }
                }
            }
            Primitive::Poly { radius, segments, border } => {
                let n = segments.clamp(3, 256);
                let r = radius * sx;
                let pt = |k: i32, rr: f32| {
                    let t = k as f32 / n as f32 * std::f32::consts::TAU;
                    xf(t.cos() * rr, t.sin() * rr)
                };
                for k in 0..n {
                    if border {
                        let (a0, a1, b0, b1) = (pt(k, r - 1.0), pt(k + 1, r - 1.0), pt(k, r), pt(k + 1, r));
                        for p in [a0, b0, b1, a0, b1, a1] {
                            solid(out, p, c1);
                        }
                    } else {
                        solid(out, [cx, cy], c1);
                        solid(out, pt(k, r), c2);
                        solid(out, pt(k + 1, r), c2);
                    }
                }
            }
            Primitive::Ring { radius, thickness, segments } => {
                let n = segments.clamp(3, 256);
                let (ri, ro) = ((radius - thickness * 0.5) * sx, (radius + thickness * 0.5) * sx);
                let pt = |k: i32, rr: f32| {
                    let t = k as f32 / n as f32 * std::f32::consts::TAU;
                    xf(t.cos() * rr, t.sin() * rr)
                };
                for k in 0..n {
                    let (a0, a1, b0, b1) = (pt(k, ri), pt(k + 1, ri), pt(k, ro), pt(k + 1, ro));
                    for p in [a0, b0, b1, a0, b1, a1] {
                        solid(out, p, c1);
                    }
                }
            }
        }
        return None;
    }

    let file = lib.files.get(vm.file)?;
    let sprite = *file.sprites.get(usize::try_from(vm.sprite).ok()?)?;
    if sprite.texture == u32::MAX {
        return None;
    }

    let w = sprite.size[0] * vm.scale[0] * vm.scale2[0];
    let h = sprite.size[1] * vm.scale[1] * vm.scale2[1];
    // Anchor: 0 = centre, 1 = left/top, 2 = right/bottom.
    let ax = match vm.anchor[0] {
        1 => 0.0,
        2 => -w,
        _ => -w * 0.5,
    } - vm.anchor_offset[0] * vm.scale[0];
    let ay = match vm.anchor[1] {
        1 => 0.0,
        2 => -h,
        _ => -h * 0.5,
    } - vm.anchor_offset[1] * vm.scale[1];

    let [u0, v0, u1, v1] = sprite.uv;
    let du = (u1 - u0) * vm.uv_scale[0];
    let dv = (v1 - v0) * vm.uv_scale[1];
    let (u0, v0) = (u0 + vm.uv_scroll[0], v0 + vm.uv_scroll[1]);
    let (u1, v1) = (u0 + du, v0 + dv);

    let (tl, tr, bl, br) = match vm.color_mode {
        1 => (c2, c2, c2, c2),
        2 => (c1, c2, c1, c2),
        3 => (c1, c1, c2, c2),
        _ => (c1, c1, c1, c1),
    };

    // 3D render modes: apply X/Y rotation and project orthographically. APPROX
    let corner = |x: f32, y: f32| -> [f32; 2] {
        if vm.render_mode >= 8 && (vm.rot[0] != 0.0 || vm.rot[1] != 0.0) {
            let (sx, cxr) = vm.rot[0].sin_cos();
            let (sy, cyr) = vm.rot[1].sin_cos();
            let y1 = y * cxr;
            let z1 = y * sx;
            let x2 = x * cyr + z1 * sy;
            xf(x2, y1)
        } else {
            xf(x, y)
        }
    };
    let p_tl = corner(ax, ay);
    let p_tr = corner(ax + w, ay);
    let p_bl = corner(ax, ay + h);
    let p_br = corner(ax + w, ay + h);
    let v = |p, uv, color| Vertex { pos: p, uv, color };
    out.extend_from_slice(&[
        v(p_tl, [u0, v0], tl),
        v(p_tr, [u1, v0], tr),
        v(p_br, [u1, v1], br),
        v(p_tl, [u0, v0], tl),
        v(p_br, [u1, v1], br),
        v(p_bl, [u0, v1], bl),
    ]);
    Some(sprite.texture)
}
