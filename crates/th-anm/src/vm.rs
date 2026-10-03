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
    /// Normalised depth 0..1 (0.0 for 2D, which is drawn without depth testing).
    pub depth: f32,
}

/// How a batch interacts with the depth buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum DepthMode {
    /// 2D UI/game layers: always drawn, no depth.
    #[default]
    Off,
    /// 3D, depth-tested but not written (translucent layers; ANM `zWriteDisable(1)`).
    Test,
    /// 3D, depth-tested and written.
    TestWrite,
}

/// Consecutive triangles sharing a texture and blend mode.
#[derive(Debug)]
pub struct DrawBatch {
    /// `None` = untextured (solid colour primitives).
    pub texture: Option<u32>,
    pub blend: BlendMode,
    pub depth: DepthMode,
    pub layer: i32,
    /// Restrict drawing to this canvas rectangle `[x, y, w, h]` (e.g. one playfield).
    pub clip: Option<[f32; 4]>,
    pub vertices: Vec<Vertex>,
}

impl DrawBatch {
    /// An untextured rectangle in canvas space.
    pub fn solid_rect(rect: [f32; 4], color: [f32; 4]) -> Self {
        let [x, y, w, h] = rect;
        let v = |px: f32, py: f32| Vertex { pos: [px, py], uv: [0.0; 2], color, w: 1.0, fog: 0.0, depth: 0.0 };
        Self {
            texture: None,
            blend: BlendMode::Replace,
            depth: DepthMode::Off,
            layer: i32::MIN,
            clip: Some(rect),
            vertices: vec![v(x, y), v(x + w, y), v(x + w, y + h), v(x, y), v(x + w, y + h), v(x, y + h)],
        }
    }
}

pub type DrawList = Vec<DrawBatch>;

/// Which group of VMs to draw. A stage frame is drawn as `Backdrop`, then `Stage`, then `Ui`.
#[derive(Clone, Copy)]
pub enum Pass<'a> {
    /// 2D sprites placed behind the 3D stage (STD sprite slots, e.g. distant scenery).
    Backdrop,
    /// 3D stage geometry seen through a camera.
    Stage(&'a Camera),
    /// Ordinary 2D layers (game field, UI, the ANM viewer).
    Ui,
}

/// ANM `effectNew(1)` (effect table 0x5ca6d0 entry 1: init FUN_004d9880, update
/// FUN_004d9960, interrupt FUN_004da890): particles converging on the VM, as used for a
/// boss's entrance (`EffChargePoint2`). One group of children (scripts 143 x3 and 144 of
/// the same file) is spawned per frame while the group timer is below 50; each follows a
/// Hermite curve from 300 units out along the VM's rotation to 150 units out, then into
/// the centre, `I0` frames per curve. The effect deletes itself once no child is left.
#[derive(Clone, Debug, Default)]
pub struct ChargeIn {
    /// Group timer (+0x1928 previous / +0x192c current). Interrupt 1 skips it ahead by 300.
    timer: i32,
    timer_prev: i32,
    particles: Vec<ChargeParticle>,
}

#[derive(Clone, Debug)]
struct ChargeParticle {
    id: Option<VmId>,
    /// 0 new, 1 on the outer curve, 2 on the inner curve.
    state: u8,
    /// Frames since the child was created (stands in for the child's own +0x4f8 timer).
    age: i32,
    /// End point and end tangent of the outer curve, where the inner one starts.
    mid: [f32; 3],
    mid_tangent: [f32; 3],
}

impl Vm {
    /// The built-in effect's interrupt callback (+0x604; FUN_004da890 for effect 1:
    /// interrupt 1 moves the group timer 300 frames on, so no further groups spawn).
    fn effect_interrupt(&mut self, n: i32) {
        if let (Some(fx), 1) = (self.charge.as_mut(), n) {
            fx.timer_prev = fx.timer;
            fx.timer += 300;
        }
    }
}

/// Built-in ribbon effects created by ANM `effectNew` (508). Each is a strip of points whose
/// shape is shifted along the ribbon over time, so disturbances travel along it.
#[derive(Clone, Debug)]
pub struct Ribbon {
    kind: RibbonKind,
    /// Offset from each point to the next, in the sprite's local 3D space.
    deltas: Vec<[f32; 3]>,
    timer: u32,
    /// Effect 5: current sway angle of the first segment.
    angle: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RibbonKind {
    /// Effect 5: a strand hanging 200 units above its origin, swaying (e.g. stage 2's weeds).
    Strand,
    /// Effect 6: a rising wisp of smoke (e.g. stage 13's incense).
    Smoke,
}

impl Ribbon {
    fn new(effect: i32) -> Option<Self> {
        match effect {
            5 => Some(Self { kind: RibbonKind::Strand, deltas: vec![[0.0, 0.0, 4.0]; 40], timer: 0, angle: 10f32.to_radians() }),
            6 => Some(Self { kind: RibbonKind::Smoke, deltas: vec![[-3.0, 0.0, -16.0]; 16], timer: 0, angle: 0.0 }),
            _ => None,
        }
    }

    /// One frame of motion, as in th19.exe's effect update callbacks.
    fn update(&mut self, rng: &mut u32) {
        // Uniform in [-1, 1] and [0, 1].
        let mut signed = || randf01(rng) * 2.0 - 1.0;
        let shift = |d: &mut Vec<[f32; 3]>| {
            for i in (1..d.len()).rev() {
                d[i] = d[i - 1];
            }
        };
        match self.kind {
            RibbonKind::Smoke if self.timer % 4 == 0 => {
                shift(&mut self.deltas);
                // The first segment's sideways lean random-walks, kept roughly within 0..3.
                let x = self.deltas[0][0];
                self.deltas[0][0] = if x < 3.0 {
                    if x > 0.0 { x + signed() * 0.1 } else { x + (signed() * 0.5 + 0.5) * 0.1 }
                } else {
                    x - (signed() * 0.5 + 0.5) * 0.1
                };
            }
            RibbonKind::Strand if self.timer % 2 == 0 => {
                shift(&mut self.deltas);
                // The first segment's angle random-walks between roughly 10 and 30 degrees.
                let deg = 1f32.to_radians();
                let a = self.angle;
                self.angle = if a < 30f32.to_radians() {
                    if a > 10f32.to_radians() { a + signed() * deg * 2.0 } else { a + (signed() * 0.5 + 0.5) * deg }
                } else {
                    a - (signed() * 0.5 + 0.5) * deg
                };
                let (s, c) = self.angle.sin_cos();
                self.deltas[0] = [-s * 4.0, 0.0, c * 4.0];
            }
            _ => {}
        }
        self.timer += 1;
    }

    /// Strip vertices in local space: `(position, u, v, alpha 0..255)`, two per point.
    fn vertices(&self) -> Vec<([f32; 3], f32, f32, f32)> {
        let n = self.deltas.len();
        let (mut pos, mut half, grow, dv) = match self.kind {
            RibbonKind::Smoke => ([0.0f32, 0.0, 0.0], 0.0f32, true, 1.0 / 16.0),
            RibbonKind::Strand => ([0.0, 0.0, -200.0], 8.0, false, 0.025),
        };
        let mut out = Vec::with_capacity(n * 2);
        for i in 0..n {
            let (a, b) = match self.kind {
                RibbonKind::Smoke => SMOKE_ALPHA.get(i).copied().unwrap_or((128.0, 128.0)),
                RibbonKind::Strand => (128.0, 128.0),
            };
            let v = i as f32 * dv;
            out.push(([pos[0] + half, pos[1], pos[2]], 0.0, v, a));
            out.push(([pos[0] - half, pos[1], pos[2]], 1.0, v, b));
            if grow && half < 8.0 {
                half += 2.0;
            }
            for k in 0..3 {
                pos[k] += self.deltas[i][k];
            }
        }
        out
    }
}

/// Per-point alpha of the smoke ribbon's two edges: a fixed pattern of gaps that makes the
/// wisp look broken up, with both ends fading out (values observed in th19.exe).
const SMOKE_ALPHA: [(f32, f32); 16] = [
    (0.0, 0.0),
    (128.0, 0.0),
    (128.0, 128.0),
    (128.0, 128.0),
    (128.0, 128.0),
    (0.0, 128.0),
    (0.0, 128.0),
    (128.0, 0.0),
    (128.0, 128.0),
    (128.0, 128.0),
    (128.0, 128.0),
    (128.0, 128.0),
    (0.0, 128.0),
    (176.0, 0.0),
    (80.0, 80.0),
    (0.0, 0.0),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunState {
    Running,
    /// `stop`: script halted, interpolation and interrupts still work.
    Stopped,
    /// `static`: frozen entirely.
    Static,
}

#[derive(Clone, Copy, Debug)]
pub enum Primitive {
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

    /// Int variables: I0-I3 (10000-10003), 10008, 10009, 10029.
    pub ivars: [i32; 7],
    /// Float variables: F0-F3 (10004-10007), F4-F6 (10033-10035), 10027, 10028.
    pub fvars: [f32; 9],

    pub pos: [f32; 3],
    pub rot: [f32; 3],
    angle_vel: [f32; 3],
    pub scale: [f32; 2],
    scale2: [f32; 2],
    scale_growth: [f32; 2],
    pub color: [f32; 3],
    pub color2: [f32; 3],
    pub alpha: f32,
    pub alpha2: f32,
    /// ANM `423`: 0 colour 1, 1 colour 2, 2/3 gradients.
    pub color_mode: i32,

    pub sprite: i32,
    pub blend: BlendMode,
    /// ANM `zWriteDisable`: 3D sprites that should not occlude what is drawn after them.
    pub z_write_disable: bool,
    /// ANM `rotationMode` (437): order the X/Y/Z rotations are applied in for 3D sprites.
    pub rotation_mode: u8,
    pub render_mode: i32,
    pub layer: i32,
    pub visible: bool,
    anchor: [i16; 2],
    anchor_offset: [f32; 2],
    uv_scroll: [f32; 2],
    uv_vel: [f32; 2],
    uv_scale: [f32; 2],
    pub primitive: Option<Primitive>,

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
    /// A zero component keeps the sprite's own size on that axis; negative mirrors.
    pub size_override: Option<[f32; 2]>,
    /// 2D sprite belonging to the stage background (drawn in [`Pass::Backdrop`]).
    pub backdrop: bool,
    /// Built-in effect attached by `effectNew`, drawn instead of the sprite.
    pub effect: Option<Ribbon>,
    /// Draw-chain priority overriding the layer's (e.g. 41 for enemy bullets, which the
    /// bullet manager draws itself: FUN_004f6470 registers its draw at 0x29).
    pub draw_priority: Option<i32>,
    /// Built-in particle effect attached by `effectNew(1)`.
    pub charge: Option<Box<ChargeIn>>,
    /// STD layer this stage sprite is drawn in (stage quads and sprite slots).
    pub stage_layer: u8,
    /// Scissor rectangle on the canvas (e.g. a playfield), inherited by spawned VMs.
    pub clip: Option<[f32; 4]>,
    /// Sprite numbers 0..4 set by the script are replaced by these (bullets: the colour's
    /// sprites, th19.exe `FUN_004f66a0`).
    pub sprite_remap: Option<[i32; 5]>,
    /// Sum of the ancestors' z rotations, refreshed before the VM runs (variable 10026).
    inherited_rot: f32,
    /// ANM `424`: the owning bullet sets the z rotation from its direction.
    pub auto_rotate: bool,
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
            // 10027, 10028 and 10029 start as 1.0, pi and 0x10000 (th19.exe VM reset).
            ivars: [0, 0, 0, 0, 0, 0, 0x10000],
            fvars: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f32::consts::PI],
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
            z_write_disable: false,
            rotation_mode: 0,
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
            backdrop: false,
            effect: None,
            charge: None,
            draw_priority: None,
            stage_layer: 0,
            clip: None,
            sprite_remap: None,
            inherited_rot: 0.0,
            auto_rotate: false,
        }
    }

    fn remap_sprite(&self, n: i32) -> i32 {
        match self.sprite_remap {
            Some(m) if (0..5).contains(&n) => m[n as usize],
            _ => n,
        }
    }

    /// Interpolate the scale from its current value to `goal` over `time` frames.
    pub fn interp_scale(&mut self, goal: [f32; 2], time: i32, mode: i32) {
        self.i_scale.start(self.scale, goal, time, mode);
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

    /// Send interrupt `n` to a VM and its children (th19.exe `FUN_004cb830` walks the
    /// VM's child list too).
    pub fn interrupt(&mut self, id: VmId, n: i32) {
        if let Some(vm) = self.get_mut(id) {
            vm.pending_interrupt = Some(n);
            vm.effect_interrupt(n);
        }
        for s in self.slots.iter_mut() {
            if let Some(vm) = s.vm.as_mut().filter(|v| v.parent == Some(id)) {
                vm.pending_interrupt = Some(n);
                vm.effect_interrupt(n);
            }
        }
    }

    /// Draw a VM and its current children at a fixed draw-chain priority (later children
    /// inherit it when spawned).
    pub fn set_draw_priority(&mut self, id: VmId, priority: i32) {
        if let Some(vm) = self.get_mut(id) {
            vm.draw_priority = Some(priority);
        }
        for s in self.slots.iter_mut() {
            if let Some(vm) = s.vm.as_mut().filter(|v| v.parent == Some(id)) {
                vm.draw_priority = Some(priority);
            }
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
            vm.effect_interrupt(n);
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

    /// Create a root 2D VM whose drawing (and its children's) is scissored to `clip`.
    pub fn spawn_clipped(&mut self, lib: &AnmLibrary, file: usize, script: usize, pos: [f32; 2], clip: [f32; 4]) -> VmId {
        let mut vm = Vm::new(file, script, self.next_order);
        self.next_order += 1;
        vm.origin = [pos[0], pos[1], 0.0];
        vm.clip = Some(clip);
        let id = self.insert(vm);
        self.run(lib, id);
        id
    }

    /// Create a root 2D VM with its z rotation set before its first instructions run (the
    /// game passes an angle when creating effects, and scripts read it right away).
    pub fn spawn_rotated(&mut self, lib: &AnmLibrary, file: usize, script: usize, pos: [f32; 2], clip: [f32; 4], rot_z: f32) -> VmId {
        let mut vm = Vm::new(file, script, self.next_order);
        self.next_order += 1;
        vm.origin = [pos[0], pos[1], 0.0];
        vm.clip = Some(clip);
        vm.rot[2] = rot_z;
        let id = self.insert(vm);
        self.run(lib, id);
        id
    }

    /// Like [`World::spawn_clipped`], with a sprite remap applied from the first instruction.
    pub fn spawn_remapped(
        &mut self,
        lib: &AnmLibrary,
        file: usize,
        script: usize,
        pos: [f32; 2],
        clip: [f32; 4],
        remap: [i32; 5],
    ) -> VmId {
        let mut vm = Vm::new(file, script, self.next_order);
        self.next_order += 1;
        vm.origin = [pos[0], pos[1], 0.0];
        vm.clip = Some(clip);
        vm.sprite_remap = Some(remap);
        let id = self.insert(vm);
        self.run(lib, id);
        id
    }

    /// Create a 2D VM drawn behind the 3D stage ([`Pass::Backdrop`]).
    pub fn spawn_backdrop(
        &mut self,
        lib: &AnmLibrary,
        file: usize,
        script: usize,
        pos: [f32; 2],
        stage_layer: u8,
    ) -> VmId {
        // Flag it before its first instructions run so any children inherit it.
        let mut vm = Vm::new(file, script, self.next_order);
        self.next_order += 1;
        vm.origin = [pos[0], pos[1], 0.0];
        vm.backdrop = true;
        vm.stage_layer = stage_layer;
        let id = self.insert(vm);
        self.run(lib, id);
        id
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
        stage_layer: u8,
    ) -> VmId {
        let id = self.spawn_inner(lib, file, script, None, pos, true);
        if let Some(vm) = self.get_mut(id) {
            vm.size_override = size;
            vm.stage_layer = stage_layer;
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
            let mut rng = self.rng;
            let Some(vm) = self.get_mut(id) else { continue };
            if vm.state != RunState::Static {
                update_motion(vm);
                if let Some(r) = &mut vm.effect {
                    r.update(&mut rng);
                }
                vm.time += 1;
                if vm.wait > 0 {
                    vm.wait -= 1;
                }
            }
            self.rng = rng;
            if self.get(id).is_some_and(|v| v.charge.is_some() && v.state != RunState::Static) {
                self.update_charge(lib, id);
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

    /// One frame of `effectNew(1)` (FUN_004d9960).
    fn update_charge(&mut self, lib: &AnmLibrary, id: VmId) {
        let Some(vm) = self.get_mut(id) else { return };
        let Some(mut fx) = vm.charge.take() else { return };
        let (file, clip, duration, rot, spread, color) =
            (vm.file, vm.clip, vm.ivars[0], vm.rot[2], vm.fvars[6], vm.color);
        let vm = self.get(id).unwrap();
        let centre = world_pos(self, vm);
        let polar = |c: [f32; 3], r: f32, a: f32| [c[0] + a.cos() * r, c[1] + a.sin() * r, c[2]];
        let far = polar(centre, 300.0, rot);
        let near = polar(centre, 150.0, spread + rot);
        // A new group while the timer moved this frame and is below 50 (+0x192c < 0x32).
        if fx.timer != fx.timer_prev && fx.timer < 50 {
            for (k, script) in [143, 143, 143, 144].into_iter().enumerate() {
                let child = self.spawn_inner(lib, file, script, None, [0.0; 3], false);
                if let Some(c) = self.get_mut(child) {
                    c.clip = clip;
                    c.ivars[0] = duration;
                    // The fourth child's colour bytes are 0x2c - each byte of the parent's.
                    c.color = if k == 3 { color.map(|v| 0x2cu8.wrapping_sub(v as u8) as f32) } else { color };
                }
                fx.particles.push(ChargeParticle { id: Some(child), state: 0, age: 0, mid: [0.0; 3], mid_tangent: [0.0; 3] });
            }
        }
        let mut rng = self.rng;
        // rand / (RAND_MAX - 1) and rand / (RAND_MAX / 2 - 1) - 1 in the original.
        let mut r01 = || randf01(&mut rng);
        let norm = |v: [f32; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if l.abs() >= 0.01 { v.map(|x| x / l) } else { v }
        };
        let mut alive = 0;
        for p in &mut fx.particles {
            let Some(cid) = p.id else { continue };
            let Some(c) = self.get_mut(cid) else {
                p.id = None;
                continue;
            };
            if p.state == 0 {
                let (r, a) = (r01() * 150.0, (r01() * 2.0 - 1.0) * std::f32::consts::PI);
                let start = polar(far, r, a);
                let (r, a) = (r01() * 50.0, (r01() * 2.0 - 1.0) * std::f32::consts::PI);
                let mid = polar(near, r, a);
                let along = norm([mid[0] - start[0], mid[1] - start[1], mid[2] - start[2]]);
                let inward = norm([centre[0] - mid[0], centre[1] - mid[1], centre[2] - mid[2]]);
                let k = r01() * 200.0 + 200.0;
                let mid_tangent = norm([along[0] + inward[0], along[1] + inward[1], along[2] + inward[2]]).map(|x| x * k);
                let k = r01() * 100.0 + 100.0;
                let start_tangent = along.map(|x| x * k);
                c.pos = start;
                c.i_pos.start_bezier(start, start_tangent, mid, mid_tangent, duration);
                p.mid = mid;
                p.mid_tangent = mid_tangent;
                p.state = 1;
            } else if p.state == 1 && p.age >= duration + 1 {
                let (r, a) = (r01() * 20.0, (r01() * 2.0 - 1.0) * std::f32::consts::PI);
                let end = polar(centre, r, a);
                let (r, a) = (r01() * 20.0, (r01() * 2.0 - 1.0) * std::f32::consts::PI);
                let end_tangent = [a.cos() * r, a.sin() * r, 0.0];
                c.i_pos.start_bezier(p.mid, p.mid_tangent, end, end_tangent, duration);
                p.state = 2;
            }
            p.age += 1;
            alive += 1;
        }
        self.rng = rng;
        fx.timer_prev = fx.timer;
        fx.timer += 1;
        if alive == 0 {
            self.delete(id);
        } else if let Some(vm) = self.get_mut(id) {
            vm.charge = Some(fx);
        }
    }

    fn run(&mut self, lib: &AnmLibrary, id: VmId) {
        let mut spawns = Vec::new();
        let mut inherited = 0.0;
        let mut p = self.slots[id.index as usize].vm.as_ref().and_then(|v| v.parent);
        let mut depth = 0;
        while let (Some(pid), true) = (p, depth < 32) {
            let Some(pv) = self.get(pid) else { break };
            inherited += pv.rot[2];
            p = pv.parent;
            depth += 1;
        }
        if let Some(vm) = self.slots[id.index as usize].vm.as_mut() {
            vm.inherited_rot = inherited;
        }
        // ANM `copyParentVars` (509) needs the parent's variables while the child runs.
        let parent_vars = self.slots[id.index as usize]
            .vm
            .as_ref()
            .and_then(|v| v.parent)
            .and_then(|p| self.get(p))
            .map(|p| (p.ivars, p.fvars));
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
                match exec(vm, ins, rng, unknown, &mut spawns, parent_vars) {
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

        let (file, base, is_3d, backdrop, stage_layer, clip, draw_priority) = {
            let vm = self.slots[id.index as usize].vm.as_ref().unwrap();
            (vm.file, world_pos(self, vm), vm.space_3d, vm.backdrop, vm.stage_layer, vm.clip, vm.draw_priority)
        };
        for s in spawns {
            match s {
                Spawn::Child { script, offset } if script >= 0 => {
                    let o = [offset[0], offset[1], 0.0];
                    let child = self.spawn_inner(lib, file, script as usize, Some(id), o, is_3d);
                    if let Some(c) = self.get_mut(child) {
                        c.draw_priority = draw_priority;
                        c.backdrop = backdrop;
                        c.stage_layer = stage_layer;
                        c.clip = clip;
                    }
                }
                Spawn::Root { script, offset } if script >= 0 => {
                    let o = [base[0] + offset[0], base[1] + offset[1], base[2]];
                    let root = self.spawn_inner(lib, file, script as usize, None, o, is_3d);
                    if let Some(c) = self.get_mut(root) {
                        c.backdrop = backdrop;
                        c.stage_layer = stage_layer;
                        c.clip = clip;
                    }
                }
                _ => {}
            }
        }
        if delete {
            self.delete(id);
        }
    }

    /// Build the whole stage background the way th19.exe draws it: STD layers 0, 1, 2... in
    /// turn; within each layer, first that layer's sprite-slot sprites (2D, through the
    /// screen camera), then its 3D quads in spawn order.
    pub fn draw_stage(&self, lib: &AnmLibrary, camera: &Camera) -> DrawList {
        let mut vms: Vec<&Vm> =
            self.slots.iter().filter_map(|s| s.vm.as_ref()).filter(|v| v.space_3d || v.backdrop).collect();
        vms.sort_by_key(|v| (v.stage_layer, v.space_3d, v.order));
        let mut out: DrawList = Vec::new();
        let mut verts = Vec::new();
        for vm in vms {
            if !vm.visible || vm.alpha <= 0.0 && vm.color_mode == 0 && (vm.primitive.is_none() || vm.alpha2 <= 0.0) {
                continue;
            }
            verts.clear();
            let cam = vm.space_3d.then_some(camera);
            let texture = emit(self, lib, vm, cam, &mut verts);
            if verts.is_empty() {
                continue;
            }
            let depth = match (vm.space_3d, vm.z_write_disable || vm.effect.is_some()) {
                (false, _) => DepthMode::Off,
                (true, true) => DepthMode::Test,
                (true, false) => DepthMode::TestWrite,
            };
            match out.last_mut() {
                Some(b) if b.texture == texture && b.blend == vm.blend && b.depth == depth => {
                    b.vertices.extend_from_slice(&verts)
                }
                _ => out.push(DrawBatch { texture, blend: vm.blend, depth, layer: vm.layer, clip: None, vertices: verts.clone() }),
            }
        }
        out
    }

    /// Build triangles for the VMs in `pass`, in the game's draw-chain order (layer
    /// priority, or a manager's own priority), then creation order.
    pub fn draw_list(&self, lib: &AnmLibrary, pass: Pass) -> DrawList {
        let camera = match pass {
            Pass::Stage(c) => Some(c),
            _ => None,
        };
        let mut vms: Vec<&Vm> = self
            .slots
            .iter()
            .filter_map(|s| s.vm.as_ref())
            .filter(|v| match pass {
                Pass::Backdrop => !v.space_3d && v.backdrop,
                Pass::Stage(_) => v.space_3d,
                Pass::Ui => !v.space_3d && !v.backdrop,
            })
            .collect();
        // The stage loop draws its sprites in spawn order regardless of their ANM layer.
        match pass {
            Pass::Stage(_) => vms.sort_by_key(|v| v.order),
            _ => vms.sort_by_key(|v| (v.draw_priority.unwrap_or_else(|| layer_priority(v.layer)), v.order)),
        }

        let mut out: DrawList = Vec::new();
        let mut verts = Vec::new();
        for vm in vms {
            if !vm.visible || vm.alpha <= 0.0 && vm.color_mode == 0 && (vm.primitive.is_none() || vm.alpha2 <= 0.0) {
                continue;
            }
            verts.clear();
            let texture = emit(self, lib, vm, camera, &mut verts);
            if verts.is_empty() {
                continue;
            }
            // Depth writes depend only on the sprite's zWriteDisable flag (as in th19.exe).
            let depth = match (camera.is_some(), vm.z_write_disable || vm.effect.is_some()) {
                (false, _) => DepthMode::Off,
                (true, true) => DepthMode::Test,
                (true, false) => DepthMode::TestWrite,
            };
            match out.last_mut() {
                Some(b)
                    if b.texture == texture
                        && b.blend == vm.blend
                        && b.layer == vm.layer
                        && b.depth == depth
                        && b.clip == vm.clip =>
                {
                    b.vertices.extend_from_slice(&verts)
                }
                _ => out.push(DrawBatch {
                    texture,
                    blend: vm.blend,
                    depth,
                    layer: vm.layer,
                    clip: vm.clip,
                    vertices: verts.clone(),
                }),
            }
        }
        out
    }
}

/// Draw-chain priority of each 2D ANM layer, as registered by th19.exe's ANM manager
/// (FUN_004d0da0: one callback per layer, `FUN_004d5bb0(callback, priority)`). Layers are
/// not drawn in numeric order (e.g. 43 sits between 23 and 24).
fn layer_priority(layer: i32) -> i32 {
    const TABLE: [i32; 52] = [
        5, 7, 9, 10, 11, 13, 16, 18, 20, 21, 22, 24, 27, 28, 33, 34, 37, 40, 43, 46, // 0-19
        49, 50, 58, 60, 62, 63, 70, 66, 67, 71, 74, 76, 79, 81, 96, 98, 101, // 20-36
        -1, -1, -1, -1, -1, -1, // 37-42: no callback found
        61, 72, 75, 77, 78, 80, 97, 99, 102, // 43-51
    ];
    match usize::try_from(layer).ok().and_then(|l| TABLE.get(l)) {
        Some(&p) if p >= 0 => p,
        // APPROX: unregistered layers keep their numeric order after the known ones.
        _ => 200 + layer,
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
    match id {
        10000..=10003 => vm.ivars[(id - 10000) as usize] as f32,
        10004..=10007 => vm.fvars[(id - 10004) as usize],
        10008 | 10009 => vm.ivars[(id - 10004) as usize] as f32,
        10029 => vm.ivars[6] as f32,
        10033..=10035 => vm.fvars[(id - 10029) as usize],
        10027 | 10028 => vm.fvars[(id - 10020) as usize],
        // Random values scaled by variables 10028 (default pi) and 10027 (default 1).
        10010 | 10030 => (randf01(rng) * 2.0 - 1.0) * vm.fvars[8],
        10011 | 10031 => randf01(rng) * vm.fvars[7],
        10012 | 10032 => (randf01(rng) * 2.0 - 1.0) * vm.fvars[7],
        10013..=10015 => vm.pos[(id - 10013) as usize],
        10022 => next_rand(rng) as i32 as f32,
        10023..=10025 => vm.rot[(id - 10023) as usize],
        // World rotation: own plus the ancestors' (FUN_00404ae0).
        10026 => vm.rot[2] + vm.inherited_rot,
        _ => 0.0,
    }
}

fn read_ivar(vm: &Vm, id: i32, rng: &mut u32) -> i32 {
    match id {
        10000..=10003 => vm.ivars[(id - 10000) as usize],
        10008 | 10009 => vm.ivars[(id - 10004) as usize],
        10029 => vm.ivars[6],
        10022 => next_rand(rng) as i32,
        _ => read_var(vm, id, rng) as i32,
    }
}

fn write_ivar(vm: &mut Vm, id: i32, v: i32) {
    match id {
        10000..=10003 => vm.ivars[(id - 10000) as usize] = v,
        10008 | 10009 => vm.ivars[(id - 10004) as usize] = v,
        10029 => vm.ivars[6] = v,
        // Any other id resolves to the instruction's own argument (FUN_004c1400), so the
        // write is lost.
        _ => {}
    }
}

fn write_fvar(vm: &mut Vm, id: i32, v: f32) {
    match id {
        10004..=10007 => vm.fvars[(id - 10004) as usize] = v,
        10033..=10035 => vm.fvars[(id - 10029) as usize] = v,
        10027 | 10028 => vm.fvars[(id - 10020) as usize] = v,
        10013..=10015 => vm.pos[(id - 10013) as usize] = v,
        10023..=10025 => vm.rot[(id - 10023) as usize] = v,
        // Int ids (e.g. fset to I0) resolve to the instruction's own argument
        // (FUN_004c14c0): the write is lost.
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
    parent_vars: Option<([i32; 7], [f32; 9])>,
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
            vm.sprite = vm.remap_sprite(i!(0));
            vm.primitive = None;
        }
        301 => {
            let (base, n) = (i!(0), i!(1));
            let r = if n > 0 { (next_rand(rng) % n as u32) as i32 } else { 0 };
            vm.sprite = vm.remap_sprite(base + r);
            vm.primitive = None;
        }
        302 => vm.render_mode = i!(0),
        303 => vm.blend = BlendMode::from_id(i!(0)),
        304 => vm.layer = i!(0),
        305 => vm.z_write_disable = i!(0) != 0,
        306 | 307 | 311 | 312 | 313 | 314 | 315 | 316 | 317 | 318 | 319 => {} // no visual effect modelled yet
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
        // rotateAuto: a bullet's sprite turns to its direction of travel (th19.exe FUN_004f4990).
        424 => vm.auto_rotate = i!(0) != 0,
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
        437 => vm.rotation_mode = i!(0).clamp(0, 5) as u8,
        431 | 432 | 438 | 439 | 440 => {}
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
        508 if i!(0) == 1 => vm.charge = Some(Box::new(ChargeIn { timer_prev: -1, ..Default::default() })),
        508 => match Ribbon::new(i!(0)) {
            Some(r) => vm.effect = Some(r),
            None => *unknown.entry(508).or_default() += 1,
        },
        // copyParentVars: the whole variable block (th19.exe copies +0x460..+0x49c).
        509 => {
            if let Some((iv, fv)) = parent_vars {
                vm.ivars = iv;
                vm.fvars = fv;
            }
        }
        507 => {}

        // --- special drawing ---
        603 => vm.primitive = Some(Primitive::Rect { w: f!(0), h: f!(1), rotated: false, gradient: false }),
        606 => vm.primitive = Some(Primitive::Rect { w: f!(0), h: f!(1), rotated: false, gradient: true }),
        607 => vm.primitive = Some(Primitive::Rect { w: f!(0), h: f!(1), rotated: true, gradient: false }),
        608 => vm.primitive = Some(Primitive::Rect { w: f!(0), h: f!(1), rotated: true, gradient: true }),
        604 => vm.primitive = Some(Primitive::Poly { radius: f!(0), segments: i!(1), border: false }),
        605 => vm.primitive = Some(Primitive::Poly { radius: f!(0), segments: i!(1), border: true }),
        // 611, 615 and 616 all draw rings (draw types 0x13 / 0x1d / 0x1e -> FUN_004c3b60).
        611 | 615 | 616 => vm.primitive = Some(Primitive::Ring { radius: f!(0), thickness: f!(1), segments: i!(2) }),
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
    /// 2D: rotation about the screen normal. 3D: Euler angles, applied in `order`.
    rot: [f32; 3],
    /// Rotation order for 3D sprites (ANM `rotationMode`): 0 XYZ, 1 XZY, 2 YXZ, 3 YZX, 4 ZXY, 5 ZYX.
    order: u8,
    camera: Option<&'a Camera>,
    additive: bool,
    /// Stage fog applies to this sprite (only render modes 8 and 24 are fogged in th19).
    fogged: bool,
}

impl Placer<'_> {
    /// VM-local point to 3D world position. Rotations use Direct3D's left-handed
    /// conventions (`D3DXMatrixRotationX/Y/Z`, row vectors) in the order given by `order`.
    fn to_world(&self, x: f32, y: f32) -> [f32; 3] {
        self.to_world3([x, y, 0.0])
    }

    /// VM-local 3D point to world position (see [`Self::to_world`]).
    fn to_world3(&self, p: [f32; 3]) -> [f32; 3] {
        let rot_x = |[x, y, z]: [f32; 3], a: f32| {
            let (s, c) = a.sin_cos();
            [x, y * c - z * s, y * s + z * c]
        };
        let rot_y = |[x, y, z]: [f32; 3], a: f32| {
            let (s, c) = a.sin_cos();
            [x * c + z * s, y, -x * s + z * c]
        };
        let rot_z = |[x, y, z]: [f32; 3], a: f32| {
            let (s, c) = a.sin_cos();
            [x * c - y * s, x * s + y * c, z]
        };
        let [rx, ry, rz] = self.rot;
        let p = match self.order {
            1 => rot_y(rot_z(rot_x(p, rx), rz), ry),
            2 => rot_z(rot_x(rot_y(p, ry), rx), rz),
            3 => rot_x(rot_z(rot_y(p, ry), rz), rx),
            4 => rot_y(rot_x(rot_z(p, rz), rx), ry),
            5 => rot_x(rot_y(rot_z(p, rz), ry), rx),
            _ => rot_z(rot_y(rot_x(p, rx), ry), rz),
        };
        [self.origin[0] + p[0], self.origin[1] + p[1], self.origin[2] + p[2]]
    }

    fn place_2d(&self, x: f32, y: f32, uv: [f32; 2], color: [f32; 4]) -> Vertex {
        let (s, c) = self.rot[2].sin_cos();
        let pos = [self.origin[0] + x * c - y * s, self.origin[1] + x * s + y * c];
        Vertex { pos, uv, color, w: 1.0, fog: 0.0, depth: 0.0 }
    }

    fn finish_3d(&self, cam: &Camera, v: &Clipped) -> Vertex {
        let (pos, w, fog) = cam.to_screen(v.clip);
        let fog = if self.fogged { fog } else { 0.0 };
        // Fogging an additive sprite towards a colour would brighten it; fade it out instead.
        let (color, fog) = if self.additive {
            let k = 1.0 - fog;
            ([v.color[0] * k, v.color[1] * k, v.color[2] * k, v.color[3]], 0.0)
        } else {
            (v.color, fog)
        };
        Vertex { pos, uv: v.uv, color, w, fog, depth: v.clip[2] / v.clip[3] }
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
        // Corners in order: top-left, top-right, bottom-right, bottom-left (in sprite space).
        // Split along the same diagonal in *world* space whether or not the sprite is mirrored,
        // so coplanar copies of a quad (e.g. stage 2's doubled water, one flipped) get
        // bit-identical depths and the later one reliably wins the depth test.
        // The diagonal always starts at the corner that is smallest in world coordinates, which
        // covers flips by negative scale and by rotation alike.
        let key = |i: usize| -> [f32; 3] {
            if self.camera.is_some() {
                self.to_world(p[i].0, p[i].1)
            } else {
                let q = self.place_2d(p[i].0, p[i].1, [0.0; 2], [0.0; 4]).pos;
                [q[0], q[1], 0.0]
            }
        };
        let first = (0..4)
            .min_by(|&i, &j| key(i).partial_cmp(&key(j)).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(0);
        let [a, b, c, d] = [first, (first + 1) % 4, (first + 2) % 4, (first + 3) % 4];
        let v = |i: usize| (p[i].0, p[i].1, uv[i], col[i]);
        self.tri(out, [v(a), v(b), v(c)]);
        self.tri(out, [v(a), v(c), v(d)]);
    }
}

/// Sprites are drawn scaled by their creating VM's scale too (th19.exe, sprite vertex setup
/// around 0x4c8a17: own scale * scale2 * parent's scale * scale2).
fn parent_scale(world: &World, vm: &Vm) -> [f32; 2] {
    match vm.parent.and_then(|p| world.get(p)) {
        Some(p) => [p.scale[0] * p.scale2[0], p.scale[1] * p.scale2[1]],
        None => [1.0, 1.0],
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
        // Drawn at the world rotation: own plus the ancestors' (th19.exe sprite vertex
        // setup, FUN_00404ae0), so child sprites turn with their parent.
        [0.0, 0.0, vm.rot[2] + vm.inherited_rot]
    };
    let pl = Placer {
        origin: world_pos(world, vm),
        rot,
        order: vm.rotation_mode,
        camera,
        additive: matches!(vm.blend, BlendMode::Add | BlendMode::Max),
        // th19.exe enables fog while drawing stage sprites only for render modes 8 and 24.
        fogged: matches!(vm.render_mode, 8 | 24),
    };
    let z = [0.0f32; 2];

    let ps = parent_scale(world, vm);
    if let Some(prim) = vm.primitive {
        let sx = vm.scale[0] * vm.scale2[0] * ps[0];
        let sy = vm.scale[1] * vm.scale2[1] * ps[1];
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
                    // Inner edge in colour 1, outer edge in colour 2 (FUN_004c3b60).
                    pl.quad(out, [a0, b0, b1, a1], [z; 4], [c1, c2, c2, c1]);
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

    // Ribbon effects: a fogged 3D strip textured with the sprite, drawn in place of it.
    if let (Some(r), Some(cam)) = (&vm.effect, camera) {
        let pl = Placer { rot: vm.rot, fogged: true, ..pl };
        let [u0, v0, u1, v1] = sprite.uv;
        let pts: Vec<Clipped> = r
            .vertices()
            .into_iter()
            .map(|(p, u, v, a)| Clipped {
                clip: cam.clip(pl.to_world3(p)),
                uv: [u0 + u * (u1 - u0) + vm.uv_scroll[0], v0 + v * (v1 - v0) + vm.uv_scroll[1]],
                color: [1.0, 1.0, 1.0, a / 255.0],
            })
            .collect();
        for k in 0..pts.len().saturating_sub(3) / 2 + 1 {
            let (a, b, c, d) = (pts[2 * k], pts[2 * k + 1], pts[2 * k + 2], pts[2 * k + 3]);
            for tri in [[a, b, c], [b, d, c]] {
                let clipped = clip_near(&tri, Camera::NEAR);
                for i in 1..clipped.len().saturating_sub(1) {
                    for v in [&clipped[0], &clipped[i], &clipped[i + 1]] {
                        out.push(pl.finish_3d(cam, v));
                    }
                }
            }
        }
        return Some(sprite.texture);
    }

    // As in th19.exe, a stage quad's size only applies to render modes above 3.
    let base = match vm.size_override {
        Some(o) if vm.render_mode > 3 => [
            if o[0] != 0.0 { o[0] } else { sprite.size[0] },
            if o[1] != 0.0 { o[1] } else { sprite.size[1] },
        ],
        _ => sprite.size,
    };
    let w = base[0] * vm.scale[0] * vm.scale2[0] * ps[0];
    let h = base[1] * vm.scale[1] * vm.scale2[1] * ps[1];
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

    // Render modes 4 and 6: camera-facing billboards. The sprite's 3D position is projected
    // to the screen and a screen-aligned quad is drawn there, sized by how many pixels one
    // world unit covers at that depth and rotated only about the screen axis.
    // Mode 6 additionally fogs itself: past the fog start its colour blends toward the fog
    // colour and its alpha fades with the cube of the fog fraction; past the fog end it is
    // not drawn.
    if let (Some(cam), 4 | 6) = (camera, vm.render_mode) {
        let centre = pl.origin;
        let c = cam.clip(centre);
        if c[3] < Camera::NEAR || !(0.0..=1.0).contains(&(c[2] / c[3])) {
            return None;
        }
        let (p, _, _) = cam.to_screen(c);
        let edge = [centre[0] + cam.right[0], centre[1] + cam.right[1], centre[2] + cam.right[2]];
        let (q, _, _) = cam.to_screen(cam.clip(edge));
        let px_per_unit = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
        // th19.exe sizes billboards at half the projected scale (it projects into the same
        // 640x480 space used here).
        let (sx, sy) = (px_per_unit * 0.5, px_per_unit * 0.5);
        let (s, cz) = vm.rot[2].sin_cos();
        let depth = c[2] / c[3];
        let fogged = |col: [f32; 4]| -> Option<[f32; 4]> {
            if vm.render_mode != 6 {
                return Some(col);
            }
            let d = centre.iter().zip(&cam.eye).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt();
            if d <= cam.fog_near || cam.fog_far <= cam.fog_near {
                return Some(col);
            }
            let t = (d - cam.fog_near) / (cam.fog_far - cam.fog_near);
            if t >= 1.0 {
                return None;
            }
            let f = cam.fog_color;
            Some([
                col[0] + (f[0] - col[0]) * t,
                col[1] + (f[1] - col[1]) * t,
                col[2] + (f[2] - col[2]) * t,
                col[3] * (1.0 - t * t * t),
            ])
        };
        let (tl, tr, br, bl) = (fogged(tl)?, fogged(tr)?, fogged(br)?, fogged(bl)?);
        let corner = |x: f32, y: f32, uv: [f32; 2], color: [f32; 4]| {
            let (x, y) = (x * sx, y * sy);
            Vertex { pos: [p[0] + x * cz - y * s, p[1] + x * s + y * cz], uv, color, w: 1.0, fog: 0.0, depth }
        };
        let v = [
            corner(ax, ay, [u0, v0], tl),
            corner(ax + w, ay, [u1, v0], tr),
            corner(ax + w, ay + h, [u1, v1], br),
            corner(ax, ay + h, [u0, v1], bl),
        ];
        out.extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
        return Some(sprite.texture);
    }

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
