//! Headless check: run every STD stage for 600 frames and report what ends up on screen.
//! usage: cargo run --release -p th-stage --example stage_smoke -- <th19.dat>

use th_anm::{AnmLibrary, World};
use th_formats::{dat::Archive, stage::StdFile};
use th_stage::Stage;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: stage_smoke <th19.dat>")?;
    let mut dat = Archive::open(path)?;
    let mut names: Vec<String> = dat.entries().iter().map(|e| e.name.clone()).filter(|n| n.ends_with(".std")).collect();
    names.sort();
    for name in names {
        let std = StdFile::parse(&dat.read(&name)?)?;
        let mut lib = AnmLibrary::default();
        let anm = lib.load(&std.anm_name, &dat.read(&std.anm_name)?)?;
        let mut world = World::default();
        let mut stage = Stage::new(std, anm, &lib, &mut world);
        let mut on_screen = Vec::new();
        for frame in 1..=600 {
            stage.tick(&mut world);
            world.tick(&lib);
            if frame % 200 == 0 {
                let cam = stage.camera([0.0, 0.0, 640.0, 480.0]);
                let list = world.draw_list(&lib, Some(&cam));
                let verts: Vec<_> = list.iter().flat_map(|b| b.vertices.iter()).collect();
                let visible = verts.iter().filter(|v| (0.0..=640.0).contains(&v.pos[0]) && (0.0..=480.0).contains(&v.pos[1])).count();
                on_screen.push((verts.len() / 3, visible));
            }
        }
        let mut unk: Vec<_> = stage.unknown_ops.iter().map(|(k, v)| (*k, *v)).collect();
        unk.sort();
        println!(
            "{name:<12} VMs={:<4} cam=({:.0},{:.0},{:.0}) fog={:?} tris/visible-verts @200/400/600 = {on_screen:?} unhandled={unk:?}",
            world.len(), stage.pos[0], stage.pos[1], stage.pos[2], stage.fog_range
        );
    }
    Ok(())
}
