//! Round resolution. A final heart loss awards exactly one point to the other side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome { Winner(usize), Draw }

#[derive(Default)]
pub struct Round {
    pub points: [u32; 2],
    pub outcome: Option<Outcome>,
    /// Frames since the round was decided.
    pub ko_frames: u32,
    /// Rounds played to a result so far.
    pub rounds: u32,
}

/// APPROX: frames the decided round is held before the next one starts. The original
/// round transition (KO display, next-round banner, carried state) is not researched yet.
pub const NEXT_ROUND_DELAY: u32 = 180;
impl Round {
    /// Resolve after BOTH sides tick, so simultaneous final hits have no update-order bias.
    /// Wait for the player death state to reach GameOver, allowing its animation to finish.
    pub fn resolve(&mut self, defeated: [bool; 2]) {
        if self.outcome.is_some() { return; }
        self.outcome = match defeated {
            [true, false] => Some(Outcome::Winner(1)),
            [false, true] => Some(Outcome::Winner(0)),
            [true, true] => Some(Outcome::Draw),
            _ => None,
        };
        if let Some(Outcome::Winner(i)) = self.outcome { self.points[i] += 1; }
    }

    /// Count a frame after the result; true once the next round should start.
    pub fn hold(&mut self) -> bool {
        if self.outcome.is_none() { return false; }
        self.ko_frames += 1;
        self.ko_frames >= NEXT_ROUND_DELAY
    }

    /// The score carried into a fresh round.
    pub fn next(&self) -> Self {
        Self { points: self.points, outcome: None, ko_frames: 0, rounds: self.rounds + 1 }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_heart_awards_opponent_once() {
        for loser in 0..2 {
            let mut r = Round::default();
            r.resolve([false, false]);
            assert_eq!(r.outcome, None);
            let mut defeated = [false; 2]; defeated[loser] = true;
            for _ in 0..100 { r.resolve(defeated); }
            assert_eq!(r.outcome, Some(Outcome::Winner(1-loser)));
            assert_eq!(r.points[1-loser], 1);
            assert_eq!(r.points[loser], 0);
        }
    }
    #[test]
    fn decided_round_is_held_then_continues_with_the_score() {
        let mut r = Round::default();
        assert!(!r.hold());
        r.resolve([false, true]);
        for _ in 1..NEXT_ROUND_DELAY { assert!(!r.hold()); }
        assert!(r.hold());
        let n = r.next();
        assert_eq!((n.points, n.outcome, n.rounds), ([1, 0], None, 1));
    }
    #[test]
    fn simultaneous_defeat_is_draw() {
        let mut r = Round::default();
        r.resolve([true, true]);
        assert_eq!(r.outcome, Some(Outcome::Draw));
        assert_eq!(r.points, [0, 0]);
    }
}
