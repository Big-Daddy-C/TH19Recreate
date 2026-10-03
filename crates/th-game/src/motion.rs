//! Enemy movement: the position/velocity records driven by ECL movement instructions.
//!
//! Reimplements th19.exe's PosVel (`FUN_004d4dc0` computes the step, `FUN_004d4a00` applies
//! it): a record is in one of three modes —
//! * velocity: `vel = (cos angle, sin angle) * speed`, `pos += vel`;
//! * circle: `angle += speed`, `radius += radius_delta`, `pos = center + polar(angle, radius)`;
//! * ellipse: as circle, with the x axis scaled by `ratio` and the shape rotated by `ell_angle`.
//!
//! Positions are rounded to 1/100 after every step, like the original.

use th_anm::interp::Interp;

/// "Leave unchanged" marker used by movement instruction arguments.
pub const KEEP: f32 = -999999.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Velocity,
    Circle,
    Ellipse,
}

#[derive(Clone, Debug, Default)]
pub struct PosVel {
    pub pos: [f32; 2],
    pub speed: f32,
    pub angle: f32,
    pub radius: f32,
    pub radius_delta: f32,
    pub ell_angle: f32,
    pub ell_ratio: f32,
    /// Velocity in [`Mode::Velocity`], circle centre otherwise (the game stores both here).
    pub vel_or_center: [f32; 2],
    pub mode: Mode,
    /// PosVel +0x34, applied by FUN_004d4dc0; scope writes 0.5.
    pub drag: f32,
}

pub fn wrap_angle(mut a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut n = 0;
    while a > PI && n <= 32 {
        a -= TAU;
        n += 1;
    }
    while a < -PI && n <= 64 {
        a += TAU;
        n += 1;
    }
    a
}

fn round_centi(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// One movement layer (absolute or relative) with its interpolators.
#[derive(Clone, Debug, Default)]
pub struct Motion {
    pub pv: PosVel,
    pub i_pos: Interp<2>,
    pub i_angle: Interp<1>,
    pub i_speed: Interp<1>,
    pub i_radius: Interp<2>,
}

impl Motion {
    /// Advance one frame.
    pub fn step(&mut self) {
        let circular = matches!(self.pv.mode, Mode::Circle | Mode::Ellipse);
        if let Some([a]) = self.i_angle.step() {
            if !circular {
                self.pv.angle = wrap_angle(a);
            }
        }
        if let Some([s]) = self.i_speed.step() {
            self.pv.speed = s;
        }
        if let Some([r, d]) = self.i_radius.step() {
            self.pv.radius = r;
            self.pv.radius_delta = d;
        }
        let pv = &mut self.pv;
        // Position interpolation drives the record through its velocity.
        if let Some(target) = self.i_pos.step() {
            pv.mode = Mode::Velocity;
            pv.vel_or_center = [target[0] - pv.pos[0], target[1] - pv.pos[1]];
        } else {
            match pv.mode {
                Mode::Velocity => {
                    let (s, c) = pv.angle.sin_cos();
                    pv.vel_or_center = [c * pv.speed * (1.0-pv.drag), s * pv.speed * (1.0-pv.drag)];
                }
                Mode::Circle | Mode::Ellipse => {
                    pv.radius += pv.radius_delta * (1.0-pv.drag);
                    pv.angle = wrap_angle(pv.angle + pv.speed * (1.0-pv.drag));
                }
            }
        }
        match pv.mode {
            Mode::Velocity => {
                pv.pos[0] += pv.vel_or_center[0];
                pv.pos[1] += pv.vel_or_center[1];
            }
            Mode::Circle => {
                let (s, c) = pv.angle.sin_cos();
                pv.pos = [pv.vel_or_center[0] + c * pv.radius, pv.vel_or_center[1] + s * pv.radius];
            }
            Mode::Ellipse => {
                let t = wrap_angle(pv.angle - pv.ell_angle);
                let (px, py) = (t.cos() * pv.radius * pv.ell_ratio, t.sin() * pv.radius);
                let (s, c) = pv.ell_angle.sin_cos();
                pv.pos = [
                    pv.vel_or_center[0] + c * px - s * py,
                    pv.vel_or_center[1] + s * px + c * py,
                ];
            }
        }
        pv.pos = [round_centi(pv.pos[0]), round_centi(pv.pos[1])];
    }

    /// Stop every kind of movement (`ins_427`).
    pub fn stop(&mut self) {
        self.pv.speed = 0.0;
        self.pv.radius_delta = 0.0;
        self.pv.mode = Mode::Velocity;
        self.pv.vel_or_center = [0.0; 2];
        self.i_pos.active = false;
        self.i_angle.active = false;
        self.i_speed.active = false;
        self.i_radius.active = false;
    }

    /// Switch to circular motion around the current position unless already circling.
    pub fn enter_circle(&mut self, mode: Mode) {
        if !matches!(self.pv.mode, Mode::Circle | Mode::Ellipse) {
            self.pv.vel_or_center = self.pv.pos;
        }
        self.pv.mode = mode;
    }
}
