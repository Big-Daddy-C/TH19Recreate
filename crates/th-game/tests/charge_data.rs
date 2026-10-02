//! Validate charge thresholds against every character in the user's original archive.
use th_formats::{dat::Archive, sht::ShtFile};
use th_game::charge::{Gauge, Level, Settings};
#[test]
fn original_character_thresholds() {
    let path = std::env::var_os("TH19_DAT").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| std::path::PathBuf::from(p)
            .join("Downloads/th19/th19/th19.dat")));
    let Some(path) = path.filter(|p| p.is_file()) else {
        eprintln!("Original game unavailable; skipping asset validation"); return;
    };
    let mut archive = Archive::open(path).unwrap();
    let names: Vec<_> = archive.entries().iter().filter(|e| e.name.ends_with(".sht"))
        .map(|e| e.name.clone()).collect();
    assert_eq!(names.len(), 19);
    for name in names {
        let sht = ShtFile::parse(&archive.read(&name).unwrap()).unwrap();
        let s = Settings::from_sht(&sht, 100);
        println!("{name}: {}, {}, {}, {}", s.attack, s.skill, s.extra, s.boss);
        assert!(0 < s.attack && s.attack <= s.skill && s.skill <= s.extra && s.extra <= s.boss && s.boss <= 2500, "{name}");
        // Charge to the original maximum, release, then ensure it can't emit twice.
        let mut gauge = Gauge::default();
        for _ in 0..250 { gauge.tick(true, true, s.boss, s, false, false); }
        assert_eq!(gauge.tick(false, true, s.boss, s, false, false), Some(Level::Boss), "{name}");
        assert_eq!(gauge.tick(false, true, s.boss, s, false, false), None, "{name}");
    }
}
