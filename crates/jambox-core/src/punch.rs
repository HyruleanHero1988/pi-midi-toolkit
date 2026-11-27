//! EP-style punch-in FX on the master bus.
//!
//! Eight pads, layered. Buffer effects (repeat / tape / drop) share one ring;
//! who wins when several are armed is set by [`PunchBufferPrio`]. Filters,
//! crush, and slice run after. SEND is a wet-boost of the bus insert that
//! runs after this rack (so it still works on a GRAB freeze).
//!
//! When [`PunchRack::set_dry`] is on, the pre-FX bus is mixed back in after the
//! wet chain so dry and FX stay audible together. Default is off (wet replaces).
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
/// Scratch for parallel dry mix. Matches [`crate::engine::MAX_RENDER_BLOCK`].
const DRY_SCRATCH_MAX: usize = 8192;

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
    /// SEQ clip voice bus (stereo). Not cycled on the FX punch-source chip.
    Seq = 3,
}

impl PunchSource {
    pub const COUNT: usize = 4;
    pub const ALL: [Self; 4] = [Self::Keys, Self::Drums, Self::Mic, Self::Seq];

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Drums,
            2 => Self::Mic,
            3 => Self::Seq,
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
            Self::Seq => "SEQ",
        }
    }

    pub fn next(self) -> Self {
        Self::from_u8((self.as_u8() + 1) % 3)
    }
}

/// RPT / SLICE beat lengths as amount rises: 1/4, 1/8, 1/16, 1/32.
pub const PUNCH_GRID_BEATS: [f32; 4] = [1.0, 0.5, 0.25, 0.125];
/// Slider marks at the 1/8, 1/16, and 1/32 boundaries (1/4 is the bottom band).
///
/// Single source of truth for the beat grid: the UI draws a divider at each
/// value and [`punch_grid_index`] counts how many of them the amount has
/// reached, so a division changes exactly where its divider is drawn.
pub const PUNCH_GRID_TICKS: [f32; 3] = [0.25, 0.50, 0.75];
pub const PUNCH_GRID_LABELS: [&str; 4] = ["1/4", "1/8", "1/16", "1/32"];

/// Amount range `(low, high)` owned by grid division `index` (0 = 1/4 … 3 = 1/32),
/// bounded by the slider ends and [`PUNCH_GRID_TICKS`].
pub fn punch_grid_band(index: usize) -> (f32, f32) {
    let i = index.min(PUNCH_GRID_TICKS.len());
    let low = if i == 0 { 0.0 } else { PUNCH_GRID_TICKS[i - 1] };
    let high = PUNCH_GRID_TICKS.get(i).copied().unwrap_or(1.0);
    (low, high)
}

/// Division label for `amount` (`"1/4"` … `"1/32"`).
pub fn punch_grid_label(amount: f32) -> &'static str {
    PUNCH_GRID_LABELS[punch_grid_index(amount)]
}

/// Who wins when several buffer pads are armed. Picked on the FX SET page.
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
    pub const ALL: [Self; 2] = [Self::Beat, Self::Smooth];

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

/// Division selected by `amount`: a division starts exactly at its divider.
pub fn punch_grid_index(amount: f32) -> usize {
    let amount = amount.clamp(0.0, 1.0);
    PUNCH_GRID_TICKS.iter().filter(|tick| amount >= **tick).count()
}

pub fn punch_index_for_knob_cc(controller: u8) -> Option<usize> {
    PUNCH_KNOB_CCS.iter().position(|cc| *cc == controller)
}

pub fn punch_index_for_pad_note(note: u8) -> Option<usize> {
    PUNCH_PAD_NOTES.iter().position(|n| *n == note)
}

/// Where one output sample comes from. Playheads are shared by both channels,
/// so L and R always read the same position (beat-synced effects stay aligned).
#[derive(Clone, Copy)]
enum Tap {
    Live,
    Lock(f32),
    Ring(f32),
    Window { start: usize, len: usize, pos: f32 },
}

/// Per-channel storage the playheads read from.
struct TapBuffers<'a> {
    ring: &'a [f32],
    lock: &'a [f32],
    lock_n: usize,
    hold: &'a [f32],
}

impl Tap {
    #[inline]
    fn read(self, b: &TapBuffers<'_>, live: f32) -> f32 {
        match self {
            Tap::Live => live,
            Tap::Lock(pos) => read_ring_n(b.lock, b.lock_n, pos),
            Tap::Ring(pos) => read_ring(b.ring, pos),
            Tap::Window { start, len, pos } => read_window(b.hold, start, len, pos),
        }
    }
}

/// Master-bus punch-in processor. Sized in [`PunchRack::new`] (mono) or
/// [`PunchRack::new_stereo`] (adds a right-channel ring / freeze / filter set
/// that follows the same playheads).
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
    /// Right-channel twins of the buffers above. Empty on a mono rack.
    ring_r: Vec<f32>,
    heard_r: Vec<f32>,
    lock_buf_r: Vec<f32>,
    rpt_hold_buf_r: Vec<f32>,
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
    lp_l_r: f32,
    lp_b_r: f32,
    hp_l_r: f32,
    hp_b_r: f32,
    slice_phase: f32,
    crush_hold: f32,
    crush_hold_r: f32,
    crush_left: u32,
    /// Idle history. Off skips the ring until a buffer effect, LOCK, or GRAB needs it.
    history: bool,
    /// Mix the pre-FX bus back in after the wet chain (parallel dry).
    dry: bool,
    /// Pre-FX copy for the current block when [`Self::dry`] is on.
    dry_scratch: Vec<f32>,
    dry_scratch_r: Vec<f32>,
}

impl PunchRack {
    pub fn new(sample_rate: f32) -> Self {
        Self::build(sample_rate, false)
    }

    /// Stereo rack: [`Self::process_stereo`] keeps independent audio and filter
    /// state per channel while sharing every playhead and gate.
    pub fn new_stereo(sample_rate: f32) -> Self {
        Self::build(sample_rate, true)
    }

    pub fn is_stereo(&self) -> bool {
        !self.ring_r.is_empty()
    }

    fn build(sample_rate: f32, stereo: bool) -> Self {
        let sample_rate = sample_rate.max(8000.0);
        let n = (sample_rate * RING_SEC) as usize + 64;
        let n = n.max(64);
        let right = |on: bool| if on { vec![0.0; n] } else { Vec::new() };
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
            ring_r: right(stereo),
            heard_r: right(stereo),
            lock_buf_r: right(stereo),
            rpt_hold_buf_r: right(stereo),
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
            lp_l_r: 0.0,
            lp_b_r: 0.0,
            hp_l_r: 0.0,
            hp_b_r: 0.0,
            slice_phase: 0.0,
            crush_hold: 0.0,
            crush_hold_r: 0.0,
            crush_left: 0,
            history: true,
            dry: false,
            dry_scratch: vec![0.0; DRY_SCRATCH_MAX],
            dry_scratch_r: if stereo {
                vec![0.0; DRY_SCRATCH_MAX]
            } else {
                Vec::new()
            },
        }
    }

    pub fn set_history(&mut self, on: bool) {
        self.history = on;
    }

    pub fn dry(&self) -> bool {
        self.dry
    }

    pub fn set_dry(&mut self, on: bool) {
        self.dry = on;
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
            if !self.heard_r.is_empty() {
                self.rpt_hold_buf_r[i] = self.heard_r[src];
            }
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
            if !self.ring_r.is_empty() {
                self.lock_buf_r[i] = self.ring_r[src];
            }
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
        for buf in [
            &mut self.ring_r,
            &mut self.heard_r,
            &mut self.lock_buf_r,
            &mut self.rpt_hold_buf_r,
        ] {
            buf.iter_mut().for_each(|s| *s = 0.0);
        }
        self.write = 0;
        self.filled = 0;
        self.rpt_read = 0.0;
        self.tape_read = 0.0;
        self.drop_read = 0.0;
        self.lp_l = 0.0;
        self.lp_b = 0.0;
        self.hp_l = 0.0;
        self.hp_b = 0.0;
        self.lp_l_r = 0.0;
        self.lp_b_r = 0.0;
        self.hp_l_r = 0.0;
        self.hp_b_r = 0.0;
        self.crush_hold = 0.0;
        self.crush_hold_r = 0.0;
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
        self.process_inner(buf, None, bpm);
    }

    /// Process L and R with one set of gates and playheads but separate audio
    /// history and filter state. On a mono rack the right channel is a copy of
    /// the processed left (use [`Self::new_stereo`] for true stereo).
    pub fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32], bpm: f32) {
        let n = left.len().min(right.len());
        if !self.is_stereo() {
            self.process_inner(&mut left[..n], None, bpm);
            right[..n].copy_from_slice(&left[..n]);
            return;
        }
        self.process_inner(&mut left[..n], Some(&mut right[..n]), bpm);
    }

    fn process_inner(&mut self, buf: &mut [f32], mut right: Option<&mut [f32]>, bpm: f32) {
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
        let filter_want = self.amount[PUNCH_LPF as usize] > ENGAGE
            || self.amount[PUNCH_HPF as usize] > ENGAGE
            || self.amount[PUNCH_CRUSH as usize] > ENGAGE
            || self.amount[PUNCH_SLICE as usize] > ENGAGE;
        // Parallel dry: keep the pre-FX bus and add it back after the wet chain.
        // Skip when nothing will change the buffer (avoids doubling on SEND-only
        // or a baked TAPE/DROP that already left the ring).
        let keep_dry = self.dry
            && (tape_want
                || rpt_want
                || drop_want
                || filter_want
                || (self.locked && self.lock_len >= 32)
                || self.grabbing);
        let dry_n = if keep_dry {
            let n = buf.len().min(self.dry_scratch.len());
            self.dry_scratch[..n].copy_from_slice(&buf[..n]);
            if let Some(r) = right.as_deref() {
                let nr = n.min(r.len()).min(self.dry_scratch_r.len());
                if nr > 0 {
                    self.dry_scratch_r[..nr].copy_from_slice(&r[..nr]);
                }
            }
            n
        } else {
            0
        };
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
        // The 4-second ring is one store per sample, but it runs on keys, drums,
        // and mic even when no pad is armed. HIST off skips that idle pass.
        // TAPE / RPT / DROP / LOCK / GRAB still record while they are on.
        let record = self.history || tape_want || rpt_want || drop_want || self.locked || self.grabbing;

        if record {
            for i in 0..buf.len() {
                let live = buf[i];
                let idx = self.write;
                self.ring[idx] = live;
                let live_r = match right.as_deref() {
                    Some(r) => {
                        let v = r[i];
                        self.ring_r[idx] = v;
                        v
                    }
                    None => 0.0,
                };
                self.write = (self.write + 1) % nring;
                if self.filled < nring {
                    self.filled += 1;
                }

                let tap = match voice {
                    BufferVoice::Live if locked => {
                        let pos = self.lock_read;
                        self.lock_read = (self.lock_read + 1.0).rem_euclid(lock_n as f32);
                        Tap::Lock(pos)
                    }
                    BufferVoice::Live => Tap::Live,
                    BufferVoice::Tape => {
                        self.tape_rate += (tape_target - self.tape_rate) * tape_slew;
                        let pos = self.tape_read;
                        let tap = if locked {
                            let next = (pos + self.tape_rate).rem_euclid(lock_n as f32);
                            self.lock_read = next;
                            self.tape_read = next;
                            Tap::Lock(pos)
                        } else {
                            self.tape_read = (pos + self.tape_rate).rem_euclid(nring as f32);
                            Tap::Ring(pos)
                        };
                        tap
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
                        let tap = if self.rpt_capture_len >= 32 && !locked {
                            let cap = self.rpt_capture_len;
                            let len = self.rpt_len.clamp(1, cap);
                            Tap::Window {
                                start: cap - len,
                                len,
                                pos: self.rpt_read,
                            }
                        } else if locked {
                            Tap::Lock(self.rpt_start as f32 + self.rpt_read)
                        } else {
                            Tap::Ring(self.rpt_start as f32 + self.rpt_read)
                        };
                        self.rpt_read += step;
                        let span = self.rpt_len.max(1) as f32;
                        while self.rpt_read >= span {
                            self.rpt_read -= span;
                        }
                        tap
                    }
                    BufferVoice::Drop => {
                        let pos = self.drop_read;
                        if locked {
                            let next = (pos + drop_rate).rem_euclid(lock_n as f32);
                            self.lock_read = next;
                            self.drop_read = next;
                            Tap::Lock(pos)
                        } else {
                            self.drop_read = (pos + drop_rate).rem_euclid(nring as f32);
                            Tap::Ring(pos)
                        }
                    }
                };
                let wet = tap.read(
                    &TapBuffers {
                        ring: &self.ring,
                        lock: &self.lock_buf,
                        lock_n,
                        hold: &self.rpt_hold_buf,
                    },
                    live,
                );
                // While RPT plays from its freeze, keep recording the live bus
                // into `heard`. Writing the loop back would poison the next
                // Refresh capture so every hit re-grabs the first sample.
                let heard_sample = if rpt_active && self.rpt_capture_len >= 32 && !locked {
                    live
                } else {
                    wet
                };
                self.heard[idx] = heard_sample;
                buf[i] = wet;
                let wet_r = if let Some(r) = right.as_deref_mut() {
                    let w = tap.read(
                        &TapBuffers {
                            ring: &self.ring_r,
                            lock: &self.lock_buf_r,
                            lock_n,
                            hold: &self.rpt_hold_buf_r,
                        },
                        live_r,
                    );
                    let heard_r = if rpt_active && self.rpt_capture_len >= 32 && !locked {
                        live_r
                    } else {
                        w
                    };
                    self.heard_r[idx] = heard_r;
                    r[i] = w;
                    Some(w)
                } else {
                    None
                };
                // Record what just came out. A tape already in that sound must not
                // be slowed again when the loop starts.
                if self.grabbing {
                    let cap = self.lock_buf.len();
                    if self.lock_len < cap {
                        self.lock_buf[self.lock_len] = wet;
                        if let Some(w) = wet_r {
                            self.lock_buf_r[self.lock_len] = w;
                        }
                        self.lock_len += 1;
                    }
                }
            }
        }

        let lpf = self.amount[PUNCH_LPF as usize];
        if lpf > ENGAGE {
            let tone = 1.0 - lpf * 0.92;
            apply_svf_lowpass(buf, tone, &mut self.lp_l, &mut self.lp_b, sr);
            if let Some(r) = right.as_deref_mut() {
                apply_svf_lowpass(r, tone, &mut self.lp_l_r, &mut self.lp_b_r, sr);
            }
        }
        let hpf = self.amount[PUNCH_HPF as usize];
        if hpf > ENGAGE {
            apply_svf_highpass(buf, hpf, &mut self.hp_l, &mut self.hp_b, sr);
            if let Some(r) = right.as_deref_mut() {
                apply_svf_highpass(r, hpf, &mut self.hp_l_r, &mut self.hp_b_r, sr);
            }
        }
        let crush = self.amount[PUNCH_CRUSH as usize];
        if crush > ENGAGE {
            let left_at_start = self.crush_left;
            apply_crush(buf, crush, &mut self.crush_hold, &mut self.crush_left);
            if let Some(r) = right.as_deref_mut() {
                let mut left_r = left_at_start;
                apply_crush(r, crush, &mut self.crush_hold_r, &mut left_r);
            }
        }
        let slice = self.amount[PUNCH_SLICE as usize];
        if slice > ENGAGE {
            // SLICE is a live amplitude gate — never clamp the period to ring
            // fill. HIST-off leaves `filled` at 0, and the old path collapsed
            // every division to 32 samples (~1.5 kHz buzz).
            let period = slice_period_samples(slice, bpm, sr, self.grid_mode) as f32;
            let phase_at_start = self.slice_phase;
            apply_slice(buf, period, &mut self.slice_phase);
            if let Some(r) = right.as_deref_mut() {
                let mut phase_r = phase_at_start;
                apply_slice(r, period, &mut phase_r);
            }
        }

        if keep_dry && dry_n > 0 {
            for i in 0..dry_n {
                buf[i] += self.dry_scratch[i];
            }
            if let Some(r) = right.as_deref_mut() {
                let nr = dry_n.min(r.len()).min(self.dry_scratch_r.len());
                for i in 0..nr {
                    r[i] += self.dry_scratch_r[i];
                }
            }
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
    beat_samples(beats, bpm, sr)
}

fn beat_samples(beats: f32, bpm: f32, sr: f32) -> usize {
    let sec = beats * 60.0 / bpm.max(20.0);
    (sec * sr.max(8000.0)).round() as usize
}

/// Floor so the gate never enters ultrasonic / buzz territory.
/// Matches the shortest Beat-grid division (1/32) at the given tempo.
fn min_slice_samples(bpm: f32, sr: f32) -> usize {
    beat_samples(PUNCH_GRID_BEATS[3], bpm, sr).max(32)
}

/// SLICE gate period in samples. Beat mode snaps to the punch grid; Smooth
/// lerps continuously from 1/4 → 1/32. Independent of ring / lock fill.
fn slice_period_samples(amount: f32, bpm: f32, sr: f32, mode: PunchGridMode) -> usize {
    let floor = min_slice_samples(bpm, sr);
    let len = match mode {
        PunchGridMode::Beat => rpt_loop_samples(amount, bpm, sr),
        PunchGridMode::Smooth => {
            let long = beat_samples(PUNCH_GRID_BEATS[0], bpm, sr) as f32;
            let short = beat_samples(PUNCH_GRID_BEATS[3], bpm, sr) as f32;
            let t = amount.clamp(0.0, 1.0);
            (long + (short - long) * t).round() as usize
        }
    };
    len.max(floor)
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
        assert_eq!(punch_grid_index(f32::NAN), 0);
        assert!(punch_slot_has_grid(PUNCH_RPT as usize));
        assert!(punch_slot_has_grid(PUNCH_SLICE as usize));
        assert!(!punch_slot_has_grid(PUNCH_TAPE as usize));
        assert_eq!(PUNCH_GRID_LABELS[punch_grid_index(0.1)], "1/4");
        assert_eq!(PUNCH_GRID_LABELS[punch_grid_index(0.4)], "1/8");
    }

    #[test]
    fn grid_division_changes_exactly_at_each_divider() {
        for (i, tick) in PUNCH_GRID_TICKS.iter().enumerate() {
            assert_eq!(punch_grid_index(*tick - 1e-4), i, "just below divider {i}");
            assert_eq!(punch_grid_index(*tick), i + 1, "at divider {i}");
            assert_eq!(punch_grid_index(*tick + 1e-4), i + 1, "just above divider {i}");
        }
        assert_eq!(punch_grid_index(0.0), 0);
        assert_eq!(punch_grid_index(1.0), PUNCH_GRID_LABELS.len() - 1);
        assert_eq!(PUNCH_GRID_TICKS.len() + 1, PUNCH_GRID_LABELS.len());
        assert_eq!(PUNCH_GRID_LABELS.len(), PUNCH_GRID_BEATS.len());
    }

    #[test]
    fn grid_bands_tile_the_slider_and_agree_with_the_index() {
        let mut prev_high = 0.0;
        for i in 0..PUNCH_GRID_LABELS.len() {
            let (low, high) = punch_grid_band(i);
            assert_eq!(low, prev_high, "band {i} must start where the last ended");
            assert!(high > low);
            let mid = 0.5 * (low + high);
            assert_eq!(punch_grid_index(mid), i);
            assert_eq!(punch_grid_label(mid), PUNCH_GRID_LABELS[i]);
            prev_high = high;
        }
        assert_eq!(prev_high, 1.0);
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
    fn refresh_mode_recaptures_on_each_new_engage() {
        let mut p = PunchRack::new(48_000.0);
        p.set_rpt_mode(PunchRptMode::Refresh);
        let loop_n = rpt_loop_samples(0.1, 120.0, 48_000.0);
        // First hit freezes a loud sustained tone.
        let mut a = vec![0.9f32; loop_n];
        p.process(&mut a, 120.0);
        p.set_amount(PUNCH_RPT, 0.1);
        // While stuttering A, the live bus is a soft tone. If RPT writes its
        // freeze back into `heard`, every later Refresh hit still grabs A.
        let mut under = vec![0.2f32; loop_n * 8];
        p.process(&mut under, 120.0);
        let mean_a = under.iter().map(|s| s.abs()).sum::<f32>() / under.len() as f32;
        assert!(mean_a > 0.7, "first engage must loop loud A, mean={mean_a}");
        // Immediate re-arm (pad tap-off / tap-on).
        p.set_amount(PUNCH_RPT, 0.0);
        let mut gap = vec![0.2f32; 64];
        p.process(&mut gap, 120.0);
        p.set_amount(PUNCH_RPT, 0.1);
        let mut loop_b = vec![0.0f32; loop_n * 2];
        p.process(&mut loop_b, 120.0);
        let mean_b = loop_b.iter().map(|s| s.abs()).sum::<f32>() / loop_b.len() as f32;
        assert!(
            mean_b > 0.15 && mean_b < 0.35,
            "Refresh must re-capture the soft live bus, not keep A, mean={mean_b}"
        );
    }

    #[test]
    fn hold_mode_keeps_the_first_capture_across_releases() {
        let mut p = PunchRack::new(48_000.0);
        p.set_rpt_mode(PunchRptMode::Hold);
        let loop_n = rpt_loop_samples(0.1, 120.0, 48_000.0);
        let mut a = vec![0.9f32; loop_n];
        p.process(&mut a, 120.0);
        p.set_amount(PUNCH_RPT, 0.1);
        let mut loop_a = vec![0.2f32; loop_n * 2];
        p.process(&mut loop_a, 120.0);
        p.set_amount(PUNCH_RPT, 0.0);
        let mut b = vec![0.2f32; loop_n * 2];
        p.process(&mut b, 120.0);
        p.set_amount(PUNCH_RPT, 0.1);
        let mut loop_held = vec![0.0f32; loop_n * 2];
        p.process(&mut loop_held, 120.0);
        let mean = loop_held.iter().map(|s| s.abs()).sum::<f32>() / loop_held.len() as f32;
        assert!(
            mean > 0.7,
            "Hold must keep loud A until CLEAR, mean={mean}"
        );
        p.clear();
        // After CLEAR, prime soft history and engage again.
        let mut after_clear = vec![0.15f32; loop_n];
        p.process(&mut after_clear, 120.0);
        p.set_amount(PUNCH_RPT, 0.1);
        let mut out = vec![0.15f32; loop_n * 2];
        p.process(&mut out, 120.0);
        let mean_clear = out.iter().map(|s| s.abs()).sum::<f32>() / out.len() as f32;
        assert!(
            mean_clear > 0.05 && mean_clear < 0.35,
            "CLEAR must drop the held A so a new capture can take, mean={mean_clear}"
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
    fn slice_period_never_below_thirty_second_note() {
        let sr = 48_000.0;
        for &bpm in &[60.0_f32, 100.0, 120.0, 140.0, 180.0, 240.0] {
            let floor = min_slice_samples(bpm, sr);
            let want_32 = beat_samples(0.125, bpm, sr);
            assert_eq!(floor, want_32.max(32), "floor @ {bpm} BPM");
            for &amount in &[0.0_f32, 0.2, 0.5, 0.75, 1.0] {
                for mode in PunchGridMode::ALL {
                    let period = slice_period_samples(amount, bpm, sr, mode);
                    assert!(
                        period >= floor,
                        "amount={amount} mode={mode:?} bpm={bpm}: period {period} < floor {floor}"
                    );
                }
            }
            // Beat bands map to exact grid lengths.
            assert_eq!(
                slice_period_samples(0.1, bpm, sr, PunchGridMode::Beat),
                beat_samples(1.0, bpm, sr)
            );
            assert_eq!(
                slice_period_samples(0.9, bpm, sr, PunchGridMode::Beat),
                beat_samples(0.125, bpm, sr)
            );
        }
    }

    #[test]
    fn slice_with_empty_history_stays_musical_not_buzz() {
        // HIST off → filled stays 0. Old code clamped the gate to 32 samples.
        let sr = 48_000.0;
        let bpm = 120.0;
        let mut p = PunchRack::new(sr);
        p.set_history(false);
        p.set_grid_mode(PunchGridMode::Beat);
        p.set_amount(PUNCH_SLICE, 1.0); // 1/32
        let mut buf = vec![0.5f32; 8_000];
        p.process(&mut buf, bpm);

        // Closed runs (~55% of period) should track 1/32 @ 120 BPM ≈ 1500 samples,
        // not the old 32-sample buzz (~18-sample closed runs).
        let period = slice_period_samples(1.0, bpm, sr, PunchGridMode::Beat);
        assert!(period >= 1400, "1/32 @ 120 must be ~1500 samples, got {period}");
        let mut closed_runs = Vec::new();
        let mut run = 0usize;
        let mut in_silence = buf[0].abs() < 1e-5;
        for &s in &buf {
            let silent = s.abs() < 1e-5;
            if silent == in_silence {
                run += 1;
            } else {
                if in_silence {
                    closed_runs.push(run);
                }
                in_silence = silent;
                run = 1;
            }
        }
        if in_silence {
            closed_runs.push(run);
        }
        let avg = closed_runs.iter().sum::<usize>() as f32 / closed_runs.len().max(1) as f32;
        let expect_closed = period as f32 * 0.55;
        assert!(
            avg > 400.0,
            "empty-history SLICE must not buzz: avg closed run {avg} (want ~{expect_closed}), runs={closed_runs:?}"
        );
    }

    #[test]
    fn send_boost_opens_a_dry_mix() {
        assert!((boost_send_mix(0.0, 1.0) - 0.95).abs() < 1e-5);
        assert!(boost_send_mix(0.4, 0.0) < 0.41);
        assert!(boost_send_mix(0.4, 1.0) > 0.9);
    }

    #[test]
    fn dry_on_keeps_the_pre_fx_bus_alongside_slice() {
        let mut wet_only = PunchRack::new(48_000.0);
        wet_only.set_amount(PUNCH_SLICE, 0.9);
        let mut dry_on = PunchRack::new(48_000.0);
        dry_on.set_dry(true);
        dry_on.set_amount(PUNCH_SLICE, 0.9);
        let mut wet_buf = vec![0.5f32; 8000];
        let mut dry_buf = vec![0.5f32; 8000];
        wet_only.process(&mut wet_buf, 120.0);
        dry_on.process(&mut dry_buf, 120.0);
        let wet_silent = wet_buf.iter().filter(|s| s.abs() < 1e-5).count();
        let dry_silent = dry_buf.iter().filter(|s| s.abs() < 1e-5).count();
        assert!(wet_silent > 1000, "wet-only slice must gate, silent={wet_silent}");
        assert!(
            dry_silent < 10,
            "dry+slice must keep the pre-FX tone in the gaps, silent={dry_silent}"
        );
        let dry_loud = dry_buf.iter().filter(|s| (*s - 1.0).abs() < 0.05).count();
        assert!(
            dry_loud > 1000,
            "open slice slots should be dry+wet (~1.0), loud={dry_loud}"
        );
    }

    #[test]
    fn dry_off_is_the_default_and_send_alone_does_not_double() {
        let mut p = PunchRack::new(48_000.0);
        assert!(!p.dry());
        p.set_amount(PUNCH_SEND, 1.0);
        p.set_dry(true);
        let mut buf = vec![0.4f32; 256];
        p.process(&mut buf, 120.0);
        assert!(
            buf.iter().all(|v| (*v - 0.4).abs() < 1e-6),
            "SEND-only must not double when dry is on"
        );
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
    fn history_off_leaves_the_idle_ring_empty() {
        let mut p = PunchRack::new(48_000.0);
        p.set_history(false);
        let mut loud = vec![0.9f32; 4000];
        p.process(&mut loud, 120.0);
        assert!(loud.iter().all(|v| (*v - 0.9).abs() < 1e-5));
        p.set_locked(true);
        let mut out = vec![0.0f32; 512];
        p.process(&mut out, 120.0);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(
            peak < 0.05,
            "HIST off still kept a pre-roll, peak={peak}"
        );
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

    fn distinct_lr(len: usize, offset: usize) -> (Vec<f32>, Vec<f32>) {
        let l = (0..len)
            .map(|i| 0.6 * ((i + offset) as f32 * 0.071).sin() + 0.2 * (((i + offset) % 37) as f32 / 37.0 - 0.5))
            .collect();
        let r = (0..len)
            .map(|i| 0.5 * (((i + offset) % 91) as f32 / 91.0 - 0.5) + 0.3 * ((i + offset) as f32 * 0.23).cos())
            .collect();
        (l, r)
    }

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()))
    }

    /// Run a stereo rack and two independent mono racks on the same L / R
    /// signals, through a prime phase, an arm phase, and a release phase.
    /// Returns (stereo L, stereo R, mono L, mono R, dry L, dry R) for the arm phase.
    fn run_pair(
        setup: impl Fn(&mut PunchRack),
        arm: impl Fn(&mut PunchRack),
    ) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut st = PunchRack::new_stereo(48_000.0);
        let mut ml = PunchRack::new(48_000.0);
        let mut mr = PunchRack::new(48_000.0);
        for rack in [&mut st, &mut ml, &mut mr] {
            setup(rack);
        }
        let block = 256;
        let mut off = 0;
        let mut feed = |st: &mut PunchRack, ml: &mut PunchRack, mr: &mut PunchRack, blocks: usize| {
            let mut out = (vec![], vec![], vec![], vec![], vec![], vec![]);
            for _ in 0..blocks {
                let (l, r) = distinct_lr(block, off);
                off += block;
                let (mut sl, mut sr) = (l.clone(), r.clone());
                let (mut a, mut b) = (l.clone(), r.clone());
                st.process_stereo(&mut sl, &mut sr, 120.0);
                ml.process(&mut a, 120.0);
                mr.process(&mut b, 120.0);
                out.0.extend(sl);
                out.1.extend(sr);
                out.2.extend(a);
                out.3.extend(b);
                out.4.extend(l);
                out.5.extend(r);
            }
            out
        };
        feed(&mut st, &mut ml, &mut mr, 40);
        for rack in [&mut st, &mut ml, &mut mr] {
            arm(rack);
        }
        feed(&mut st, &mut ml, &mut mr, 60)
    }

    fn assert_both_wet_and_independent(name: &str, f: impl Fn(&mut PunchRack), prime: impl Fn(&mut PunchRack)) {
        let (sl, sr, ml, mr, dl, dr) = run_pair(prime, f);
        assert!(max_diff(&sl, &dl) > 0.02, "{name}: left must be wet");
        assert!(max_diff(&sr, &dr) > 0.02, "{name}: right must be wet");
        assert!(max_diff(&sl, &sr) > 0.02, "{name}: L and R must stay distinct");
        assert!(max_diff(&sl, &ml) < 1e-6, "{name}: stereo L must equal a mono rack on L");
        assert!(max_diff(&sr, &mr) < 1e-6, "{name}: stereo R must equal a mono rack on R");
    }

    #[test]
    fn stereo_filters_process_both_channels_with_own_state() {
        assert_both_wet_and_independent("LPF", |p| p.set_amount(PUNCH_LPF, 0.9), |_| {});
        assert_both_wet_and_independent("HPF", |p| p.set_amount(PUNCH_HPF, 0.9), |_| {});
    }

    #[test]
    fn stereo_crush_and_slice_process_both_channels() {
        assert_both_wet_and_independent("CRUSH", |p| p.set_amount(PUNCH_CRUSH, 1.0), |_| {});
        assert_both_wet_and_independent("SLICE", |p| p.set_amount(PUNCH_SLICE, 0.9), |_| {});
    }

    #[test]
    fn stereo_buffer_effects_process_both_channels() {
        assert_both_wet_and_independent("RPT", |p| p.set_amount(PUNCH_RPT, 0.6), |_| {});
        assert_both_wet_and_independent("TAPE", |p| p.set_amount(PUNCH_TAPE, 1.0), |_| {});
        assert_both_wet_and_independent("DROP", |p| p.set_amount(PUNCH_DROP, 1.0), |_| {});
        assert_both_wet_and_independent(
            "RPT smooth",
            |p| p.set_amount(PUNCH_RPT, 0.3),
            |p| p.set_grid_mode(PunchGridMode::Smooth),
        );
    }

    #[test]
    fn stereo_buffer_priorities_stay_per_channel() {
        for prio in PunchBufferPrio::ALL {
            assert_both_wet_and_independent(
                prio.label(),
                |p| {
                    p.set_amount(PUNCH_TAPE, 0.8);
                    p.set_amount(PUNCH_RPT, 0.7);
                    p.set_amount(PUNCH_DROP, 0.6);
                },
                |p| p.set_buffer_prio(prio),
            );
        }
    }

    #[test]
    fn stereo_lock_and_grab_replay_each_channel() {
        assert_both_wet_and_independent(
            "LOCK",
            |p| p.set_locked(true),
            |_| {},
        );
        let mut st = PunchRack::new_stereo(48_000.0);
        let mut ml = PunchRack::new(48_000.0);
        let mut mr = PunchRack::new(48_000.0);
        let (l, r) = distinct_lr(1024, 0);
        let run = |st: &mut PunchRack, ml: &mut PunchRack, mr: &mut PunchRack| {
            let (mut sl, mut sr) = (l.clone(), r.clone());
            let (mut a, mut b) = (l.clone(), r.clone());
            st.process_stereo(&mut sl, &mut sr, 120.0);
            ml.process(&mut a, 120.0);
            mr.process(&mut b, 120.0);
            (sl, sr, a, b)
        };
        for rack in [&mut st, &mut ml, &mut mr] {
            rack.set_grabbing(true);
        }
        run(&mut st, &mut ml, &mut mr);
        for rack in [&mut st, &mut ml, &mut mr] {
            rack.set_grabbing(false);
        }
        let silence = vec![0.0f32; 2048];
        let (mut sl, mut sr) = (silence.clone(), silence.clone());
        let (mut a, mut b) = (silence.clone(), silence.clone());
        st.process_stereo(&mut sl, &mut sr, 120.0);
        ml.process(&mut a, 120.0);
        mr.process(&mut b, 120.0);
        assert!(sl.iter().any(|v| v.abs() > 0.1), "grab replays left");
        assert!(sr.iter().any(|v| v.abs() > 0.1), "grab replays right");
        assert!(max_diff(&sl, &a) < 1e-6 && max_diff(&sr, &b) < 1e-6);
        assert!(max_diff(&sl, &sr) > 0.05, "grab must keep L and R distinct");
    }

    #[test]
    fn stereo_beat_synced_effects_stay_phase_aligned() {
        let mut p = PunchRack::new_stereo(48_000.0);
        let loop_n = rpt_loop_samples(0.1, 120.0, 48_000.0);
        // Same impulse timing in both channels, different amplitude.
        let mut l = vec![0.0f32; loop_n];
        let mut r = vec![0.0f32; loop_n];
        l[0] = 1.0;
        r[0] = -0.5;
        p.process_stereo(&mut l, &mut r, 120.0);
        p.set_amount(PUNCH_RPT, 0.1);
        let mut l = vec![0.0f32; loop_n * 3];
        let mut r = vec![0.0f32; loop_n * 3];
        p.process_stereo(&mut l, &mut r, 120.0);
        let hits = |buf: &[f32]| -> Vec<usize> {
            (0..buf.len()).filter(|i| buf[*i].abs() > 0.2).collect()
        };
        let (hl, hr) = (hits(&l), hits(&r));
        assert!(hl.len() >= 3, "left must repeat, hits={hl:?}");
        assert_eq!(hl, hr, "RPT hits must land on the same samples in L and R");

        let mut p = PunchRack::new_stereo(48_000.0);
        p.set_amount(PUNCH_SLICE, 0.9);
        let mut l = vec![0.5f32; 8000];
        let mut r = vec![-0.3f32; 8000];
        p.process_stereo(&mut l, &mut r, 120.0);
        let gate_l: Vec<bool> = l.iter().map(|s| s.abs() < 1e-5).collect();
        let gate_r: Vec<bool> = r.iter().map(|s| s.abs() < 1e-5).collect();
        assert!(gate_l.iter().any(|g| *g) && gate_l.iter().any(|g| !*g));
        assert_eq!(gate_l, gate_r, "SLICE gate must open and close together");
    }

    #[test]
    fn stereo_no_cross_channel_bleed() {
        let mut p = PunchRack::new_stereo(48_000.0);
        p.set_amount(PUNCH_LPF, 0.5);
        p.set_amount(PUNCH_HPF, 0.3);
        p.set_amount(PUNCH_CRUSH, 0.5);
        p.set_amount(PUNCH_TAPE, 0.5);
        let (l, _) = distinct_lr(8192, 0);
        let mut left = l;
        let mut right = vec![0.0f32; 8192];
        p.process_stereo(&mut left, &mut right, 120.0);
        assert!(left.iter().any(|v| v.abs() > 0.05));
        assert!(
            right.iter().all(|v| v.abs() < 1e-9),
            "silent right must stay silent while left is loud"
        );
    }

    #[test]
    fn stereo_idle_rack_is_transparent() {
        let mut p = PunchRack::new_stereo(48_000.0);
        let (l, r) = distinct_lr(512, 0);
        let (mut a, mut b) = (l.clone(), r.clone());
        p.process_stereo(&mut a, &mut b, 120.0);
        assert!(max_diff(&a, &l) < 1e-6 && max_diff(&b, &r) < 1e-6);
    }
}
