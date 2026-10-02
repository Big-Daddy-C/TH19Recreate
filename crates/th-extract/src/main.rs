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
                for sc in &e.scripts {
                    println!("   script {}", sc.id);
                    for i in &sc.instrs {
                        let ints: Vec<i32> = (0..i.args.len() / 4).map(|n| i.arg_i32(n)).collect();
                        println!("      t={:<4} op={:<4} mask={:#x} {:?}", i.time, i.opcode, i.param_mask, ints);
                    }
                }
            }
        }
        "ecl-info" => {
            let mut ops: std::collections::BTreeMap<u16, usize> = Default::default();
            for i in 0..archive.entries().len() {
                let name = archive.entries()[i].name.clone();
                if !name.ends_with(".ecl") {
                    continue;
                }
                match th_formats::ecl::EclFile::parse(&archive.read_entry(i)?) {
                    Ok(e) => {
                        let n: usize = e.subs.iter().map(|s| s.instrs.len()).sum();
                        for s in &e.subs {
                            for ins in &s.instrs {
                                *ops.entry(ins.opcode).or_default() += 1;
                            }
                        }
                        let first: Vec<_> = e.subs.iter().take(3).map(|s| s.name.as_str()).collect();
                        println!("{name:<14} anim={:?} ecli={:?} subs={:<4} instrs={n:<6} e.g. {first:?}", e.anim, e.ecli, e.subs.len());
                    }
                    Err(err) => println!("{name:<14} ERROR {err}"),
                }
            }
            println!("{} distinct opcodes: {:?}", ops.len(), ops.keys().collect::<Vec<_>>());
        }
        "std-info" => {
            for i in 0..archive.entries().len() {
                let name = archive.entries()[i].name.clone();
                if !name.ends_with(".std") {
                    continue;
                }
                match th_formats::stage::StdFile::parse(&archive.read_entry(i)?) {
                    Ok(s) => {
                        let quads: usize = s.objects.iter().map(|o| o.quads.len()).sum();
                        let mut ops: Vec<u16> = s.script.iter().map(|i| i.opcode).collect();
                        ops.sort();
                        ops.dedup();
                        println!(
                            "{name:<12} anm={:<12} objects={:<3} quads={:<4} instances={:<4} script={:<4} ops={ops:?}",
                            s.anm_name, s.objects.len(), quads, s.instances.len(), s.script.len()
                        );
                    }
                    Err(err) => println!("{name:<12} ERROR {err}"),
                }
            }
        }
        "msg-info" => {
            // Structure only: dialogue text is never printed.
            let (mut text_bytes, mut sjis_ok) = (0usize, 0usize);
            let mut ops = std::collections::BTreeSet::new();
            for i in 0..archive.entries().len() {
                let name = archive.entries()[i].name.clone();
                if !name.ends_with(".msg") {
                    continue;
                }
                let bytes = archive.read_entry(i)?;
                let parsed = if name.starts_with("end") || name.starts_with("staff") {
                    th_formats::msg::MsgFile::parse_ending(&bytes)
                } else {
                    th_formats::msg::MsgFile::parse(&bytes)
                };
                match parsed {
                    Ok(m) => {
                        let used = m.entries.iter().filter(|e| !e.instrs.is_empty()).count();
                        let n: usize = m.entries.iter().map(|e| e.instrs.len()).sum();
                        for e in &m.entries {
                            for ins in &e.instrs {
                                ops.insert(ins.opcode);
                                if ins.data.len() > 8 && ins.opcode == 17 {
                                    let t = th_formats::msg::decode_text(&ins.data[4..]);
                                    text_bytes += t.len();
                                    sjis_ok += sjis_valid(&t);
                                }
                            }
                        }
                        println!("{name:<12} entries={:<3} used={used:<3} instrs={n}", m.entries.len());
                    }
                    Err(err) => println!("{name:<12} ERROR {err}"),
                }
            }
            println!("opcodes: {ops:?}");
            if text_bytes > 0 {
                println!("op17 text: {text_bytes} bytes, {:.1}% valid Shift-JIS", 100.0 * sjis_ok as f64 / text_bytes as f64);
            }
        }
        "bgm-info" => {
            for t in th_formats::bgm::parse_fmt(&archive.read("thbgm.fmt")?)? {
                let secs = t.track_size as f64 / (t.sample_rate as f64 * t.channels as f64 * t.bits_per_sample as f64 / 8.0);
                println!(
                    "{:<16} offset={:<10} intro={:<9} size={:<9} {}Hz {}ch {}bit ({secs:.1}s)",
                    t.name, t.offset, t.intro_size, t.track_size, t.sample_rate, t.channels, t.bits_per_sample
                );
            }
        }
        "std-dump" => {
            let name = args.get(2).ok_or("std-dump <file.std>")?;
            let s = th_formats::stage::StdFile::parse(&archive.read(name)?)?;
            println!("anm: {}", s.anm_name);
            for o in &s.objects {
                println!("object {} flags={} pos={:?} size={:?}", o.id, o.flags, o.pos, o.size);
                for q in &o.quads {
                    println!("   quad kind={} script={} pos={:?} size={:?}", q.kind, q.script, q.pos, q.size);
                }
            }
            for i in s.instances.iter().take(12) {
                println!("instance obj={} pos={:?}", i.object, i.pos);
            }
            for i in &s.script {
                let ints: Vec<i32> = (0..i.args.len() / 4).map(|n| i.arg_i32(n)).collect();
                let floats: Vec<f32> = (0..i.args.len() / 4).map(|n| i.arg_f32(n)).collect();
                println!("t={:<5} op={:<3} ints={ints:?} floats={floats:?}", i.time, i.opcode);
            }
        }
        "anm-textures" => {
            // Writes the embedded PNG/JPEG images of one .anm file (for local debugging only).
            let name = args.get(2).ok_or("anm-textures <file.anm> [out_dir]")?;
            let out = PathBuf::from(args.get(3).map(String::as_str).unwrap_or("data/textures"));
            std::fs::create_dir_all(&out)?;
            let a = th_formats::anm::AnmFile::parse(&archive.read(name)?)?;
            for e in &a.entries {
                if let Some(t) = &e.texture {
                    let ext = magic(&t.data);
                    let file = out.join(format!("{}.{ext}", e.name.replace(['/', '\\'], "_")));
                    std::fs::write(&file, &t.data)?;
                    println!("{}", file.display());
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

/// Count bytes that form valid Shift-JIS sequences (sanity check for text decoding).
fn sjis_valid(t: &[u8]) -> usize {
    let (mut i, mut ok) = (0, 0);
    while i < t.len() {
        let b = t[i];
        if b < 0x80 || (0xa1..=0xdf).contains(&b) {
            ok += 1;
            i += 1;
        } else if ((0x81..=0x9f).contains(&b) || (0xe0..=0xfc).contains(&b))
            && t.get(i + 1).is_some_and(|&c| (0x40..=0xfc).contains(&c) && c != 0x7f)
        {
            ok += 2;
            i += 2;
        } else {
            i += 1;
        }
    }
    ok
}
