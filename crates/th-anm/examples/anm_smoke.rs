//! Headless smoke test: load every .anm in th19.dat, run every script for 300 frames.
//! usage: cargo run --release -p th-anm --example anm_smoke -- <path/to/th19.dat>

use th_anm::{AnmLibrary, World};
use th_formats::dat::Archive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: anm_smoke <th19.dat>")?;
    let mut dat = Archive::open(path)?;
    let mut lib = AnmLibrary::default();
    let names: Vec<String> = dat.entries().iter().map(|e| e.name.clone()).filter(|n| n.ends_with(".anm")).collect();
    let t0 = std::time::Instant::now();
    for n in &names {
        let bytes = dat.read(n)?;
        lib.load(n, &bytes)?;
    }
    let tex_mb: usize = lib.textures.iter().map(|t| t.rgba.len()).sum::<usize>() / (1 << 20);
    println!("loaded {} files, {} textures ({tex_mb} MiB RGBA) in {:?}", lib.files.len(), lib.textures.len(), t0.elapsed());

    let mut world = World::default();
    let mut scripts = 0;
    let mut tris = 0;
    for f in 0..lib.files.len() {
        for s in 0..lib.files[f].script_count() {
            world.clear();
            world.spawn(&lib, f, s, [320.0, 240.0]);
            for _ in 0..300 {
                world.tick(&lib);
            }
            tris += world.draw_list(&lib, th_anm::Pass::Ui).iter().map(|b| b.vertices.len() / 3).sum::<usize>();
            scripts += 1;
        }
    }
    println!("ran {scripts} scripts x 300 frames, {tris} triangles at the end");
    let mut unknown: Vec<_> = world.unknown_ops.iter().collect();
    unknown.sort_by_key(|(_, &c)| std::cmp::Reverse(c));
    println!("unhandled opcodes (op, hits): {unknown:?}");
    Ok(())
}
