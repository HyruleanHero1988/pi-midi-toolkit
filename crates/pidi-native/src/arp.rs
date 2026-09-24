//! Authored key-relative arpeggiator UI state.

use jambox_core::{ArpDivision, ArpOrder, MAX_ARP_OCTAVES, MAX_ARP_STEPS};

use crate::session::OutMode;

const GATE_PRESETS: [u8; 4] = [50, 70, 90, 127];

#[derive(Debug, Clone)]
pub struct ArpUi {
    pub order: ArpOrder,
    pub division: ArpDivision,
    pub octaves: u8,
    pub gate: u8,
    pub latch: bool,
    pub steps: [i8; MAX_ARP_STEPS],
    pub len: u8,
    pub selected: usize,
    pub out: OutMode,
}

impl Default for ArpUi {
    fn default() -> Self {
        Self::new()
    }
}

impl ArpUi {
    pub fn new() -> Self {
        let mut steps = [0i8; MAX_ARP_STEPS];
        steps[0] = 0;
        steps[1] = 4;
        steps[2] = 7;
        Self {
            order: ArpOrder::Up,
            division: ArpDivision::Sixteenth,
            octaves: 1,
            gate: 90,
            latch: false,
            steps,
            len: 3,
            selected: 0,
            out: OutMode::Both,
        }
    }

    pub fn selected_interval(&self) -> i8 {
        self.steps[self.selected.min(MAX_ARP_STEPS - 1)]
    }

    pub fn interval_label(interval: i8) -> String {
        format!("{interval:+}")
    }

    pub fn gate_label(&self) -> String {
        format!("{}%", (u16::from(self.gate) * 100 / 127).max(1))
    }

    pub fn cycle_order(&mut self) {
        let i = ArpOrder::ALL
            .iter()
            .position(|o| *o == self.order)
            .unwrap_or(0);
        self.order = ArpOrder::ALL[(i + 1) % ArpOrder::ALL.len()];
    }

    pub fn cycle_division(&mut self) {
        let i = ArpDivision::ALL
            .iter()
            .position(|d| *d == self.division)
            .unwrap_or(0);
        self.division = ArpDivision::ALL[(i + 1) % ArpDivision::ALL.len()];
    }

    pub fn cycle_gate(&mut self) {
        let i = GATE_PRESETS.iter().position(|g| *g == self.gate).unwrap_or(0);
        self.gate = GATE_PRESETS[(i + 1) % GATE_PRESETS.len()];
    }

    pub fn bump_octaves(&mut self, delta: i8) {
        let next = i16::from(self.octaves) + i16::from(delta);
        self.octaves = next.clamp(0, i16::from(MAX_ARP_OCTAVES)) as u8;
    }

    pub fn bump_interval(&mut self, delta: i8) {
        let i = self.selected.min(MAX_ARP_STEPS - 1);
        self.steps[i] = (self.steps[i] + delta).clamp(-24, 24);
    }

    pub fn add_step(&mut self) {
        if (self.len as usize) >= MAX_ARP_STEPS {
            return;
        }
        let insert = (self.selected + 1).min(self.len as usize);
        for j in (insert..self.len as usize).rev() {
            self.steps[j + 1] = self.steps[j];
        }
        self.steps[insert] = 0;
        self.len += 1;
        self.selected = insert;
    }

    pub fn del_step(&mut self) {
        if self.len <= 1 {
            self.steps[0] = 0;
            self.selected = 0;
            return;
        }
        let i = self.selected.min(self.len as usize - 1);
        for j in i..(self.len as usize - 1) {
            self.steps[j] = self.steps[j + 1];
        }
        self.len -= 1;
        if self.selected >= self.len as usize {
            self.selected = self.len as usize - 1;
        }
    }

    pub fn select_step(&mut self, index: usize) {
        if index < self.len as usize {
            self.selected = index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jambox_core::ArpOrder;

    #[test]
    fn add_and_delete_keep_a_live_pattern() {
        let mut arp = ArpUi::new();
        arp.selected = 2;
        arp.add_step();
        assert_eq!(arp.len, 4);
        assert_eq!(arp.selected, 3);
        arp.del_step();
        assert_eq!(arp.len, 3);
    }

    #[test]
    fn interval_labels_show_sign() {
        assert_eq!(ArpUi::interval_label(0), "+0");
        assert_eq!(ArpUi::interval_label(7), "+7");
        assert_eq!(ArpUi::interval_label(-5), "-5");
    }

    #[test]
    fn chrome_labels_fit_retro_font() {
        let arp = ArpUi::new();
        let labels = [
            arp.order.label().to_string(),
            arp.division.label().to_string(),
            arp.gate_label(),
            arp.out.short_label().to_string(),
            ArpUi::interval_label(-5),
            ArpUi::interval_label(12),
            "OCT-".into(),
            "OCT+".into(),
            "-".into(),
            "+".into(),
            "ADD".into(),
            "DEL".into(),
            "KEY-".into(),
            "KEY+".into(),
            "HOLD".into(),
            "LATCH".into(),
            "PLAY A ROOT  C4".into(),
            "LATCHED  C4  NEXT KEY".into(),
        ];
        for label in &labels {
            for ch in label.chars() {
                assert!(
                    ch.is_ascii() && (ch.is_ascii_graphic() || ch == ' '),
                    "arp label {label:?} uses {ch:?}, which the 5x7 font cannot draw"
                );
            }
        }
        for order in ArpOrder::ALL {
            for ch in order.label().chars() {
                assert!(
                    ch.is_ascii() && ch.is_ascii_graphic(),
                    "order {} uses {ch:?}",
                    order.label()
                );
            }
        }
    }
}
