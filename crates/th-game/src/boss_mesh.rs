//! ECL 621's boss-area scene mesh (FUN_005092a0, FUN_005063b0).
use th_anm::{BlendMode, DepthMode, DrawBatch, Vertex, SCENE_TEXTURE};

#[derive(Clone, Debug)]
pub struct BossMesh {
    pub target_radius: f32,
    pub color: u32,
    radius: f32,
    phase: [f32; 2],
    draw_radius: f32,
    draw_phase: [f32; 2],
}

impl BossMesh {
    pub fn new(target_radius: f32, color: u32) -> Self {
        Self {
            target_radius,
            color,
            radius: 16.0,
            phase: [0.0; 2],
            draw_radius: 16.0,
            draw_phase: [0.0; 2],
        }
    }

    pub fn tick(&mut self) {
        self.draw_radius = self.radius;
        self.draw_phase = self.phase;
        if self.radius < self.target_radius {
            self.radius += 2.0;
        }
        self.phase[0] = (self.phase[0] + std::f32::consts::PI / 16.0) % std::f32::consts::TAU;
        self.phase[1] = (self.phase[1] + std::f32::consts::PI / 32.0) % std::f32::consts::TAU;
    }

    pub fn batch(&self, center: [f32; 2], field: [f32; 4]) -> DrawBatch {
        // Handler 00504248 passes 17x17. 00509314 sets a square of side 2r+40,
        // starting at the boss minus r+20; UVs sample the unwarped scene position.
        let r = self.draw_radius;
        let size = r * 2.0 + 40.0;
        let mut grid = Vec::with_capacity(17 * 17);
        let [mut px, mut py] = self.draw_phase;
        let rgb = [
            (self.color >> 16) & 255,
            (self.color >> 8) & 255,
            self.color & 255,
        ];
        for i in 0..17 {
            for j in 0..17 {
                let rest = [
                    center[0] - r - 20.0 + size * i as f32 / 16.0,
                    center[1] - r - 20.0 + size * j as f32 / 16.0,
                ];
                let d = [rest[0] - center[0], rest[1] - center[1]];
                let d2 = d[0] * d[0] + d[1] * d[1];
                let inside = d2 <= r * r;
                let weight = (1.0 - d2 / (r * r)).max(0.0);
                let length = d2.sqrt();
                let radial = if length >= 0.01 {
                    [d[0] / length, d[1] / length]
                } else {
                    d
                };
                let mut color = [1.0; 4];
                for k in 0..3 {
                    // Original integer conversion truncates the interpolated channel.
                    color[k] = (255.0 - (255 - rgb[k]) as f32 * weight) as u8 as f32 / 255.0;
                }
                color[3] = if inside { 1.0 } else { 0.0 };
                grid.push(Vertex {
                    pos: [
                        rest[0] + weight * (32.0 * radial[0] + 8.0 * px.sin()),
                        rest[1] + weight * (32.0 * radial[1] + 8.0 * py.sin()),
                    ],
                    uv: [
                        rest[0].clamp(1.0, 639.0) / 640.0,
                        rest[1].clamp(1.0, 479.0) / 480.0,
                    ],
                    color,
                    w: 1.0,
                    fog: 0.0,
                    depth: 0.0,
                });
                // Both phases advance per vertex, including vertices outside the circle.
                px += std::f32::consts::PI / 32.0;
                py -= std::f32::consts::PI / 64.0;
            }
        }
        let mut vertices = Vec::with_capacity(16 * 16 * 6);
        for i in 0..16 {
            for j in 0..16 {
                let a = grid[i * 17 + j];
                let b = grid[(i + 1) * 17 + j];
                let c = grid[(i + 1) * 17 + j + 1];
                let d = grid[i * 17 + j + 1];
                vertices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        DrawBatch {
            texture: Some(SCENE_TEXTURE),
            blend: BlendMode::Normal,
            depth: DepthMode::Off,
            layer: 39,
            clip: Some(field),
            vertices,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn center_uses_script_color_edges_are_clear_and_wobble_moves() {
        let mut mesh = BossMesh::new(160.0, 0x00f00f80);
        for _ in 0..80 {
            mesh.tick();
        }
        let field = [16.0, 16.0, 296.0, 448.0];
        let before = mesh.batch([164.0, 160.0], field);
        let center = before
            .vertices
            .iter()
            .find(|v| v.uv == [164.0 / 640.0, 160.0 / 480.0])
            .unwrap();
        assert_eq!(
            center.color,
            [240.0 / 255.0, 15.0 / 255.0, 128.0 / 255.0, 1.0]
        );
        assert!(before.vertices.iter().any(|v| v.color[3] == 0.0));
        assert_eq!(before.clip, Some(field));
        mesh.tick();
        let after = mesh.batch([164.0, 160.0], field);
        assert!(before
            .vertices
            .iter()
            .zip(&after.vertices)
            .any(|(a, b)| a.pos != b.pos));
    }
}
