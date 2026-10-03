//! The game's computer player (`CPUInf`, `CPUHitInf`, `CPUHitReimuInf` / `CPUHitMarisaInf`): a
//! controller that turns the field into the same input bits a human presses. Ported for Reimu and
//! Marisa at Normal difficulty. Marisa differs from Reimu only in the target choice (`FUN_004f9810`:
//! no third-distance bonus for spirits above her) and the approach rule (`FUN_004f8d10`).
//!
//! One frame is `FUN_004fa2a0` (ramp), `FUN_004f9ac0` (collect hazards and targets) and the
//! brain `FUN_004fa3e0`, with Reimu's target choice `FUN_004f8930` and approach rule
//! `FUN_004f8c50` (vtable 0x5a8430, slots +0, +8). Not ported: laser hazards (the game has no
//! lasers yet), rectangular enemy hit boxes, the item-chasing branch (no items yet), the story
//! mode helper objects, and the other characters' target rules.
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::character::Character;
use crate::enemy::flag;
use crate::player::{input, State};
use crate::side::Side;

/// `0x598f70`: for each of the 18 candidates (9 directions, unfocused then focused) the 16 other
/// candidates to try, in order, when it would be hit.
#[rustfmt::skip]
const ALTERNATIVES: [[u8; 16]; 9] = [
    [2, 7, 8, 4, 3, 6, 5, 1, 11, 16, 17, 13, 12, 15, 14, 10],
    [0, 4, 3, 7, 8, 6, 5, 2, 9, 13, 12, 16, 17, 15, 14, 11],
    [0, 7, 8, 4, 3, 6, 5, 1, 9, 16, 17, 13, 12, 15, 14, 10],
    [7, 0, 2, 5, 8, 6, 4, 1, 16, 9, 11, 14, 17, 15, 13, 10],
    [8, 0, 2, 6, 7, 5, 3, 1, 17, 9, 11, 15, 16, 14, 12, 10],
    [3, 7, 0, 2, 8, 4, 6, 1, 12, 16, 9, 11, 17, 13, 15, 10],
    [4, 8, 0, 2, 7, 3, 5, 1, 13, 17, 9, 11, 16, 12, 14, 10],
    [3, 8, 5, 2, 0, 4, 6, 1, 12, 17, 14, 11, 9, 13, 15, 10],
    [4, 7, 2, 0, 6, 3, 5, 1, 13, 16, 15, 11, 9, 12, 14, 10],
];

/// Movement direction (1 up, 2 down, 3 left, 4 right, 5..8 the diagonals) as input bits.
const DIR_KEYS: [u32; 9] = [
    0,
    input::UP,
    input::DOWN,
    input::LEFT,
    input::RIGHT,
    input::UP | input::LEFT,
    input::UP | input::RIGHT,
    input::DOWN | input::LEFT,
    input::DOWN | input::RIGHT,
];

/// Every candidate was hit even at the smallest margin: the bot bombs.
const ALL_DANGEROUS: u32 = 0x3ffff;
/// Normal difficulty: the mistake chance (`CPUInf+0x38`, percent) starts at 10 and rises by one
/// every 600 frames once 0x708 frames have passed, up to 100 (`FUN_004fb6f0`, `FUN_004fa2a0`).
const START_LEVEL: i32 = 10;
const LEVEL_INTERVAL: i32 = 600;
const LEVEL_AFTER: i32 = 0x707;

struct Hazard {
    pos: [f32; 2],
    radius: f32,
    /// Direction of travel and speed (bullets: velocity; enemies: this frame's displacement).
    angle: f32,
    speed: f32,
}

struct Target {
    /// Index in `Side::enemies`.
    index: usize,
    pos: [f32; 2],
    /// Internal spirit kind set and not disabled (flags 0x700000 / 0x80000).
    spirit: bool,
    /// A spirit kind that is disabled: never chosen in the first pass.
    disabled_spirit: bool,
    cooldown: i32,
}

#[derive(Debug, Clone)]
pub struct Bot {
    /// Charge tier the bot is working towards (+0x36eeb0; 2, 3 or 4).
    mode: i32,
    /// Frames to keep the evasive move (+0x36eea0).
    hold: i32,
    /// Frames of charging left after meeting a spirit (+0x36eea4).
    charge_hold: i32,
    /// Spirit approaches so far (+0x36eea8, at most 50 per round).
    spirit_count: i32,
    /// Direction and focus chosen last frame as `dir + 9 * focus` (+0x36ee9c).
    last_choice: i32,
    /// `CPUInf+0x3c` frame counter and `+0x38` mistake chance.
    frames: i32,
    level: i32,
    /// The previous frame's output, for tests and the viewer.
    pub keys: u32,
}

impl Default for Bot {
    fn default() -> Self {
        Self::new()
    }
}

impl Bot {
    pub fn new() -> Self {
        Self { mode: 2, hold: 0, charge_hold: 0, spirit_count: 0, last_choice: 0, frames: 0, level: START_LEVEL, keys: 0 }
    }

    /// The mistake chance in percent.
    pub fn level(&self) -> i32 {
        self.level
    }

    /// Choose this frame's input for `side`. Does nothing without a player.
    pub fn think(&mut self, side: &mut Side) -> u32 {
        let Some(p) = side.player.as_ref() else { return 0 };
        // FUN_004fa2a0: the mistake chance grows with the round.
        self.frames += 1;
        if self.frames % LEVEL_INTERVAL == 0 && self.level < 100 && self.frames > LEVEL_AFTER {
            self.level += 1;
        }
        let pos = p.pos;
        let p_character = p.character;
        let speeds = [p.sht.speed, p.sht.speed_focused, p.sht.speed_diagonal, p.sht.speed_focused_diagonal];
        let meter = if p.free_charge { 2500 } else { p.meter }.clamp(0, 2500);
        let charge_now = p.charge.value;
        let (skill, extra, boss) = {
            let s = p.charge_settings;
            (s.skill.clamp(0, 2500), s.extra.clamp(0, 2500), s.boss.clamp(0, 2500))
        };

        // FUN_004f9ac0: hazards (bullets, then enemy bodies) and shootable enemies.
        let mut hazards: Vec<Hazard> = side
            .st
            .bullets
            .iter()
            .filter(|b| b.state == 1 && !b.delete)
            .map(|b| Hazard { pos: b.pos, radius: b.hitbox, angle: b.vel[1].atan2(b.vel[0]), speed: b.speed })
            .collect();
        let mut targets = Vec::new();
        for (index, e) in side.enemies.iter().enumerate() {
            let d = &e.data;
            if d.delete {
                continue;
            }
            let delta = d.delta;
            if d.flags & (flag::NO_HITBOX | flag::HIDDEN) == 0 {
                hazards.push(Hazard {
                    pos: d.pos,
                    radius: d.hitbox[0] / 2.0,
                    angle: delta[1].atan2(delta[0]),
                    speed: (delta[0] * delta[0] + delta[1] * delta[1]).sqrt(),
                });
            }
            if d.flags & (flag::NO_HURTBOX | flag::INVINCIBLE | flag::HIDDEN) == 0 {
                targets.push(Target {
                    index,
                    pos: d.pos,
                    spirit: d.spirit_kind != 0 && !d.spirit_disabled,
                    disabled_spirit: d.spirit_kind != 0 && d.spirit_disabled,
                    cooldown: d.bot_cooldown,
                });
            }
        }

        // FUN_004fa3e0.
        let mut dir;
        let mut focus = false;
        let mut charge = false;
        // Danger of each candidate move, retrying with a smaller margin when everything is hit.
        let mut mask = ALL_DANGEROUS;
        for margin in [3.0 * 10.0, 3.0 * 5.0, 3.0] {
            mask = danger_mask(&hazards, pos, speeds, margin);
            if mask != ALL_DANGEROUS {
                break;
            }
        }
        if mask == ALL_DANGEROUS {
            self.keys = input::BOMB;
            return input::BOMB;
        }

        // Which tier to charge towards (modes 2 / 3 / 4 are skill, EX and boss).
        let wants = |threshold: i32| meter >= threshold && charge_now < threshold;
        let trigger = if self.mode == 2 && wants(skill) {
            true
        } else {
            (self.mode == 3 && wants(extra)) || wants(boss)
        };
        if trigger {
            charge = true;
            self.mode = (side.st.rand() % 3) as i32 + 2;
        }

        let marisa = p_character == Character::Marisa;
        let chosen = pick_target(&targets, pos, !marisa);
        // The cooldowns tick down whenever the enemy is looked at.
        for t in targets.iter().filter(|t| t.cooldown >= 1) {
            side.enemies[t.index].data.bot_cooldown -= 1;
        }
        if self.hold < 1 {
            if self.charge_hold > 0 {
                charge = true;
                self.charge_hold -= 1;
            }
            match chosen {
                None => dir = home_direction(pos),
                Some(t) => {
                    // FUN_004f8c50: line up under the target.
                    let target = &targets[t];
                    if target.pos[0] - 64.0 < pos[0] && pos[0] < target.pos[0] + 64.0 {
                        focus = true;
                    }
                    let (mut tolerance, mut below, mut above) = (12.0, 160.0, 256.0);
                    if target.spirit {
                        self.spirit_count += 1;
                        charge = true;
                        self.charge_hold = 20;
                        if pos[1] > 128.0 {
                            if marisa {
                                // Close under the spirit: 16 to 40 pixels below it.
                                (below, above) = (16.0, 40.0);
                            } else {
                                (tolerance, below) = (8.0, 80.0);
                            }
                        }
                    }
                    dir = approach(target.pos, pos, tolerance, below, above);
                }
            }
            let before = dir;
            let stop = match find_threat(&hazards, pos) {
                Some(h) => steer(&hazards[h], pos, &mut dir),
                None => false,
            };
            if !stop {
                // Keep away from the side walls and the bottom.
                if pos[0] < -136.0 {
                    dir = 4;
                    self.hold = 4;
                } else if pos[0] > 136.0 {
                    dir = 3;
                    self.hold = 4;
                }
                if pos[1] > 400.0 {
                    dir = match dir {
                        0 | 2 => 1,
                        3 => 5,
                        7 => 3,
                        4 => 6,
                        8 => 4,
                        d => d,
                    };
                    self.hold = 4;
                }
            }
            self.after_choice(&mut dir, &mut focus, before, chosen.map(|t| targets[t].index), side, mask);
        } else {
            self.hold -= 1;
            dir = (self.last_choice % 9) as usize;
            focus = self.last_choice / 9 != 0;
            if chosen.is_some_and(|t| targets[t].spirit) && self.spirit_count < 50 {
                charge = true;
                self.charge_hold = 15;
                self.spirit_count += 1;
            }
            self.after_choice(&mut dir, &mut focus, 0, None, side, mask);
        }

        self.last_choice = dir as i32 + focus as i32 * 9;
        let mut keys = DIR_KEYS[dir];
        if !targets.is_empty() {
            keys |= input::SHOT;
        }
        if focus {
            keys |= input::FOCUS;
        }
        if charge {
            keys |= input::CHARGE;
        }
        self.keys = keys;
        keys
    }

    /// The tail of the brain: dodge a hit candidate, then note a direction reversal.
    fn after_choice(&mut self, dir: &mut usize, focus: &mut bool, before: usize, target: Option<usize>, side: &mut Side, mask: u32) {
        let candidate = *dir + 9 * *focus as usize;
        if mask & (1 << candidate) != 0 {
            let ignore = self.level > 0 && ((side.st.rand() % 100) as i32) < self.level;
            if !ignore {
                for &alt in &ALTERNATIVES[candidate % 9] {
                    if mask & (1 << alt) == 0 {
                        *focus = alt / 9 != 0;
                        *dir = (alt % 9) as usize;
                        self.hold = 8;
                        break;
                    }
                }
            }
        }
        // Turning the opposite way makes the bot drop its target for ten frames.
        if let (Some(index), true) = (target, *dir != before) {
            let reversed = match *dir {
                3 | 5 | 7 => matches!(before, 4 | 6 | 8),
                4 | 6 | 8 => matches!(before, 3 | 5 | 7),
                _ => false,
            };
            if reversed {
                side.enemies[index].data.bot_cooldown = 10;
            }
        }
    }
}

/// `FUN_004f8f60`: one bit per candidate (`dir + 9 * focus`) that would end the frame inside a
/// hazard grown by `margin`.
fn danger_mask(hazards: &[Hazard], pos: [f32; 2], speeds: [f32; 4], margin: f32) -> u32 {
    let [straight, straight_focus, diagonal, diagonal_focus] = speeds;
    let mut candidates = [pos; 18];
    for (group, (s, d)) in [(straight, diagonal), (straight_focus, diagonal_focus)].into_iter().enumerate() {
        let c = &mut candidates[group * 9..group * 9 + 9];
        c[1][1] -= s;
        c[2][1] += s;
        c[3][0] -= s;
        c[4][0] += s;
        c[5] = [pos[0] - d, pos[1] - d];
        c[6] = [pos[0] + d, pos[1] - d];
        c[7] = [pos[0] - d, pos[1] + d];
        c[8] = [pos[0] + d, pos[1] + d];
    }
    for c in &mut candidates {
        c[0] = c[0].clamp(-140.0, 140.0);
        c[1] = c[1].clamp(32.0, 432.0);
    }
    let mut mask = 0;
    for h in hazards {
        for (k, c) in candidates.iter().enumerate() {
            let (dx, dy) = (c[0] - h.pos[0], c[1] - h.pos[1]);
            if dx * dx + dy * dy < h.radius * h.radius + margin * margin {
                mask |= 1 << k;
            }
        }
    }
    mask
}

/// `FUN_004f8930`: the enemy to go for.
fn pick_target(targets: &[Target], pos: [f32; 2], spirit_bonus: bool) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut best_d = 998001.0f32;
    for (i, t) in targets.iter().enumerate() {
        let (dx, dy) = (pos[0] - t.pos[0], pos[1] - t.pos[1]);
        let mut d = dx * dx + dy * dy;
        if spirit_bonus && t.spirit && dx.abs() < 64.0 {
            d /= 3.0;
        }
        if t.cooldown >= 1 || t.disabled_spirit {
            continue;
        }
        let reachable = (d <= 2304.0 && pos[1] >= t.pos[1] + 16.0)
            || (t.pos[0].abs() < 100.0 && pos[1] - t.pos[1] < 256.0 && pos[1] >= t.pos[1] - 32.0);
        if reachable && d < best_d {
            best = Some(i);
            best_d = d;
        }
    }
    if best.is_some() {
        return best;
    }
    // Fallback pass: the same reach test without cooldowns or the spirit rules.
    let mut best_d = 998001.0f32;
    for (i, t) in targets.iter().enumerate() {
        let (dx, dy) = (pos[0] - t.pos[0], pos[1] - t.pos[1]);
        let d = dx * dx + dy * dy;
        let reachable = (d <= 2304.0 && t.pos[1] + 16.0 <= pos[1])
            || (t.pos[0].abs() < 100.0 && pos[1] - t.pos[1] < 256.0 && t.pos[1] - 32.0 <= pos[1]);
        if reachable && d < best_d {
            best = Some(i);
            best_d = d;
        }
    }
    best
}

/// `FUN_004f8dd0`: move under the target, `below` to 256 pixels beneath it.
fn approach(target: [f32; 2], pos: [f32; 2], tolerance: f32, below: f32, above: f32) -> usize {
    let mut dir = 0;
    if target[0] - tolerance > pos[0] {
        dir = 4;
    } else if pos[0] > target[0] + tolerance {
        dir = 3;
    }
    if pos[1] > target[1] + above {
        dir = match dir {
            4 => 6,
            3 => 5,
            _ => 1,
        };
    } else if below + target[1] > pos[1] {
        dir = match dir {
            4 => 8,
            3 => 7,
            _ => 2,
        };
    }
    dir
}

/// With nothing to shoot or collect: drift back to the home area near the bottom centre.
fn home_direction(pos: [f32; 2]) -> usize {
    let mut dir = 0;
    if pos[1] >= 320.0 {
        if pos[1] > 416.0 {
            dir = 1;
        }
    } else {
        dir = 2;
    }
    if pos[0] >= -80.0 {
        if pos[0] > 80.0 {
            dir = if dir != 2 { (dir == 1) as usize * 2 + 3 } else { 7 };
        }
    } else {
        dir = if dir != 2 { (dir == 1) as usize * 2 + 4 } else { 8 };
    }
    dir
}

fn wrap(a: f32) -> f32 {
    let mut a = a;
    for _ in 0..34 {
        if a > PI {
            a -= TAU;
        } else if a < -PI {
            a += TAU;
        } else {
            break;
        }
    }
    a
}

/// `FUN_004d3720`: the hazard's straight path passes within `reach` of the player.
fn on_path(h: &Hazard, pos: [f32; 2], reach: f32) -> bool {
    let (lx, ly) = (pos[0] - h.pos[0], pos[1] - h.pos[1]);
    let (s, c) = h.angle.sin_cos();
    let along = c * lx + s * ly;
    let across = -s * lx + c * ly;
    across.abs() <= reach && -reach <= along && (0.0 <= along || along * along + across * across <= reach * reach)
}

/// `FUN_004f9530`: the hazard to steer away from, if any.
fn find_threat(hazards: &[Hazard], pos: [f32; 2]) -> Option<usize> {
    let mut found = None;
    let mut best = FRAC_PI_2;
    for (i, h) in hazards.iter().enumerate() {
        let (dx, dy) = (pos[0] - h.pos[0], pos[1] - h.pos[1]);
        let d2 = dx * dx + dy * dy - h.radius * h.radius * h.speed;
        if d2 >= 16384.0 {
            continue;
        }
        let mut to_player = dy.atan2(dx);
        if h.angle - to_player <= PI {
            if to_player - h.angle > PI {
                to_player -= TAU;
            }
        } else {
            to_player += TAU;
        }
        let off = (h.angle - to_player).abs();
        if d2 > 1024.0 {
            if off < 0.5235988 && on_path(h, pos, h.radius + 16.0) && off < best {
                best = off;
                found = Some(i);
            }
        } else if off < FRAC_PI_2 {
            found = Some(i);
        }
    }
    found
}

/// The evasive direction for a threatening hazard (the block after `FUN_004f9530` in
/// `FUN_004fa3e0`). Returns true when the bot should just stand still (the hazard is not
/// heading at it), which also skips the wall rules.
fn steer(h: &Hazard, pos: [f32; 2], dir: &mut usize) -> bool {
    let (dx, dy) = (pos[0] - h.pos[0], pos[1] - h.pos[1]);
    let mut flee = wrap(dy.atan2(dx));
    if dx * dx + dy * dy >= h.radius * h.radius + 32.0 * 32.0 {
        let mut a = flee;
        let off = if h.angle - a > PI {
            h.angle - (a + TAU)
        } else {
            if a - h.angle > PI {
                a -= TAU;
            }
            h.angle - a
        };
        let off = wrap(off);
        if off.abs() >= PI / 180.0 * 20.0 {
            *dir = 0;
            return true;
        }
        flee = if off > 0.0 { wrap(flee - FRAC_PI_2) } else { wrap(flee + FRAC_PI_2) };
    }
    // Bend the escape away from the side walls and the top / bottom.
    let eighth = PI / 8.0;
    if pos[0] < -128.0 {
        if flee <= -7.0 * eighth {
            flee = wrap(flee + FRAC_PI_2);
        } else if flee >= 7.0 * eighth {
            flee = wrap(flee - FRAC_PI_2);
        }
    } else if pos[0] > 128.0 {
        if (-eighth..=0.0).contains(&flee) {
            flee = wrap(flee - FRAC_PI_2);
        } else if (0.0..=eighth).contains(&flee) {
            flee = wrap(flee + FRAC_PI_2);
        }
    }
    if pos[1] < 128.0 {
        if (-5.0 * eighth..=-4.0 * eighth).contains(&flee) {
            flee = wrap(flee - FRAC_PI_2);
        } else if (-4.0 * eighth..=-3.0 * eighth).contains(&flee) {
            flee = wrap(flee + FRAC_PI_2);
        }
    } else if pos[1] > 416.0 {
        if (3.0 * eighth..=4.0 * eighth).contains(&flee) {
            flee = wrap(flee - FRAC_PI_2);
        } else if (4.0 * eighth..=5.0 * eighth).contains(&flee) {
            flee = wrap(flee + FRAC_PI_2);
        }
    }
    *dir = if flee <= -7.0 * eighth || flee >= 7.0 * eighth {
        3
    } else if flee <= -5.0 * eighth {
        5
    } else if flee <= -3.0 * eighth {
        1
    } else if flee <= -eighth {
        6
    } else if flee <= eighth {
        4
    } else if flee <= 3.0 * eighth {
        8
    } else if flee <= 5.0 * eighth {
        2
    } else {
        7
    };
    false
}

/// Whether the bot is alive and able to act (used by the viewer to pick keys).
pub fn active(side: &Side) -> bool {
    side.player.as_ref().is_some_and(|p| p.state != State::GameOver)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hazard(pos: [f32; 2], angle: f32) -> Hazard {
        Hazard { pos, radius: 4.0, angle, speed: 2.0 }
    }

    #[test]
    fn a_hazard_marks_only_the_candidates_it_reaches() {
        // A bullet 6 px above the player (reach 4 + margin 3 = 5): only moving up is dangerous.
        let h = [hazard([0.0, 194.0], 0.0)];
        let speeds = [4.0, 2.0, 2.8, 1.4];
        let mask = danger_mask(&h, [0.0, 200.0], speeds, 3.0);
        assert_eq!(mask & 1, 0, "standing still is safe at 6 px with a 3 px margin");
        assert_ne!(mask & (1 << 1), 0, "moving up (4 px) lands 2 px from the bullet");
        assert_eq!(mask & (1 << 2), 0, "moving down is safe");
        let wide = danger_mask(&h, [0.0, 200.0], speeds, 30.0);
        assert_ne!(wide & 1, 0, "a 30 px margin reaches the player");
        // Candidates are clamped to the field: pressed against the left wall, left is "stay".
        let wall = danger_mask(&[hazard([-140.0, 200.0], 0.0)], [-140.0, 200.0], speeds, 3.0);
        assert_ne!(wall & (1 << 3), 0);
    }

    #[test]
    fn the_nearest_reachable_enemy_is_chosen_and_cooldowns_skip_it() {
        let t = |x: f32, y: f32, cooldown: i32| Target { index: 0, pos: [x, y], spirit: false, disabled_spirit: false, cooldown };
        let targets = [t(80.0, 100.0, 0), t(10.0, 150.0, 0), t(-5.0, 340.0, 0)];
        let player = [0.0, 380.0];
        // The 340 one is below the 16 px rule (the player is 40 below it): near and above.
        assert_eq!(pick_target(&targets, player, true), Some(2));
        let skipped = [t(80.0, 100.0, 0), t(10.0, 150.0, 0), t(-5.0, 340.0, 3)];
        assert_eq!(pick_target(&skipped, player, true), Some(1), "next best within 256 above and |x| < 100");
        assert_eq!(pick_target(&[t(150.0, 100.0, 0)], player, true), None, "off to the side and out of range");
    }

    #[test]
    fn approach_aligns_under_the_target_in_the_right_band() {
        assert_eq!(approach([50.0, 100.0], [0.0, 300.0], 12.0, 160.0, 256.0), 4, "target to the right, already in band");
        assert_eq!(approach([-50.0, 100.0], [0.0, 400.0], 12.0, 160.0, 256.0), 5, "too far below: up and left");
        assert_eq!(approach([0.0, 250.0], [0.0, 300.0], 12.0, 160.0, 256.0), 2, "too close: back off down");
        assert_eq!(approach([0.0, 100.0], [0.0, 300.0], 12.0, 160.0, 256.0), 0, "aligned and 200 below: hold");
    }

    #[test]
    fn steering_sidesteps_a_bullet_aimed_at_the_player_and_ignores_others() {
        // A bullet 80 px above, travelling straight down at the player.
        let h = hazard([0.0, 120.0], std::f32::consts::FRAC_PI_2);
        let mut dir = 0;
        assert_eq!(find_threat(&[hazard([0.0, 120.0], FRAC_PI_2)], [0.0, 200.0]), Some(0));
        assert!(!steer(&h, [0.0, 200.0], &mut dir));
        assert!([3, 4].contains(&dir), "moves sideways: {dir}");
        // The same bullet well off to the side of its path: stand still.
        let h2 = hazard([0.0, 120.0], FRAC_PI_2 + 0.6);
        let mut dir = 5;
        assert!(steer(&h2, [0.0, 200.0], &mut dir));
        assert_eq!(dir, 0);
        // Nothing within 128 px: nothing to dodge.
        assert_eq!(find_threat(&[hazard([0.0, 20.0], FRAC_PI_2)], [0.0, 200.0]), None);
    }

    #[test]
    fn home_is_the_bottom_centre() {
        assert_eq!(home_direction([0.0, 380.0]), 0);
        assert_eq!(home_direction([0.0, 200.0]), 2);
        assert_eq!(home_direction([120.0, 380.0]), 3);
        assert_eq!(home_direction([-120.0, 200.0]), 8);
        assert_eq!(home_direction([120.0, 430.0]), 5);
    }
}
