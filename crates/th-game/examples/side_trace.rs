//! Headless run of one side's stage script with real data.
//! `cargo run --release -p th-game --example side_trace -- <th19.dat> [frames]`

use th_anm::{AnmLibrary, World};
use th_ecl::Program;
use th_formats::dat::Archive;
use th_game::{side, Side, FIELDS};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dat = Archive::open(args.first().ok_or("usage: side_trace <th19.dat> [frames]")?)?;
    let frames: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3600);
    let prog = Program::load("pl00.ecl", |n| dat.read(n).ok())?;
    let mut lib = AnmLibrary::default();
    let mut anm_files = [None; 8];
    for (slot, name) in prog.files[0].1.anim.clone().iter().enumerate() {
        anm_files[slot + 2] = Some(lib.load(name, &dat.read(name)?)?);
    }
    let mut world = World::new(1);
    let mut s = Side::new(FIELDS[0], 12345, side::roll_wave_sequence(777), anm_files);
    s.start(&prog);
    let mut last = usize::MAX;
    for f in 0..frames {
        s.tick(std::slice::from_ref(&prog), 0, &mut world, &lib);
        world.tick(&lib);
        if s.enemies.len() != last && (f % 30 == 0 || s.enemies.len() < 3) {
            let subs: Vec<String> = s
                .enemies
                .iter()
                .take(4)
                .map(|e| format!("#{} {} ({:.0},{:.0})", e.data.id, e.data.sub, e.data.pos[0], e.data.pos[1]))
                .collect();
            println!("f{f:<5} enemies={:<3} vms={:<4} {}", s.enemies.len(), world.len(), subs.join(" "));
            last = s.enemies.len();
        }
    }
    println!("unknown ops: {:?}", s.st.unknown_ops);
    println!("unknown vars: {:?}", s.st.unknown_vars);
    Ok(())
}
