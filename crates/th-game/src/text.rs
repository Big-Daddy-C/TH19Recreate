//! The game's text object (`AsciiInf`, `FUN_004e5850`) for the big proportional font (style 6:
//! glyph sprite = character code + 0x120 in `ascii.anm`'s `ascii_big` entry).
use th_anm::{AnmLibrary, VmId, World};

/// Draw-chain priority of the text object's layer-0 strings (`FUN_004e7430` registers
/// `FUN_004e6ef0` -> `FUN_004e6b60(0)` at priority 100; layers 1-4 draw at 0x3b, 0x52, 0x49 and
/// 0x53). Every string the port draws is on layer 0 (+0x1979c = 0).
pub const DRAW_PRIORITY: i32 = 100;

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

/// Style 0 of the text object (`FUN_004e5850` default case): the 14x14 glyph sprite
/// `character - 0x20` of `ascii.anm`'s first entry, advancing a fixed 9 units (+0x197b0) times
/// the scale; drawn from each glyph's top-left. `centre` = horizontal alignment 0 (centred on
/// `pos`), otherwise left-aligned (1); vertical alignment 1 puts the glyph tops at `pos[1]`.
/// `argb` is the D3D colour (+0x1977c).
pub fn small_text(world: &mut World, lib: &AnmLibrary, file: usize, text: &str, pos: [f32; 2], centre: bool, argb: u32) -> Vec<VmId> {
    let _ = lib;
    const ADVANCE: f32 = 9.0;
    let color = [((argb >> 16) & 0xff) as f32, ((argb >> 8) & 0xff) as f32, (argb & 0xff) as f32];
    let mut x = if centre { pos[0] - text.len() as f32 * ADVANCE / 2.0 } else { pos[0] };
    let mut out = Vec::new();
    for c in text.bytes() {
        if c != b' ' && c >= 0x20 {
            out.push(world.spawn_sprite(file, c as i32 - 0x20, [x, pos[1]], [1.0; 2], color, DRAW_PRIORITY));
        }
        x += ADVANCE;
    }
    out
}

/// Style 4 of the text object (`FUN_004e5850` cases 4/5/10/11): the 16x16 digit glyph
/// `0xef + character - '0'` of `ascii.anm`, advancing 12 units (4 for ','), centred on `pos[0]`
/// (horizontal alignment 0) with the glyph tops at `pos[1]`. White.
pub fn digit_text(world: &mut World, file: usize, text: &str, pos: [f32; 2]) -> Vec<VmId> {
    let advance = |c: u8| if c == b',' { 4.0 } else { 12.0 };
    let total: f32 = text.bytes().map(advance).sum();
    let mut x = pos[0] - total / 2.0;
    let mut out = Vec::new();
    for c in text.bytes() {
        if c.is_ascii_digit() {
            out.push(world.spawn_sprite(file, 0xef + (c - b'0') as i32, [x, pos[1]], [1.0; 2], [255.0; 3], DRAW_PRIORITY));
        }
        x += advance(c);
    }
    out
}
