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

#[test]
fn spirit_types_cross_fields_run_original_scripts_and_scope_disables_them() {
    use th_game::spirit::{Kind,Request};
    let Some(mut g)=setup() else {return};
    for kind in [Kind::Pure,Kind::Wolf,Kind::Otter,Kind::Eagle] {
        g.sides[0].st.outgoing_spirits.push(Request {kind,origin:[0.0,200.0],level:0});
    }
    dispatch_charges(&mut g.sides);
    assert_eq!(g.sides[1].st.pending_spirits.len(),4);
    assert!(g.sides[0].st.pending_spirits.is_empty());
    for p in &g.sides[1].st.pending_spirits { assert!((60..=179).contains(&p.delay)); }
    for _ in 0..182 {g.tick([0;2]);}
    for kind in 1..=4 {
        let e=g.sides[1].enemies.iter().find(|e| e.data.spirit_kind==kind).expect("original spirit script");
        assert!(e.data.hp>0); assert!(e.data.vms[0].is_some());
        assert!(e.data.scope_disable_sub.as_ref().unwrap().starts_with("Ghost"));
    }
    assert!(!g.sides[1].st.unknown_ops.contains_key(&1027));
    // Place each spirit at the active scope centre; the original sub determines HP/speed.
    for _ in 0..20 {g.tick([0,input::CHARGE]);}
    let pos=g.sides[1].player.as_ref().unwrap().pos;
    for e in &mut g.sides[1].enemies {e.data.pos=pos;e.data.abs.pv.pos=pos;}
    g.tick([0,input::CHARGE]);
    for e in g.sides[1].enemies.iter().filter(|e| e.data.spirit_kind!=0) {
        assert!(e.data.spirit_disabled);
        let (base,per_level)=match e.data.spirit_kind {1=>(50,10),2=>(200,30),_=>(150,30)};
        assert_eq!(e.data.hp,base+per_level*e.data.ivars[0],"original Ghost*HP_D");
        assert!((e.data.abs.pv.speed-0.02).abs()<0.001);
    }
    assert_eq!(Kind::from_distance(68.0*68.0),Kind::Wolf);
    assert_eq!(Kind::from_distance(128.0*128.0),Kind::Otter);
    assert_eq!(Kind::from_distance(128.0*128.0+1.0),Kind::Eagle);
}

#[test]
fn scope_slows_live_bullet_motion_and_acceleration_without_canceling_it() {
    let Some(mut g)=setup() else {return};
    for _ in 0..20 {g.tick([input::CHARGE,0]);}
    let pos=g.sides[0].player.as_ref().unwrap().pos;
    let mut et=th_game::bullet::Et {sprite:0,aim:1,angle:[0.0;2],count:[1,1],speed:[2.0;2],..Default::default()};
    et.ex[0]=th_game::bullet::Ex {kind:th_game::bullet::ex::ACCEL,a:100,r:0.2,s:0.0,..Default::default()};
    g.sides[0].st.fire(&et,[pos[0],pos[1]-30.0],&mut g.world,&g.lib);
    g.tick([input::CHARGE,0]);
    let b=&g.sides[0].st.bullets[0]; assert_eq!(b.scope_time_scale,0.5);
    let before=b.pos; let speed=b.speed;
    g.tick([input::CHARGE,0]);
    let b=&g.sides[0].st.bullets[0];
    assert!((b.speed-speed-0.1).abs()<0.001);
    assert!((b.pos[0]-before[0]-b.vel[0]*0.5).abs()<0.001);
    assert!(!b.delete);
    g.tick([0;2]);
    assert_eq!(g.sides[0].st.bullets[0].scope_time_scale,1.0);
}

#[test]
fn boss_magic_circle_uses_original_tilted_effect_script() {
    let Some(mut g) = setup() else { return };
    g.sides[1].player.as_mut().unwrap().godmode = true;
    g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
    for _ in 0..200 { g.tick([0; 2]); }
    let boss = g.sides[1].enemies.iter().find(|e| e.data.boss_slot.is_some()).unwrap();
    let vm = g.world.get(boss.data.vms[1].expect("magic circle slot")).unwrap();
    let mesh = boss.data.boss_mesh.as_ref().expect("ECL 621 boss-area mesh");
    assert_eq!(mesh.target_radius, 160.0);
    assert_eq!(mesh.color, 0x00f00f80);
    assert!(!g.sides[1].st.unknown_ops.contains_key(&621));
    assert_eq!(g.lib.files[vm.file].name, "effect.anm");
    assert_eq!(vm.sprite, 33);
    assert_eq!(vm.render_mode, 8);
    assert!(vm.visible);
    assert!(vm.rot[0].abs() > 0.1 && vm.rot[1].abs() > 0.1);
    let texture = g.lib.files[vm.file].sprites[33].texture;
    assert!(g.world.draw_list(&g.lib, th_anm::Pass::Ui).iter().any(|b| b.texture == Some(texture)));
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

/// Level bookkeeping from th19.exe FUN_004e79e0 (+1 after dispatch, cap 7). Each attack
/// plays at the level shown before release: HUD "Lv N" = rank N-1.
#[test]
fn ex_levels_advance_per_use_and_attacks_play_at_the_shown_level() {
    let Some(mut g) = setup() else { return };
    g.sides[1].player.as_mut().unwrap().godmode = true;
    let mut ranks = Vec::new();
    for use_no in 1..=3u8 {
        g.charge(0, 170);
        assert_eq!(g.sides[0].player.as_ref().unwrap().last_charge, Some(Level::Extra));
        assert_eq!(g.sides[0].st.extra_level, use_no);
        assert_eq!(g.sides[1].st.incoming_extra_level, use_no - 1);
        g.tick([0; 2]);
        let rank = g.sides[1]
            .enemies
            .iter()
            .filter(|e| e.data.sub == "ExtraAttack")
            .filter_map(|e| e.data.script_rank)
            .max();
        ranks.push(rank);
        for _ in 0..40 {
            g.tick([0; 2]);
        }
    }
    assert_eq!(ranks, vec![Some(0), Some(1), Some(2)]);
    // The other side's levels are independent.
    assert_eq!(g.sides[1].st.extra_level, 0);
}

#[test]
fn boss_level_advances_and_caps_at_seven() {
    let Some(mut g) = setup() else { return };
    g.sides[0].st.boss_level = 7;
    g.charge(0, 250);
    assert_eq!(g.sides[0].player.as_ref().unwrap().last_charge, Some(Level::Boss));
    assert_eq!(g.sides[0].st.boss_level, 7);
    assert_eq!(g.sides[1].st.incoming_boss_level, 7);
}

#[test]
fn godmode_ignores_hits_and_death_raises_low_levels() {
    let Some(mut g) = setup() else { return };
    let et = th_game::bullet::Et { sprite: 0, count: [1, 1], speed: [0.0; 2], ..Default::default() };
    // Godmode: a bullet on the player changes nothing.
    g.sides[0].player.as_mut().unwrap().godmode = true;
    let pos = g.sides[0].player.as_ref().unwrap().pos;
    g.sides[0].st.fire(&et, pos, &mut g.world, &g.lib);
    for _ in 0..20 {
        g.tick([0; 2]);
    }
    let p = g.sides[0].player.as_ref().unwrap();
    assert_eq!((p.hits, p.barrier, p.lives), (0, Barrier::Up, 2));
    // Without godmode and without the barrier, a hit costs a heart; EX/boss levels
    // below 3 rise by one (FUN_00549830).
    {
        let p = g.sides[0].player.as_mut().unwrap();
        p.godmode = false;
        p.barrier = Barrier::Broken;
    }
    g.sides[0].st.extra_level = 2;
    g.sides[0].st.boss_level = 3;
    g.sides[0].st.bullets.clear();
    let pos = g.sides[0].player.as_ref().unwrap().pos;
    g.sides[0].st.fire(&et, pos, &mut g.world, &g.lib);
    for _ in 0..30 {
        g.tick([0; 2]);
    }
    let p = g.sides[0].player.as_ref().unwrap();
    assert_eq!(p.lives, 1);
    assert_eq!(g.sides[0].st.extra_level, 3);
    assert_eq!(g.sides[0].st.boss_level, 3);
}

/// The attack's script picks rank-dependent values (ECL 570/571) and rank-masked
/// instructions, so different levels must produce different bullet patterns.
#[test]
fn extra_attack_pattern_depends_on_level() {
    let mut counts = Vec::new();
    for rank in [0u8, 3, 7] {
        let Some(mut g) = setup() else { return };
        g.sides[1].player.as_mut().unwrap().godmode = true;
        g.sides[0].st.extra_level = rank;
        g.charge(0, 170);
        assert_eq!(g.sides[1].st.incoming_extra_level, rank);
        let mut peak = 0;
        let mut kinds = std::collections::BTreeSet::new();
        for _ in 0..300 {
            g.tick([0; 2]);
            peak = peak.max(g.sides[1].st.bullets.len());
            kinds.extend(g.sides[1].st.bullets.iter().map(|b| b.kind));
        }
        println!("EX rank {rank}: peak bullets {peak}, sprite kinds {kinds:?}");
        counts.push(peak);
    }
    assert!(counts.windows(2).any(|w| w[0] != w[1]), "levels should change the pattern: {counts:?}");
}

/// Boss00Boss / BossStartInit (pl00.ecl, default.ecl): Lv1-2 (rank 0-1) one non-spell
/// (5000 HP), Lv3-4 a spell card (1500 HP), Lv5-8 a non-spell (6500 HP) that becomes a
/// spell at 1500 HP. Every phase ends in BossDead.
#[test]
fn boss_hp_and_phases_follow_the_rank() {
    for (stored, hp, first) in [(0, 5000, "BossDead"), (1, 5000, "BossDead"), (2, 1500, "BossDead"),
        (3, 1500, "BossDead"), (4, 6500, "Boss00BossCard1H"), (5, 6500, "Boss00BossCard1H"),
        (6, 6500, "Boss00BossCard1H"), (7, 6500, "Boss00BossCard1H")] {
        let Some(mut g) = setup() else { return };
        g.sides[1].player.as_mut().unwrap().godmode = true;
        g.sides[0].st.boss_level = stored;
        g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
        g.tick([0; 2]);
        assert_eq!(g.sides[1].st.incoming_boss_level, stored);
        for _ in 0..119 {
            g.tick([0; 2]);
        }
        let boss = || g.sides[1].enemies.iter().find(|e| e.data.boss_slot.is_some()).map(|e| &e.data);
        let b = boss().expect("boss present");
        assert_eq!((b.hp, b.max_hp), (hp, hp), "Lv {}", stored + 1);
        assert_eq!(b.phase_interrupt.as_ref().map(|p| p.2.as_str()), Some(first));
        if first != "BossDead" {
            for e in &mut g.sides[1].enemies {
                if e.data.boss_slot.is_some() {
                    e.data.hp = 1500;
                }
            }
            g.tick([0; 2]);
            let b = g.sides[1].enemies.iter().find(|e| e.data.boss_slot.is_some()).unwrap();
            assert_eq!(b.data.phase_interrupt.as_ref().map(|p| p.2.as_str()), Some("BossDead"));
        }
    }
}

#[test]
fn instant_charge_keys_release_each_tier_and_clear_empties_fields() {
    let Some(mut g) = setup() else { return };
    for (level, expect) in [(Level::Attack, Level::Attack), (Level::Skill, Level::Skill),
        (Level::Extra, Level::Extra), (Level::Boss, Level::Boss)] {
        g.sides[0].player.as_mut().unwrap().instant_charge(level);
        g.tick([0; 2]);
        assert_eq!(g.sides[0].player.as_ref().unwrap().last_charge, Some(expect));
        for _ in 0..12 {
            g.tick([0; 2]);
        }
    }
    assert!(g.sides[1].has_boss());
    // With the opponent's boss up, tier 4 falls back to EX like a held charge.
    g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
    g.tick([0; 2]);
    assert_eq!(g.sides[0].player.as_ref().unwrap().last_charge, Some(Level::Extra));
    for _ in 0..60 {
        g.tick([0; 2]);
    }
    assert!(!g.sides[1].st.bullets.is_empty() || !g.sides[1].enemies.is_empty());
    for s in &mut g.sides {
        s.dev_clear(&mut g.world);
    }
    assert!(g.sides.iter().all(|s| s.enemies.is_empty() && s.st.bullets.is_empty() && !s.has_boss()));
    g.tick([0; 2]);
    // A new boss can be sent after clearing.
    g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
    g.tick([0; 2]);
    assert_eq!(g.sides[0].player.as_ref().unwrap().last_charge, Some(Level::Boss));
}

/// BossStartInit plays EffChargePoint2 twice: effect.anm scripts 147/148/152/151, each an
/// ANM effectNew(1) that spawns converging particles (scripts 143/144) and removes itself.
#[test]
fn boss_entrance_spawns_converging_particles() {
    let Some(mut g) = setup() else { return };
    let before = g.world.len();
    g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
    let mut peak = 0;
    for _ in 0..60 {
        g.tick([0; 2]);
        peak = peak.max(g.world.len());
    }
    assert!(peak >= before + 4 * 4 * 40, "expected the entrance particles, peak {peak} from {before}");
    assert!(!g.world.unknown_ops.contains_key(&508));
    for _ in 0..400 {
        g.tick([0; 2]);
    }
    assert!(g.world.len() < peak / 2, "particles must expire");
}

/// ECL 522 declares the spell card: damage then comes off HP x 7 (cards take 1/7) and the
/// boss file's script 0 draws the spell background until 523 ends the card.
#[test]
fn spell_cards_take_a_seventh_of_damage_and_show_their_background() {
    let Some(mut g) = setup() else { return };
    g.sides[1].player.as_mut().unwrap().godmode = true;
    g.sides[0].st.boss_level = 2; // Lv3: spell card
    g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
    for _ in 0..200 {
        g.tick([0; 2]);
    }
    assert!(g.sides[1].st.spell_background.is_some(), "spell background");
    let boss = g.sides[1].enemies.iter_mut().find(|e| e.data.boss_slot.is_some()).unwrap();
    let hp = boss.data.hp;
    boss.data.damage(700, [0.0, 400.0]);
    assert_eq!(boss.data.hp, hp - 100);
    boss.data.damage(100_000, [0.0, 400.0]);
    for _ in 0..90 {
        g.tick([0; 2]);
    }
    assert!(g.sides[1].st.spell_background.is_none(), "BossDead ends the card");
    if let Some(boss)=g.sides[1].enemies.iter().find(|e|e.data.boss_slot.is_some()) {
        assert!(boss.data.spell_hp7.is_none(),"523 clears spell damage mode when BossDead ends the card");
    }
}

/// The bullet manager draws enemy bullets at draw-chain priority 0x29, above the layer-4
/// spell background, whatever layer their script sets.
#[test]
fn enemy_bullets_draw_above_the_spell_background() {
    let Some(mut g) = setup() else { return };
    g.sides[1].player.as_mut().unwrap().godmode = true;
    g.sides[0].st.boss_level = 2;
    g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
    for _ in 0..400 {
        g.tick([0; 2]);
    }
    let orb = g.sides[1].st.bullets.iter().find(|b| b.kind == 44).and_then(|b| b.vm).expect("card orbs");
    assert_eq!(g.world.get(orb).unwrap().draw_priority, Some(th_game::bullet::BULLET_DRAW_PRIORITY));
    assert!(g.sides[1].st.spell_background.is_some());
}

/// 316 is anmPlay (base+5+variant), whereas 317 is anmInterrupt.
#[test]
fn reimu_spell_uses_casting_sprite_and_declaration_flash() {
    let Some(mut g)=setup() else {return};
    g.sides[1].player.as_mut().unwrap().godmode=true;
    g.sides[0].st.boss_level=2;
    g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
    let mut flash=None; let mut cast=false;let mut normal_after_cast=false;
    for _ in 0..500 {
        g.tick([0;2]);
        if flash.is_none() {flash=g.sides[1].st.spell_start_effect;}
        if let Some(boss)=g.sides[1].enemies.iter().find(|e|e.data.boss_slot.is_some()) {
            if let Some(vm)=boss.data.vms[0].and_then(|id|g.world.get(id)) {
                if vm.script==9 {cast=true;}
                else if cast && (4..=8).contains(&vm.script) {normal_after_cast=true;}
            }
        }
    }
    assert!(cast,"Original pl00b.anm script9 must play at firing");
    assert!(normal_after_cast,"Movement must restore directional sprites after casting");
    let flash=flash.expect("Spell declaration owns effect.anm script6");
    assert_eq!(g.world.get(flash).unwrap().script,6);
    for e in &mut g.sides[1].enemies {if e.data.boss_slot.is_some(){e.data.damage(100000,[0.0,400.0]);}}
    for _ in 0..90 {g.tick([0;2]);}
    assert!(g.sides[1].st.spell_start_effect.is_none());
    assert!(g.world.get(flash).is_none(),"Spell declaration effect must be cleaned up");
}

#[test]
fn appearing_spell_bullet_moves_and_accelerates_on_completion_tick() {
    let Some(mut g)=setup() else {return};
    use th_game::bullet::{Et,Ex,ex};
    let mut et=Et {sprite:44,color:3,aim:1,angle:[0.0;2],speed:[1.5,1.0],count:[1,1],..Default::default()};
    et.ex[0]=Ex {kind:ex::SPAWN_ANIM,a:1,..Default::default()};
    et.ex[1]=Ex {kind:ex::ACCEL,a:600,r:0.01,s:std::f32::consts::FRAC_PI_2,..Default::default()};
    // Original Card1_at appends an asynchronous four-bounce behavior after acceleration.
    et.ex[2]=Ex {kind:ex::BOUNCE,asynchronous:true,a:4,b:13,r:-1000000.0,..Default::default()};
    g.sides[0].st.fire(&et,[0.0,128.0],&mut g.world,&g.lib);
    let b=&g.sides[0].st.bullets[0];let before=b.pos;let id=b.vm.unwrap();
    g.world.get_mut(id).unwrap().ivars[0]=1; // Appear animation's completion flag.
    g.tick([0;2]);
    let b=&g.sides[0].st.bullets[0];
    assert_eq!(b.state,1);
    assert!((b.pos[0]-before[0]-2.25).abs()<0.0001);
    assert!((b.pos[1]-before[1]-0.01).abs()<0.0001,"Acceleration runs on the transition tick");
    for _ in 0..110 {g.tick([0;2]);}
    let b=&g.sides[0].st.bullets[0];
    assert!(b.vel[0]<0.0,"Card orbs reflect off the right boundary while acceleration runs");
    assert!(b.vel[1]>1.0);
}
#[test]
fn spell_card_orbs_cover_the_scripted_angular_spread() {
    let Some(mut g)=setup() else {return};
    g.sides[1].player.as_mut().unwrap().godmode=true;
    g.sides[0].st.boss_level=2;
    g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
    let mut ids=std::collections::HashSet::new();let mut angles=Vec::new();
    for _ in 0..350 {
        g.tick([0;2]);
        for b in g.sides[1].st.bullets.iter().filter(|b|b.kind==44 || b.kind==1) {
            if ids.insert(b.vm.unwrap()) {angles.push(b.angle);}
        }
    }
    assert!(angles.len()>=13,"First Card1_at call emits thirteen shots");
    let first=&angles[..13];
    let minimum=first.iter().copied().fold(f32::INFINITY,f32::min);
    let maximum=first.iter().copied().fold(f32::NEG_INFINITY,f32::max);
    assert!(maximum-minimum>5.0,"Card1_at's +/-TAU argument must survive its call frame: {first:?}");
}

#[test]
fn scope_expands_follows_releases_and_restarts_from_original_anm() {
    let Some(mut g) = setup() else {return};
    g.tick([input::FOCUS,0]);
    assert!(!g.sides[0].player.as_ref().unwrap().scope.active(), "Focus alone does not charge");
    g.tick([input::CHARGE,0]);
    let p=g.sides[0].player.as_ref().unwrap();
    let id=p.scope.vm.unwrap();
    assert!((p.scope.radius-7.1425).abs()<0.001);
    assert_eq!(g.world.get(id).unwrap().script,15);
    for _ in 1..10 {g.tick([input::CHARGE|input::RIGHT,0]);}
    let p=g.sides[0].player.as_ref().unwrap();
    assert!((p.scope.radius-48.25).abs()<0.001);
    assert_eq!(p.scope.pos,p.pos);
    let at=th_game::enemy::field_to_canvas(FIELDS[0],p.pos);
    assert_eq!(g.world.get(id).unwrap().origin,[at[0],at[1],0.0]);
    for _ in 10..20 {g.tick([input::CHARGE,0]);}
    let p=g.sides[0].player.as_ref().unwrap();
    assert_eq!(p.scope.radius,64.0);
    assert!(p.scope.contains_circle([p.pos[0]+63.0,p.pos[1]],0.0));
    assert!(!p.scope.contains_circle([p.pos[0]+65.0,p.pos[1]],2.0), "Squared radii, not sum of radii");
    assert!(p.scope.intersects_rect([p.pos[0]+66.0,p.pos[1]],[8.0,4.0],0.0));
    assert!(!p.scope.intersects_rect([p.pos[0]+70.0,p.pos[1]],[8.0,4.0],0.0));
    assert!(!g.sides[1].player.as_ref().unwrap().scope.active());
    g.tick([0;2]);
    assert!(g.world.get(id).is_none());
    assert!(!g.sides[0].player.as_ref().unwrap().scope.active());
    for _ in 0..11 {g.tick([input::CHARGE,0]);}
    assert!((g.sides[0].player.as_ref().unwrap().scope.radius-7.1425).abs()<0.001);
    g.sides[0].player.as_mut().unwrap().state=State::Knockback;
    g.tick([input::CHARGE,0]);
    assert!(!g.sides[0].player.as_ref().unwrap().scope.active());
}

#[test]
fn all_reimu_boss_levels_produce_the_scripted_first_spell_volley() {
    // Initial jumps land on decrement BEFORE the body: Card1 emits 3*13/14;
    // Card1H emits 2*(2*18/20/26/30). Counts come from the source rank tables.
    for (rank,expected) in [(2,39),(3,42),(4,72),(5,80),(6,104),(7,120)] {
        let Some(mut g)=setup() else {return};
        g.sides[1].player.as_mut().unwrap().godmode=true;
        g.sides[0].st.boss_level=rank;
        g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
        for _ in 0..120 {g.tick([0;2]);}
        if rank>=4 {
            let boss=g.sides[1].enemies.iter_mut().find(|e|e.data.boss_slot.is_some()).unwrap();
            boss.data.hp=1500;
        }
        let mut ids=std::collections::HashSet::new();
        let mut started=false;let mut quiet=0;
        for _ in 0..400 {
            g.tick([0;2]);
            let before=ids.len();
            for b in g.sides[1].st.bullets.iter().filter(|b|b.kind==44||b.kind==1) {
                if let Some(id)=b.vm {ids.insert(id);}
            }
            if ids.len()>before {started=true;quiet=0;} else if started {quiet+=1;}
            if quiet>10 {break;}
        }
        assert_eq!(ids.len(),expected,"Reimu boss level {} first spell volley",rank+1);
        println!("Reimu level {} first spell volley: {}",rank+1,ids.len());
    }
}

#[test]
fn lower_reimu_nonspells_fire_ranked_five_way_fans() {
    // Boss00Boss1_at: two simultaneous shooters, five ways, rank table 4/5 volleys.
    for (rank,expected) in [(0,40),(1,50)] {
        let Some(mut g)=setup() else {return};
        g.sides[1].player.as_mut().unwrap().godmode=true;
        g.sides[0].st.boss_level=rank;
        g.sides[0].player.as_mut().unwrap().instant_charge(Level::Boss);
        let mut ids=std::collections::HashSet::new();let mut started=false;let mut quiet=0;
        for _ in 0..400 {
            g.tick([0;2]);let before=ids.len();
            for b in g.sides[1].st.bullets.iter().filter(|b|b.kind==11) {
                if let Some(id)=b.vm {ids.insert(id);}
            }
            if ids.len()>before {started=true;quiet=0;} else if started {quiet+=1;}
            if quiet>25 {break;}
        }
        assert_eq!(ids.len(),expected,"Reimu level {} fan volley",rank+1);
    }
}

/// enemy.anm scripts 265-267 (the animals' attack forms) set ANM 424 modes 1/2/1; the
/// owner turns them by their travel direction every frame (FUN_005089d0).
#[test]
fn animal_spirits_face_their_travel_direction() {
    use th_game::side::SpawnRequest;
    for (sub, x, y) in [("GhostWolf", 0.0, 60.0), ("GhostOtterL", -100.0, 200.0), ("GhostEagle", 80.0, 60.0)] {
        let Some(mut g) = setup() else { return };
        g.sides[1].st.request(SpawnRequest { id: 0, difficulty: 0, sub: sub.into(), pos: [x, y], hp: 200,
            score: 0, item: -1, mirrored: false, ivars: [1, 0, 1, 0], fvars: [0.0; 8] });
        let mut checked = 0;
        for _ in 0..120 {
            g.tick([0; 2]);
            let Some(e) = g.sides[1].enemies.iter().find(|e| e.data.sub == sub) else { continue };
            let Some(vm) = e.data.vms[0].and_then(|id| g.world.get(id)) else { continue };
            if !(1..=2).contains(&vm.rotate_mode) || e.data.delta[0].hypot(e.data.delta[1]) < 0.05 {
                continue;
            }
            let a = e.data.delta[1].atan2(e.data.delta[0]);
            let expect = if vm.rotate_mode == 2 && a.abs() > std::f32::consts::FRAC_PI_2 {
                th_game::motion::wrap_angle(a + std::f32::consts::PI)
            } else {
                a
            };
            assert!((vm.rot[2] - expect).abs() < 1e-4, "{sub}: rot {} expected {expect}", vm.rot[2]);
            checked += 1;
        }
        assert!(checked > 10, "{sub}: moving attack form not found ({checked})");
    }
}

/// A collider remembers its last-hit enemy only within a frame (FUN_0051fa30 clears +0xa8 on
/// every tick, hit interval +0xa4 = 1), so piercing charge orbs that stop beside a scoped
/// spirit hit it every frame and destroy it almost at once (HP 50 + 10 * I0 once quelled).
#[test]
fn charge_orbs_hit_an_overlapping_spirit_every_frame() {
    use th_game::side::SpawnRequest;
    let Some(mut g) = setup() else { return };
    g.sides[0].st.request(SpawnRequest { id: 0, difficulty: 0, sub: "GhostPure".into(), pos: [0.0, 300.0],
        hp: 200, score: 0, item: -1, mirrored: false, ivars: [2, 0, 2, 0], fvars: [0.0; 8] });
    let mut released = None;
    let mut hp = vec![];
    let mut dead_at = None;
    for f in 0..200usize {
        g.tick([if f < 60 { input::CHARGE } else { 0 }, 0]);
        if released.is_none() && g.sides[0].player.as_ref().unwrap().last_charge == Some(Level::Attack) {
            released = Some(f);
        }
        match g.sides[0].enemies.iter().find(|e| e.data.sub == "GhostPure") {
            Some(e) if released.is_some() => hp.push(e.data.hp),
            Some(_) => {}
            None if released.is_some() => { dead_at = Some(f); break; }
            None => {}
        }
    }
    let released = released.expect("charge released");
    let dead_at = dead_at.expect("spirit destroyed");
    assert!(dead_at - released < 20, "released {released}, destroyed {dead_at}");
    // The two centre orbs overlap it continuously: HP falls on consecutive frames.
    assert!(hp.windows(2).filter(|w| w[1] < w[0]).count() >= 2, "{hp:?}");
}

/// Kill explosions are colliders too: an enemy inside one keeps taking damage every frame
/// for its whole lifetime (GhostPureBlast: 10 damage, 40 frames).
#[test]
fn explosions_damage_overlapping_enemies_every_frame() {
    use th_game::side::SpawnRequest;
    let Some(mut g) = setup() else { return };
    let req = |x: f32| SpawnRequest { id: 0, difficulty: 0, sub: "GhostPure".into(), pos: [x, 100.0], hp: 200,
        score: 0, item: -1, mirrored: false, ivars: [2, 0, 2, 0], fvars: [0.0; 8] };
    g.sides[1].st.request(req(0.0));
    g.sides[1].st.request(req(8.0));
    for _ in 0..80 { g.tick([0; 2]); }
    let ids: Vec<i32> = g.sides[1].enemies.iter().filter(|e| e.data.sub == "GhostPure").map(|e| e.data.id).collect();
    assert_eq!(ids.len(), 2);
    g.sides[1].enemies.iter_mut().find(|e| e.data.id == ids[0]).unwrap().data.killed = true;
    let mut hp = vec![];
    for _ in 0..30 {
        g.tick([0; 2]);
        hp.push(g.sides[1].enemies.iter().find(|e| e.data.id == ids[1]).unwrap().data.hp);
    }
    let falling = hp.windows(2).filter(|w| w[1] < w[0]).count();
    assert!(falling >= 15, "the neighbour should lose HP on most frames: {hp:?}");
}

fn put_bullet(g: &mut Game, side: usize, kind: i32, color: i32, pos: [f32; 2], invuln: bool) {
    use th_game::bullet::{fire, Ctx, Et};
    let s = &mut g.sides[side];
    let types = s.st.bullet_types.clone();
    let anm = s.st.anm_files[0].expect("bullet.anm");
    let field = s.st.field;
    let mut rand = || 0.5;
    let mut ctx = Ctx { world: &mut g.world, lib: &g.lib, types: &types, anm, field, player: [0.0, 400.0], rand: &mut rand };
    let et = Et { sprite: kind, color, absolute: true, angle: [std::f32::consts::FRAC_PI_2, 0.0], speed: [0.0, 0.0],
        count: [1, 1], aim: 1, ..Et::default() };
    let mut out = Vec::new();
    fire(&et, pos, &mut ctx, &mut out);
    for b in &mut out { b.invuln = if invuln { 600 } else { 0 }; b.state = 1; }
    s.st.bullets.append(&mut out);
}

/// FUN_0051c8c0: a kill explosion cancels live bullets of types 0..3 (invulnerable ones too),
/// leaves other types alone, counts a chain hit for each, and sends a converted bullet to
/// the opponent (FUN_0051c650 / FUN_0051bd50).
#[test]
fn explosions_cancel_small_bullets_and_send_them_to_the_opponent() {
    use th_game::side::SpawnRequest;
    let Some(mut g) = setup() else { return };
    g.sides[1].st.request(SpawnRequest { id: 0, difficulty: 0, sub: "GhostPure".into(), pos: [0.0, 200.0], hp: 200,
        score: 0, item: -1, mirrored: false, ivars: [2, 0, 2, 0], fvars: [0.0; 8] });
    for _ in 0..70 { g.tick([0; 2]); }
    g.sides[1].st.bullets.clear();
    // The spirit has drifted; put the bullets around where it will die.
    let at = g.sides[1].enemies.iter().find(|e| e.data.sub == "GhostPure").unwrap().data.pos;
    put_bullet(&mut g, 1, 0, 1, [at[0] + 6.0, at[1]], false);
    put_bullet(&mut g, 1, 1, 2, [at[0] - 6.0, at[1]], true);
    put_bullet(&mut g, 1, 5, 2, [at[0], at[1] + 6.0], false);
    put_bullet(&mut g, 1, 0, 1, [at[0] + 130.0, at[1]], false);
    let chain_before = g.sides[1].st.chain;
    g.sides[1].enemies.iter_mut().find(|e| e.data.sub == "GhostPure").unwrap().data.killed = true;
    for _ in 0..25 { g.tick([0; 2]); }
    let kinds: Vec<i32> = g.sides[1].st.bullets.iter().map(|b| b.kind).collect();
    assert!(kinds.contains(&5), "type 5 is not cancelled by explosions: {kinds:?}");
    assert_eq!(kinds.iter().filter(|&&k| k < 4).count(), 1, "only the far small bullet survives: {kinds:?}");
    assert_eq!(g.sides[1].st.chain, chain_before + 1 + 2, "kill + one chain hit per cancelled bullet");
    // The two converted bullets arrive within 160 frames and fire on the other field.
    let before = g.sides[0].st.bullets.len();
    let mut arrived = false;
    for _ in 0..180 {
        g.tick([0; 2]);
        if g.sides[0].st.bullets.len() > before { arrived = true; break; }
    }
    assert!(arrived, "converted bullets must arrive on the opponent's field");
}

/// Cancel eligibility differs by mechanism: ECL 615/616 (FUN_004ee880) cancels invulnerable
/// bullets, the player's cancels (FUN_004ee770) do not; both need state 1 or 2.
#[test]
fn script_cancel_ignores_invulnerability_but_player_cancel_respects_it() {
    let Some(mut g) = setup() else { return };
    put_bullet(&mut g, 1, 0, 1, [10.0, 100.0], true);
    put_bullet(&mut g, 1, 0, 1, [-10.0, 100.0], false);
    let (world, lib) = (&g.world, &g.lib);
    let b = &g.sides[1].st.bullets;
    assert!(b[0].within_cancel([0.0, 100.0], 30.0, world, lib));
    assert!(!b[0].intersects_cancel([0.0, 100.0], 30.0, world, lib), "invulnerable: player cancel skips it");
    assert!(b[1].within_cancel([0.0, 100.0], 30.0, world, lib) && b[1].intersects_cancel([0.0, 100.0], 30.0, world, lib));
    assert!(!b[1].within_cancel([0.0, 300.0], 30.0, world, lib));
    // bullet.anm cancel script by cancel mode (type 0 colour 1: mode 6 -> colours[1][3]).
    let ty = &g.sides[1].st.bullet_types[0];
    assert_eq!(b[1].cancel_script(ty), Some(ty.colours[1][3]));
}
