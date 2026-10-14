//! The game's run-time text surfaces (`FUN_004cd480` -> `FUN_004b2a50`): a string drawn with a
//! Windows font into a sprite's rectangle of a `text.anm` surface. th19.exe draws spell card
//! names with font slot 0 (`DAT_0062b09c`): 48 px Meiryo when the system has Meiryo (it
//! enumerates font families for "メイリオ", `FUN_004b3630`), otherwise 32 px MS Gothic; white
//! text over a black outline (the shadow colour drawn at the eight neighbouring offsets).
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};

pub struct TextFont {
    font: FontArc,
    /// GDI cell height of font slot 0 (CreateFontW height 0x30 Meiryo / 0x20 MS Gothic).
    px: f32,
    /// Meiryo surfaces are read 6 rows lower (`FUN_004b2a50`: source rect top += 6).
    skip_rows: u32,
}

impl TextFont {
    pub fn load() -> Option<Self> {
        let windir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        for (file, px, skip) in [("meiryo.ttc", 48.0, 6), ("msgothic.ttc", 32.0, 0)] {
            let Ok(bytes) = std::fs::read(windir.join("Fonts").join(file)) else { continue };
            if let Ok(font) = ab_glyph::FontVec::try_from_vec_and_index(bytes, 0) {
                return Some(Self { font: FontArc::new(font), px, skip_rows: skip });
            }
        }
        None
    }

    /// Render `text` into a `w` x `h` RGBA image the way `FUN_004b2a50` fills a sprite
    /// rectangle: drawn from (x + 2, 2), outline first, then the text; Meiryo is read from row 6.
    /// APPROX: the original starts at `x = sprite width - 16 * bytes`, which is laid out for the
    /// 32 px font; this right-aligns by the measured width instead.
    pub fn render(&self, text: &str, w: u32, h: u32) -> Vec<u8> {
        let scaled = self.font.as_scaled(PxScale::from(self.px));
        let width: f32 = text.chars().map(|c| scaled.h_advance(self.font.glyph_id(c))).sum();
        let x0 = (w as f32 - width - 4.0).max(0.0);
        let buf_h = h + self.skip_rows + 4;
        let mut cover_text = vec![0f32; (w * buf_h) as usize];
        let mut cover_edge = vec![0f32; (w * buf_h) as usize];
        let draw = |dx: f32, dy: f32, into: &mut Vec<f32>| {
            let mut x = x0 + dx;
            for c in text.chars() {
                let id = self.font.glyph_id(c);
                let glyph = id.with_scale_and_position(PxScale::from(self.px), ab_glyph::point(x, dy + scaled.ascent()));
                if let Some(outlined) = self.font.outline_glyph(glyph) {
                    let b = outlined.px_bounds();
                    outlined.draw(|gx, gy, v| {
                        let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                        if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < buf_h {
                            let i = (py as u32 * w + px as u32) as usize;
                            into[i] = into[i].max(v);
                        }
                    });
                }
                x += scaled.h_advance(id);
            }
        };
        for (dx, dy) in [(1.0, -1.0), (-1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (2.0, 0.0), (-2.0, 0.0), (0.0, -2.0), (0.0, 2.0)] {
            draw(2.0 + dx, 2.0 + dy, &mut cover_edge);
        }
        draw(2.0, 2.0, &mut cover_text);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let src = ((y + self.skip_rows) * w + x) as usize;
                let (t, e) = (cover_text[src], cover_edge[src]);
                let a = t.max(e);
                let v = (t * 255.0).round() as u8;
                let o = ((y * w + x) * 4) as usize;
                rgba[o..o + 4].copy_from_slice(&[v, v, v, (a * 255.0).round() as u8]);
            }
        }
        rgba
    }
}
