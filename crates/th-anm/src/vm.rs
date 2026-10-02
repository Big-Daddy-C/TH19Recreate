//! ANM VM instances and the world that owns them.
//!
//! Opcode numbering is the v8 (TH13+) instruction set as documented by the
//! community (thtk's format tables and ExpHP's thpages). Behaviour that is not yet
//! confirmed against th19.exe is marked `APPROX` so it can be audited later.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use th_formats::anm::Instr;

use crate::camera::Camera;
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
    /// Canvas position (already perspective-divided for 3D VMs).
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// Clip-space w, for perspective-correct texturing (1.0 for 2D).
    pub w: f32,
    /// Fog factor 0..1 towards the stage fog colour (0.0 for 2D).
    pub fog: f32,
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
    pub origin: [f32; 3],
    /// Positioned in 3D stage space and drawn through a [`Camera`] (STD quads).
    pub space_3d: bool,
    /// Draw the sprite at this size instead of its own (STD quads specify their size).
    pub size_override: Option<[f32; 2]>,
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
            origin: [0.0; 3],
            space_3d: false,
            size_override: None,
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

    /// Interrupt every VM in 3D stage space (STD `ins_19`).
    pub fn interrupt_3d(&mut self, n: i32) {
        for vm in self.slots.iter_mut().filter_map(|s| s.vm.as_mut()).filter(|v| v.space_3d) {
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
        self.spawn_inner(lib, file, script, None, [pos[0], pos[1], 0.0], false)
    }

    /// Create a root VM positioned in 3D stage space (drawn only through a [`Camera`]).
    /// `size` stretches its sprite to that size (STD quads), like the original.
    pub fn spawn_3d(
        &mut self,
        lib: &AnmLibrary,
        file: usize,
        script: usize,
        pos: [f32; 3],
        size: Option<[f32; 2]>,
    ) -> VmId {
        let id = self.spawn_inner(lib, file, script, None, pos, true);
        if let Some(vm) = self.get_mut(id) {
            vm.size_override = size;
        }
        id
    }

    fn spawn_inner(
        &mut self,
        lib: &AnmLibrary,
        file: usize,
        script: usize,
        parent: Option<VmId>,
        origin: [f32; 3],
        space_3d: bool,
    ) -> VmId {
        let mut vm = Vm::new(file, script, self.next_order);
        self.next_order += 1;
        vm.parent = parent;
        vm.origin = origin;
        vm.space_3d = space_3d;
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

        let (file, base, is_3d) = {
            let vm = self.slots[id.index as usize].vm.as_ref().unwrap();
            (vm.file, world_pos(self, vm), vm.space_3d)
        };
        for s in spawns {
            match s {
                Spawn::Child { script, offset } if script >= 0 => {
                    let o = [offset[0], offset[1], 0.0];
                    self.spawn_inner(lib, file, script as usize, Some(id), o, is_3d);
                }
                Spawn::Root { script, offset } if script >= 0 => {
                    let o = [base[0] + offset[0], base[1] + offset[1], base[2]];
                    self.spawn_inner(lib, file, script as usize, None, o, is_3d);
                }
                _ => {}
            }
        }
        if delete {
            self.delete(id);
        }
    }

    /// Build triangles for every visible VM, sorted by layer then creation order.
    /// With a camera, only 3D (stage) VMs are drawn; without one, only 2D VMs.
    pub fn draw_list(&self, lib: &AnmLibrary, camera: Option<&Camera>) -> DrawList {
        let mut vms: Vec<&Vm> = self
            .slots
            .iter()
            .filter_map(|s| s.vm.as_ref())
            .filter(|v| v.space_3d == camera.is_some())
            .collect();
        vms.sort_by_key(|v| (v.layer, v.order));

        let mut out: DrawList = Vec::new();
        let mut verts = Vec::new();
        for vm in vms {
            if !vm.visible || vm.alpha <= 0.0 && vm.color_mode == 0 {
                continue;
            }
            verts.clear();
            let texture = emit(self, lib, vm, camera, &mut verts);
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

fn world_pos(world: &World, vm: &Vm) -> [f32; 3] {
    let mut p = [vm.pos[0] + vm.origin[0], vm.pos[1] + vm.origin[1], vm.pos[2] + vm.origin[2]];
    let mut parent = vm.parent;
    let mut depth = 0;
    while let (Some(pid), true) = (parent, depth < 32) {
        let Some(pv) = world.get(pid) else { break };
        for i in 0..3 {
            p[i] += pv.pos[i] + pv.origin[i];
        }
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

/// A vertex in clip space, before the perspective divide.
#[derive(Clone, Copy)]
struct Clipped {
    clip: [f32; 4],
    uv: [f32; 2],
    color: [f32; 4],
}

/// Sutherland–Hodgman clip of a convex polygon against the plane `w = near`.
fn clip_near(poly: &[Clipped], near: f32) -> Vec<Clipped> {
    let lerp = |a: &Clipped, b: &Clipped, t: f32| {
        let m = |x: f32, y: f32| x + (y - x) * t;
        Clipped {
            clip: std::array::from_fn(|i| m(a.clip[i], b.clip[i])),
            uv: std::array::from_fn(|i| m(a.uv[i], b.uv[i])),
            color: std::array::from_fn(|i| m(a.color[i], b.color[i])),
        }
    };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (a, b) = (&poly[i], &poly[(i + 1) % poly.len()]);
        let (da, db) = (a.clip[3] - near, b.clip[3] - near);
        if da >= 0.0 {
            out.push(*a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            out.push(lerp(a, b, da / (da - db)));
        }
    }
    out
}

/// Maps a VM-local point (relative to its anchor, in pixels) to a screen vertex.
struct Placer<'a> {
    origin: [f32; 3],
    /// 2D: rotation about the screen normal. 3D: Euler rotation applied X, then Y, then Z.
    rot: [f32; 3],
    camera: Option<&'a Camera>,
    additive: bool,
}

impl Placer<'_> {
    /// VM-local point to 3D world position (rotation applied X, then Y, then Z).
    fn to_world(&self, x: f32, y: f32) -> [f32; 3] {
        // APPROX: rotation order for 3D sprites is not yet confirmed against th19.exe.
        let [rx, ry, rz] = self.rot;
        let (sx, cx) = rx.sin_cos();
        let (sy, cy) = ry.sin_cos();
        let (sz, cz) = rz.sin_cos();
        let (x1, y1, z1) = (x, y * cx, y * sx);
        let (x2, y2, z2) = (x1 * cy + z1 * sy, y1, -x1 * sy + z1 * cy);
        let (x3, y3, z3) = (x2 * cz - y2 * sz, x2 * sz + y2 * cz, z2);
        [self.origin[0] + x3, self.origin[1] + y3, self.origin[2] + z3]
    }

    fn place_2d(&self, x: f32, y: f32, uv: [f32; 2], color: [f32; 4]) -> Vertex {
        let (s, c) = self.rot[2].sin_cos();
        let pos = [self.origin[0] + x * c - y * s, self.origin[1] + x * s + y * c];
        Vertex { pos, uv, color, w: 1.0, fog: 0.0 }
    }

    fn finish_3d(&self, cam: &Camera, v: &Clipped) -> Vertex {
        let (pos, w, fog) = cam.to_screen(v.clip);
        // Fogging an additive sprite towards a colour would brighten it; fade it out instead.
        let (color, fog) = if self.additive {
            let k = 1.0 - fog;
            ([v.color[0] * k, v.color[1] * k, v.color[2] * k, v.color[3]], 0.0)
        } else {
            (v.color, fog)
        };
        Vertex { pos, uv: v.uv, color, w, fog }
    }

    /// Push a triangle. In 3D it is clipped against the camera's near plane first,
    /// so geometry passing beside or under the camera is cut rather than dropped.
    fn tri(&self, out: &mut Vec<Vertex>, v: [(f32, f32, [f32; 2], [f32; 4]); 3]) {
        let Some(cam) = self.camera else {
            for (x, y, uv, c) in v {
                out.push(self.place_2d(x, y, uv, c));
            }
            return;
        };
        let poly = v.map(|(x, y, uv, color)| Clipped { clip: cam.clip(self.to_world(x, y)), uv, color });
        let clipped = clip_near(&poly, Camera::NEAR);
        for i in 1..clipped.len().saturating_sub(1) {
            for c in [&clipped[0], &clipped[i], &clipped[i + 1]] {
                out.push(self.finish_3d(cam, c));
            }
        }
    }

    fn quad(&self, out: &mut Vec<Vertex>, p: [(f32, f32); 4], uv: [[f32; 2]; 4], col: [[f32; 4]; 4]) {
        // Corners in order: top-left, top-right, bottom-right, bottom-left.
        self.tri(out, [(p[0].0, p[0].1, uv[0], col[0]), (p[1].0, p[1].1, uv[1], col[1]), (p[2].0, p[2].1, uv[2], col[2])]);
        self.tri(out, [(p[0].0, p[0].1, uv[0], col[0]), (p[2].0, p[2].1, uv[2], col[2]), (p[3].0, p[3].1, uv[3], col[3])]);
    }
}

/// Emit triangles for one VM; returns the texture it needs.
fn emit(world: &World, lib: &AnmLibrary, vm: &Vm, camera: Option<&Camera>, out: &mut Vec<Vertex>) -> Option<u32> {
    let c1 = rgba(vm.color, vm.alpha);
    let c2 = rgba(vm.color2, vm.alpha2);
    let rot = if camera.is_some() {
        vm.rot
    } else if matches!(vm.render_mode, 0 | 2) {
        [0.0; 3]
    } else {
        [0.0, 0.0, vm.rot[2]]
    };
    let pl = Placer {
        origin: world_pos(world, vm),
        rot,
        camera,
        additive: matches!(vm.blend, BlendMode::Add | BlendMode::Max),
    };
    let z = [0.0f32; 2];

    if let Some(prim) = vm.primitive {
        let sx = vm.scale[0] * vm.scale2[0];
        let sy = vm.scale[1] * vm.scale2[1];
        let rect = |out: &mut Vec<Vertex>, x0: f32, y0: f32, x1: f32, y1: f32, l: [f32; 4], r: [f32; 4]| {
            pl.quad(out, [(x0, y0), (x1, y0), (x1, y1), (x0, y1)], [z; 4], [l, r, r, l]);
        };
        let ring_pt = |k: i32, n: i32, rr: f32| {
            let t = k as f32 / n as f32 * std::f32::consts::TAU;
            (t.cos() * rr, t.sin() * rr)
        };
        match prim {
            Primitive::Rect { w, h, rotated, gradient } => {
                let (hw, hh) = (w * sx * 0.5, h * sy * 0.5);
                let (l, r) = if gradient { (c1, c2) } else { (c1, c1) };
                if rotated {
                    rect(out, -hw, -hh, hw, hh, l, r);
                } else {
                    Placer { rot: [0.0; 3], ..pl }.quad(out, [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)], [z; 4], [l, r, r, l]);
                }
            }
            Primitive::RectBorder { w, h } => {
                let (hw, hh, t) = (w * sx * 0.5, h * sy * 0.5, 1.0);
                for (x0, y0, x1, y1) in [(-hw, -hh, hw, -hh + t), (-hw, hh - t, hw, hh), (-hw, -hh, -hw + t, hh), (hw - t, -hh, hw, hh)] {
                    rect(out, x0, y0, x1, y1, c1, c1);
                }
            }
            Primitive::Poly { radius, segments, border } => {
                let n = segments.clamp(3, 256);
                let r = radius * sx;
                for k in 0..n {
                    let (b0, b1) = (ring_pt(k, n, r), ring_pt(k + 1, n, r));
                    if border {
                        let (a0, a1) = (ring_pt(k, n, r - 1.0), ring_pt(k + 1, n, r - 1.0));
                        pl.quad(out, [a0, b0, b1, a1], [z; 4], [c1; 4]);
                    } else {
                        pl.tri(out, [(0.0, 0.0, z, c1), (b0.0, b0.1, z, c2), (b1.0, b1.1, z, c2)]);
                    }
                }
            }
            Primitive::Ring { radius, thickness, segments } => {
                let n = segments.clamp(3, 256);
                let (ri, ro) = ((radius - thickness * 0.5) * sx, (radius + thickness * 0.5) * sx);
                for k in 0..n {
                    let (a0, a1) = (ring_pt(k, n, ri), ring_pt(k + 1, n, ri));
                    let (b0, b1) = (ring_pt(k, n, ro), ring_pt(k + 1, n, ro));
                    pl.quad(out, [a0, b0, b1, a1], [z; 4], [c1; 4]);
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

    let base = vm.size_override.unwrap_or(sprite.size);
    let w = base[0] * vm.scale[0] * vm.scale2[0];
    let h = base[1] * vm.scale[1] * vm.scale2[1];
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
    pl.quad(
        out,
        [(ax, ay), (ax + w, ay), (ax + w, ay + h), (ax, ay + h)],
        [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
        [tl, tr, br, bl],
    );
    Some(sprite.texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(w: f32) -> Clipped {
        Clipped { clip: [0.0, 0.0, 0.0, w], uv: [w, 0.0], color: [1.0; 4] }
    }

    #[test]
    fn near_clip_keeps_front_and_cuts_crossing_edges() {
        // Fully in front: unchanged.
        assert_eq!(clip_near(&[v(5.0), v(6.0), v(7.0)], 1.0).len(), 3);
        // Fully behind: gone.
        assert!(clip_near(&[v(-5.0), v(-6.0), v(0.5)], 1.0).is_empty());
        // One corner behind: becomes a quad, new corners sit exactly on the plane.
        let q = clip_near(&[v(-1.0), v(3.0), v(3.0)], 1.0);
        assert_eq!(q.len(), 4);
        assert!(q.iter().all(|c| c.clip[3] >= 1.0 - 1e-5));
        // Attributes are interpolated along the cut edge (uv.x mirrors w here).
        assert!(q.iter().any(|c| (c.uv[0] - 1.0).abs() < 1e-5));
    }
}
