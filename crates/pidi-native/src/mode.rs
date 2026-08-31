//! Top-level kiosk modes. Native-only; no Python/Tk coupling.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiMode {
    Home,
    Synth,
    Fm,
    Drums,
    Seq,
    Pads,
    Kaoss,
    Chords,
    Songs,
    Presets,
    Fx,
    Mix,
    Map,
    Log,
    Settings,
}

impl UiMode {
    pub const ALL: [UiMode; 15] = [
        UiMode::Home,
        UiMode::Synth,
        UiMode::Fm,
        UiMode::Drums,
        UiMode::Seq,
        UiMode::Pads,
        UiMode::Kaoss,
        UiMode::Chords,
        UiMode::Songs,
        UiMode::Presets,
        UiMode::Fx,
        UiMode::Mix,
        UiMode::Map,
        UiMode::Log,
        UiMode::Settings,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "HOME",
            Self::Synth => "SYN",
            Self::Fm => "FM",
            Self::Drums => "KIT",
            Self::Seq => "SEQ",
            Self::Pads => "PAD",
            Self::Kaoss => "KAO",
            Self::Chords => "CHD",
            Self::Songs => "SNG",
            Self::Presets => "PRE",
            Self::Fx => "FX",
            Self::Mix => "MIX",
            Self::Map => "MAP",
            Self::Log => "LOG",
            Self::Settings => "SET",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Home => "HOME",
            Self::Synth => "SYNTH",
            Self::Fm => "FM",
            Self::Drums => "DRUMS",
            Self::Seq => "SEQ",
            Self::Pads => "PADS",
            Self::Kaoss => "KAOSS",
            Self::Chords => "CHORDS",
            Self::Songs => "SONGS",
            Self::Presets => "PRESETS",
            Self::Fx => "FX",
            Self::Mix => "MIX",
            Self::Map => "MAP",
            Self::Log => "LOG",
            Self::Settings => "SETTINGS",
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip() {
        assert_eq!(UiMode::ALL.len(), 15);
        for (i, mode) in UiMode::ALL.iter().enumerate() {
            assert_eq!(UiMode::from_index(i), Some(*mode));
            assert_eq!(mode.index(), i);
            assert!(!mode.label().is_empty());
        }
    }
}
