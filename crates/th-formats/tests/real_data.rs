//! Parses every file in the user's own th19.dat with the matching parser.
//! Skipped (passes trivially) when no game copy is available; set TH19_DAT to enable.

use std::path::PathBuf;

use th_formats::{anm, bgm, dat::Archive, ecl, msg, sht, stage};

fn dat_path() -> Option<PathBuf> {
    let p = std::env::var_os("TH19_DAT")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads/th19/th19/th19.dat")))?;
    p.is_file().then_some(p)
}

#[test]
fn every_file_parses() {
    let Some(path) = dat_path() else {
        eprintln!("th19.dat not found; skipping real-data test");
        return;
    };
    let mut dat = Archive::open(path).expect("open archive");
    let names: Vec<String> = dat.entries().iter().map(|e| e.name.clone()).collect();
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for name in &names {
        let bytes = dat.read(name).expect("read entry");
        let ext = name.rsplit('.').next().unwrap_or("");
        let res = match ext {
            "anm" => anm::AnmFile::parse(&bytes).map(drop),
            "ecl" => ecl::EclFile::parse(&bytes).map(drop),
            "std" => stage::StdFile::parse(&bytes).map(drop),
            "sht" => sht::ShtFile::parse(&bytes).map(drop),
            "msg" if name.starts_with("end") || name.starts_with("staff") => msg::MsgFile::parse_ending(&bytes).map(drop),
            "msg" => msg::MsgFile::parse(&bytes).map(drop),
            "fmt" => bgm::parse_fmt(&bytes).map(drop),
            _ => continue,
        };
        res.unwrap_or_else(|e| panic!("{name}: {e}"));
        *counts.entry(match ext {
            "anm" => "anm",
            "ecl" => "ecl",
            "std" => "std",
            "sht" => "sht",
            "msg" => "msg",
            _ => "fmt",
        }).or_default() += 1;
    }
    // The archive lists title.anm and se_cardget.wav twice; the copies are identical,
    // so name lookup (which keeps the last entry) is unambiguous.
    let mut seen = std::collections::HashMap::<String, Vec<u8>>::new();
    for i in 0..names.len() {
        let key = names[i].to_ascii_lowercase();
        let bytes = dat.read_entry(i).unwrap();
        if let Some(prev) = seen.get(&key) {
            assert!(prev == &bytes, "duplicate entry {key} differs between copies");
        } else {
            seen.insert(key, bytes);
        }
    }

    assert_eq!(counts.get("anm"), Some(&125));
    assert_eq!(counts.get("ecl"), Some(&43));
    assert_eq!(counts.get("std"), Some(&17));
    assert_eq!(counts.get("sht"), Some(&19));
    assert_eq!(counts.get("msg"), Some(&58));
}
