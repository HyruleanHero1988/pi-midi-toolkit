//! Wi-Fi helpers for the SET → WIFI panel (NetworkManager / nmcli parity with Tk).

use std::path::Path;

pub const DEFAULT_IFACE: &str = "wlan0";
pub const LIST_VISIBLE: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: i32,
    pub security: String,
    pub in_use: bool,
}

impl WifiNetwork {
    pub fn is_open(&self) -> bool {
        let sec = self.security.trim().to_ascii_uppercase();
        sec.is_empty() || sec == "--" || sec == "NONE"
    }

    pub fn label(&self) -> String {
        let mark = if self.in_use { "* " } else { "" };
        let kind = if self.is_open() { "open" } else { "secured" };
        if self.signal > 0 {
            format!("{mark}{} · {}% · {kind}", self.ssid, self.signal)
        } else {
            format!("{mark}{} · {kind}", self.ssid)
        }
    }
}

/// Split an nmcli `-t` line, honoring backslash escapes (SSID may contain `:`).
pub fn split_nmcli_line(line: &str, expected: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' && i + 1 < chars.len() {
            buf.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if ch == ':' && parts.len() + 1 < expected {
            parts.push(std::mem::take(&mut buf));
            i += 1;
            continue;
        }
        buf.push(ch);
        i += 1;
    }
    parts.push(buf);
    while parts.len() < expected {
        parts.push(String::new());
    }
    parts.truncate(expected);
    parts
}

/// Parse `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list`.
pub fn parse_wifi_list(output: &str) -> Vec<WifiNetwork> {
    use std::collections::HashMap;
    let mut found: HashMap<String, WifiNetwork> = HashMap::new();
    for raw in output.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let parts = split_nmcli_line(line, 4);
        let in_use = parts[0].trim();
        let ssid = parts[1].trim().to_string();
        if ssid.is_empty() || ssid == "--" {
            continue;
        }
        let signal = parts[2].trim().parse::<i32>().unwrap_or(0);
        let security = parts[3].trim().to_string();
        let active = matches!(
            in_use.to_ascii_lowercase().as_str(),
            "yes" | "*" | "1" | "true"
        );
        match found.get_mut(&ssid) {
            Some(prev) => {
                if signal > prev.signal || (active && !prev.in_use) {
                    prev.signal = signal.max(prev.signal);
                    prev.security = security;
                }
                prev.in_use = prev.in_use || active;
            }
            None => {
                found.insert(
                    ssid.clone(),
                    WifiNetwork {
                        ssid,
                        signal,
                        security,
                        in_use: active,
                    },
                );
            }
        }
    }
    let mut networks: Vec<_> = found.into_values().collect();
    networks.sort_by(|a, b| {
        b.in_use
            .cmp(&a.in_use)
            .then(b.signal.cmp(&a.signal))
            .then(a.ssid.to_ascii_lowercase().cmp(&b.ssid.to_ascii_lowercase()))
    });
    networks
}

pub fn save_wifi_credentials(path: &Path, ssid: &str, password: &str, iface: &str) -> std::io::Result<()> {
    let body = format!(
        "WIFI_SSID={ssid}\nWIFI_PASSWORD={password}\nWIFI_IFACE={}\n",
        if iface.is_empty() {
            DEFAULT_IFACE
        } else {
            iface
        }
    );
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Switch-style OSK key: (label, action, column span). Actions are inserted
/// text or command tokens (`shift`, `sym`, `abc`, `back`, `space`, `ok`, `pad`).
pub type KeySpec = (&'static str, &'static str, u8);

pub const KB_COLS: i32 = 12;

pub fn keyboard_abc_rows(shift: bool) -> Vec<Vec<KeySpec>> {
    let letters = |lower: &str| -> Vec<KeySpec> {
        lower
            .chars()
            .map(|c| {
                if shift && c.is_ascii_alphabetic() {
                    let u = c.to_ascii_uppercase();
                    // Static labels for uppercase — map known letters.
                    match u {
                        'Q' => ("Q", "Q", 1),
                        'W' => ("W", "W", 1),
                        'E' => ("E", "E", 1),
                        'R' => ("R", "R", 1),
                        'T' => ("T", "T", 1),
                        'Y' => ("Y", "Y", 1),
                        'U' => ("U", "U", 1),
                        'I' => ("I", "I", 1),
                        'O' => ("O", "O", 1),
                        'P' => ("P", "P", 1),
                        'A' => ("A", "A", 1),
                        'S' => ("S", "S", 1),
                        'D' => ("D", "D", 1),
                        'F' => ("F", "F", 1),
                        'G' => ("G", "G", 1),
                        'H' => ("H", "H", 1),
                        'J' => ("J", "J", 1),
                        'K' => ("K", "K", 1),
                        'L' => ("L", "L", 1),
                        'Z' => ("Z", "Z", 1),
                        'X' => ("X", "X", 1),
                        'C' => ("C", "C", 1),
                        'V' => ("V", "V", 1),
                        'B' => ("B", "B", 1),
                        'N' => ("N", "N", 1),
                        'M' => ("M", "M", 1),
                        _ => ("?", "?", 1),
                    }
                } else {
                    match c {
                        'q' => ("q", "q", 1),
                        'w' => ("w", "w", 1),
                        'e' => ("e", "e", 1),
                        'r' => ("r", "r", 1),
                        't' => ("t", "t", 1),
                        'y' => ("y", "y", 1),
                        'u' => ("u", "u", 1),
                        'i' => ("i", "i", 1),
                        'o' => ("o", "o", 1),
                        'p' => ("p", "p", 1),
                        'a' => ("a", "a", 1),
                        's' => ("s", "s", 1),
                        'd' => ("d", "d", 1),
                        'f' => ("f", "f", 1),
                        'g' => ("g", "g", 1),
                        'h' => ("h", "h", 1),
                        'j' => ("j", "j", 1),
                        'k' => ("k", "k", 1),
                        'l' => ("l", "l", 1),
                        'z' => ("z", "z", 1),
                        'x' => ("x", "x", 1),
                        'c' => ("c", "c", 1),
                        'v' => ("v", "v", 1),
                        'b' => ("b", "b", 1),
                        'n' => ("n", "n", 1),
                        'm' => ("m", "m", 1),
                        _ => ("?", "?", 1),
                    }
                }
            })
            .collect()
    };
    let mut row2 = letters("qwertyuiop");
    row2.push(("/", "/", 1));
    row2.push(("", "pad", 1));
    let mut row3 = letters("asdfghjkl");
    row3.push((":", ":", 1));
    row3.push(("'", "'", 1));
    row3.push(("", "pad", 1));
    let mut row4 = letters("zxcvbnm");
    row4.extend([
        (",", ",", 1),
        (".", ".", 1),
        ("?", "?", 1),
        ("!", "!", 1),
        ("", "pad", 1),
    ]);
    vec![
        vec![
            ("1", "1", 1),
            ("2", "2", 1),
            ("3", "3", 1),
            ("4", "4", 1),
            ("5", "5", 1),
            ("6", "6", 1),
            ("7", "7", 1),
            ("8", "8", 1),
            ("9", "9", 1),
            ("0", "0", 1),
            ("-", "-", 1),
            ("⌫", "back", 1),
        ],
        row2,
        row3,
        row4,
        vec![
            ("⇧", "shift", 2),
            ("#+=", "sym", 2),
            ("Space", "space", 5),
            ("OK", "ok", 3),
        ],
    ]
}

pub fn keyboard_sym_rows() -> Vec<Vec<KeySpec>> {
    vec![
        vec![
            ("1", "1", 1),
            ("2", "2", 1),
            ("3", "3", 1),
            ("4", "4", 1),
            ("5", "5", 1),
            ("6", "6", 1),
            ("7", "7", 1),
            ("8", "8", 1),
            ("9", "9", 1),
            ("0", "0", 1),
            ("_", "_", 1),
            ("⌫", "back", 1),
        ],
        vec![
            ("!", "!", 1),
            ("@", "@", 1),
            ("#", "#", 1),
            ("$", "$", 1),
            ("%", "%", 1),
            ("^", "^", 1),
            ("&", "&", 1),
            ("*", "*", 1),
            ("(", "(", 1),
            (")", ")", 1),
            ("+", "+", 1),
            ("=", "=", 1),
        ],
        vec![
            ("~", "~", 1),
            ("`", "`", 1),
            ("{", "{", 1),
            ("}", "}", 1),
            ("[", "[", 1),
            ("]", "]", 1),
            ("\\", "\\", 1),
            ("|", "|", 1),
            (";", ";", 1),
            ("\"", "\"", 1),
            ("<", "<", 1),
            (">", ">", 1),
        ],
        vec![
            ("ABC", "abc", 3),
            ("Space", "space", 6),
            ("OK", "ok", 3),
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wifi_list_dedupes_and_sorts() {
        let raw = "\
:Cafe:40:WPA2
yes:HomeNet:80:WPA2
:Cafe:55:WPA2
:OpenGuest:20:
:Weird\\:Name:10:WPA3
";
        let nets = parse_wifi_list(raw);
        assert_eq!(nets[0].ssid, "HomeNet");
        assert!(nets[0].in_use);
        let cafe = nets.iter().find(|n| n.ssid == "Cafe").unwrap();
        assert_eq!(cafe.signal, 55);
        let guest = nets.iter().find(|n| n.ssid == "OpenGuest").unwrap();
        assert!(guest.is_open());
        let weird = nets.iter().find(|n| n.ssid == "Weird:Name").unwrap();
        assert_eq!(weird.signal, 10);
    }

    #[test]
    fn keyboard_has_common_wifi_punctuation() {
        let mut chars = std::collections::HashSet::new();
        for shift in [false, true] {
            for row in keyboard_abc_rows(shift) {
                for (_l, action, _) in row {
                    if !matches!(
                        action,
                        "shift" | "sym" | "abc" | "back" | "space" | "ok" | "pad"
                    ) {
                        chars.insert(action);
                    }
                }
            }
        }
        for row in keyboard_sym_rows() {
            for (_l, action, _) in row {
                if !matches!(
                    action,
                    "shift" | "sym" | "abc" | "back" | "space" | "ok" | "pad"
                ) {
                    chars.insert(action);
                }
            }
        }
        for need in ["@", "-", "_", "!", "?", ".", ",", "#"] {
            assert!(chars.contains(need), "missing {need}");
        }
    }
}
