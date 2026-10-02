//! Headless ECL run with a stub host: spawns enemies, counts host opcodes.
//! `cargo run --release -p th-ecl --example ecl_trace -- <th19.dat> [file.ecl] [frames]`

use std::collections::BTreeMap;

use th_ecl::{Args, Context, Flow, Host, Program, Threads};
use th_formats::dat::Archive;
use th_formats::ecl::Instr;

#[derive(Default)]
struct Stub {
    rng: u32,
    ops: BTreeMap<u16, u32>,
    spawns: Vec<(String, f32, f32)>,
    log: Vec<String>,
    frame: u32,
}

impl Host for Stub {
    fn get_int(&mut self, _id: i32) -> i32 {
        0
    }
    fn get_float(&mut self, _id: i32) -> f32 {
        0.0
    }
    fn set_int(&mut self, _id: i32, _v: i32) {}
    fn set_float(&mut self, _id: i32, _v: f32) {}
    fn rand(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }
    fn exec(&mut self, ctx: &mut Context, _prog: &Program, ins: &Instr) -> Flow {
        *self.ops.entry(ins.opcode).or_default() += 1;
        if (300..=304).contains(&ins.opcode) {
            let mut a = Args::new(ins);
            let sub = a.string();
            let x = a.float(ctx, self);
            let y = a.float(ctx, self);
            if self.log.len() < 40 {
                self.log.push(format!("f{:<5} op{} {sub} ({x}, {y})", self.frame, ins.opcode));
            }
            self.spawns.push((sub, x, y));
        }
        Flow::Next
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dat = Archive::open(args.first().ok_or("usage: ecl_trace <th19.dat> [file.ecl] [frames]")?)?;
    let file = args.get(1).map(String::as_str).unwrap_or("pl00.ecl");
    let frames: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3600);
    let prog = Program::load(file, |n| dat.read(n).ok())?;
    println!("loaded {} files", prog.files.len());
    let main = prog.sub("main").ok_or("no main sub")?;

    let mut host = Stub { rng: 0x1234_5678, ..Default::default() };
    let mut stage = Threads::new(Context::new(main));
    let mut enemies: Vec<Threads> = Vec::new();
    let mut peak = 0;
    for f in 0..frames {
        host.frame = f;
        let before = host.spawns.len();
        stage.run(&prog, &mut host, 1.0);
        let mut i = 0;
        while i < enemies.len() {
            if enemies[i].run(&prog, &mut host, 1.0) == Flow::End {
                enemies.remove(i);
            } else {
                i += 1;
            }
        }
        let new: Vec<_> = host.spawns.drain(before..).collect();
        for (sub, _, _) in new {
            if let Some(r) = prog.sub(&sub) {
                enemies.push(Threads::new(Context::new(r)));
            } else {
                println!("missing sub {sub}");
            }
        }
        peak = peak.max(enemies.len());
    }
    for l in &host.log {
        println!("{l}");
    }
    let unknown: BTreeMap<u16, u32> = stage
        .main
        .iter()
        .chain(&stage.asyncs)
        .chain(enemies.iter().flat_map(|e| e.main.iter().chain(&e.asyncs)))
        .flat_map(|c| c.unknown.iter().map(|(k, v)| (*k, *v)))
        .collect();
    println!("frames={frames} live enemies={} peak={peak}", enemies.len());
    println!("host ops: {:?}", host.ops);
    println!("unhandled core ops: {unknown:?}");
    Ok(())
}
