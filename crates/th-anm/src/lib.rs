//! ANM virtual machine: runs the sprite/animation scripts stored in `.anm` files
//! and turns them into textured triangles for a renderer.
//!
//! This crate knows nothing about the GPU. A renderer uploads [`AnmLibrary::textures`]
//! once, then each frame calls [`World::tick`] and draws [`World::draw_list`].

pub mod interp;
pub mod vm;

use std::collections::HashMap;

use th_formats::anm::{AnmFile, Script};

pub use vm::{BlendMode, DrawList, DrawBatch, Vertex, VmId, World};

/// A decoded RGBA8 texture owned by the library. Index into `AnmLibrary::textures` is the texture id.
pub struct TextureImage {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct SpriteDef {
    pub texture: u32,
    /// Normalised UV rectangle `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
    /// Size in game pixels.
    pub size: [f32; 2],
}

pub struct LoadedAnm {
    pub name: String,
    pub file: AnmFile,
    /// File-global sprite index -> sprite.
    pub sprites: Vec<SpriteDef>,
    /// File-global script index -> (entry, script) location.
    script_index: Vec<(usize, usize)>,
}

impl LoadedAnm {
    pub fn script(&self, id: usize) -> Option<&Script> {
        let &(e, s) = self.script_index.get(id)?;
        Some(&self.file.entries[e].scripts[s])
    }

    pub fn script_count(&self) -> usize {
        self.script_index.len()
    }
}

#[derive(Default)]
pub struct AnmLibrary {
    pub files: Vec<LoadedAnm>,
    pub textures: Vec<TextureImage>,
    by_name: HashMap<String, usize>,
}

#[derive(Debug)]
pub enum LoadError {
    Format(th_formats::Error),
    Image(String, image::ImageError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Format(e) => write!(f, "{e}"),
            LoadError::Image(n, e) => write!(f, "decoding texture {n}: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl AnmLibrary {
    pub fn file_id(&self, name: &str) -> Option<usize> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }

    /// Parse an `.anm` file and decode all of its textures. Returns the file id.
    pub fn load(&mut self, name: &str, bytes: &[u8]) -> Result<usize, LoadError> {
        if let Some(id) = self.file_id(name) {
            return Ok(id);
        }
        let file = AnmFile::parse(bytes).map_err(LoadError::Format)?;

        let mut sprites = Vec::new();
        let mut script_index = Vec::new();
        for (ei, entry) in file.entries.iter().enumerate() {
            let tex_id = match &entry.texture {
                Some(t) => {
                    let img = image::load_from_memory(&t.data)
                        .map_err(|e| LoadError::Image(entry.name.clone(), e))?
                        .to_rgba8();
                    self.textures.push(TextureImage {
                        name: entry.name.clone(),
                        width: img.width(),
                        height: img.height(),
                        rgba: img.into_raw(),
                    });
                    self.textures.len() as u32 - 1
                }
                // Entries without data reuse a texture loaded elsewhere (by name); fall back to "white".
                None => self
                    .textures
                    .iter()
                    .position(|t| t.name == entry.name)
                    .map_or(u32::MAX, |i| i as u32),
            };
            // TH19 embeds a cropped PNG: the header size is the padded canvas, but sprite
            // rectangles are in the PNG's own pixel space, so normalise against the real image.
            let (w, h) = match self.textures.get(tex_id as usize) {
                Some(t) => (t.width as f32, t.height as f32),
                None => (entry.width.max(1) as f32, entry.height.max(1) as f32),
            };
            // "lowres scale" images are authored at 2x the 640x480 game coordinate space.
            let px_to_game = if entry.lowres_scale { 0.5 } else { 1.0 };
            for s in &entry.sprites {
                let (x, y) = (s.x - entry.x as f32, s.y - entry.y as f32);
                let idx = s.id as usize;
                if sprites.len() <= idx {
                    sprites.resize(
                        idx + 1,
                        SpriteDef { texture: u32::MAX, uv: [0.0; 4], size: [0.0; 2] },
                    );
                }
                sprites[idx] = SpriteDef {
                    texture: tex_id,
                    uv: [x / w, y / h, (x + s.w) / w, (y + s.h) / h],
                    size: [s.w * px_to_game, s.h * px_to_game],
                };
            }
            for si in 0..entry.scripts.len() {
                script_index.push((ei, si));
            }
        }

        self.files.push(LoadedAnm { name: name.to_string(), file, sprites, script_index });
        let id = self.files.len() - 1;
        self.by_name.insert(name.to_ascii_lowercase(), id);
        Ok(id)
    }
}
