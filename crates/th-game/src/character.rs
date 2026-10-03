//! Playable characters. Each one has its own `plNN.sht` / `.anm` / `.ecl` (and `plNNb.anm` for
//! the sprites of the attacks it sends).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Character {
    #[default]
    Reimu,
    Marisa,
    Sanae,
    Ran,
}

impl Character {
    pub const ALL: [Character; 4] = [Character::Reimu, Character::Marisa, Character::Sanae, Character::Ran];

    /// Character-select number: Reimu1, Marisa2, Sanae3, Ran4.
    pub fn number(self) -> usize {
        self as usize + 1
    }

    pub fn name(self) -> &'static str {
        match self {
            Character::Reimu => "Reimu",
            Character::Marisa => "Marisa",
            Character::Sanae => "Sanae",
            Character::Ran => "Ran",
        }
    }

    /// File prefix: `pl00` for Reimu, `pl01` for Marisa.
    pub fn prefix(self) -> String {
        format!("pl{:02}", self as usize)
    }

    /// The character `delta` places along the select list (wrapping).
    pub fn shifted(self, delta: i32) -> Self {
        let n = Self::ALL.len() as i32;
        Self::ALL[(self as i32 + delta).rem_euclid(n) as usize]
    }
}
