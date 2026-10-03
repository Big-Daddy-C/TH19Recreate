use th_anm::{AnmLibrary, Pass, World};
use th_formats::dat::Archive;

#[test]
fn original_boss_circle_is_tilted_and_keeps_spinning_after_stop() {
    let Some(home) = std::env::var_os("USERPROFILE") else {
        return;
    };
    let path = std::env::var_os("TH19_DAT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(home).join("Downloads/th19/th19/th19.dat"));
    if !path.is_file() {
        eprintln!("Original assets unavailable; skipped boss-circle regression");
        return;
    }
    let mut dat = Archive::open(path).unwrap();
    let mut lib = AnmLibrary::default();
    let file = lib
        .load("effect.anm", &dat.read("effect.anm").unwrap())
        .unwrap();
    let mut world = World::default();
    let id = world.spawn(&lib, file, 98, [320.0, 240.0]);
    for _ in 0..100 {
        world.tick(&lib);
    }
    let vm = world.get(id).unwrap();
    assert_eq!(vm.render_mode, 8);
    assert_eq!(vm.rotation_mode, 3);
    let size = lib.files[file].sprites[33].size;
    let flat_area = size[0] * size[1] * vm.scale[0] * vm.scale[1];
    let before = world.draw_list(&lib, Pass::Ui);
    let vertices = &before[0].vertices;
    let area: f32 = vertices
        .chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [t[0].pos, t[1].pos, t[2].pos];
            ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5
        })
        .sum();
    // A Z-only render preserves area. The original script's X/Y tilt compresses it.
    assert!(
        area / flat_area > 0.4 && area / flat_area < 0.9,
        "area ratio {}",
        area / flat_area
    );
    let z = vm.rot[2];
    for _ in 0..15 {
        world.tick(&lib);
    }
    // Script 98's velocity is -0.75 degrees per frame.
    let dz = world.get(id).unwrap().rot[2] - z;
    assert!((dz - (-0.75_f32).to_radians() * 15.0).abs() < 0.001);
    let after = world.draw_list(&lib, Pass::Ui);
    let corner = vertices[0];
    let moved = after[0]
        .vertices
        .iter()
        .find(|v| v.uv == corner.uv)
        .unwrap();
    assert!((moved.pos[0] - corner.pos[0]).hypot(moved.pos[1] - corner.pos[1]) > 10.0);
    // Mode 0 remains unrotated, even when the same VM holds nonzero XYZ angles.
    world.get_mut(id).unwrap().render_mode = 0;
    let flat = world.draw_list(&lib, Pass::Ui);
    let min = flat[0]
        .vertices
        .iter()
        .map(|v| v.pos[0])
        .fold(f32::INFINITY, f32::min);
    let max = flat[0]
        .vertices
        .iter()
        .map(|v| v.pos[0])
        .fold(f32::NEG_INFINITY, f32::max);
    assert!((max - min - size[0] * world.get(id).unwrap().scale[0]).abs() < 0.01);
}
