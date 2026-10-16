//! Playable characters. Each one has its own `plNN.sht` / `.anm` / `.ecl` (and `plNNb.anm` for
//! the sprites of the attacks it sends).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Character {
    #[default]
    Reimu,
    Marisa,
    Sanae,
    Ran,
    Aunn,
    Nazrin,
    Seiran,
    Orin,
    Tsukasa,
    Mamizou,
    Yachie,
    Saki,
}

impl Character {
    pub const ALL: [Character; 12] = [Character::Reimu, Character::Marisa, Character::Sanae, Character::Ran,
        Character::Aunn, Character::Nazrin, Character::Seiran, Character::Orin,
        Character::Tsukasa, Character::Mamizou, Character::Yachie, Character::Saki];

    /// Character-select number: Reimu 1, Marisa 2, Sanae 3, Ran 4, Aunn 5, Nazrin 6, Seiran 7, Orin 8,
    /// Tsukasa 9, Mamizou 10, Yachie 11, Saki 12.
    pub fn number(self) -> usize {
        self as usize + 1
    }

    pub fn name(self) -> &'static str {
        match self {
            Character::Reimu => "Reimu",
            Character::Marisa => "Marisa",
            Character::Sanae => "Sanae",
            Character::Ran => "Ran",
            Character::Aunn => "Aunn",
            Character::Nazrin => "Nazrin",
            Character::Seiran => "Seiran",
            Character::Orin => "Orin",
            Character::Tsukasa => "Tsukasa",
            Character::Mamizou => "Mamizou",
            Character::Yachie => "Yachie",
            Character::Saki => "Saki",
        }
    }

    /// The characters from Aunn on, whose aura classes answer the body and bullet tests through
    /// their own [7] callback (`scope::Probe`).
    pub fn uses_probe(self) -> bool {
        self as usize >= Character::Aunn as usize
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
