use std::rc::Rc;
use th_anm::{AnmLibrary, World};
use th_formats::{
    dat::Archive,
    exe::{bullet_types, Exe},
    sht::ShtFile,
};
use th_game::side::{dispatch_charges, prepare_charge_targets};
use th_game::{
    charge::Level,
    player::{input, Barrier, Player, State},
    Side, FIELDS,
};
struct Game {
    sides: Vec<Side>,
    prog: th_ecl::Program,
    world: World,
    lib: AnmLibrary,
}
fn setup() -> Option<Game> {
    let home = std::env::var_os("USERPROFILE")?;
    let path = std::env::var_os("TH19_DAT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(home).join("Downloads/th19/th19/th19.dat"));
    if !path.is_file() {
        eprintln!("Original assets unavailable; skipped");
        return None;
    }
    let types = Rc::new(bullet_types(
        &Exe::parse(std::fs::read(path.with_file_name("th19.exe")).unwrap()).unwrap(),
    ));
    let mut dat = Archive::open(path).unwrap();
    let prog = th_ecl::Program::load("pl00.ecl", |name| dat.read(name).ok()).unwrap();
    let mut lib = AnmLibrary::default();
    let mut files = [None; 8];
    let names: Vec<_> = ["bullet.anm".to_string(), "effect.anm".to_string()]
        .into_iter()
        .chain(prog.files[0].1.anim.clone())
        .collect();
    for (i, name) in names.iter().enumerate() {
        files[i] = Some(lib.load(name, &dat.read(name).unwrap()).unwrap());
    }
    let player = lib
        .load("pl00.anm", &dat.read("pl00.anm").unwrap())
        .unwrap();
    let aura = lib
        .load("aura.anm", &dat.read("aura.anm").unwrap())
        .unwrap();
    let sht = ShtFile::parse(&dat.read("pl00.sht").unwrap()).unwrap();
    let mut sides: Vec<_> = FIELDS
        .into_iter()
        .map(|field| Side::new(field, 77, [0; 256], files))
        .collect();
    for side in &mut sides {
        side.st.bullet_types = types.clone();
        let mut p = Player::new(sht.clone(), player);
        p.state = State::Normal;
        p.free_charge = true;
        p.aura_anm = Some(aura);
        p.effect_anm = files[1];
        side.player = Some(p);
    }
    Some(Game {
        sides,
        prog,
        world: World::default(),
        lib,
    })
}
impl Game {
    fn tick(&mut self, keys: [u32; 2]) {
        prepare_charge_targets(&mut self.sides);
        for (side, key) in self.sides.iter_mut().zip(keys) {
            side.tick(&self.prog, key, &mut self.world, &self.lib);
        }
        dispatch_charges(&mut self.sides);
        self.world.tick(&self.lib);
    }
    fn charge(&mut self, side: usize, frames: usize) {
        for _ in 0..frames {
            let mut keys = [0; 2];
            keys[side] = input::CHARGE;
            self.tick(keys);
        }
        self.tick([0; 2]);
    }
}
#[test]
fn skill_is_local_clears_bullets_expires_and_does_not_restore_barrier() {
    let Some(mut g) = setup() else { return };
    g.sides[0].player.as_mut().unwrap().barrier = Barrier::Broken;
    for _ in 0..100 {
        g.tick([input::CHARGE, 0]);
    }
    let et = th_game::bullet::Et {
        sprite: 0,
        count: [1, 1],
        speed: [0.0; 2],
        ..Default::default()
    };
    g.sides[0].st.fire(&et, [0.0, 400.0], &mut g.world, &g.lib);
    assert!(!g.sides[0].st.bullets.is_empty());
    g.tick([0; 2]);
    let p = g.sides[0].player.as_ref().unwrap();
    assert_eq!(p.last_charge, Some(Level::Skill));
    assert_eq!(p.skills.len(), 1);
    assert!(p.invuln >= 39);
    assert_eq!(p.barrier, Barrier::Broken);
    assert_eq!(p.meter, 500);
    assert!(g.sides[0].st.bullets.is_empty());
    assert!(g.sides[1].enemies.is_empty());
    for _ in 0..130 {
        g.tick([0; 2]);
    }
    assert!(g.sides[0].player.as_ref().unwrap().skills.is_empty());
}
#[test]
fn extra_attack_runs_original_script_on_opponent_and_restores_shield() {
    let Some(mut g) = setup() else { return };
    g.sides[0].player.as_mut().unwrap().barrier = Barrier::Broken;
    g.charge(0, 170);
    assert_eq!(
        g.sides[0].player.as_ref().unwrap().last_charge,
        Some(Level::Extra)
    );
    assert_eq!(g.sides[0].player.as_ref().unwrap().barrier, Barrier::Up);
    assert!(g.sides[1].st.spawn.iter().any(|r| r.sub == "ExtraAttack"));
    assert!(!g.sides[0].st.spawn.iter().any(|r| r.sub == "ExtraAttack"));
    for _ in 0..3 {
        g.tick([0; 2]);
    }
    assert!(!g.sides[1].st.bullets.is_empty());
    assert!(g.sides[1]
        .st
        .bullets
        .iter()
        .any(|b| b.kind == 46 || b.kind == 47));
    println!(
        "EX unknown {:?}, vars {:?}",
        g.sides[1].st.unknown_ops, g.sides[1].st.unknown_vars
    );
    assert_eq!(g.sides[0].st.extra_level, 1);
}
#[test]
fn boss_fires_falls_back_to_extra_and_cleans_up_on_defeat() {
    let Some(mut g) = setup() else { return };
    g.sides[1].player.as_mut().unwrap().invuln = 10000;
    g.charge(0, 250);
    assert!(g.sides[1].has_boss());
    let mut bullets = 0;
    for _ in 0..600 {
        g.tick([0; 2]);
        bullets = bullets.max(g.sides[1].st.bullets.len());
    }
    println!(
        "BOSS peak bullets {bullets}, unknown {:?}, vars {:?}, enemies {:?}",
        g.sides[1].st.unknown_ops,
        g.sides[1].st.unknown_vars,
        g.sides[1]
            .enemies
            .iter()
            .map(|e| (&e.data.sub, e.data.hp, e.data.pos, e.data.boss_slot))
            .collect::<Vec<_>>()
    );
    assert!(g.sides[1].has_boss());
    assert!(bullets > 0, "Boss must actually fire");
    g.charge(0, 250);
    assert_eq!(
        g.sides[0].player.as_ref().unwrap().last_charge,
        Some(Level::Extra)
    );
    assert_eq!(g.sides[1].st.boss_ids.len(), 1);
    for e in &mut g.sides[1].enemies {
        if e.data.boss_slot.is_some() {
            e.data.damage(100000, [0.0, 400.0]);
        }
    }
    for _ in 0..65 {
        g.tick([0; 2]);
    }
    assert!(
        !g.sides[1].has_boss(),
        "Boss slot must be released on scripted defeat"
    );
}
#[test]
fn sending_from_right_uses_left_field() {
    let Some(mut g) = setup() else { return };
    g.charge(1, 170);
    assert!(g.sides[0].st.spawn.iter().any(|r| r.sub == "ExtraAttack"));
    assert!(!g.sides[1].st.spawn.iter().any(|r| r.sub == "ExtraAttack"));
}

#[test]
fn simultaneous_bosses_timeout_and_release_both_slots() {
    let Some(mut g) = setup() else { return };
    for side in &mut g.sides {
        side.player.as_mut().unwrap().invuln = 10000;
    }
    for _ in 0..250 {
        g.tick([input::CHARGE; 2]);
    }
    g.tick([0; 2]);
    assert!(g.sides.iter().all(Side::has_boss));
    assert!(g
        .sides
        .iter()
        .all(|s| s.st.spawn.iter().filter(|r| r.sub == "BossAttack").count() == 1));
    for _ in 0..400 {
        g.tick([0; 2]);
    }
    assert!(g.sides.iter().all(|s| s.st.boss_ids.len() == 1));
    for _ in 0..1700 {
        g.tick([0; 2]);
    }
    assert!(
        g.sides.iter().all(|s| !s.has_boss()),
        "Scripted timeouts must release both boss slots"
    );
}
#[test]
fn skill_damages_nearby_enemies_without_the_basic_charge_shots() {
    let Some(mut g) = setup() else { return };
    g.charge(0, 100);
    let p = g.sides[0].player.as_mut().unwrap();
    p.bullets.clear();
    let mut enemies = [th_game::Enemy {
        threads: th_ecl::Threads::new(th_ecl::Context::new(th_ecl::SubRef { file: 0, sub: 0 })),
        data: th_game::EnemyData {
            id: 100,
            pos: p.pos,
            hurtbox: [10.0; 2],
            hp: 100,
            ..Default::default()
        },
    }];
    for _ in 0..4 {
        p.update(0, &mut enemies, &mut g.world, &g.lib, FIELDS[0]);
    }
    assert_eq!(
        enemies[0].data.hp, 100,
        "First pulse already elapsed before target insertion"
    );
    p.update(0, &mut enemies, &mut g.world, &g.lib, FIELDS[0]);
    assert_eq!(
        enemies[0].data.hp, 88,
        "Three overlapping skill circles each deal four damage"
    );
    enemies[0].data.invuln_time = 100;
    for _ in 0..5 {
        p.update(0, &mut enemies, &mut g.world, &g.lib, FIELDS[0]);
    }
    assert_eq!(
        enemies[0].data.hp, 88,
        "Skill must respect enemy damage invulnerability"
    );
}
