//! Minimal 3D camera math for stage backgrounds (Direct3D-style left-handed conventions).

pub type Vec3 = [f32; 3];
/// Row-major 4x4 matrix, used as `row_vector * M` like Direct3D.
pub type Mat4 = [[f32; 4]; 4];

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(a: Vec3) -> Vec3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { a }
}

/// `D3DXMatrixLookAtLH`.
pub fn look_at_lh(eye: Vec3, at: Vec3, up: Vec3) -> Mat4 {
    let z = normalize(sub(at, eye));
    let x = normalize(cross(up, z));
    let y = cross(z, x);
    [
        [x[0], y[0], z[0], 0.0],
        [x[1], y[1], z[1], 0.0],
        [x[2], y[2], z[2], 0.0],
        [-dot(x, eye), -dot(y, eye), -dot(z, eye), 1.0],
    ]
}

/// `D3DXMatrixPerspectiveFovLH`.
pub fn perspective_fov_lh(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let ys = 1.0 / (fovy * 0.5).tan();
    let xs = ys / aspect;
    let q = far / (far - near);
    [[xs, 0.0, 0.0, 0.0], [0.0, ys, 0.0, 0.0], [0.0, 0.0, q, 1.0], [0.0, 0.0, -near * q, 0.0]]
}

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

pub fn transform(p: Vec3, m: &Mat4) -> [f32; 4] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut r = [0.0; 4];
    for (j, out) in r.iter_mut().enumerate() {
        *out = (0..4).map(|k| v[k] * m[k][j]).sum();
    }
    r
}

/// Everything the ANM renderer needs to draw 3D (stage) VMs.
#[derive(Clone, Debug)]
pub struct Camera {
    pub view_proj: Mat4,
    /// Destination rectangle on the 640x480 canvas: `[x, y, w, h]`.
    pub viewport: [f32; 4],
    pub fog_near: f32,
    pub fog_far: f32,
}

impl Camera {
    pub fn new(eye: Vec3, facing: Vec3, up: Vec3, fovy: f32, viewport: [f32; 4], fog: (f32, f32)) -> Self {
        let at = [eye[0] + facing[0], eye[1] + facing[1], eye[2] + facing[2]];
        let view = look_at_lh(eye, at, up);
        // Near/far planes as used by th19.exe's stage camera.
        let proj = perspective_fov_lh(fovy, viewport[2] / viewport[3], Self::NEAR, 8000.0);
        Self { view_proj: mul(&view, &proj), viewport, fog_near: fog.0, fog_far: fog.1 }
    }

    /// Minimum view depth; geometry closer than this is clipped away.
    pub const NEAR: f32 = 30.0;

    /// World point to homogeneous clip coordinates.
    pub fn clip(&self, p: Vec3) -> [f32; 4] {
        transform(p, &self.view_proj)
    }

    /// Clip coordinates (with `w >= NEAR`) to canvas `[x, y]`, clip `w` and fog factor.
    pub fn to_screen(&self, c: [f32; 4]) -> ([f32; 2], f32, f32) {
        let w = c[3];
        let [vx, vy, vw, vh] = self.viewport;
        let x = vx + (c[0] / w * 0.5 + 0.5) * vw;
        let y = vy + (0.5 - c[1] / w * 0.5) * vh;
        let fog = if self.fog_far > self.fog_near {
            ((w - self.fog_near) / (self.fog_far - self.fog_near)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ([x, y], w, fog)
    }

    /// Project a world point, or `None` if it is behind the near plane.
    pub fn project(&self, p: Vec3) -> Option<([f32; 2], f32, f32)> {
        let c = self.clip(p);
        (c[3] >= Self::NEAR).then(|| self.to_screen(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_projects_to_viewport_centre() {
        let cam = Camera::new([0.0, 0.0, -800.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 0.6, [0.0, 0.0, 640.0, 480.0], (0.0, 1.0));
        let (p, w, _) = cam.project([0.0, 0.0, 0.0]).unwrap();
        assert!((p[0] - 320.0).abs() < 1e-3 && (p[1] - 240.0).abs() < 1e-3);
        assert!((w - 800.0).abs() < 1e-3);
        // +x is to the right, +y is up on screen (left-handed, y-up view).
        let (p, _, _) = cam.project([100.0, 100.0, 0.0]).unwrap();
        assert!(p[0] > 320.0 && p[1] < 240.0);
    }
}
