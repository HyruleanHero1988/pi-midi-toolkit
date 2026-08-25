//! Top-level kiosk modes. Native-only; no Python/Tk coupling.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiMode {
    Home,
    Synth,
    Seq,
    Pads,
    Kaoss,
    Songs,
    Presets,
    Map,
    Log,
    Settings,
}

impl UiMode {
    pub const ALL: [UiMode; 10] = [
        UiMode::Home,
        UiMode::Synth,
        UiMode::Seq,
        UiMode::Pads,
        UiMode::Kaoss,
        UiMode::Songs,
        UiMode::Presets,
        UiMode::Map,
        UiMode::Log,
        UiMode::Settings,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "HOME",
            Self::Synth => "SYN",
            Self::Seq => "SEQ",
            Self::Pads => "PAD",
            Self::Kaoss => "KAO",
            Self::Songs => "SNG",
            Self::Presets => "PRE",
            Self::Map => "MAP",
            Self::Log => "LOG",
            Self::Settings => "SET",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Home => "HOME",
            Self::Synth => "SYNTH",
            Self::Seq => "SEQ",
            Self::Pads => "PADS",
            Self::Kaoss => "KAOSS",
            Self::Songs => "SONGS",
            Self::Presets => "PRESETS",
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
    fn ten_modes_round_trip() {
        assert_eq!(UiMode::ALL.len(), 10);
        for (i, mode) in UiMode::ALL.iter().enumerate() {
            assert_eq!(UiMode::from_index(i), Some(*mode));
            assert_eq!(mode.index(), i);
            assert!(!mode.label().is_empty());
        }
    }
}
