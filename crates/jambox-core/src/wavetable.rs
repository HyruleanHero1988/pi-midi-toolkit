//! Single-cycle wavetable bank and A/B morph blending.
//!
//! Tables are loaded once (host side, off the audio thread). The morph table is a
//! preallocated scratch buffer rebuilt only when A/B/blend actually change.

/// Samples per single-cycle table. Power of two so phase wrap is a mask.
pub const TABLE_SIZE: usize = 2048;
/// Mask for wrapping a table index.
pub const TABLE_MASK: usize = TABLE_SIZE - 1;
/// Peak amplitude tables are normalized to.
pub const TABLE_PEAK: f32 = 0.90;

/// Named single-cycle tables plus the live morph blend.
///
/// `morph = 0.0` is pure A, `1.0` is pure B.
pub struct WaveBank {
    names: Vec<String>,
    tables: Vec<[f32; TABLE_SIZE]>,
    morph_table: [f32; TABLE_SIZE],
    index_a: usize,
    index_b: usize,
    morph: f32,
    dirty: bool,
}

impl WaveBank {
    /// Build a bank with the four procedural built-ins.
    pub fn with_builtins() -> Self {
        let mut bank = Self {
            names: Vec::new(),
            tables: Vec::new(),
            morph_table: [0.0; TABLE_SIZE],
            index_a: 0,
            index_b: 0,
            morph: 0.0,
            dirty: true,
        };
        let mut sine = [0.0f32; TABLE_SIZE];
        let mut square = [0.0f32; TABLE_SIZE];
        let mut saw = [0.0f32; TABLE_SIZE];
        let mut triangle = [0.0f32; TABLE_SIZE];
        for i in 0..TABLE_SIZE {
            let t = i as f32 / TABLE_SIZE as f32;
            sine[i] = (t * std::f32::consts::TAU).sin() * TABLE_PEAK;
            square[i] = if t < 0.5 { TABLE_PEAK } else { -TABLE_PEAK };
            saw[i] = (2.0 * (t - (t + 0.5).floor())) * TABLE_PEAK;
            triangle[i] = (2.0 * (2.0 * (t - (t + 0.5).floor())).abs() - 1.0) * TABLE_PEAK;
        }
        bank.push("sine", sine);
        bank.push("square", square);
        bank.push("saw", saw);
        bank.push("triangle", triangle);
        bank.index_b = 1.min(bank.len() - 1);
        bank.rebuild_morph();
        bank
    }

    fn push(&mut self, name: &str, table: [f32; TABLE_SIZE]) {
        self.names.push(name.to_string());
        self.tables.push(table);
    }

    /// Add or replace a table by name. Off the audio thread only (allocates).
    pub fn insert(&mut self, name: &str, samples: &[f32]) -> usize {
        let table = resample_cycle(samples);
        let key = name.to_ascii_lowercase();
        if let Some(idx) = self.names.iter().position(|n| *n == key) {
            self.tables[idx] = table;
            self.dirty = true;
            idx
        } else {
            self.push(&key, table);
            self.dirty = true;
            self.names.len() - 1
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        let key = name.to_ascii_lowercase();
        self.names.iter().position(|n| *n == key)
    }

    pub fn table(&self, index: usize) -> &[f32; TABLE_SIZE] {
        &self.tables[index.min(self.tables.len() - 1)]
    }

    pub fn morph_pair(&self) -> (usize, usize, f32) {
        (self.index_a, self.index_b, self.morph)
    }

    /// Index of the endpoint the blend is nearer to — the FX insert slot in use.
    pub fn nearer_index(&self) -> usize {
        if self.morph < 0.5 {
            self.index_a
        } else {
            self.index_b
        }
    }

    pub fn set_morph_pair(&mut self, a: usize, b: usize) {
        let max = self.len().saturating_sub(1);
        self.index_a = a.min(max);
        self.index_b = b.min(max);
        self.dirty = true;
    }

    pub fn set_morph(&mut self, morph: f32) {
        let morph = morph.clamp(0.0, 1.0);
        if (morph - self.morph).abs() > f32::EPSILON {
            self.morph = morph;
            self.dirty = true;
        }
    }

    /// Rebuild the blended table if A/B/morph changed. Cheap and alloc-free.
    pub fn rebuild_morph(&mut self) {
        if !self.dirty {
            return;
        }
        let a = &self.tables[self.index_a.min(self.tables.len() - 1)];
        let b = &self.tables[self.index_b.min(self.tables.len() - 1)];
        let frac = self.morph;
        let inv = 1.0 - frac;
        for i in 0..TABLE_SIZE {
            self.morph_table[i] = a[i] * inv + b[i] * frac;
        }
        self.dirty = false;
    }

    pub fn morph_table(&self) -> &[f32; TABLE_SIZE] {
        &self.morph_table
    }

    /// Wavetable for a live voice group. Morph A/B always hear the blend, even
    /// after the nearer endpoint (and FX slot) flips at 50%.
    pub fn table_for_live_group(&self, group: usize) -> &[f32; TABLE_SIZE] {
        let (a, b, _) = self.morph_pair();
        if a != b && (group == a || group == b) {
            self.morph_table()
        } else {
            self.table(group)
        }
    }

    /// Freeze the live morph blend as a new named voice (host side; allocates).
    pub fn bake_morph_as(&mut self, name: &str) -> usize {
        self.rebuild_morph();
        let baked = self.morph_table;
        let idx = self.insert(name, &baked);
        self.set_morph_pair(idx, idx);
        self.set_morph(0.0);
        self.rebuild_morph();
        idx
    }
}

/// Resample an arbitrary-length cycle to `TABLE_SIZE` and normalize to `TABLE_PEAK`.
pub fn resample_cycle(input: &[f32]) -> [f32; TABLE_SIZE] {
    let mut out = [0.0f32; TABLE_SIZE];
    if input.is_empty() {
        return out;
    }
    let n = input.len();
    for (i, slot) in out.iter_mut().enumerate() {
        let pos = (i as f64 / TABLE_SIZE as f64) * n as f64;
        let i0 = pos.floor() as usize % n;
        let i1 = (i0 + 1) % n;
        let frac = (pos - pos.floor()) as f32;
        *slot = input[i0] * (1.0 - frac) + input[i1] * frac;
    }
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 1e-9 {
        let scale = TABLE_PEAK / peak;
        for v in out.iter_mut() {
            *v *= scale;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_present_and_normalized() {
        let bank = WaveBank::with_builtins();
        assert_eq!(bank.len(), 4);
        assert_eq!(bank.index_of("saw"), Some(2));
        let peak = bank.table(0).iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - TABLE_PEAK).abs() < 1e-3);
    }

    #[test]
    fn morph_blends_between_endpoints() {
        let mut bank = WaveBank::with_builtins();
        bank.set_morph_pair(0, 2); // sine → saw
        bank.set_morph(0.0);
        bank.rebuild_morph();
        assert!((bank.morph_table()[10] - bank.table(0)[10]).abs() < 1e-6);
        bank.set_morph(1.0);
        bank.rebuild_morph();
        assert!((bank.morph_table()[10] - bank.table(2)[10]).abs() < 1e-6);
        bank.set_morph(0.5);
        bank.rebuild_morph();
        let expect = 0.5 * bank.table(0)[10] + 0.5 * bank.table(2)[10];
        assert!((bank.morph_table()[10] - expect).abs() < 1e-6);
    }

    #[test]
    fn nearer_index_follows_blend() {
        let mut bank = WaveBank::with_builtins();
        bank.set_morph_pair(1, 3);
        bank.set_morph(0.2);
        assert_eq!(bank.nearer_index(), 1);
        bank.set_morph(0.8);
        assert_eq!(bank.nearer_index(), 3);
    }

    #[test]
    fn live_group_keeps_the_blend_after_the_halfway_flip() {
        let mut bank = WaveBank::with_builtins();
        bank.set_morph_pair(0, 2);
        bank.set_morph(0.9);
        bank.rebuild_morph();
        let blended = bank.morph_table()[10];
        assert!((bank.table_for_live_group(0)[10] - blended).abs() < 1e-6);
        assert!((bank.table_for_live_group(2)[10] - blended).abs() < 1e-6);
        assert!((bank.table_for_live_group(0)[10] - bank.table(0)[10]).abs() > 1e-3);
        assert_eq!(bank.table_for_live_group(1)[10], bank.table(1)[10]);
    }

    #[test]
    fn bake_morph_adds_voice_and_selects_it() {
        let mut bank = WaveBank::with_builtins();
        bank.set_morph_pair(0, 2);
        bank.set_morph(0.5);
        bank.rebuild_morph();
        let idx = bank.bake_morph_as("halfsaw");
        assert_eq!(bank.len(), 5);
        assert_eq!(bank.index_of("halfsaw"), Some(idx));
        let (a, b, m) = bank.morph_pair();
        assert_eq!((a, b), (idx, idx));
        assert_eq!(m, 0.0);
    }

    #[test]
    fn resample_handles_short_input() {
        let table = resample_cycle(&[0.0, 1.0, 0.0, -1.0]);
        let peak = table.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - TABLE_PEAK).abs() < 1e-4);
    }
}
