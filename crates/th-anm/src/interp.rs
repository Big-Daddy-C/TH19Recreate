//! The shared interpolator used by ANM (and later ECL/STD) "*Time" instructions.
//!
//! Mode numbering and formulas follow the community documentation of the
//! modern (TH13+) games; see docs/anm.md.

use std::f32::consts::FRAC_PI_2;

/// Easing curve for `x` in `[0, 1]`. Modes 7, 8 and 17 are handled by [`Interp`].
pub fn ease(mode: i32, x: f32) -> f32 {
    let flip = |f: &dyn Fn(f32) -> f32, x: f32| 1.0 - f(1.0 - x);
    let split = |f: &dyn Fn(f32) -> f32, g: &dyn Fn(f32) -> f32, x: f32| {
        if x < 0.5 {
            0.5 * f(2.0 * x)
        } else {
            0.5 + 0.5 * g(2.0 * x - 1.0)
        }
    };
    let in2 = |x: f32| x * x;
    let in3 = |x: f32| x * x * x;
    let in4 = |x: f32| x * x * x * x;
    let out2 = |x: f32| flip(&in2, x);
    let out3 = |x: f32| flip(&in3, x);
    let out4 = |x: f32| flip(&in4, x);
    let out_sin = |x: f32| (x * FRAC_PI_2).sin();
    let in_sin = |x: f32| flip(&out_sin, x);
    let back = |t: f32, x: f32| {
        // Parabola through (0,0) and (1,1) with its extremum at x = t.
        let a = (1.0 - t) * (1.0 - t);
        let c = t * t / a;
        (((x - t) * (x - t)) / a - c) / (1.0 - c)
    };
    match mode {
        0 => x,
        1 => in2(x),
        2 => in3(x),
        3 => in4(x),
        4 => out2(x),
        5 => out3(x),
        6 => out4(x),
        8 => x * x * (3.0 - 2.0 * x),
        9 => split(&in2, &out2, x),
        10 => split(&in3, &out3, x),
        11 => split(&in4, &out4, x),
        12 => split(&out2, &in2, x),
        13 => split(&out3, &in3, x),
        14 => split(&out4, &in4, x),
        15 => (x >= 1.0) as i32 as f32,
        16 => (x > 0.0) as i32 as f32,
        18 => out_sin(x),
        19 => in_sin(x),
        20 => split(&out_sin, &in_sin, x),
        21 => split(&in_sin, &out_sin, x),
        22..=26 => back([0.25, 0.3, 0.35, 0.38, 0.4][(mode - 22) as usize], x),
        27..=31 => 1.0 - back([0.25, 0.3, 0.35, 0.38, 0.4][(mode - 27) as usize], 1.0 - x),
        _ => x,
    }
}

/// A small fixed-size float vector, so one interpolator type covers scalars, 2D and 3D values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct V<const N: usize>(pub [f32; N]);

impl<const N: usize> Default for V<N> {
    fn default() -> Self {
        V([0.0; N])
    }
}

impl<const N: usize> V<N> {
    fn zip(self, o: Self, f: impl Fn(f32, f32) -> f32) -> Self {
        let mut r = self.0;
        for i in 0..N {
            r[i] = f(self.0[i], o.0[i]);
        }
        V(r)
    }
    fn scale(self, k: f32) -> Self {
        V(self.0.map(|v| v * k))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Interp<const N: usize> {
    pub initial: V<N>,
    pub goal: V<N>,
    pub extra1: V<N>,
    pub extra2: V<N>,
    pub time: i32,
    pub end_time: i32,
    pub mode: i32,
    pub active: bool,
}

impl<const N: usize> Default for Interp<N> {
    fn default() -> Self {
        Self {
            initial: V::default(),
            goal: V::default(),
            extra1: V::default(),
            extra2: V::default(),
            time: 0,
            end_time: 0,
            mode: 0,
            active: false,
        }
    }
}

impl<const N: usize> Interp<N> {
    pub fn start(&mut self, current: [f32; N], goal: [f32; N], duration: i32, mode: i32) {
        *self = Self {
            initial: V(current),
            goal: V(goal),
            extra1: V::default(),
            extra2: V::default(),
            time: 0,
            end_time: duration,
            mode,
            active: true,
        };
    }

    /// Cubic Hermite curve: `initial -> goal` with tangents `extra1`, `extra2`.
    pub fn start_bezier(&mut self, current: [f32; N], p1: [f32; N], p2: [f32; N], p3: [f32; N], duration: i32) {
        self.start(current, p2, duration, 8);
        self.extra1 = V(p1);
        self.extra2 = V(p3);
    }

    /// Advance one frame. Returns the new value while active.
    pub fn step(&mut self) -> Option<[f32; N]> {
        if !self.active {
            return None;
        }
        self.time += 1;
        let done = self.end_time >= 0 && self.time >= self.end_time;
        let value = match self.mode {
            7 => {
                self.initial = self.initial.zip(self.goal, |a, b| a + b);
                self.initial
            }
            17 => {
                self.initial = self.initial.zip(self.extra2, |a, b| a + b);
                self.extra2 = self.extra2.zip(self.goal, |a, b| a + b);
                self.initial
            }
            _ if done => self.goal,
            8 => {
                let t = self.time as f32 / self.end_time as f32;
                let (t2, t3) = (t * t, t * t * t);
                let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
                let h10 = t3 - 2.0 * t2 + t;
                let h01 = -2.0 * t3 + 3.0 * t2;
                let h11 = t3 - t2;
                self.initial
                    .scale(h00)
                    .zip(self.extra1.scale(h10), |a, b| a + b)
                    .zip(self.goal.scale(h01), |a, b| a + b)
                    .zip(self.extra2.scale(h11), |a, b| a + b)
            }
            m => {
                let k = ease(m, self.time as f32 / self.end_time as f32);
                self.initial.zip(self.goal, |a, b| a + (b - a) * k)
            }
        };
        if done {
            self.active = false;
        }
        Some(value.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints() {
        for m in (0..=31).filter(|m| ![7, 15, 16, 17].contains(m)) {
            assert!(ease(m, 0.0).abs() < 1e-4, "mode {m} at 0 = {}", ease(m, 0.0));
            assert!((ease(m, 1.0) - 1.0).abs() < 1e-4, "mode {m} at 1 = {}", ease(m, 1.0));
        }
    }

    #[test]
    fn back_extremum() {
        // Documented extremum of mode 26: f(0.4) = -0.8
        assert!((ease(26, 0.4) + 0.8).abs() < 1e-3);
        assert!((ease(31, 0.6) - 1.8).abs() < 1e-3);
    }

    #[test]
    fn linear_reaches_goal() {
        let mut i = Interp::<1>::default();
        i.start([0.0], [10.0], 5, 0);
        let vals: Vec<f32> = std::iter::from_fn(|| i.step().map(|v| v[0])).collect();
        assert_eq!(vals, vec![2.0, 4.0, 6.0, 8.0, 10.0]);
    }
}
