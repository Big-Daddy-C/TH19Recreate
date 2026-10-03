//! Reimu's C2 skill: EXReigekiInf (0053b920/0053b3a0), using pl00.anm script20.
use crate::enemy::field_to_canvas;
use th_anm::{interp::Interp, AnmLibrary, VmId, World};

pub struct DamageCircle {
    pub pos: [f32; 2],
    pub radius: f32,
    pub damage: i32,
    pub remaining: i32,
    pub growth: f32,
    /// A full-height column of this size centred on `pos` (Marisa's bomb) instead of a circle.
    pub rect: Option<[f32; 2]>,
}
pub struct Skill {
    pub time: i32,
    center: [f32; 2],
    phase: f32,
    radius: f32,
    interp: Interp<1>,
    vms: [VmId; 3],
}
impl Skill {
    pub fn new(
        pos: [f32; 2],
        phase: f32,
        file: usize,
        world: &mut World,
        lib: &AnmLibrary,
        field: [f32; 4],
    ) -> Self {
        let mut interp = Interp::default();
        interp.start([0.0], [64.0], 22, 1);
        Self {
            time: 0,
            center: pos,
            phase,
            radius: 0.0,
            interp,
            vms: std::array::from_fn(|_| {
                world.spawn_clipped(lib, file, 20, field_to_canvas(field, pos), field)
            }),
        }
    }
    pub fn tick(
        &mut self,
        world: &mut World,
        field: [f32; 4],
        cancel: &mut Vec<([f32; 2], f32)>,
        damage: &mut Vec<DamageCircle>,
    ) -> bool {
        if self.time >= 105 {
            for id in self.vms {
                world.delete(id);
            }
            return false;
        }
        if self.time == 22 {
            self.interp.start([64.0], [0.0], 22, 1);
        }
        if self.time == 45 {
            for id in self.vms {
                world.interrupt(id, 1);
            }
        }
        if let Some([r]) = self.interp.step() {
            self.radius = r;
        }
        let points: [[f32; 2]; 3] = std::array::from_fn(|i| {
            let a = self.phase + i as f32 * std::f32::consts::TAU / 3.0;
            [
                self.center[0] + a.cos() * self.radius,
                self.center[1] + a.sin() * self.radius,
            ]
        });
        for (id, pos) in self.vms.into_iter().zip(points) {
            if let Some(vm) = world.get_mut(id) {
                let p = field_to_canvas(field, pos);
                vm.origin = [p[0], p[1], 0.0];
            }
        }
        self.phase += std::f32::consts::PI / 18.0;
        if self.time < 45 {
            for pos in points {
                cancel.push((pos, 64.0));
                if self.time % 5 == 0 {
                    damage.push(DamageCircle {
                        pos,
                        radius: 64.0,
                        damage: 4,
                        remaining: 1,
                        growth: 0.0, rect: None,
                    });
                }
            }
        } else {
            cancel.push((points[0], 128.0 + ((self.time - 45) * 64 / 60) as f32));
            damage.push(DamageCircle {
                pos: points[0],
                radius: 64.0,
                damage: 1,
                remaining: 24,
                growth: 1.0, rect: None,
            });
        }
        self.time += 1;
        true
    }
}
