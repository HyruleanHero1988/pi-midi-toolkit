//! Port-name helpers shared by the engine and the kiosk.
//!
//! ALSA/`midir` names look like `U2MIDI PRO:U2MIDI PRO MIDI 1 20:0`.

/// Virtual / loopback ports the appliance should not auto-grab.
pub fn is_virtual_port_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("through") || n.contains("jambox")
}

/// Drop the ALSA `client:port` suffix and prefer the name after the first colon.
pub fn short_port_label(name: &str) -> &str {
    let trimmed = match name.rsplit_once(' ') {
        Some((left, rest))
            if rest.contains(':')
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b':') =>
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
pub fn pick_port_name<'a>(names: impl IntoIterator<Item = &'a str>, filter: &str) -> Option<String> {
    matching_port_names(names, filter).into_iter().next()
}

/// Cycle Auto → each hardware port → Auto. Virtual ports are skipped unless
/// they are the only options.
pub fn cycle_port_filter(current: &str, names: &[String]) -> String {
    let hardware: Vec<&String> = names
        .iter()
        .filter(|n| !is_virtual_port_name(n))
        .collect();
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
}
