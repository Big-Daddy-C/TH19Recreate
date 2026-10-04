//! The game's text object (`AsciiInf`, `FUN_004e5850`) for the big proportional font (style 6:
//! glyph sprite = character code + 0x120 in `ascii.anm`'s `ascii_big` entry).
use th_anm::{AnmLibrary, VmId, World};

/// AnmManager draw-chain priority of the text object (`FUN_004d5bb0(.., 0x1f)`).
pub const DRAW_PRIORITY: i32 = 0x1f;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Draw `text` with its anchor at `pos` (canvas coordinates): `Right` ends at `pos`, `Left`
/// starts at it, and the glyphs sit on a baseline at `pos[1]` (vertical alignment 2: the box
/// top is half the line height times the scale above). Returns the glyph VMs.
pub fn big_text(
    world: &mut World,
    lib: &AnmLibrary,
    file: usize,
    text: &str,
    pos: [f32; 2],
    scale: f32,
    align: Align,
    color: [f32; 3],
) -> Vec<VmId> {
    big_text_at(world, lib, file, text, pos, scale, align, color, DRAW_PRIORITY)
}

/// [`big_text`] with an explicit draw priority.
#[allow(clippy::too_many_arguments)]
pub fn big_text_at(
    world: &mut World,
    lib: &AnmLibrary,
    file: usize,
    text: &str,
    pos: [f32; 2],
    scale: f32,
    align: Align,
    color: [f32; 3],
    priority: i32,
) -> Vec<VmId> {
    let glyph = |c: u8| 0x120 + c as i32;
    let advance = |sprite: i32| {
        lib.files[file].sprites.get(sprite as usize).map_or(0.0, |s| s.size[0]) * scale
    };
    let total: f32 = text.bytes().map(|c| advance(glyph(c))).sum();
    let mut x = match align {
        Align::Left => pos[0],
        Align::Right => pos[0] - total,
    };
    let y = pos[1] - 16.0 * scale;
    let mut out = Vec::new();
    for c in text.bytes() {
        let sprite = glyph(c);
        if c != b' ' {
            out.push(world.spawn_sprite(file, sprite, [x, y], [scale; 2], color, priority));
        }
        x += advance(sprite);
    }
    out
}
