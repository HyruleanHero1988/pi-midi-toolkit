//! EP-style punch-in FX on the master bus.
//!
//! Eight pads, layered. Buffer effects (repeat / tape / drop) share one ring;
//! who wins when several are armed is set by [`PunchBufferPrio`]. Filters,
//! crush, and slice run after. SEND is a wet-boost of the bus insert that
//! runs after this rack (so it still works on a GRAB freeze).
//!
//! Amounts are 0..1. The UI maps pad Y (and latch) onto them; this unit only
//! hears the current amount per slot. `process` never allocates.

pub const PUNCH_PAD_COUNT: usize = 8;
pub const PUNCH_RPT: u8 = 0;
pub const PUNCH_TAPE: u8 = 1;
pub const PUNCH_LPF: u8 = 2;
pub const PUNCH_HPF: u8 = 3;
pub const PUNCH_SEND: u8 = 4;
pub const PUNCH_SLICE: u8 = 5;
pub const PUNCH_DROP: u8 = 6;
pub const PUNCH_CRUSH: u8 = 7;

/// Continuous history kept for punch pads + LOCK grabs (vaporwave chorus length).
const RING_SEC: f32 = 4.0;
const ENGAGE: f32 = 0.04;

pub const PUNCH_LABELS: [&str; PUNCH_PAD_COUNT] =
    ["RPT", "TAPE", "LPF", "HPF", "SEND", "SLICE", "DROP", "CRUSH"];
/// MPK mini factory knobs 1–8 (Prog Select → Pad 1).
pub const PUNCH_KNOB_CCS: [u8; PUNCH_PAD_COUNT] = [70, 71, 72, 73, 74, 75, 76, 77];
/// MPK factory Bank A pads, row-swapped to match the FX grid.
pub const PUNCH_PAD_NOTES: [u8; PUNCH_PAD_COUNT] = [40, 41, 42, 43, 36, 37, 38, 39];

/// Which mix bus a punch buffer records and plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PunchSource {
    #[default]
    Keys = 0,
    Drums = 1,
    Mic = 2,
}

impl PunchSource {
    pub const COUNT: usize = 3;

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Drums,
            2 => Self::Mic,
            _ => Self::Keys,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Keys => "KEYS",
            Self::Drums => "DRM",
            Self::Mic => "MIC",
        }
    }

    pub fn next(self) -> Self {
        Self::from_u8((self.as_u8() + 1) % Self::COUNT as u8)
    }
}

/// RPT / SLICE beat lengths as amount rises: 1/4, 1/8, 1/16, 1/32.
pub const PUNCH_GRID_BEATS: [f32; 4] = [1.0, 0.5, 0.25, 0.125];
/// Slider marks at the 1/8, 1/16, and 1/32 boundaries (1/4 is the bottom band).
pub const PUNCH_GRID_TICKS: [f32; 3] = [0.25, 0.50, 0.75];
pub const PUNCH_GRID_LABELS: [&str; 4] = ["1/4", "1/8", "1/16", "1/32"];

/// Who wins when several buffer pads are armed. Cycles on the FX page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PunchBufferPrio {
    /// Classic bounce: TAPE owns the ring; release it and RPT/DROP take over.
    #[default]
    TapeRptDrop = 0,
    /// Stutter wins over drag — RPT can interrupt TAPE.
    RptTapeDrop = 1,
    /// DROP wins over TAPE / RPT.
    DropTapeRpt = 2,
    /// RPT loop played at TAPE rate when both are armed (repeat-on-tape).
    RptOnTape = 3,
    /// Whichever of TAPE / RPT / DROP was turned on first keeps the sound.
    /// A later RPT loops that sound, so a drop's pitch stays put.
    Engage = 4,
}

impl PunchBufferPrio {
    pub const ALL: [PunchBufferPrio; 5] = [
        Self::TapeRptDrop,
        Self::Engage,
        Self::RptTapeDrop,
        Self::DropTapeRpt,
        Self::RptOnTape,
    ];

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::RptTapeDrop,
            2 => Self::DropTapeRpt,
            3 => Self::RptOnTape,
            4 => Self::Engage,
            _ => Self::TapeRptDrop,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Short FX-page label.
    pub fn label(self) -> &'static str {
        match self {
            Self::TapeRptDrop => "T>R>D",
            Self::RptTapeDrop => "R>T>D",
            Self::DropTapeRpt => "D>T>R",
            Self::RptOnTape => "R×T",
            Self::Engage => "1ST",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::TapeRptDrop => Self::Engage,
            Self::Engage => Self::RptTapeDrop,
            Self::RptTapeDrop => Self::DropTapeRpt,
            Self::DropTapeRpt => Self::RptOnTape,
            Self::RptOnTape => Self::TapeRptDrop,
        }
    }
}

/// RPT / SLICE length: beat divisions, or a smooth span of the captured audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PunchGridMode {
    /// 1/4, 1/8, 1/16, 1/32 bands.
    #[default]
    Beat = 0,
    /// Bottom of the pad loops the whole capture; the window shortens in a
    /// straight line toward a short stutter at the top.
    Smooth = 1,
}

impl PunchGridMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Smooth,
            _ => Self::Beat,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Beat => "BEAT",
            Self::Smooth => "SMOOTH",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Beat => Self::Smooth,
            Self::Smooth => Self::Beat,
        }
    }
}

/// Whether a new RPT hit grabs the sound just heard, or keeps the last slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PunchRptMode {
    /// Each hit freezes the audio that just played.
    #[default]
    Refresh = 0,
    /// Keep that freeze across releases until CLEAR or a mode change.
    Hold = 1,
}

impl PunchRptMode {
    pub const ALL: [PunchRptMode; 2] = [Self::Refresh, Self::Hold];

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Hold,
            _ => Self::Refresh,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Refresh => "RPT↻",
            Self::Hold => "RPTH",
        }
    }

    pub fn next(self) -> Self {
        Self::from_u8((self.as_u8() + 1) % Self::ALL.len() as u8)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BufferVoice {
    Live,
    Tape,
    Rpt,
    Drop,
    /// RPT content advanced at the tape motor rate.
    RptAtTape,
    /// RPT content advanced at the drop rate.
    RptAtDrop,
}

fn pick_buffer_voice(
    prio: PunchBufferPrio,
    tape: bool,
    rpt: bool,
    drop: bool,
    order: &[u8],
) -> BufferVoice {
    match prio {
        PunchBufferPrio::Engage => pick_engage_voice(order, tape, rpt, drop),
        PunchBufferPrio::TapeRptDrop => {
            if tape {
                BufferVoice::Tape
            } else if rpt {
                BufferVoice::Rpt
            } else if drop {
                BufferVoice::Drop
            } else {
                BufferVoice::Live
            }
        }
        PunchBufferPrio::RptTapeDrop => {
            if rpt {
                BufferVoice::Rpt
            } else if tape {
                BufferVoice::Tape
            } else if drop {
                BufferVoice::Drop
            } else {
                BufferVoice::Live
            }
        }
        PunchBufferPrio::DropTapeRpt => {
            if drop {
                BufferVoice::Drop
            } else if tape {
                BufferVoice::Tape
            } else if rpt {
                BufferVoice::Rpt
            } else {
                BufferVoice::Live
            }
        }
        PunchBufferPrio::RptOnTape => {
            if tape && rpt {
                BufferVoice::RptAtTape
            } else if tape {
                BufferVoice::Tape
            } else if rpt {
                BufferVoice::Rpt
            } else if drop {
                BufferVoice::Drop
            } else {
                BufferVoice::Live
            }
        }
    }
}

/// Oldest engaged effect is the sound. A later repeat loops it (pitch stays).
/// A later drop or tape only changes the rate of a repeat that was already on.
fn pick_engage_voice(order: &[u8], tape: bool, rpt: bool, drop: bool) -> BufferVoice {
    const TAPE: u8 = 0;
    const RPT: u8 = 1;
    const DROP: u8 = 2;
    let mut first = None;
    let mut rpt_on = false;
    let mut rate = None;
    for kind in order {
        let on = match *kind {
            TAPE => tape,
            RPT => rpt,
            DROP => drop,
            _ => false,
        };
        if !on {
            continue;
        }
        if first.is_none() {
            first = Some(*kind);
        }
        if *kind == RPT {
            rpt_on = true;
        } else if first == Some(RPT) && rate.is_none() {
            rate = Some(*kind);
        }
    }
    match first {
        Some(DROP) | Some(TAPE) if rpt_on => BufferVoice::Rpt,
        Some(DROP) => BufferVoice::Drop,
        Some(TAPE) => BufferVoice::Tape,
        Some(RPT) => match rate {
            Some(DROP) => BufferVoice::RptAtDrop,
            Some(TAPE) => BufferVoice::RptAtTape,
            _ => BufferVoice::Rpt,
        },
        _ => BufferVoice::Live,
    }
}

pub fn punch_slot_has_grid(index: usize) -> bool {
    index == PUNCH_RPT as usize || index == PUNCH_SLICE as usize
}

pub fn punch_grid_index(amount: f32) -> usize {
    (amount.clamp(0.0, 1.0) * 3.999).floor() as usize
}

pub fn punch_index_for_knob_cc(controller: u8) -> Option<usize> {
    PUNCH_KNOB_CCS.iter().position(|cc| *cc == controller)
}

pub fn punch_index_for_pad_note(note: u8) -> Option<usize> {
    PUNCH_PAD_NOTES.iter().position(|n| *n == note)
}

/// Master-bus punch-in processor. Sized in [`PunchRack::new`].
pub struct PunchRack {
    sample_rate: f32,
    amount: [f32; PUNCH_PAD_COUNT],
    buffer_prio: PunchBufferPrio,
    /// TAPE=0, RPT=1, DROP=2, oldest first. Filled as each one turns on.
    engage_order: [u8; 3],
    engage_len: u8,
    rpt_mode: PunchRptMode,
    grid_mode: PunchGridMode,
    ring: Vec<f32>,
    /// What actually left this processor (drop pitch included), same index as `ring`.
    heard: Vec<f32>,
    write: usize,
    /// How many samples of valid history are in `ring` (caps at ring len).
    filled: usize,
    /// Locked freeze loop: ignores live bus until released.
    locked: bool,
    /// Hold-to-grab: append live master into `lock_buf` until released.
    grabbing: bool,
    lock_buf: Vec<f32>,
    lock_len: usize,
    lock_read: f32,
    /// Freeze already contains the tape/drop that was heard. Don't run them again
    /// until that slider moves.
    lock_baked: bool,
    baked_tape: f32,
    baked_drop: f32,
    /// Frozen RPT slice when [`PunchRptMode::Hold`] is engaged.
    rpt_hold_buf: Vec<f32>,
    rpt_hold_len: usize,
    /// Chronological snapshot taken when RPT engaged (oldest → newest).
    rpt_capture_len: usize,
    rpt_on: bool,
    rpt_start: usize,
    rpt_len: usize,
    rpt_read: f32,
    tape_on: bool,
    tape_rate: f32,
    tape_read: f32,
    drop_read: f32,
    lp_l: f32,
    lp_b: f32,
    hp_l: f32,
    hp_b: f32,
    slice_phase: f32,
    crush_hold: f32,
    crush_left: u32,
}

impl PunchRack {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(8000.0);
        let n = (sample_rate * RING_SEC) as usize + 64;
        let n = n.max(64);
        Self {
            sample_rate,
            amount: [0.0; PUNCH_PAD_COUNT],
            buffer_prio: PunchBufferPrio::TapeRptDrop,
            engage_order: [0; 3],
            engage_len: 0,
            rpt_mode: PunchRptMode::Refresh,
            grid_mode: PunchGridMode::Beat,
            ring: vec![0.0; n],
            heard: vec![0.0; n],
            write: 0,
            filled: 0,
            locked: false,
            grabbing: false,
            lock_buf: vec![0.0; n],
            lock_len: 0,
            lock_read: 0.0,
            lock_baked: false,
            baked_tape: 0.0,
            baked_drop: 0.0,
            rpt_hold_buf: vec![0.0; n],
            rpt_hold_len: 0,
            rpt_capture_len: 0,
            rpt_on: false,
            rpt_start: 0,
            rpt_len: 1,
            rpt_read: 0.0,
            tape_on: false,
            tape_rate: 1.0,
            tape_read: 0.0,
            drop_read: 0.0,
            lp_l: 0.0,
            lp_b: 0.0,
            hp_l: 0.0,
            hp_b: 0.0,
            slice_phase: 0.0,
            crush_hold: 0.0,
            crush_left: 0,
        }
    }

    pub fn buffer_prio(&self) -> PunchBufferPrio {
        self.buffer_prio
    }

    pub fn set_buffer_prio(&mut self, prio: PunchBufferPrio) {
        self.buffer_prio = prio;
    }

    pub fn rpt_mode(&self) -> PunchRptMode {
        self.rpt_mode
    }

    pub fn set_rpt_mode(&mut self, mode: PunchRptMode) {
        if mode != PunchRptMode::Hold {
            self.rpt_hold_len = 0;
            self.rpt_capture_len = 0;
        }
        self.rpt_mode = mode;
    }

    pub fn grid_mode(&self) -> PunchGridMode {
        self.grid_mode
    }

    pub fn set_grid_mode(&mut self, mode: PunchGridMode) {
        self.grid_mode = mode;
    }

    /// Copy the audio that has actually been heard, oldest first, ending at
    /// the write head. This is the wet output (so a drop keeps its pitch),
    /// not the dry ring the drop playhead is still reading.
    fn capture_heard(&mut self) {
        let nring = self.heard.len();
        let n = self.filled.min(nring).min(self.rpt_hold_buf.len());
        if n == 0 {
            self.rpt_capture_len = 0;
            self.rpt_hold_len = 0;
            return;
        }
        for i in 0..n {
            let src = if self.filled >= nring {
                (self.write + i) % nring
            } else {
                i
            };
            self.rpt_hold_buf[i] = self.heard[src];
        }
        self.rpt_capture_len = n;
        self.rpt_hold_len = n;
    }

    fn loop_len_for(&self, amount: f32, bpm: f32) -> usize {
        let available = if self.locked && self.lock_len > 0 {
            self.lock_len
        } else if self.rpt_capture_len > 0 {
            self.rpt_capture_len
        } else {
            self.filled.max(32)
        };
        self.span_samples(amount, bpm, available)
    }

    fn span_samples(&self, amount: f32, bpm: f32, available: usize) -> usize {
        let available = available.max(32);
        let len = match self.grid_mode {
            PunchGridMode::Beat => rpt_loop_samples(amount, bpm, self.sample_rate),
            PunchGridMode::Smooth => smooth_loop_samples(amount, available),
        };
        len.clamp(32, available)
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn grabbing(&self) -> bool {
        self.grabbing
    }

    /// Hold-to-grab: `true` starts recording what you hear into the freeze
    /// buffer; `false` commits that buffer as a loop (or discards if too short).
    /// Tape and drop keep their current rate — restarting them is a second
    /// pitch swoop on audio that already has the effect.
    pub fn set_grabbing(&mut self, active: bool) {
        if active {
            self.grabbing = true;
            self.locked = false;
            self.lock_len = 0;
            self.lock_read = 0.0;
            self.lock_baked = false;
            self.rpt_on = false;
        } else {
            self.grabbing = false;
            if self.lock_len >= 32 {
                self.locked = true;
                self.lock_read = 0.0;
                self.rpt_on = false;
                self.lock_baked = true;
                self.baked_tape = self.amount[PUNCH_TAPE as usize];
                self.baked_drop = self.amount[PUNCH_DROP as usize];
            } else {
                self.lock_len = 0;
                self.lock_read = 0.0;
                self.lock_baked = false;
            }
        }
    }

    /// Tape/drop already printed into this freeze. Skip them until the slider moves.
    fn effect_baked(&self, slot: usize) -> bool {
        if !self.locked || !self.lock_baked {
            return false;
        }
        let frozen = if slot == PUNCH_TAPE as usize {
            self.baked_tape
        } else if slot == PUNCH_DROP as usize {
            self.baked_drop
        } else {
            return false;
        };
        (self.amount[slot] - frozen).abs() < 0.02
    }

    /// Unlock / clear the freeze loop. `locked=true` is legacy: grab the
    /// recent rolling history (tap-style). Prefer [`Self::set_grabbing`] for
    /// hold-to-capture.
    pub fn set_locked(&mut self, locked: bool) {
        self.grabbing = false;
        if locked {
            self.capture_lock();
        } else {
            self.locked = false;
            self.lock_len = 0;
            self.lock_read = 0.0;
            self.lock_baked = false;
            self.rpt_on = false;
            self.tape_on = false;
        }
    }

    fn capture_lock(&mut self) {
        let nring = self.ring.len();
        let len = self.filled.max(32).min(nring);
        // Chronological: oldest captured → newest, ending at write.
        for i in 0..len {
            let src = (self.write + nring - len + i) % nring;
            self.lock_buf[i] = self.ring[src];
        }
        self.lock_len = len;
        self.lock_read = 0.0;
        self.locked = true;
        self.rpt_on = false;
        self.tape_on = false;
        self.tape_rate = 1.0;
        self.drop_read = 0.0;
    }

    pub fn amount(&self, slot: u8) -> f32 {
        self.amount[slot_index(slot)]
    }

    pub fn send_amount(&self) -> f32 {
        self.amount[PUNCH_SEND as usize]
    }

    pub fn set_amount(&mut self, slot: u8, value: f32) {
        let idx = slot_index(slot);
        let prev = self.amount[idx];
        let value = value.clamp(0.0, 1.0);
        self.amount[idx] = value;
        let kind = match slot {
            PUNCH_TAPE => Some(0),
            PUNCH_RPT => Some(1),
            PUNCH_DROP => Some(2),
            _ => None,
        };
        if let Some(kind) = kind {
            let was = prev > ENGAGE;
            let now = value > ENGAGE;
            if was != now {
                self.note_engage(kind, now);
            }
        }
    }

    fn note_engage(&mut self, kind: u8, on: bool) {
        let len = self.engage_len as usize;
        let pos = self.engage_order[..len].iter().position(|k| *k == kind);
        if on {
            if pos.is_none() && len < self.engage_order.len() {
                self.engage_order[len] = kind;
                self.engage_len = (len + 1) as u8;
            }
        } else if let Some(i) = pos {
            for j in i..len - 1 {
                self.engage_order[j] = self.engage_order[j + 1];
            }
            self.engage_len = (len - 1) as u8;
        }
    }

    pub fn clear(&mut self) {
        self.amount = [0.0; PUNCH_PAD_COUNT];
        self.engage_len = 0;
        self.rpt_on = false;
        self.tape_on = false;
        self.tape_rate = 1.0;
        self.slice_phase = 0.0;
        self.crush_left = 0;
        self.locked = false;
        self.grabbing = false;
        self.lock_len = 0;
        self.lock_read = 0.0;
        self.lock_baked = false;
        self.rpt_hold_len = 0;
        self.rpt_capture_len = 0;
    }
    pub fn reset(&mut self) {
        self.clear();
        self.ring.iter_mut().for_each(|s| *s = 0.0);
        self.heard.iter_mut().for_each(|s| *s = 0.0);
        self.lock_buf.iter_mut().for_each(|s| *s = 0.0);
        self.rpt_hold_buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.filled = 0;
        self.rpt_read = 0.0;
        self.tape_read = 0.0;
        self.drop_read = 0.0;
        self.lp_l = 0.0;
        self.lp_b = 0.0;
        self.hp_l = 0.0;
        self.hp_b = 0.0;
        self.crush_hold = 0.0;
    }

    pub fn is_idle(&self) -> bool {
        self.amount.iter().all(|a| *a <= ENGAGE)
            && !self.tape_on
            && !self.rpt_on
            && !self.locked
            && !self.grabbing
    }

    /// Process in place. `bpm` drives repeat / slice grid. Allocation-free.
    pub fn process(&mut self, buf: &mut [f32], bpm: f32) {
        if buf.is_empty() {
            return;
        }
        let sr = self.sample_rate;
        let nring = self.ring.len();
        let rpt = self.amount[PUNCH_RPT as usize];
        let tape = self.amount[PUNCH_TAPE as usize];
        let drop = self.amount[PUNCH_DROP as usize];
        let tape_want = tape > ENGAGE && !self.effect_baked(PUNCH_TAPE as usize);
        let rpt_want = rpt > ENGAGE;
        let drop_want = drop > ENGAGE && !self.effect_baked(PUNCH_DROP as usize);
        let voice = pick_buffer_voice(
            self.buffer_prio,
            tape_want,
            rpt_want,
            drop_want,
            &self.engage_order[..self.engage_len as usize],
        );

        let tape_active =
            matches!(voice, BufferVoice::Tape | BufferVoice::RptAtTape);
        let rpt_active = matches!(
            voice,
            BufferVoice::Rpt | BufferVoice::RptAtTape | BufferVoice::RptAtDrop
        );

        if tape_active && !self.tape_on {
            self.tape_on = true;
            self.tape_rate = 1.0;
            if self.locked {
                self.tape_read = self.lock_read;
            } else {
                self.tape_read = self.write as f32;
            }
        } else if !tape_active {
            self.tape_on = false;
            self.tape_rate = 1.0;
        }

        if rpt_active && !self.rpt_on {
            if self.locked && self.lock_len > 0 {
                self.rpt_len = self.loop_len_for(rpt, bpm);
                self.rpt_start = (self.lock_read as usize) % self.lock_len.max(1);
                self.rpt_capture_len = 0;
            } else if self.rpt_mode == PunchRptMode::Hold && self.rpt_capture_len >= 32 {
                self.rpt_len = self.loop_len_for(rpt, bpm);
            } else {
                // Freeze the tail that just played. Reading the live ring
                // instead walks forward into audio that arrives after the hit.
                self.capture_heard();
                self.rpt_len = self.loop_len_for(rpt, bpm);
            }
            self.rpt_read = 0.0;
            self.rpt_on = true;
        } else if rpt_active {
            let len = self.loop_len_for(rpt, bpm);
            if len != self.rpt_len {
                self.rpt_len = len;
                self.rpt_read %= self.rpt_len.max(1) as f32;
            }
        } else {
            self.rpt_on = false;
            // Hold keeps the slice so the next hit replays the same sound.
            if self.rpt_mode != PunchRptMode::Hold {
                self.rpt_hold_len = 0;
                self.rpt_capture_len = 0;
            }
        }

        if matches!(voice, BufferVoice::Drop) && self.drop_read == 0.0 {
            self.drop_read = if self.locked {
                self.lock_read
            } else {
                self.write as f32
            };
        }
        if !matches!(voice, BufferVoice::Drop) {
            self.drop_read = 0.0;
        }

        let tape_target = tape_target_rate(tape);
        let tape_slew = 1.0 - (-1.0 / (0.14 * sr).max(1.0)).exp();
        let drop_rate = if drop_want {
            2f32.powf(-drop * 1.7)
        } else {
            1.0
        };

        let locked = self.locked && self.lock_len >= 32;
        let lock_n = self.lock_len;

        for s in buf.iter_mut() {
            let idx = self.write;
            self.ring[idx] = *s;
            let live = *s;
            self.write = (self.write + 1) % nring;
            if self.filled < nring {
                self.filled += 1;
            }

            let wet = match voice {
                BufferVoice::Live if locked => {
                    let sample = read_ring_n(&self.lock_buf, lock_n, self.lock_read);
                    self.lock_read = (self.lock_read + 1.0).rem_euclid(lock_n as f32);
                    sample
                }
                BufferVoice::Live => live,
                BufferVoice::Tape => {
                    self.tape_rate += (tape_target - self.tape_rate) * tape_slew;
                    let (sample, next) = if locked {
                        let sample = read_ring_n(&self.lock_buf, lock_n, self.tape_read);
                        let next =
                            (self.tape_read + self.tape_rate).rem_euclid(lock_n as f32);
                        self.lock_read = next;
                        (sample, next)
                    } else {
                        let sample = read_ring(&self.ring, self.tape_read);
                        let next =
                            (self.tape_read + self.tape_rate).rem_euclid(nring as f32);
                        (sample, next)
                    };
                    self.tape_read = next;
                    sample
                }
                BufferVoice::Rpt | BufferVoice::RptAtTape | BufferVoice::RptAtDrop => {
                    if matches!(voice, BufferVoice::RptAtTape) {
                        self.tape_rate += (tape_target - self.tape_rate) * tape_slew;
                    }
                    let step = match voice {
                        BufferVoice::RptAtTape => self.tape_rate,
                        BufferVoice::RptAtDrop => drop_rate,
                        _ => 1.0,
                    };
                    let sample = if self.rpt_capture_len >= 32 && !locked {
                        let cap = self.rpt_capture_len;
                        let len = self.rpt_len.clamp(1, cap);
                        let start = cap - len;
                        read_window(&self.rpt_hold_buf, start, len, self.rpt_read)
                    } else if locked {
                        read_ring_n(
                            &self.lock_buf,
                            lock_n,
                            self.rpt_start as f32 + self.rpt_read,
                        )
                    } else {
                        read_ring(&self.ring, self.rpt_start as f32 + self.rpt_read)
                    };
                    self.rpt_read += step;
                    let span = self.rpt_len.max(1) as f32;
                    while self.rpt_read >= span {
                        self.rpt_read -= span;
                    }
                    sample
                }
                BufferVoice::Drop => {
                    let (sample, next) = if locked {
                        let sample = read_ring_n(&self.lock_buf, lock_n, self.drop_read);
                        let next =
                            (self.drop_read + drop_rate).rem_euclid(lock_n as f32);
                        self.lock_read = next;
                        (sample, next)
                    } else {
                        let sample = read_ring(&self.ring, self.drop_read);
                        let next =
                            (self.drop_read + drop_rate).rem_euclid(nring as f32);
                        (sample, next)
                    };
                    self.drop_read = next;
                    sample
                }
            };
            self.heard[idx] = wet;
            // Record what just came out. A tape already in that sound must not
            // be slowed again when the loop starts.
            if self.grabbing {
                let cap = self.lock_buf.len();
                if self.lock_len < cap {
                    self.lock_buf[self.lock_len] = wet;
                    self.lock_len += 1;
                }
            }
            *s = wet;
        }

        let lpf = self.amount[PUNCH_LPF as usize];
        if lpf > ENGAGE {
            apply_svf_lowpass(buf, 1.0 - lpf * 0.92, &mut self.lp_l, &mut self.lp_b, sr);
        }
        let hpf = self.amount[PUNCH_HPF as usize];
        if hpf > ENGAGE {
            apply_svf_highpass(buf, hpf, &mut self.hp_l, &mut self.hp_b, sr);
        }
        let crush = self.amount[PUNCH_CRUSH as usize];
        if crush > ENGAGE {
            apply_crush(buf, crush, &mut self.crush_hold, &mut self.crush_left);
        }
        let slice = self.amount[PUNCH_SLICE as usize];
        if slice > ENGAGE {
            let available = if self.locked && self.lock_len > 0 {
                self.lock_len
            } else {
                self.filled.max(32)
            };
            let period = self.span_samples(slice, bpm, available).max(8) as f32;
            apply_slice(buf, period, &mut self.slice_phase);
        }
    }
}

/// Light press is a slight drag; full press is slow-mo. Never reaches 0.
fn tape_target_rate(amount: f32) -> f32 {
    let t = amount.clamp(0.0, 1.0);
    0.88 - t * 0.60
}

fn slot_index(slot: u8) -> usize {
    (slot as usize).min(PUNCH_PAD_COUNT - 1)
}

fn read_ring(ring: &[f32], pos: f32) -> f32 {
    read_ring_n(ring, ring.len(), pos)
}

fn read_ring_n(ring: &[f32], len: usize, pos: f32) -> f32 {
    let len = len.max(1).min(ring.len());
    let n = len as f32;
    let p = pos.rem_euclid(n);
    let i0 = p.floor() as usize % len;
    let i1 = (i0 + 1) % len;
    let frac = p - p.floor();
    ring[i0] * (1.0 - frac) + ring[i1] * frac
}

fn read_window(buf: &[f32], start: usize, len: usize, pos: f32) -> f32 {
    let len = len.max(1);
    let p = pos.rem_euclid(len as f32);
    let i0 = start + (p.floor() as usize % len);
    let i1 = start + ((p.floor() as usize + 1) % len);
    let frac = p - p.floor();
    let a = buf.get(i0).copied().unwrap_or(0.0);
    let b = buf.get(i1).copied().unwrap_or(0.0);
    a * (1.0 - frac) + b * frac
}

/// 1/4 → 1/8 → 1/16 → 1/32 of a beat as amount rises.
fn rpt_loop_samples(amount: f32, bpm: f32, sr: f32) -> usize {
    let beats = PUNCH_GRID_BEATS[punch_grid_index(amount).min(3)];
    let sec = beats * 60.0 / bpm.max(20.0);
    (sec * sr).round() as usize
}

/// The old smooth curve spent 0–85% on loops too long to use. Slider zero is
/// that 85% length; the old 85%–100% span fills the whole bar, ending at the
/// short stutter.
const SMOOTH_USEFUL_START: f32 = 0.85;

fn smooth_loop_samples(amount: f32, available: usize) -> usize {
    let available = available.max(32);
    let short = (available / 48).clamp(32, available);
    let t = amount.clamp(0.0, 1.0);
    let t = SMOOTH_USEFUL_START + (1.0 - SMOOTH_USEFUL_START) * t;
    let len = available as f32 + (short as f32 - available as f32) * t;
    (len.round() as usize).clamp(short, available)
}

fn apply_svf_lowpass(buf: &mut [f32], tone: f32, lp: &mut f32, bp: &mut f32, sample_rate: f32) {
    let tone = tone.clamp(0.0, 1.0);
    if tone >= 0.985 {
        *lp = buf.last().copied().unwrap_or(*lp);
        *bp = 0.0;
        return;
    }
    let sr = sample_rate.max(8000.0);
    let fc = 90.0 * (8000.0_f32 / 90.0).powf(tone);
    let fc = fc.min(sr * 0.14);
    let f = (2.0 * std::f32::consts::PI * fc / sr).sin();
    let damp = 0.38 + 0.62 * tone;
    let mut l = *lp;
    let mut b = *bp;
    for s in buf.iter_mut() {
        l += f * b;
        let hp = *s - l - damp * b;
        b += f * hp;
        *s = l;
    }
    *lp = l;
    *bp = b;
}

fn apply_svf_highpass(buf: &mut [f32], amount: f32, lp: &mut f32, bp: &mut f32, sample_rate: f32) {
    let amount = amount.clamp(0.0, 1.0);
    let sr = sample_rate.max(8000.0);
    let fc = 40.0 * (4000.0_f32 / 40.0).powf(amount);
    let fc = fc.min(sr * 0.20);
    let f = (2.0 * std::f32::consts::PI * fc / sr).sin();
    let damp = 0.45;
    let mut l = *lp;
    let mut b = *bp;
    for s in buf.iter_mut() {
        l += f * b;
        let hp = *s - l - damp * b;
        b += f * hp;
        *s = hp;
    }
    *lp = l;
    *bp = b;
}

fn apply_crush(buf: &mut [f32], amount: f32, hold: &mut f32, left: &mut u32) {
    let bits = 12.0 - amount * 10.0;
    let levels = 2f32.powf(bits.clamp(1.5, 12.0));
    let hold_n = 1 + (amount * 28.0) as u32;
    for s in buf.iter_mut() {
        if *left == 0 {
            *hold = (*s * levels).round() / levels;
            *left = hold_n;
        }
        *s = *hold;
        *left = left.saturating_sub(1);
    }
}

fn apply_slice(buf: &mut [f32], period: f32, phase: &mut f32) {
    let period = period.max(8.0);
    let inc = 1.0 / period;
    for s in buf.iter_mut() {
        *phase += inc;
        if *phase >= 1.0 {
            *phase -= 1.0;
        }
        // Hard gate, ~45% open — EP pad 7 style amplitude chop.
        if *phase > 0.45 {
            *s = 0.0;
        }
    }
}

/// Linear interpolation of bus wet toward full when SEND is down.
pub fn boost_send_mix(base: f32, send: f32) -> f32 {
    let send = send.clamp(0.0, 1.0);
    (base + send * (1.0 - base) * 0.95).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpk_factory_knobs_and_bank_a_map_left_to_right() {
        assert_eq!(punch_index_for_knob_cc(70), Some(PUNCH_RPT as usize));
        assert_eq!(punch_index_for_knob_cc(77), Some(PUNCH_CRUSH as usize));
        assert_eq!(punch_index_for_pad_note(40), Some(PUNCH_RPT as usize));
        assert_eq!(punch_index_for_pad_note(36), Some(PUNCH_SEND as usize));
        assert_eq!(punch_index_for_pad_note(39), Some(PUNCH_CRUSH as usize));
        assert!(punch_index_for_knob_cc(1).is_none());
        assert!(punch_index_for_pad_note(60).is_none());
    }

    #[test]
    fn grid_index_matches_quarter_steps() {
        assert_eq!(punch_grid_index(0.0), 0);
        assert_eq!(punch_grid_index(0.24), 0);
        assert_eq!(punch_grid_index(0.26), 1);
        assert_eq!(punch_grid_index(0.51), 2);
        assert_eq!(punch_grid_index(0.76), 3);
        assert_eq!(punch_grid_index(1.0), 3);
        assert!(punch_slot_has_grid(PUNCH_RPT as usize));
        assert!(punch_slot_has_grid(PUNCH_SLICE as usize));
        assert!(!punch_slot_has_grid(PUNCH_TAPE as usize));
        assert_eq!(PUNCH_GRID_LABELS[punch_grid_index(0.1)], "1/4");
        assert_eq!(PUNCH_GRID_LABELS[punch_grid_index(0.4)], "1/8");
    }

    #[test]
    fn smooth_bar_starts_where_85_percent_used_to_be() {
        let available = 4800;
        let short = (available / 48).clamp(32, available);
        let old = |amount: f32| {
            let len = available as f32 + (short as f32 - available as f32) * amount;
            (len.round() as usize).clamp(short, available)
        };
        assert_eq!(smooth_loop_samples(0.0, available), old(0.85));
        assert_eq!(smooth_loop_samples(1.0, available), old(1.0));
        assert_eq!(smooth_loop_samples(0.5, available), old(0.925));
    }

    #[test]
    fn repeat_replays_the_tail_not_the_empty_wrap() {
        let mut p = PunchRack::new(48_000.0);
        // Short history: a ramp, then a loud tail. The old wrap started in
        // the unfilled end of the ring (silence / "forwards").
        let mut prime = vec![0.05f32; 400];
        for s in prime.iter_mut().rev().take(40) {
            *s = 0.9;
        }
        p.process(&mut prime, 120.0);
        p.set_grid_mode(PunchGridMode::Smooth);
        p.set_amount(PUNCH_RPT, 0.05);
        let mut out = vec![0.0f32; 400];
        p.process(&mut out, 120.0);
        assert!(
            out[0] > 0.02 && out[0] < 0.2,
            "loop starts at the beginning of what was heard, got {}",
            out[0]
        );
        assert!(
            out[360..].iter().any(|s| *s > 0.5),
            "the loud tail just heard must be inside the loop"
        );
    }

    #[test]
    fn grab_with_tape_replays_what_was_heard() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_TAPE, 1.0);
        // Settle the tape rate so the grab is not the startup swoop.
        let mut settle = vec![0.0f32; 48_000];
        for i in (0..settle.len()).step_by(100) {
            settle[i] = 1.0;
        }
        p.process(&mut settle, 120.0);
        p.set_grabbing(true);
        let mut held = vec![0.0f32; 2000];
        for i in (0..held.len()).step_by(100) {
            held[i] = 1.0;
        }
        p.process(&mut held, 120.0);
        p.set_grabbing(false);
        let mut out = vec![0.0f32; 2000];
        p.process(&mut out, 120.0);
        let err = held
            .iter()
            .zip(out.iter())
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / held.len() as f32;
        assert!(
            err < 0.02,
            "locked grab must replay the taped sound, not slow it again, err={err}"
        );
    }

    #[test]
    fn repeat_keeps_the_dropped_pitch_instead_of_the_dry_tail() {
        let mut p = PunchRack::new(48_000.0);
        p.set_buffer_prio(PunchBufferPrio::RptTapeDrop);
        // Loud, then quiet. Drop's playhead is still in the loud part; the
        // dry write head has already moved into the quiet tail.
        let mut prime = vec![0.1f32; 6000];
        for s in prime.iter_mut().take(2500) {
            *s = 0.8;
        }
        p.set_amount(PUNCH_DROP, 1.0);
        p.process(&mut prime, 120.0);
        p.set_amount(PUNCH_DROP, 0.0);
        p.set_grid_mode(PunchGridMode::Smooth);
        p.set_amount(PUNCH_RPT, 0.9);
        let mut out = vec![0.0f32; 800];
        p.process(&mut out, 120.0);
        let mean = out.iter().map(|s| s.abs()).sum::<f32>() / out.len() as f32;
        assert!(
            mean > 0.5,
            "repeat must loop the dropped sound, not the dry tail, mean={mean}"
        );
    }

    #[test]
    fn idle_rack_leaves_signal() {
        let mut p = PunchRack::new(48_000.0);
        let mut buf = [0.4f32; 64];
        p.process(&mut buf, 120.0);
        assert!(buf.iter().all(|v| (*v - 0.4).abs() < 1e-6));
    }

    #[test]
    fn beat_repeat_loops_a_captured_impulse() {
        let mut p = PunchRack::new(48_000.0);
        let loop_n = rpt_loop_samples(0.1, 120.0, 48_000.0);
        // Prime the ring with silence, then one hit, then engage.
        let mut prime = vec![0.0f32; loop_n];
        prime[0] = 1.0;
        p.process(&mut prime, 120.0);
        p.set_amount(PUNCH_RPT, 0.1);
        let mut out = vec![0.0f32; loop_n * 2];
        p.process(&mut out, 120.0);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.5, "repeat should reprint the hit, peak={peak}");
        // Second loop pass should also carry energy.
        let late = out[loop_n..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(late > 0.5, "loop must wrap, late={late}");
    }

    #[test]
    fn tape_drag_stays_audible() {
        let mut p = PunchRack::new(48_000.0);
        let mut prime = vec![0.3f32; 2048];
        p.process(&mut prime, 120.0);
        p.set_amount(PUNCH_TAPE, 1.0);
        let mut out = vec![0.3f32; 48_000];
        p.process(&mut out, 120.0);
        let tail = out[out.len() - 64..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(tail > 0.15, "full tape must keep moving, tail={tail}");
        assert!(tape_target_rate(1.0) > 0.2);
        assert!(tape_target_rate(0.1) > tape_target_rate(1.0));
    }

    #[test]
    fn lpf_darkens_a_hot_signal() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_LPF, 1.0);
        let mut buf = vec![0.0f32; 256];
        for (i, s) in buf.iter_mut().enumerate() {
            *s = if i % 2 == 0 { 0.8 } else { -0.8 };
        }
        p.process(&mut buf, 120.0);
        let peak = buf.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak < 0.55, "deep LPF should kill the Nyquist square, peak={peak}");
    }

    #[test]
    fn crush_quantizes_amplitude() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_CRUSH, 1.0);
        let mut buf = [0.37f32; 32];
        p.process(&mut buf, 120.0);
        let unique: Vec<i32> = {
            let mut v: Vec<i32> = buf.iter().map(|s| (s * 1000.0).round() as i32).collect();
            v.sort();
            v.dedup();
            v
        };
        assert!(
            unique.len() <= 4,
            "heavy crush should collapse levels, got {unique:?}"
        );
    }

    #[test]
    fn slice_gates_a_steady_tone() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_SLICE, 0.9);
        let mut buf = vec![0.5f32; 8000];
        p.process(&mut buf, 120.0);
        let silent = buf.iter().filter(|s| s.abs() < 1e-5).count();
        let loud = buf.iter().filter(|s| s.abs() > 0.4).count();
        assert!(silent > 1000, "slicer must close, silent={silent}");
        assert!(loud > 1000, "slicer must open, loud={loud}");
    }

    #[test]
    fn send_boost_opens_a_dry_mix() {
        assert!((boost_send_mix(0.0, 1.0) - 0.95).abs() < 1e-5);
        assert!(boost_send_mix(0.4, 0.0) < 0.41);
        assert!(boost_send_mix(0.4, 1.0) > 0.9);
    }

    #[test]
    fn reset_kills_a_latched_repeat() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_RPT, 0.8);
        let mut buf = [0.6f32; 128];
        p.process(&mut buf, 120.0);
        p.reset();
        assert!(p.is_idle());
        let mut quiet = [0.2f32; 64];
        p.process(&mut quiet, 120.0);
        assert!(quiet.iter().all(|v| (*v - 0.2).abs() < 1e-6));
    }

    #[test]
    fn hold_grab_captures_only_while_held() {
        let mut p = PunchRack::new(48_000.0);
        // Live silence before grab must not enter the freeze.
        let mut pre = vec![0.0f32; 512];
        p.process(&mut pre, 120.0);
        p.set_grabbing(true);
        assert!(p.grabbing());
        assert!(!p.locked());
        let mut held = vec![0.8f32; 1024];
        p.process(&mut held, 120.0);
        // Audio after release must not append — only the hold window loops.
        p.set_grabbing(false);
        assert!(p.locked());
        assert!(!p.grabbing());
        let mut out = vec![0.0f32; 2048];
        p.process(&mut out, 120.0);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.5, "grabbed hold must replay, peak={peak}");
        p.set_locked(false);
        let mut live = vec![0.12f32; 256];
        p.process(&mut live, 120.0);
        assert!(
            live.iter().all(|v| (*v - 0.12).abs() < 1e-5),
            "unlock must return live"
        );
    }

    #[test]
    fn lock_freezes_history_and_ignores_live() {
        let mut p = PunchRack::new(48_000.0);
        // Prime ring with a loud tone, then lock.
        let mut prime = vec![0.7f32; 4096];
        p.process(&mut prime, 120.0);
        p.set_locked(true);
        assert!(p.locked());
        // Feed silence while locked — output must stay loud from the freeze.
        let mut out = vec![0.0f32; 2048];
        p.process(&mut out, 120.0);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.5, "locked loop must replay freeze, peak={peak}");
        p.set_locked(false);
        assert!(!p.locked());
        let mut live = vec![0.15f32; 256];
        p.process(&mut live, 120.0);
        assert!(
            live.iter().all(|v| (*v - 0.15).abs() < 1e-5),
            "unlock must return live"
        );
    }

    #[test]
    fn lock_plus_tape_keeps_moving() {
        let mut p = PunchRack::new(48_000.0);
        let mut prime = vec![0.4f32; 8192];
        p.process(&mut prime, 120.0);
        p.set_locked(true);
        p.set_amount(PUNCH_TAPE, 1.0);
        let mut out = vec![0.0f32; 48_000];
        p.process(&mut out, 120.0);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.2, "tape over lock must stay audible, peak={peak}");
    }

    #[test]
    fn rpt_first_lets_repeat_beat_tape() {
        let mut p = PunchRack::new(48_000.0);
        p.set_buffer_prio(PunchBufferPrio::RptTapeDrop);
        // Short 1/32 grid so the primed hit sits inside the loop window.
        let loop_n = rpt_loop_samples(0.9, 120.0, 48_000.0);
        let mut prime = vec![0.0f32; loop_n];
        prime[0] = 1.0;
        p.process(&mut prime, 120.0);
        p.set_amount(PUNCH_TAPE, 1.0);
        p.set_amount(PUNCH_RPT, 0.9);
        let mut out = vec![0.0f32; loop_n * 2];
        p.process(&mut out, 120.0);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(
            peak > 0.4,
            "RPT-first must stutter even with TAPE armed, peak={peak}"
        );
    }

    #[test]
    fn rpt_on_tape_mode_label_cycles() {
        let mut m = PunchBufferPrio::TapeRptDrop;
        assert_eq!(m.label(), "T>R>D");
        m = m.next();
        assert_eq!(m, PunchBufferPrio::Engage);
        assert_eq!(m.label(), "1ST");
        m = m.next();
        assert_eq!(m, PunchBufferPrio::RptTapeDrop);
        m = m.next();
        assert_eq!(m, PunchBufferPrio::DropTapeRpt);
        m = m.next();
        assert_eq!(m, PunchBufferPrio::RptOnTape);
        assert_eq!(m.label(), "R×T");
        m = m.next();
        assert_eq!(m, PunchBufferPrio::TapeRptDrop);
    }

    #[test]
    fn engage_order_drop_then_repeat_keeps_the_dropped_pitch() {
        let mut p = PunchRack::new(48_000.0);
        p.set_buffer_prio(PunchBufferPrio::Engage);
        // Rising ramp. Drop's playhead lags the write head, so the sound you
        // hear is an earlier, lower value than the dry tail.
        let mut prime = vec![0.0f32; 4000];
        for (i, s) in prime.iter_mut().enumerate() {
            *s = i as f32 * 0.0001;
        }
        p.set_amount(PUNCH_DROP, 1.0);
        p.process(&mut prime, 120.0);
        p.set_grid_mode(PunchGridMode::Smooth);
        p.set_amount(PUNCH_RPT, 0.95);
        let mut out = vec![0.0f32; 800];
        p.process(&mut out, 120.0);
        let mean = out.iter().sum::<f32>() / out.len() as f32;
        let min = out.iter().copied().fold(f32::MAX, f32::min);
        let max = out.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            mean < 0.25,
            "repeat must stay on the dropped sound, not the dry tail, mean={mean}"
        );
        assert!(
            max - min < 0.02,
            "repeat must loop, not keep pitching, range={}",
            max - min
        );
    }
}
