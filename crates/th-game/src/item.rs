//! Items (`ItemInf`, `FUN_00521d60` spawn, `FUN_00520d40` update). Only the meter orbs (item
//! types 9..=14) are ported: collecting one adds `rate * 15 / 100` to the meter, 15 at the
//! default rate of 100. Other item types are not ported.
use std::f32::consts::FRAC_PI_2;

use th_anm::{AnmLibrary, VmId, World};

use crate::enemy::field_to_canvas;
use crate::player::{Player, State};
use crate::side::SideState;

/// `0x59afc0`: the bullet.anm script that shows each item type (types 9..=14).
const IDLE_SCRIPT: [i32; 6] = [123, 124, 125, 126, 127, 128];
/// Player defaults (`FUN_00549250`): +0x20b0 homing speed, +0x20b4 pickup radius, +0x20b8
/// attraction radius, +0x20c0 the line above which every item homes in.
const HOMING_SPEED: f32 = 5.0;
const PICKUP_RADIUS: f32 = 30.0;
const ATTRACT_RADIUS: f32 = 70.0;
const COLLECT_LINE: f32 = 128.0;
/// Meter per orb at the default gain rate of 100% (`rate * 0xf / 100`).
pub const ORB_METER: i32 = 15;
/// Default orb rate (`+0xff8e8`): 100 per call is one orb.
pub const DEFAULT_RATE: i32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Spawn delay (state 5).
    Wait,
    /// Thrown up, slowing (state 2).
    Rising,
    /// Drifting down (state 1).
    Falling,
    /// Homing on the player (state 3).
    Homing,
    /// Pulled by the player coming near (state 4).
    Attracted,
}

pub struct Item {
    pub kind: i32,
    pub pos: [f32; 2],
    vel: [f32; 2],
    phase: Phase,
    delay: i32,
    speed: f32,
    vm: Option<VmId>,
}

/// Item bookkeeping of one side (`+0xff8d8`, `+0xff8e8`, `+0xff8ec`).
#[derive(Default)]
pub struct Items {
    pub list: Vec<Item>,
    /// Items spawned so far; its low two bits stagger the spawn delay.
    counter: i32,
    rate: i32,
    accumulator: i32,
    /// Sound requests from this tick (`FUN_00520d40` plays 0x25 at a collected item's x).
    pub sounds: Vec<(u32, i32)>,
}

impl Items {
    pub fn new() -> Self {
        Self { rate: DEFAULT_RATE, ..Default::default() }
    }

    /// `FUN_00521d60`, orb types 9..=14: an item thrown from `pos` at `angle` and `speed`.
    pub fn spawn(&mut self, kind: i32, pos: [f32; 2], angle: f32, speed: f32) {
        if !(9..=14).contains(&kind) {
            return;
        }
        self.counter = self.counter.wrapping_add(1);
        self.list.push(Item {
            kind,
            pos,
            vel: [angle.cos() * speed, angle.sin() * speed],
            phase: Phase::Wait,
            delay: self.counter.rem_euclid(4),
            speed: 0.0,
            vm: None,
        });
    }

    /// `FUN_00521c60`: orbs for one drop. The rate (100 = one orb) accumulates; each full 100
    /// throws an orb in a fan of +-10 degrees around straight up at speed 2.
    pub fn drop_orbs(&mut self, pos: [f32; 2], mut roll: impl FnMut() -> f32) {
        self.accumulator += self.rate;
        while self.accumulator >= 100 {
            self.spawn(9, pos, roll() * 0.17453292 - FRAC_PI_2, 2.0);
            self.accumulator -= 100;
        }
    }

    pub fn clear(&mut self, world: &mut World) {
        for i in self.list.drain(..) {
            if let Some(vm) = i.vm {
                world.delete(vm);
            }
        }
    }

    /// `FUN_00520d40`: one frame. Returns the number of orbs the player collected.
    pub fn update(&mut self, players: &[&Player], st: &SideState, world: &mut World, lib: &AnmLibrary) -> Vec<i32> {
        let mut collected = vec![0; players.len().max(1)];
        let file = st.anm_files[0];
        self.list.retain_mut(|it| {
            if it.phase == Phase::Wait {
                it.delay -= 1;
                if it.delay < 0 {
                    it.phase = Phase::Rising;
                    if let (Some(file), Some(&script)) = (file, IDLE_SCRIPT.get((it.kind - 9) as usize)) {
                        let at = field_to_canvas(st.field, it.pos);
                        it.vm = Some(world.spawn_clipped(lib, file, script as usize, at, st.field));
                    }
                }
                return true;
            }
            // Each item follows the living player nearest to it.
            let near = players.iter().enumerate().min_by(|a, b| {
                let key = |p: &Player| (matches!(p.state, State::Dead), (p.pos[0] - it.pos[0]).powi(2) + (p.pos[1] - it.pos[1]).powi(2));
                key(a.1).partial_cmp(&key(b.1)).unwrap_or(std::cmp::Ordering::Equal)
            });
            let idx = near.map_or(0, |(i, _)| i);
            let (ppos, pstate) = near.map_or(([0.0, 1000.0], State::Dead), |(_, p)| (p.pos, p.state));
            let present = near.is_some();
            let alive = !matches!(pstate, State::Dead | State::Hit);
            let mut gone = false;
            let homing_now = present && alive && ppos[1] <= COLLECT_LINE;
            match it.phase {
                Phase::Rising => {
                    it.pos[0] += it.vel[0];
                    it.pos[1] += it.vel[1];
                    it.vel[1] += 0.03;
                    if it.vel[1] >= 0.0 {
                        it.phase = Phase::Falling;
                    }
                    gone = it.pos[1] > 472.0 || it.pos[0].abs() >= 156.0;
                }
                Phase::Falling => {
                    if homing_now {
                        it.phase = Phase::Homing;
                        it.speed = HOMING_SPEED;
                        home(it, ppos);
                    } else {
                        it.pos[0] += it.vel[0];
                        it.pos[1] += it.vel[1];
                        it.vel[1] += 0.03;
                        if it.vel[1] >= 0.0 {
                            it.vel[0] = 0.0;
                        }
                        it.vel[1] = it.vel[1].min(2.0);
                        gone = it.pos[1] > 472.0 || it.pos[0].abs() >= 156.0;
                    }
                }
                Phase::Homing | Phase::Attracted => {
                    if it.phase == Phase::Attracted && homing_now {
                        it.phase = Phase::Homing;
                        it.speed = HOMING_SPEED;
                    }
                    home(it, ppos);
                    it.pos[0] += it.vel[0];
                    it.pos[1] += it.vel[1];
                    if it.speed < 12.0 {
                        it.speed += 0.2;
                    }
                    if pstate == State::Hit {
                        it.vel = [0.0; 2];
                        it.phase = Phase::Falling;
                    }
                }
                Phase::Wait => {}
            }
            if !gone && present && pstate != State::Dead {
                let d2 = (ppos[0] - it.pos[0]).powi(2) + (ppos[1] - it.pos[1]).powi(2);
                if d2 < PICKUP_RADIUS * PICKUP_RADIUS {
                    collected[idx] += 1;
                    if crate::sfx::on(crate::sfx::Cat::Item) { self.sounds.push((0x25, (it.pos[0] * 1000.0 / 192.0) as i32)); }
                    gone = true;
                } else if !matches!(it.phase, Phase::Attracted | Phase::Homing | Phase::Rising)
                    && d2 < ATTRACT_RADIUS * ATTRACT_RADIUS
                {
                    it.phase = Phase::Attracted;
                    it.speed = HOMING_SPEED / 3.0;
                }
            }
            if gone {
                if let Some(vm) = it.vm {
                    world.delete(vm);
                }
                return false;
            }
            if let Some(v) = it.vm.and_then(|id| world.get_mut(id)) {
                let at = field_to_canvas(st.field, it.pos);
                v.origin = [at[0], at[1], 0.0];
            }
            true
        });
        collected
    }
}

/// Aim the item's velocity at the player at its current speed.
fn home(it: &mut Item, target: [f32; 2]) {
    let (dx, dy) = (target[0] - it.pos[0], target[1] - it.pos[1]);
    let angle = if dx == 0.0 && dy == 0.0 { FRAC_PI_2 } else { dy.atan2(dx) };
    it.vel = [angle.cos() * it.speed, angle.sin() * it.speed];
}

/// Meter gained by `orbs` collected orbs (`rate * 0xf / 100`, rate 100).
pub fn meter_for(orbs: i32) -> i32 {
    orbs * ORB_METER
}
