//! Named hardware maps so PiDI can follow whichever class-compliant
//! controller is plugged in, instead of baking one brand into the engine.
//!
//! Lookup is by ALSA/`midir` port name. Unknown devices fall back to the
//! MPK mini factory map (the original desk controller) so existing presets
//! keep working. Overlay JSON in `controllers/` can replace a builtin by `id`.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// How a rotary reports CC values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KnobEncoding {
    /// Absolute 0..=127, like MPK mini pots.
    #[default]
    Absolute,
    /// Endless encoder: 1..=63 = +N, 65..=127 = −N, 64 = 0.
    RelativeSigned,
}

/// One physical MIDI controller’s factory (or commissioned) map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControllerProfile {
    pub id: String,
    #[serde(default)]
    pub name_hints: Vec<String>,
    #[serde(default = "default_true")]
    pub keybed: bool,
    /// SET→FULL VEL default while this profile is the live keybed.
    #[serde(default = "default_true")]
    pub full_vel_default: bool,
    #[serde(default)]
    pub knob_encoding: KnobEncoding,
    /// Bank 1 knobs (MPK 70–77). Empty / 0 slots are unused.
    #[serde(default = "mpk_knob_ccs")]
    pub knob_ccs: [u8; 8],
    /// Bank 2 knobs (SMK encoder bank). Unused when all zeros.
    #[serde(default)]
    pub knob_bank2_ccs: [u8; 8],
    /// Restrict knobs to this 0-based channel. `None` = any channel.
    #[serde(default)]
    pub knob_channel: Option<u8>,
    #[serde(default)]
    pub fader_ccs: [u8; 8],
    #[serde(default)]
    pub fader_channel: Option<u8>,
    /// FX punch pads, left-to-right matching the on-screen grid.
    #[serde(default = "mpk_punch_pad_notes")]
    pub punch_pad_notes: [u8; 8],
    /// Phrase / kit cells 0..15 in screen order (top-left → bottom-right).
    #[serde(default = "mpk_phrase_notes")]
    pub phrase_notes: [u8; 16],
    /// Pad notes that must not wrap into the drum kit (transport / shift).
    #[serde(default)]
    pub transport_notes: Vec<u8>,
    /// MIDI-out name fragments for RGB / LED feedback (never the DIN synth).
    #[serde(default)]
    pub feedback_port_hints: Vec<String>,
    /// USB audio interface on this device — do not steal Pi analog I/O.
    #[serde(default)]
    pub skip_audio: bool,
}

fn default_true() -> bool {
    true
}

pub fn mpk_knob_ccs() -> [u8; 8] {
    [70, 71, 72, 73, 74, 75, 76, 77]
}

pub fn mpk_punch_pad_notes() -> [u8; 8] {
    [40, 41, 42, 43, 36, 37, 38, 39]
}

/// MPK Bank A+B, row-swapped so screen A1 is the top-left pad (note 40).
pub fn mpk_phrase_notes() -> [u8; 16] {
    let mut notes = [0u8; 16];
    for cell in 0..16 {
        notes[cell] = mpk_note_for_phrase_cell(cell);
    }
    notes
}

/// Factory MPK pad notes start at 36; each 8-pad bank is row-swapped.
pub fn mpk_note_for_phrase_cell(cell: usize) -> u8 {
    let c = cell & 0x0f;
    let bank = c & !7;
    let within = (c & 7) as u8;
    36 + bank as u8 + ((within + 4) & 7)
}

impl ControllerProfile {
    pub fn generic() -> Self {
        Self {
            id: "generic".into(),
            name_hints: Vec::new(),
            keybed: false,
            full_vel_default: true,
            knob_encoding: KnobEncoding::Absolute,
            knob_ccs: mpk_knob_ccs(),
            knob_bank2_ccs: [0; 8],
            knob_channel: None,
            fader_ccs: [0; 8],
            fader_channel: None,
            punch_pad_notes: mpk_punch_pad_notes(),
            phrase_notes: mpk_phrase_notes(),
            transport_notes: Vec::new(),
            feedback_port_hints: Vec::new(),
            skip_audio: false,
        }
    }

    /// Akai MPK mini mk3 factory MPC program (Prog Select → Pad 1).
    pub fn mpk_mini() -> Self {
        Self {
            id: "mpk-mini".into(),
            name_hints: vec!["mpk".into()],
            keybed: true,
            full_vel_default: true,
            ..Self::generic()
        }
    }

    /// M-VAVE SMK-37 Elite (and Pro / Starrykey / MKE-P37 siblings).
    ///
    /// Knob CCs match the common MidiSuite / DAW maps (48–55). Pad notes start
    /// as MPK-compatible so a MidiSuite MPC remap still works; later commits
    /// switch the native 4×4 grid. Edit `controllers/smk-37-elite.json` after
    /// a learn pass if this unit's factory map differs.
    pub fn smk_37_elite() -> Self {
        Self {
            id: "smk-37-elite".into(),
            name_hints: vec![
                "smk-37".into(),
                "smk37".into(),
                "smk".into(),
                "m-vave".into(),
                "m vave".into(),
                "mvave".into(),
                "starrykey".into(),
                "mke-p37".into(),
                "mke_p37".into(),
            ],
            keybed: true,
            full_vel_default: true,
            knob_encoding: KnobEncoding::Absolute,
            knob_ccs: [48, 49, 50, 51, 52, 53, 54, 55],
            knob_bank2_ccs: [0; 8],
            knob_channel: None,
            fader_ccs: [0; 8],
            fader_channel: None,
            punch_pad_notes: mpk_punch_pad_notes(),
            phrase_notes: mpk_phrase_notes(),
            transport_notes: Vec::new(),
            feedback_port_hints: vec!["external".into(), "third".into()],
            skip_audio: true,
        }
    }

    pub fn matches_port(&self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        let n = name.to_ascii_lowercase();
        self.name_hints
            .iter()
            .any(|hint| !hint.is_empty() && n.contains(&hint.to_ascii_lowercase()))
    }

    pub fn punch_index_for_knob(&self, controller: u8) -> Option<usize> {
        self.knob_ccs
            .iter()
            .position(|&cc| cc != 0 && cc == controller)
    }

    pub fn punch_index_for_pad(&self, note: u8) -> Option<usize> {
        self.punch_pad_notes.iter().position(|&n| n == note)
    }

    pub fn note_for_phrase_cell(&self, cell: usize) -> u8 {
        self.phrase_notes[cell & 0x0f]
    }

    pub fn phrase_cell_for_note(&self, note: u8) -> Option<usize> {
        self.phrase_notes.iter().position(|&n| n == note)
    }

    pub fn is_transport_note(&self, note: u8) -> bool {
        self.transport_notes.iter().any(|&n| n == note)
    }

    pub fn knob_index(&self, controller: u8) -> Option<usize> {
        self.knob_ccs
            .iter()
            .position(|&cc| cc != 0 && cc == controller)
    }

    pub fn knob_bank2_index(&self, controller: u8) -> Option<usize> {
        self.knob_bank2_ccs
            .iter()
            .position(|&cc| cc != 0 && cc == controller)
    }

    pub fn fader_index(&self, controller: u8) -> Option<usize> {
        self.fader_ccs
            .iter()
            .position(|&cc| cc != 0 && cc == controller)
    }

    pub fn channel_ok(expected: Option<u8>, channel: u8) -> bool {
        expected.map(|ch| ch == channel).unwrap_or(true)
    }

    pub fn knobs_on_channel(&self, channel: u8) -> bool {
        Self::channel_ok(self.knob_channel, channel)
    }

    pub fn faders_on_channel(&self, channel: u8) -> bool {
        Self::channel_ok(self.fader_channel, channel)
    }
}

/// Built-in maps. Named maps are matched before generic.
pub fn builtin_profiles() -> Vec<ControllerProfile> {
    vec![
        ControllerProfile::smk_37_elite(),
        ControllerProfile::mpk_mini(),
        ControllerProfile::generic(),
    ]
}

fn registry() -> &'static Mutex<Vec<ControllerProfile>> {
    static REG: OnceLock<Mutex<Vec<ControllerProfile>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(builtin_profiles()))
}

/// Live maps (builtins plus any overlay JSON already applied).
pub fn installed_profiles() -> Vec<ControllerProfile> {
    registry().lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Pick a profile for this ALSA/`midir` name. Unrecognised ports use generic
/// (MPK-compatible knobs/pads) so a new class-compliant keyboard still plays.
pub fn profile_for_port_name(name: &str) -> ControllerProfile {
    let profiles = installed_profiles();
    profile_for_port_name_in(name, &profiles)
}

/// Load `*.json` overlays from `dir` (single object or array per file).
pub fn apply_overlay_dir(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut profiles = installed_profiles();
    let mut files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    files.sort_by_key(|e| e.file_name());
    for entry in files {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Err(err) = merge_profile_json(&mut profiles, &text) {
            eprintln!("controller overlay {}: {err}", path.display());
        }
    }
    *registry().lock().unwrap_or_else(|p| p.into_inner()) = profiles;
}

/// Factory presets, then user `controllers/` so a learn-pass JSON wins.
pub fn apply_default_overlay_dirs() {
    if let Ok(repo) = std::env::var("PIDI_REPO_ROOT") {
        let trimmed = repo.trim();
        if !trimmed.is_empty() {
            apply_overlay_dir(&Path::new(trimmed).join("presets/controllers"));
        }
    }
    let user = std::env::var("PIDI_CONTROLLERS_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("PIDI_DATA_ROOT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|r| Path::new(&r).join("controllers"))
        })
        .or_else(|| {
            std::env::var("XDG_DATA_HOME")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|x| Path::new(&x).join("pidi/controllers"))
        })
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| Path::new(&h).join(".local/share/pidi/controllers"))
        });
    if let Some(dir) = user {
        apply_overlay_dir(&dir);
    }
}

pub fn profile_for_port_name_in(name: &str, profiles: &[ControllerProfile]) -> ControllerProfile {
    let n = name.to_ascii_lowercase();
    profiles
        .iter()
        .filter(|p| p.id != "generic" && p.matches_port(&n))
        .max_by_key(|p| p.name_hints.iter().map(|h| h.len()).max().unwrap_or(0))
        .cloned()
        .unwrap_or_else(ControllerProfile::generic)
}

/// Replace or append profiles loaded from a JSON array or a single object.
pub fn merge_profile_json(
    profiles: &mut Vec<ControllerProfile>,
    json: &str,
) -> Result<(), serde_json::Error> {
    let trimmed = json.trim();
    if trimmed.starts_with('[') {
        let extra: Vec<ControllerProfile> = serde_json::from_str(trimmed)?;
        for profile in extra {
            upsert_profile(profiles, profile);
        }
    } else {
        let profile: ControllerProfile = serde_json::from_str(trimmed)?;
        upsert_profile(profiles, profile);
    }
    Ok(())
}

fn upsert_profile(profiles: &mut Vec<ControllerProfile>, profile: ControllerProfile) {
    if let Some(existing) = profiles.iter_mut().find(|p| p.id == profile.id) {
        *existing = profile;
    } else {
        // Keep generic last so named maps win lookup.
        let generic_at = profiles.iter().position(|p| p.id == "generic");
        if let Some(i) = generic_at {
            profiles.insert(i, profile);
        } else {
            profiles.push(profile);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpk_phrase_row_swap_matches_legacy() {
        assert_eq!(mpk_note_for_phrase_cell(0), 40);
        assert_eq!(mpk_note_for_phrase_cell(4), 36);
        let p = ControllerProfile::mpk_mini();
        assert_eq!(p.note_for_phrase_cell(0), 40);
        assert_eq!(p.punch_index_for_pad(40), Some(0));
        assert_eq!(p.punch_index_for_knob(70), Some(0));
        assert_eq!(p.punch_index_for_knob(1), None);
    }

    #[test]
    fn port_name_selects_mpk_not_generic() {
        reset_installed_profiles();
        let p = profile_for_port_name("MPK mini 3");
        assert_eq!(p.id, "mpk-mini");
        let din = profile_for_port_name("U2MIDI PRO:U2MIDI PRO MIDI 1 20:0");
        assert_eq!(din.id, "generic");
    }

    #[test]
    fn port_name_selects_smk_elite_not_mpk() {
        reset_installed_profiles();
        let p = profile_for_port_name("SMK-37 Elite-Master:SMK-37 Elite-Master MIDI 1 20:0");
        assert_eq!(p.id, "smk-37-elite");
        assert_eq!(p.knob_ccs[0], 48);
        assert!(p.skip_audio);
        let mpk = profile_for_port_name("MPK mini 3");
        assert_eq!(mpk.id, "mpk-mini");
        assert_eq!(mpk.knob_ccs[0], 70);
    }

    #[test]
    fn json_overlay_replaces_builtin_by_id() {
        let mut profiles = builtin_profiles();
        merge_profile_json(
            &mut profiles,
            r#"{"id":"mpk-mini","name_hints":["mpk"],"knob_ccs":[10,11,12,13,14,15,16,17]}"#,
        )
        .unwrap();
        let p = profile_for_port_name_in("MPK mini 3", &profiles);
        assert_eq!(p.knob_ccs[0], 10);
    }

    #[test]
    fn smk_profile_round_trips_through_json() {
        let p = ControllerProfile::smk_37_elite();
        let text = serde_json::to_string_pretty(&p).unwrap();
        let back: ControllerProfile = serde_json::from_str(&text).unwrap();
        assert_eq!(p, back);
        assert_eq!(p.knob_ccs, [48, 49, 50, 51, 52, 53, 54, 55]);
    }
}

#[cfg(test)]
pub fn reset_installed_profiles() {
    *registry().lock().unwrap_or_else(|p| p.into_inner()) = builtin_profiles();
}
