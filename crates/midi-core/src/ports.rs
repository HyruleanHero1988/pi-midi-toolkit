//! Port-name helpers shared by the engine and the kiosk.
//!
//! ALSA/`midir` names look like `U2MIDI PRO:U2MIDI PRO MIDI 1 20:0`.

/// Virtual / loopback ports the appliance should not auto-grab.
pub fn is_virtual_port_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("through") || n.contains("jambox")
}

/// Controllers known to already have piano keys (hide the on-screen keyboard).
const KEYBED_HINTS: &[&str] = &[
    "mpk",
    "keystation",
    "oxygen",
    "lpk",
    "minilab",
    "launchkey",
    "keystep",
    "seaboard",
    "smk",
    "m-vave",
    "m vave",
    "mvave",
    "vave",
    "elite",
    "starrykey",
    "mke-p37",
    "mke_p37",
];

/// True when this ALSA/`midir` name looks like a hardware keybed.
pub fn port_looks_like_keybed(name: &str) -> bool {
    if name.is_empty() || is_virtual_port_name(name) {
        return false;
    }
    let n = name.to_ascii_lowercase();
    KEYBED_HINTS.iter().any(|hint| n.contains(hint))
}

/// USB MIDI or composite USB audio that belongs to a keyboard, not the DIN
/// synth or the Pi headphone jack. Auto-pick skips these unless the user
/// filter names them.
pub fn port_looks_like_controller_io(name: &str) -> bool {
    if name.is_empty() || is_virtual_port_name(name) {
        return false;
    }
    let profile = crate::profile_for_port_name(name);
    if profile.skip_audio || (profile.keybed && profile.id != "generic") {
        return true;
    }
    port_looks_like_keybed(name)
}

/// Drop the ALSA `client:port` suffix and prefer the name after the first colon.
pub fn short_port_label(name: &str) -> &str {
    let trimmed = match name.rsplit_once(' ') {
        Some((left, rest))
            if rest.contains(':') && rest.bytes().all(|b| b.is_ascii_digit() || b == b':') =>
        {
            left
        }
        _ => name,
    };
    match trimmed.split_once(':') {
        Some((_, rest)) if !rest.is_empty() => rest,
        _ => trimmed,
    }
}

/// Names that should be opened for this filter.
///
/// Empty filter = every class-compliant hardware port (skip Through / engine
/// loopback). A non-empty filter is a case-insensitive substring.
pub fn matching_port_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
    filter: &str,
) -> Vec<String> {
    let filter_lc = filter.trim().to_ascii_lowercase();
    names
        .into_iter()
        .filter(|name| {
            if filter_lc.is_empty() {
                !is_virtual_port_name(name)
            } else {
                name.to_ascii_lowercase().contains(&filter_lc)
            }
        })
        .map(str::to_string)
        .collect()
}

/// First matching name. Empty filter → first non-virtual hardware port.
pub fn pick_port_name<'a>(
    names: impl IntoIterator<Item = &'a str>,
    filter: &str,
) -> Option<String> {
    matching_port_names(names, filter).into_iter().next()
}

/// MIDI *thru / DIN* output. Empty filter skips keybeds so an SMK or MPK
/// cannot steal the U2MIDI port just because it enumerated first.
pub fn pick_thru_output_name<'a>(
    names: impl IntoIterator<Item = &'a str>,
    filter: &str,
) -> Option<String> {
    if !filter.trim().is_empty() {
        return pick_port_name(names, filter);
    }
    matching_port_names(names, "")
        .into_iter()
        .find(|n| !port_looks_like_controller_io(n))
}

/// Cycle Auto → each hardware port → Auto. Virtual ports are skipped unless
/// they are the only options.
pub fn cycle_port_filter(current: &str, names: &[String]) -> String {
    let hardware: Vec<&String> = names.iter().filter(|n| !is_virtual_port_name(n)).collect();
    let list: Vec<&String> = if hardware.is_empty() {
        names.iter().collect()
    } else {
        hardware
    };
    if list.is_empty() {
        return String::new();
    }
    let current_lc = current.trim().to_ascii_lowercase();
    let idx = if current_lc.is_empty() {
        None
    } else {
        list.iter()
            .position(|n| n.to_ascii_lowercase().contains(&current_lc))
    };
    match idx {
        None => list[0].clone(),
        Some(i) if i + 1 < list.len() => list[i + 1].clone(),
        Some(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_label_strips_alsa_suffix() {
        assert_eq!(
            short_port_label("U2MIDI PRO:U2MIDI PRO MIDI 1 20:0"),
            "U2MIDI PRO MIDI 1"
        );
        assert_eq!(short_port_label("MPK mini 3"), "MPK mini 3");
    }

    #[test]
    fn empty_filter_opens_every_hardware_port() {
        let names = [
            "Midi Through:Midi Through Port-0 14:0",
            "jambox-out:jambox-out 129:0",
            "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0",
            "Keystation Mini 32",
        ];
        assert_eq!(
            matching_port_names(names, ""),
            vec![
                "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0".to_string(),
                "Keystation Mini 32".to_string(),
            ]
        );
        assert_eq!(
            pick_port_name(names, "").as_deref(),
            Some("U2MIDI PRO:U2MIDI PRO MIDI 1 20:0")
        );
        assert!(matching_port_names(names, "MPK").is_empty());
        assert_eq!(
            pick_port_name(names, "U2MIDI").as_deref(),
            Some("U2MIDI PRO:U2MIDI PRO MIDI 1 20:0")
        );
    }

    #[test]
    fn cycle_walks_hardware_then_auto() {
        let names = vec![
            "Midi Through:Midi Through Port-0 14:0".into(),
            "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0".into(),
            "MPK mini 3".into(),
        ];
        let first = cycle_port_filter("", &names);
        assert!(first.contains("U2MIDI"));
        let second = cycle_port_filter(&first, &names);
        assert!(second.contains("MPK"));
        let third = cycle_port_filter(&second, &names);
        assert!(third.is_empty());
    }

    #[test]
    fn keybed_names_match_mpk_not_din() {
        assert!(port_looks_like_keybed("MPK mini 3"));
        assert!(port_looks_like_keybed("Keystation Mini 32"));
        assert!(!port_looks_like_keybed("U2MIDI PRO:U2MIDI PRO MIDI 1 20:0"));
        assert!(!port_looks_like_keybed(
            "Midi Through:Midi Through Port-0 14:0"
        ));
        assert!(!port_looks_like_keybed(""));
    }

    #[test]
    fn keybed_names_match_smk_37_family() {
        assert!(port_looks_like_keybed("SMK-37 Elite"));
        assert!(port_looks_like_keybed(
            "SMK-37 Elite-Master:SMK-37 Elite-Master MIDI 1 20:0"
        ));
        assert!(port_looks_like_keybed("M-VAVE SMK-37 Pro"));
        assert!(port_looks_like_keybed("Starrykey 37 Play"));
        assert!(port_looks_like_keybed("MKE-P37"));
        assert!(!port_looks_like_keybed("U2MIDI PRO:U2MIDI PRO MIDI 1 20:0"));
    }

    #[test]
    fn auto_midi_out_skips_keybeds_for_the_din_interface() {
        let names = [
            "SMK-37 Elite-Master:SMK-37 Elite-Master MIDI 1 20:0",
            "MPK mini 3",
            "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0",
        ];
        assert_eq!(
            pick_thru_output_name(names, "").as_deref(),
            Some("U2MIDI PRO:U2MIDI PRO MIDI 1 20:0")
        );
        assert_eq!(
            pick_thru_output_name(names, "SMK").as_deref(),
            Some("SMK-37 Elite-Master:SMK-37 Elite-Master MIDI 1 20:0"),
            "an explicit OUT filter can still target the controller"
        );
        assert!(port_looks_like_controller_io("SMK-37 Elite USB Audio"));
        assert!(!port_looks_like_controller_io(
            "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0"
        ));
    }
}
