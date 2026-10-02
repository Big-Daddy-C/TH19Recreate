use std::path::PathBuf;

use th_formats::dat::Archive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dat) = args.first() else {
        eprintln!("usage: th-extract <th19.dat> [list | extract <out_dir>]");
        std::process::exit(2);
    };
    let mut archive = Archive::open(dat)?;
    match args.get(1).map(String::as_str).unwrap_or("list") {
        "list" => {
            for e in archive.entries() {
                println!("{:>10} {:>10}  {}", e.size, e.zsize, e.name);
            }
            println!("{} entries", archive.entries().len());
        }
        "extract" => {
            let out = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("data/raw"));
            std::fs::create_dir_all(&out)?;
            for i in 0..archive.entries().len() {
                let name = archive.entries()[i].name.clone();
                let bytes = archive.read_entry(i)?;
                std::fs::write(out.join(&name), bytes)?;
            }
            println!("extracted {} files to {}", archive.entries().len(), out.display());
        }
        "anm-info" => {
            for i in 0..archive.entries().len() {
                let name = archive.entries()[i].name.clone();
                if !name.ends_with(".anm") {
                    continue;
                }
                let bytes = archive.read_entry(i)?;
                match th_formats::anm::AnmFile::parse(&bytes) {
                    Ok(a) => {
                        let fmts: Vec<_> = a
                            .entries
                            .iter()
                            .filter_map(|e| e.texture.as_ref())
                            .map(|t| format!("{}:{}", t.format, magic(&t.data)))
                            .collect();
                        println!(
                            "{name:<20} entries={:<3} sprites={:<5} scripts={:<5} tex={:?}",
                            a.entries.len(),
                            a.sprite_count(),
                            a.script_count(),
                            fmts.iter().take(4).collect::<Vec<_>>()
                        );
                    }
                    Err(e) => println!("{name:<20} ERROR {e}"),
                }
            }
        }
        "anm-dump" => {
            let name = args.get(2).ok_or("anm-dump <file.anm>")?;
            let a = th_formats::anm::AnmFile::parse(&archive.read(name)?)?;
            for e in &a.entries {
                let t = e.texture.as_ref();
                println!(
                    "entry {:<28} hdr={}x{} @({},{}) lowres={} tex={:?} sprites={} scripts={}",
                    e.name, e.width, e.height, e.x, e.y, e.lowres_scale,
                    t.map(|t| (t.width, t.height, t.data.len())), e.sprites.len(), e.scripts.len()
                );
                for s in e.sprites.iter().take(3) {
                    println!("   sprite {:>4} ({}, {}, {}x{})", s.id, s.x, s.y, s.w, s.h);
                }
                for sc in e.scripts.iter().take(2) {
                    println!("   script {}", sc.id);
                    for i in sc.instrs.iter().take(12) {
                        let ints: Vec<i32> = (0..i.args.len() / 4).map(|n| i.arg_i32(n)).collect();
                        println!("      t={:<4} op={:<4} mask={:#x} {:?}", i.time, i.opcode, i.param_mask, ints);
                    }
                }
            }
        }
        other => return Err(format!("unknown command {other}").into()),
    }
    Ok(())
}

fn magic(d: &[u8]) -> &'static str {
    match d {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xff, 0xd8, ..] => "jpg",
        _ => "raw",
    }
}
