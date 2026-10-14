//! VS rounds and matches, from th19.exe: the result (`FUN_00515540`), the time limit
//! (`FUN_00516450`), the result banners and the next round (`FUN_00512fd0`), and the match
//! settings read at each round's start (`FUN_005177a0` from the option byte `DAT_0062bb10`).

/// How a round was decided (the result flags at GameManager +0x140).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The loser ran out of hearts.
    Normal,
    /// The winner never lost a heart this round (0x100000).
    Complete,
    /// The time ran out (0x200000).
    Decision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Winner(usize, Kind),
    Draw,
}

/// One side's numbers used to decide a round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Status +0x54: hearts left (for a 2 vs 2 team, whether anyone has one counts).
    pub lives: i32,
    /// Status +0x64: hearts lost this round.
    pub deaths: u32,
    /// Status +0x4c / +0x50: EX and boss attacks sent this round (`FUN_0050e820` /
    /// `FUN_0050e7d0`).
    pub ex_sent: u32,
    pub boss_sent: u32,
    /// Status +0x24.
    pub meter: i32,
}

/// The match settings from the VS option byte (th19.cfg +0xf0, `DAT_0062bb10`; the shipped
/// configuration holds 0x54).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Round wins needed: `((option >> 4) & 3) + 1` (`DAT_0062bb24`).
    pub wins_needed: u32,
    /// Frames per round before a decision: table 0x5a85c0 `[option & 0xf]` (0 = none).
    pub time_limit: i32,
    /// `(option >> 4) & 3`: which round banners are used (`FUN_0050efc0`).
    pub banner_style: u32,
}

/// Table 0x5a85c0: the time limit choices in frames.
pub const TIME_LIMITS: [i32; 7] = [0, 10800, 18000, 25200, 36000, 54000, 72000];

impl Rules {
    pub fn from_option(option: u32) -> Self {
        Self {
            wins_needed: ((option >> 4) & 3) + 1,
            time_limit: TIME_LIMITS.get((option & 0xf) as usize).copied().unwrap_or(0).clamp(0, 0x57030),
            banner_style: (option >> 4) & 3,
        }
    }
}

impl Default for Rules {
    /// The shipped th19.cfg option byte, 0x54.
    fn default() -> Self {
        Self::from_option(0x54)
    }
}

/// Frames after the result when the banners appear (`FUN_00512fd0`: GameManager +0x28 == 0x78).
pub const BANNER_FRAME: u32 = 120;
/// Frames after the result when the next round starts (`> 0x13f`).
pub const NEXT_ROUND_FRAME: u32 = 320;

#[derive(Clone, Debug)]
pub struct Round {
    /// Round wins (status +0xbc, at most 9).
    pub points: [u32; 2],
    pub outcome: Option<Outcome>,
    /// Frames since the round was decided.
    pub ko_frames: u32,
    /// Rounds finished in this match.
    pub rounds: u32,
    pub rules: Rules,
    /// Frames left before a decision (`DAT_0062bb20`), when there is a time limit.
    pub time_left: i32,
}

impl Default for Round {
    fn default() -> Self {
        Self::new(Rules::default())
    }
}

impl Round {
    pub fn new(rules: Rules) -> Self {
        Self { points: [0; 2], outcome: None, ko_frames: 0, rounds: 0, rules, time_left: rules.time_limit }
    }

    /// `FUN_00515540`. Called once a player out of hearts finishes dying (`FUN_00546320`
    /// state 2 at 30 frames) or when the time runs out.
    pub fn decide(&mut self, t: [Tally; 2]) {
        if self.outcome.is_some() {
            return;
        }
        let complete = |w: usize| if t[w].deaths == 0 { Kind::Complete } else { Kind::Normal };
        let outcome = match (t[0].lives < 1, t[1].lives < 1) {
            (true, false) => Outcome::Winner(1, complete(1)),
            (false, true) => Outcome::Winner(0, complete(0)),
            // VS (mode 2): both out is a draw.
            (true, true) => Outcome::Draw,
            // A decision: fewer hearts lost, then more EX + boss attacks sent, then the
            // larger meter.
            (false, false) => {
                let sent = |s: &Tally| s.ex_sent.min(999) + s.boss_sent.min(999);
                if t[0].deaths.min(999) < t[1].deaths.min(999) {
                    Outcome::Winner(0, Kind::Decision)
                } else if t[1].deaths.min(999) < t[0].deaths.min(999) {
                    Outcome::Winner(1, Kind::Decision)
                } else if sent(&t[1]) < sent(&t[0]) {
                    Outcome::Winner(0, Kind::Decision)
                } else if sent(&t[0]) < sent(&t[1]) {
                    Outcome::Winner(1, Kind::Decision)
                } else if t[1].meter < t[0].meter {
                    Outcome::Winner(0, Kind::Decision)
                } else if t[0].meter < t[1].meter {
                    Outcome::Winner(1, Kind::Decision)
                } else {
                    Outcome::Draw
                }
            }
        };
        if let Outcome::Winner(w, _) = outcome {
            self.points[w] = (self.points[w] + 1).min(9);
        }
        self.outcome = Some(outcome);
        self.ko_frames = 0;
    }

    /// One frame of the time limit while the round is undecided (`FUN_00516450`). Returns
    /// true when it has just run out.
    pub fn tick_time(&mut self) -> bool {
        if self.outcome.is_some() || self.rules.time_limit <= 0 {
            return false;
        }
        self.time_left = (self.time_left - 1).clamp(0, 0x57030);
        self.time_left < 1
    }

    /// Whether the match has a winner (`FUN_00412cd0` >= `DAT_0062bb24`).
    pub fn match_winner(&self) -> Option<usize> {
        (0..2).find(|&i| self.points[i] >= self.rules.wins_needed)
    }

    /// Count a frame after the result; true once the next round (or match) should start.
    pub fn hold(&mut self) -> bool {
        if self.outcome.is_none() {
            return false;
        }
        self.ko_frames += 1;
        self.ko_frames > NEXT_ROUND_FRAME
    }

    /// The state carried into the next round: the score, or a fresh match once someone won.
    pub fn next(&self) -> Self {
        if self.match_winner().is_some() {
            return Self::new(self.rules);
        }
        Self { points: self.points, outcome: None, ko_frames: 0, rounds: self.rounds + 1, rules: self.rules, time_left: self.rules.time_limit }
    }

    /// The round-start banner script (`FUN_0050efc0`, front.anm): with banner style 1, Round 1,
    /// Round 2, then Final Round by the round wins so far; style 2+, Round 1-4 then Final; style
    /// 0, "Let's Fire!".
    pub fn banner_script(&self) -> usize {
        let played = self.points[0].min(9) + self.points[1].min(9);
        match self.rules.banner_style {
            0 => 0x15b,
            1 => match played { 0 => 0x15f, 1 => 0x163, _ => 0x16f },
            _ => match played { 0 => 0x15f, 1 => 0x163, 2 => 0x167, 3 => 0x16b, _ => 0x16f },
        }
    }

    /// The result banner for `side` (`FUN_00512fd0`): Victory / Complete Victory / Decision
    /// Victory, Defeat / Complete Defeat / Decision Defeat, or Draw.
    pub fn result_script(&self, side: usize) -> Option<usize> {
        Some(match self.outcome? {
            Outcome::Draw => 0x15a,
            Outcome::Winner(w, kind) => match (w == side, kind) {
                (true, Kind::Normal) => 0x14e,
                (true, Kind::Complete) => 0x151,
                (true, Kind::Decision) => 0x154,
                (false, Kind::Normal) => 0x157,
                (false, Kind::Complete) => 0x158,
                (false, Kind::Decision) => 0x159,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tally(lives: i32, deaths: u32) -> Tally { Tally { lives, deaths, ..Default::default() } }

    #[test]
    fn shipped_option_byte() {
        let r = Rules::default();
        assert_eq!((r.wins_needed, r.time_limit, r.banner_style), (2, 36000, 1));
    }

    #[test]
    fn ko_awards_one_point_and_complete_victory_without_deaths() {
        let mut r = Round::default();
        r.decide([tally(0, 2), tally(2, 0)]);
        assert_eq!(r.outcome, Some(Outcome::Winner(1, Kind::Complete)));
        assert_eq!(r.points, [0, 1]);
        r.decide([tally(2, 0), tally(0, 2)]);
        assert_eq!(r.points, [0, 1], "decided once");
        let mut r = Round::default();
        r.decide([tally(1, 1), tally(0, 2)]);
        assert_eq!(r.outcome, Some(Outcome::Winner(0, Kind::Normal)));
        assert_eq!((r.result_script(0), r.result_script(1)), (Some(0x14e), Some(0x157)));
    }

    #[test]
    fn simultaneous_defeat_is_draw() {
        let mut r = Round::default();
        r.decide([tally(0, 2), tally(0, 2)]);
        assert_eq!(r.outcome, Some(Outcome::Draw));
        assert_eq!(r.points, [0, 0]);
        assert_eq!(r.result_script(0), Some(0x15a));
    }

    #[test]
    fn decision_by_deaths_then_attacks_then_meter() {
        let mut r = Round::default();
        r.decide([tally(1, 1), tally(2, 0)]);
        assert_eq!(r.outcome, Some(Outcome::Winner(1, Kind::Decision)));
        let mut r = Round::default();
        let a = Tally { lives: 2, ex_sent: 3, ..Default::default() };
        let b = Tally { lives: 2, boss_sent: 1, ..Default::default() };
        r.decide([a, b]);
        assert_eq!(r.outcome, Some(Outcome::Winner(0, Kind::Decision)));
        let mut r = Round::default();
        r.decide([Tally { lives: 2, meter: 100, ..Default::default() }, Tally { lives: 2, meter: 900, ..Default::default() }]);
        assert_eq!(r.outcome, Some(Outcome::Winner(1, Kind::Decision)));
    }

    #[test]
    fn time_limit_runs_out_and_best_of_three() {
        let mut r = Round::new(Rules { time_limit: 3, ..Rules::default() });
        assert!(!r.tick_time());
        assert!(!r.tick_time());
        assert!(r.tick_time());
        let mut r = Round::default();
        assert_eq!(r.banner_script(), 0x15f);
        r.decide([tally(2, 0), tally(0, 2)]);
        for _ in 0..NEXT_ROUND_FRAME { assert!(!r.hold()); }
        assert!(r.hold());
        let mut n = r.next();
        assert_eq!((n.points, n.rounds), ([1, 0], 1));
        assert_eq!(n.banner_script(), 0x163);
        n.decide([tally(2, 0), tally(0, 2)]);
        assert_eq!(n.match_winner(), Some(0));
        assert_eq!(n.next().points, [0, 0], "a won match starts over");
    }
}
