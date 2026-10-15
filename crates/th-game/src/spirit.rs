//! GhostCtrl dispatch: 005450e0, 0051bb00, 0051b6c0, 0051b480, 0051d4e0.
//! NPC behavior and graphics come from default.ecl and enemy.anm.
use th_anm::{interp::Interp, AnmLibrary, VmId, World};
use crate::{enemy::field_to_canvas, side::{SideState, SpawnRequest}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Pure = 1, Wolf = 2, Otter = 3, Eagle = 4 }
impl Kind {
    pub fn from_distance(distance_squared: f32) -> Self {
        if distance_squared <= 68.0*68.0 { Self::Wolf }
        else if distance_squared <= 128.0*128.0 { Self::Otter }
        else { Self::Eagle }
    }
    fn sub(self, x: f32) -> &'static str {
        match self { Self::Pure => "GhostPure", Self::Wolf => "GhostWolf",
            Self::Otter if x < 0.0 => "GhostOtterR", Self::Otter => "GhostOtterL", Self::Eagle => "GhostEagle" }
    }
}
pub struct Request { pub kind: Kind, pub origin: [f32;2], pub level: i32 }
pub struct Pending {
    pub kind: Kind, pub target: [f32;2], pub level: i32,
    pub delay: i32, pub time: i32,
    curve: Interp<2>, vm: Option<VmId>, start: [f32;2],
}
impl Pending {
    pub fn delete(self, world: &mut World) { if let Some(id)=self.vm { world.delete(id); } }
    pub fn new(req: Request, source_field: [f32;4], receiver: &mut SideState) -> Self {
        let delay = receiver.rand16() as i32 % 120 + 60; // FUN_0051b6c0: FUN_00401960() % 0x78 + 0x3c
        let mut target = [receiver.randf2()*148.0, (receiver.randf()-1.0)*64.0];
        match req.kind {
            Kind::Wolf => target[0] = (receiver.player_pos[0]+receiver.randf2()*48.0).clamp(-140.0,140.0),
            Kind::Otter => {
                target[1] = receiver.randf2()*128.0+144.0;
                target[0] = receiver.randf2()*64.0-148.0;
                if receiver.rand() & 1 == 0 { target[0] = -target[0]; }
                target[0] = target[0].clamp(-140.0,140.0);
            },
            _ => {}
        }
        let start = field_to_canvas(source_field, req.origin);
        let end = field_to_canvas(receiver.field, target);
        let mut curve = Interp::default();
        curve.start_bezier(start,[receiver.randf2()*100.0,-400.0],end,
            [receiver.randf2()*100.0,120.0],delay);
        Self { kind: req.kind, target, level: req.level+1, delay, time: 0, curve, vm: None, start }
    }
    pub fn tick(&mut self, ghost: Option<usize>, world: &mut World, lib: &AnmLibrary) -> Option<SpawnRequest> {
        if self.time == 0 {
            self.vm = ghost.map(|file| world.spawn(lib,file,19+3*(self.kind as usize-1),self.start));
        }
        self.time += 1;
        if let Some(pos) = self.curve.step() {
            if let Some(vm) = self.vm.and_then(|id| world.get_mut(id)) { vm.origin=[pos[0],pos[1],0.0]; }
        }
        if self.time <= self.delay { return None; }
        if let Some(id) = self.vm { world.interrupt(id,1); }
        Some(SpawnRequest { id:0,difficulty:0,sub:self.kind.sub(self.target[0]).into(),pos:self.target,
            hp:200,score:0,item:-1,mirrored:false,ivars:[self.level,0,self.level,0],fvars:[0.0;8],owner:None })
    }
}

/// Counter +0x54 is separate from the charge meter. Default pressure=0 gives120;
/// pressure/ability settings and replay RNG remain separate unported systems.
pub fn send_earned(st: &mut SideState) {
    let threshold = 120-(st.spirit_pressure.clamp(0,200) as f32/200.0*40.0) as i32;
    let limit = match st.spirit_frames { 0..3600=>40,3600..7200=>60,7200..10800=>80,_=>999 };
    while st.spirit_credit >= threshold {
        st.spirit_credit -= threshold;
        let maximum = st.spirit_pressure as f32/200.0*7.0+5.0;
        let pending = st.sent_spirits_pending+st.outgoing_spirits.len();
        if pending < limit && (pending as f32) < maximum {
            st.outgoing_spirits.push(Request { kind:Kind::Pure, origin:st.spirit_origin,level:0 });
        }
    }
    st.spirit_frames += 1;
}
