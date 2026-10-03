//! Exercise the playable input -> charge -> original SHT/ANM -> collision path.
use th_anm::{AnmLibrary, World};
use th_formats::{dat::Archive, sht::ShtFile};
use th_game::{player::{Player, State, Barrier, input}, Enemy, EnemyData, FIELDS};

fn setup() -> Option<(Player, World, AnmLibrary)> {
    let path = std::env::var_os("TH19_DAT").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| std::path::PathBuf::from(p)
            .join("Downloads/th19/th19/th19.dat")))?;
    if !path.is_file() { eprintln!("Original assets absent: skipping charge gameplay test"); return None; }
    let mut archive = Archive::open(path).unwrap();
    let sht = ShtFile::parse(&archive.read("pl00.sht").unwrap()).unwrap();
    let mut lib = AnmLibrary::default();
    let anm = lib.load("pl00.anm", &archive.read("pl00.anm").unwrap()).unwrap();
    let aura = lib.load("aura.anm", &archive.read("aura.anm").unwrap()).unwrap();
    let mut p = Player::new(sht, anm);
    p.aura_anm = Some(aura);
    p.state = State::Normal;
    Some((p, World::default(), lib))
}
fn tick(p: &mut Player, keys: u32, enemies: &mut [Enemy], world: &mut World, lib: &AnmLibrary) {
    p.update(keys, enemies, world, lib, FIELDS[0]);
    world.tick(lib);
}

#[test]
fn original_reimu_amulets_use_slow_no_target_acceleration() {
    let Some((mut p, mut world, lib)) = setup() else { return };
    let record = p.sht.slots.iter().flatten().flatten().find(|r| r.funcs[0] == 1).unwrap().clone();
    assert_eq!(record.funcs[1],1,"original homing update callback");
    assert_eq!(record.angular_velocity,0.0);
    assert_eq!(record.acceleration,0.0);
    tick(&mut p, input::SHOT, &mut [], &mut world, &lib);
    let b = p.bullets.iter().find(|b| b.homing).expect("unfocused Reimu fires homing amulets");
    let speed = b.speed;
    assert!((speed-record.speed-0.1).abs() < 0.001);
    for _ in 0..9 { tick(&mut p,0,&mut [],&mut world,&lib); }
    let b = p.bullets.iter().find(|b| b.homing && b.time==10).unwrap();
    assert!((b.speed-speed-0.9).abs() < 0.001);
    assert_eq!(b.homing_target,None);
}

#[test]
fn hold_release_original_reimu_shots_and_repeat_without_meter_cost() {
    let Some((mut p, mut world, lib)) = setup() else { return };
    assert_eq!(p.meter, 500);
    for _ in 0..49 { tick(&mut p, input::CHARGE, &mut [], &mut world, &lib); }
    tick(&mut p, 0, &mut [], &mut world, &lib);
    assert!(p.bullets.is_empty(), "early release must not fire");
    for _ in 0..10 { tick(&mut p, 0, &mut [], &mut world, &lib); }
    for _ in 0..80 { tick(&mut p, input::CHARGE, &mut [], &mut world, &lib); }
    assert_eq!(p.charge.value, 500);
    assert!(p.bullets.is_empty(), "holding at the threshold must not auto-fire");
    tick(&mut p, 0, &mut [], &mut world, &lib);
    assert_eq!(p.bullets.len(), 4);
    assert_eq!(p.meter, 500);
    assert_eq!(p.charge.value, 0);
    for (b, r) in p.bullets.iter().zip(p.sht.slots[10].as_ref().unwrap()) {
        assert!(b.charge_shot);
        assert_eq!(b.angle, r.angle);
        assert_eq!(b.damage, 7);
        assert!((b.speed - 13.05).abs() < 0.001);
        assert!(b.vm.is_some());
    }
    // Original update callback reaches 0.2, waits through frame 119, then accelerates.
    for _ in 1..120 { tick(&mut p, 0, &mut [], &mut world, &lib); }
    assert_eq!(p.bullets.len(), 4, "slot 10's 21-frame timer emits just one volley");
    assert!((p.bullets[0].speed - 0.2).abs() < 0.001);
    tick(&mut p, 0, &mut [], &mut world, &lib);
    assert!((p.bullets[0].speed - 0.4).abs() < 0.001);
    for _ in 0..50 { tick(&mut p, input::CHARGE, &mut [], &mut world, &lib); }
    tick(&mut p, 0, &mut [], &mut world, &lib);
    assert_eq!(p.bullets.iter().filter(|b| b.time == 1).count(), 4);
    assert_eq!(p.meter, 500);
}

#[test]
fn barrier_hit_cancels_pending_charge_and_does_not_recover_barrier() {
    let Some((mut p, mut world, lib)) = setup() else { return };
    for _ in 0..50 { tick(&mut p, input::CHARGE, &mut [], &mut world, &lib); }
    assert!(p.hit([0.0, 390.0], &mut world, &lib, FIELDS[0]));
    tick(&mut p, 0, &mut [], &mut world, &lib);
    assert_eq!(p.charge.value, 0);
    assert!(p.bullets.is_empty());
    assert_eq!(p.lives, 2);
    assert_eq!(p.barrier, Barrier::Broken);
    // Basic charge is free but must not grant EX-tier shield recovery.
    p.state = State::Normal;
    for _ in 0..50 { tick(&mut p, input::CHARGE, &mut [], &mut world, &lib); }
    tick(&mut p, 0, &mut [], &mut world, &lib);
    assert_eq!(p.bullets.len(), 4);
    assert_eq!(p.barrier, Barrier::Broken);
}

#[test]
fn charge_shot_pierces_and_skips_its_last_hit_enemy() {
    let Some((mut p, mut world, lib)) = setup() else { return };
    for _ in 0..50 { tick(&mut p, input::CHARGE, &mut [], &mut world, &lib); }
    tick(&mut p, 0, &mut [], &mut world, &lib);
    p.bullets.truncate(1);
    let b = &p.bullets[0];
    let target = [b.pos[0] + b.angle.cos()*12.1, b.pos[1] + b.angle.sin()*12.1];
    let mut enemies = [Enemy {
        threads: th_ecl::Threads::new(th_ecl::Context::new(th_ecl::SubRef { file: 0, sub: 0 })),
        data: EnemyData { id: 100, pos: target, hurtbox: [80.0; 2], hp: 100, entered: true, ..Default::default() },
    }];
    tick(&mut p, 0, &mut enemies, &mut world, &lib);
    assert_eq!(enemies[0].data.hp, 93);
    assert_eq!(p.bullets.len(), 1);
    tick(&mut p, 0, &mut enemies, &mut world, &lib);
    assert_eq!(enemies[0].data.hp, 93, "must not damage the same last-hit target every frame");
    enemies[0].data.id = 101;
    tick(&mut p, 0, &mut enemies, &mut world, &lib);
    assert_eq!(enemies[0].data.hp, 86, "piercing shot must still damage a different target");
}

#[test]
fn charge_orb_keeps_its_original_anm_rotation() {
    let Some((mut p, mut world, lib)) = setup() else { return };
    for _ in 0..50 { tick(&mut p, input::CHARGE, &mut [], &mut world, &lib); }
    tick(&mut p, 0, &mut [], &mut world, &lib);
    let id = p.bullets[0].vm.unwrap();
    for _ in 0..4 { tick(&mut p, 0, &mut [], &mut world, &lib); }
    let before = world.get_mut(id).unwrap().rot[2];
    tick(&mut p, 0, &mut [], &mut world, &lib);
    let after = world.get_mut(id).unwrap().rot[2];
    assert!((after - before).abs() > 0.1, "player update must not overwrite ANM spin");
}
