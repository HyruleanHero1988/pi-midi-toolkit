//! 800×480 hit-testing. Coordinates are pixels unless noted as 0..1 pad space.

use crate::mode::UiMode;

pub const SCREEN_W: i32 = 800;
pub const SCREEN_H: i32 = 480;
/// Bottom strip removed — Tk chrome lives in the top bar (`HUD_H`).
pub const NAV_H: i32 = 0;
/// Top chrome: PiDI brand + HOME/POWER + jam tabs (matches Tk nav height).
pub const HUD_H: i32 = 52;

/// Jam-mode tabs on the right of the top chrome (Tk order + chords + FM).
pub const JAM_MODES: [UiMode; 6] = [
    UiMode::Synth,
    UiMode::Fm,
    UiMode::Seq,
    UiMode::Pads,
    UiMode::Kaoss,
    UiMode::Chords,
];

/// Home grid entries (5×2). LOG / MAP live under Settings.
pub const HOME_TILES: [(UiMode, &'static str, u32); 10] = [
    (UiMode::Synth, "SYNTH", 0x458588),
    (UiMode::Fm, "FM", 0x8ec07c),
    (UiMode::Drums, "DRUMS", 0x98971a),
    (UiMode::Seq, "SEQ", 0xb16286),
    (UiMode::Pads, "PADS", 0xd79921),
    (UiMode::Kaoss, "KAOSS", 0xfe8019),
    (UiMode::Chords, "CHORDS", 0xcc241d),
    (UiMode::Songs, "SONGS", 0x689d6a),
    (UiMode::Presets, "PRESETS", 0x83a598),
    (UiMode::Settings, "SETTINGS", 0x665c54),
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Nav(UiMode),
    Kaoss { x: f32, y: f32 },
    Drum { index: usize, note: u8 },
    Division(usize),
    PhrasePad(usize),
    StopAllClips,
    PadsPlayView,
    PadsEditView,
    PadsClearArm,
    PadsTrig,
    PadsModeArm,
    PadsRec,
    PadsVolUp,
    PadsVolDown,
    PadsVoice,
    PadsChannel,
    PadsSynth,
    PadsOut,
    HomeTile(UiMode),
    Power,
    PowerShutdown,
    PowerReboot,
    PowerScreenOff,
    PowerBlankCycle,
    NavBack,
    SynthSlider(usize),
    SynthKey { note: u8 },
    SynthWaveA,
    SynthWaveB,
    SynthSwap,
    SynthVib,
    SynthVibAlways,
    SynthVibDepthUp,
    SynthVibDepthDown,
    SynthVibRateUp,
    SynthVibRateDown,
    SynthPickDone,
    SynthSaveAs,
    SynthOctUp,
    SynthOctDown,
    FmRecipe(usize),
    FmSlider(usize),
    ScrollArea(crate::scroll::ScrollKind),
    DrumMacro(usize),
    KitAllDrums,
    MapThruOn,
    MapThruOff,
    MapRefresh,
    KaossProg,
    KaossScale,
    KaossKey,
    KaossOct,
    KaossOctUp,
    KaossOctDown,
    KaossHold,
    KaossGate,
    KaossFull,
    KaossBpmUp,
    KaossBpmDown,
    KaossBpmUp5,
    KaossBpmDown5,
    KaossShowAll,
    KaossChannel,
    KaossSettings,
    KaossWipeFx,
    KaossViz,
    KaossOut,
    KaossAxes,
    KaossGridLines,
    KaossVizCells,
    KaossVizGlow,
    KaossColor,
    KaossColorPick(usize),
    KaossColorDone,
    KaossOutPick(usize),
    KaossChannelPick(u8),
    KaossGridWidthUp,
    KaossGridWidthDown,
    KaossPicker(usize),
    KaossPickerClose,
    SeqRec,
    SeqPlay,
    SeqKeep,
    SeqDrop,
    SeqUndo,
    SeqLenDouble,
    SeqLenHalve,
    SeqExtend,
    SeqStop,
    SeqClear,
    SeqBpmUp,
    SeqBpmDown,
    SeqToPad,
    SeqAllOff,
    PresetSlot(usize),
    PresetLoad,
    PresetSave,
    PresetDelete,
    PresetFactory,
    SongRow(usize),
    SongPlay,
    SongStop,
    SongPrev,
    SongNext,
    SongLoop,
    SongDelete,
    SongBpmUp,
    SongBpmDown,
    SongSaveSeq,
    SongOut,
    SettingsPanic,
    SettingsAllOff,
    SettingsFxTarget,
    SettingsFx(usize),
    SettingsWifi,
    SettingsUpdate,
    SettingsFont,
    SettingsLog,
    SettingsMap,
    LogClear,
    LogAllOff,
    ChordsButton { col: usize, row: usize },
    ChordsStrum { y: f32 },
    ChordsPalette { slot: usize },
    ChordsOut,
    ChordsHold,
    ChordsKey,
    ChordsChanges,
    ChordsArm,
    ChordsKeyPick(u8),
    ChordsChangesPick(usize),
    ChordsOverlayClose,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    pub fn pad_xy(self, px: i32, py: i32) -> (f32, f32) {
        let x = (px - self.x) as f32 / self.w.max(1) as f32;
        let y = 1.0 - (py - self.y) as f32 / self.h.max(1) as f32;
        (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub hud: Rect,
    pub content: Rect,
    pub nav: Rect,
    pub kaoss: Rect,
    pub drums: Rect,
    pub divisions: Rect,
    pub phrase_grid: Rect,
    pub stop_all: Rect,
    pub pads_out: Rect,
    pub pads_play: Rect,
    pub pads_edit: Rect,
    pub pads_clear: Rect,
    pub pads_trig: Rect,
    pub pads_mode: Rect,
    pub pads_rec: Rect,
    pub pads_vol_down: Rect,
    pub pads_vol_up: Rect,
    pub pads_voice: Rect,
    pub pads_channel: Rect,
    pub pads_synth: Rect,
    pub synth_sliders: Rect,
    pub synth_keys: Rect,
    pub synth_scope: Rect,
    pub kit_scope: Rect,
    pub kit_grid: Rect,
    pub kit_macros: Rect,
    pub kit_divisions: Rect,
    pub kit_all: Rect,
    pub synth_wave_a: Rect,
    pub synth_wave_b: Rect,
    pub synth_swap: Rect,
    pub synth_vib: Rect,
    pub synth_pick_done: Rect,
    pub synth_pick_prev: Rect,
    pub synth_pick_next: Rect,
    pub synth_pick_grid: Rect,
    pub synth_save_as: Rect,
    pub synth_oct_down: Rect,
    pub synth_oct_up: Rect,
    pub synth_pick_save_as: Rect,
    pub map_thru_on: Rect,
    pub map_thru_off: Rect,
    pub map_refresh: Rect,
    pub kaoss_prog: Rect,
    pub kaoss_scale: Rect,
    pub kaoss_key: Rect,
    pub kaoss_oct: Rect,
    pub kaoss_oct_down: Rect,
    pub kaoss_oct_up: Rect,
    pub kaoss_hold: Rect,
    pub kaoss_gate: Rect,
    pub kaoss_full: Rect,
    pub kaoss_bpm_up: Rect,
    pub kaoss_bpm_down: Rect,
    pub kaoss_bpm_up5: Rect,
    pub kaoss_bpm_down5: Rect,
    pub kaoss_show_all: Rect,
    pub kaoss_channel: Rect,
    pub kaoss_wipe_fx: Rect,
    pub kaoss_viz: Rect,
    pub kaoss_out: Rect,
    pub kaoss_settings_btn: Rect,
    pub settings_log: Rect,
    pub settings_map: Rect,
    pub seq_rec: Rect,
    pub seq_play: Rect,
    pub seq_keep: Rect,
    pub seq_drop: Rect,
    pub seq_undo: Rect,
    pub seq_len_double: Rect,
    pub seq_len_halve: Rect,
    pub seq_extend: Rect,
    pub seq_stop: Rect,
    pub seq_clear: Rect,
    pub seq_to_pad: Rect,
    pub seq_all_off: Rect,
    pub seq_bpm_up: Rect,
    pub seq_bpm_down: Rect,
    pub seq_drums: Rect,
    pub preset_grid: Rect,
    pub preset_load: Rect,
    pub preset_save: Rect,
    pub preset_delete: Rect,
    pub preset_factory: Rect,
    pub song_list: Rect,
    pub song_play: Rect,
    pub song_stop: Rect,
    pub song_prev: Rect,
    pub song_next: Rect,
    pub song_loop: Rect,
    pub song_delete: Rect,
    pub song_bpm_up: Rect,
    pub song_bpm_down: Rect,
    pub song_save_seq: Rect,
    pub song_out: Rect,
    pub settings_panic: Rect,
    pub settings_all_off: Rect,
    pub settings_fx_target: Rect,
    pub settings_fx: Rect,
    pub settings_wifi: Rect,
    pub settings_font: Rect,
    pub settings_update: Rect,
    pub log_clear: Rect,
    pub log_all_off: Rect,
    pub chords_toolbar: Rect,
    pub chords_grid: Rect,
    pub chords_strum: Rect,
    pub chords_palette: Rect,
    pub power_blank_cycle: Rect,
    pub power_shutdown: Rect,
    pub power_reboot: Rect,
    pub power_screen_off: Rect,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

impl Layout {
    fn power_menu_rects(content: Rect) -> (Rect, Rect, Rect, Rect) {
        let pad = 10;
        let header_h = 52;
        let footer_h = 68;
        let body_top = content.y + header_h;
        let body_h = content.h - header_h - footer_h;
        let blank_cycle = Rect {
            x: content.x + content.w - 200,
            y: content.y + 8,
            w: 190,
            h: 44,
        };
        let shutdown = Rect {
            x: content.x + pad,
            y: body_top + pad,
            w: content.w - pad * 2,
            h: body_h / 2 - pad - 3,
        };
        let reboot = Rect {
            x: content.x + pad,
            y: body_top + body_h / 2 + 3,
            w: content.w - pad * 2,
            h: body_h / 2 - pad - 3,
        };
        let screen_off = Rect {
            x: content.x + pad,
            y: content.y + content.h - footer_h + 8,
            w: content.w - pad * 2,
            h: footer_h - 16,
        };
        (blank_cycle, shutdown, reboot, screen_off)
    }

    pub fn new() -> Self {
        let content_h = SCREEN_H - HUD_H - NAV_H;
        let content = Rect {
            x: 0,
            y: HUD_H,
            w: SCREEN_W,
            h: content_h,
        };
        let (power_blank_cycle, power_shutdown, power_reboot, power_screen_off) =
            Self::power_menu_rects(content);
        Self {
            hud: Rect {
                x: 0,
                y: 0,
                w: SCREEN_W,
                h: HUD_H,
            },
            content,
            // Top chrome (Tk): brand / home / power / jam tabs.
            nav: Rect {
                x: 0,
                y: 0,
                w: SCREEN_W,
                h: HUD_H,
            },
            // Full-width Kaoss pad like Tk (drums live on SEQ / kit, not here).
            kaoss: Rect {
                x: 8,
                y: HUD_H + 8,
                w: SCREEN_W - 16,
                h: content_h - 112,
            },
            drums: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            divisions: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            // Two-row footer chrome (Tk parity), full width under pad+drums.
            kaoss_prog: Rect {
                x: 8,
                y: HUD_H + content_h - 100,
                w: 152,
                h: 44,
            },
            kaoss_scale: Rect {
                x: 168,
                y: HUD_H + content_h - 100,
                w: 152,
                h: 44,
            },
            kaoss_key: Rect {
                x: 328,
                y: HUD_H + content_h - 100,
                w: 152,
                h: 44,
            },
            kaoss_oct_down: Rect {
                x: 488,
                y: HUD_H + content_h - 100,
                w: 40,
                h: 44,
            },
            kaoss_oct: Rect {
                x: 532,
                y: HUD_H + content_h - 100,
                w: 68,
                h: 44,
            },
            kaoss_oct_up: Rect {
                x: 604,
                y: HUD_H + content_h - 100,
                w: 40,
                h: 44,
            },
            kaoss_hold: Rect {
                x: 648,
                y: HUD_H + content_h - 100,
                w: 144,
                h: 44,
            },
            kaoss_gate: Rect {
                x: 8,
                y: HUD_H + content_h - 48,
                w: 144,
                h: 40,
            },
            kaoss_bpm_down5: Rect {
                x: 160,
                y: HUD_H + content_h - 48,
                w: 70,
                h: 40,
            },
            kaoss_bpm_down: Rect {
                x: 234,
                y: HUD_H + content_h - 48,
                w: 70,
                h: 40,
            },
            kaoss_bpm_up: Rect {
                x: 308,
                y: HUD_H + content_h - 48,
                w: 70,
                h: 40,
            },
            kaoss_bpm_up5: Rect {
                x: 382,
                y: HUD_H + content_h - 48,
                w: 70,
                h: 40,
            },
            kaoss_show_all: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            kaoss_viz: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            kaoss_wipe_fx: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            kaoss_channel: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            kaoss_out: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            kaoss_full: Rect {
                x: 460,
                y: HUD_H + content_h - 48,
                w: 148,
                h: 40,
            },
            kaoss_settings_btn: Rect {
                x: 616,
                y: HUD_H + content_h - 48,
                w: 176,
                h: 40,
            },
            phrase_grid: Rect {
                x: 16,
                y: HUD_H + 56,
                w: 640,
                h: content_h - 120,
            },
            stop_all: Rect {
                x: 668,
                y: HUD_H + content_h - 56,
                w: 116,
                h: 48,
            },
            pads_out: Rect {
                x: 540,
                y: HUD_H + content_h - 56,
                w: 120,
                h: 48,
            },
            pads_clear: Rect {
                x: 16,
                y: HUD_H + content_h - 56,
                w: 72,
                h: 48,
            },
            pads_trig: Rect {
                x: 92,
                y: HUD_H + content_h - 56,
                w: 72,
                h: 48,
            },
            pads_mode: Rect {
                x: 168,
                y: HUD_H + content_h - 56,
                w: 64,
                h: 48,
            },
            pads_rec: Rect {
                x: 236,
                y: HUD_H + content_h - 56,
                w: 64,
                h: 48,
            },
            pads_voice: Rect {
                x: 304,
                y: HUD_H + content_h - 56,
                w: 72,
                h: 48,
            },
            pads_channel: Rect {
                x: 380,
                y: HUD_H + content_h - 56,
                w: 64,
                h: 48,
            },
            pads_synth: Rect {
                x: 448,
                y: HUD_H + content_h - 56,
                w: 64,
                h: 48,
            },
            pads_vol_down: Rect {
                x: 392,
                y: HUD_H + 8,
                w: 56,
                h: 40,
            },
            pads_vol_up: Rect {
                x: 452,
                y: HUD_H + 8,
                w: 56,
                h: 40,
            },
            pads_play: Rect {
                x: 520,
                y: HUD_H + 8,
                w: 120,
                h: 40,
            },
            pads_edit: Rect {
                x: 648,
                y: HUD_H + 8,
                w: 136,
                h: 40,
            },
            synth_sliders: Rect {
                x: 24,
                y: HUD_H + 72,
                w: 480,
                h: 160,
            },
            synth_scope: Rect {
                x: 520,
                y: HUD_H + 72,
                w: 256,
                h: 160,
            },
            kit_scope: Rect {
                x: 24,
                y: HUD_H + 36,
                w: 752,
                h: 96,
            },
            kit_grid: Rect {
                x: 24,
                y: HUD_H + 140,
                w: 752,
                h: 200,
            },
            kit_macros: Rect {
                x: 24,
                y: HUD_H + 348,
                w: 752,
                h: 44,
            },
            kit_divisions: Rect {
                x: 24,
                y: HUD_H + 400,
                w: 320,
                h: 36,
            },
            kit_all: Rect {
                x: 360,
                y: HUD_H + 396,
                w: 416,
                h: 44,
            },
            synth_keys: Rect {
                x: 24,
                y: HUD_H + 240,
                w: 752,
                h: content_h - 248,
            },
            synth_wave_a: Rect {
                x: 16,
                y: HUD_H + 12,
                w: 180,
                h: 48,
            },
            synth_wave_b: Rect {
                x: 204,
                y: HUD_H + 12,
                w: 180,
                h: 48,
            },
            synth_swap: Rect {
                x: 392,
                y: HUD_H + 12,
                w: 72,
                h: 48,
            },
            synth_vib: Rect {
                x: 472,
                y: HUD_H + 12,
                w: 100,
                h: 48,
            },
            synth_save_as: Rect {
                x: 580,
                y: HUD_H + 12,
                w: 92,
                h: 48,
            },
            synth_oct_down: Rect {
                x: 680,
                y: HUD_H + 12,
                w: 52,
                h: 48,
            },
            synth_oct_up: Rect {
                x: 736,
                y: HUD_H + 12,
                w: 52,
                h: 48,
            },
            synth_pick_grid: Rect {
                x: 16,
                y: HUD_H + 56,
                w: 768,
                h: content_h - 120,
            },
            synth_pick_prev: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            synth_pick_next: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            synth_pick_save_as: Rect {
                x: 16,
                y: HUD_H + content_h - 56,
                w: 200,
                h: 48,
            },
            synth_pick_done: Rect {
                x: 228,
                y: HUD_H + content_h - 56,
                w: 556,
                h: 48,
            },
            map_thru_on: Rect {
                x: 24,
                y: HUD_H + 120,
                w: 240,
                h: 72,
            },
            map_thru_off: Rect {
                x: 280,
                y: HUD_H + 120,
                w: 240,
                h: 72,
            },
            map_refresh: Rect {
                x: 536,
                y: HUD_H + 120,
                w: 240,
                h: 72,
            },
            // Sequencer only — drums live on the dedicated DRUM KIT page.
            seq_rec: Rect {
                x: 12,
                y: HUD_H + 56,
                w: 384,
                h: 78,
            },
            seq_play: Rect {
                x: 404,
                y: HUD_H + 56,
                w: 384,
                h: 78,
            },
            seq_keep: Rect {
                x: 12,
                y: HUD_H + 142,
                w: 252,
                h: 52,
            },
            seq_drop: Rect {
                x: 274,
                y: HUD_H + 142,
                w: 252,
                h: 52,
            },
            seq_undo: Rect {
                x: 536,
                y: HUD_H + 142,
                w: 252,
                h: 52,
            },
            seq_len_double: Rect {
                x: 12,
                y: HUD_H + 202,
                w: 168,
                h: 44,
            },
            seq_len_halve: Rect {
                x: 188,
                y: HUD_H + 202,
                w: 168,
                h: 44,
            },
            seq_extend: Rect {
                x: 364,
                y: HUD_H + 202,
                w: 424,
                h: 44,
            },
            seq_stop: Rect {
                x: 12,
                y: HUD_H + 254,
                w: 120,
                h: 44,
            },
            seq_clear: Rect {
                x: 140,
                y: HUD_H + 254,
                w: 100,
                h: 44,
            },
            seq_to_pad: Rect {
                x: 248,
                y: HUD_H + 254,
                w: 120,
                h: 44,
            },
            seq_all_off: Rect {
                x: 376,
                y: HUD_H + 254,
                w: 120,
                h: 44,
            },
            seq_bpm_down: Rect {
                x: 504,
                y: HUD_H + 254,
                w: 136,
                h: 44,
            },
            seq_bpm_up: Rect {
                x: 648,
                y: HUD_H + 254,
                w: 140,
                h: 44,
            },
            seq_drums: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            preset_grid: Rect {
                x: 24,
                y: HUD_H + 24,
                w: 752,
                h: 280,
            },
            preset_save: Rect {
                x: 24,
                y: HUD_H + 320,
                w: 160,
                h: 64,
            },
            preset_load: Rect {
                x: 196,
                y: HUD_H + 320,
                w: 160,
                h: 64,
            },
            preset_delete: Rect {
                x: 368,
                y: HUD_H + 320,
                w: 160,
                h: 64,
            },
            preset_factory: Rect {
                x: 540,
                y: HUD_H + 320,
                w: 236,
                h: 64,
            },
            song_list: Rect {
                x: 24,
                y: HUD_H + 24,
                w: 520,
                h: content_h - 148,
            },
            song_play: Rect {
                x: 560,
                y: HUD_H + 24,
                w: 216,
                h: 56,
            },
            song_stop: Rect {
                x: 560,
                y: HUD_H + 88,
                w: 216,
                h: 56,
            },
            song_loop: Rect {
                x: 560,
                y: HUD_H + 152,
                w: 100,
                h: 48,
            },
            song_delete: Rect {
                x: 668,
                y: HUD_H + 152,
                w: 108,
                h: 48,
            },
            song_bpm_down: Rect {
                x: 560,
                y: HUD_H + 208,
                w: 100,
                h: 48,
            },
            song_bpm_up: Rect {
                x: 668,
                y: HUD_H + 208,
                w: 108,
                h: 48,
            },
            song_prev: Rect {
                x: 560,
                y: HUD_H + 268,
                w: 100,
                h: 56,
            },
            song_next: Rect {
                x: 668,
                y: HUD_H + 268,
                w: 108,
                h: 56,
            },
            song_save_seq: Rect {
                x: 560,
                y: HUD_H + 332,
                w: 216,
                h: 44,
            },
            song_out: Rect {
                x: 560,
                y: HUD_H + 384,
                w: 216,
                h: 44,
            },
            settings_panic: Rect {
                x: 24,
                y: HUD_H + 24,
                w: 200,
                h: 72,
            },
            settings_all_off: Rect {
                x: 236,
                y: HUD_H + 24,
                w: 200,
                h: 72,
            },
            settings_fx_target: Rect {
                x: 448,
                y: HUD_H + 24,
                w: 328,
                h: 72,
            },
            settings_fx: Rect {
                x: 24,
                y: HUD_H + 112,
                w: 752,
                h: 200,
            },
            settings_wifi: Rect {
                x: 280,
                y: HUD_H + 328,
                w: 240,
                h: 56,
            },
            settings_font: Rect {
                x: 536,
                y: HUD_H + 328,
                w: 240,
                h: 56,
            },
            settings_update: Rect {
                x: 536,
                y: HUD_H + 392,
                w: 240,
                h: 56,
            },
            settings_log: Rect {
                x: 24,
                y: HUD_H + 328,
                w: 240,
                h: 56,
            },
            settings_map: Rect {
                x: 24,
                y: HUD_H + 392,
                w: 240,
                h: 56,
            },
            log_clear: Rect {
                x: 24,
                y: HUD_H + content_h - 56,
                w: 200,
                h: 48,
            },
            log_all_off: Rect {
                x: 240,
                y: HUD_H + content_h - 56,
                w: 200,
                h: 48,
            },
            chords_toolbar: Rect {
                x: 8,
                y: HUD_H + 6,
                w: SCREEN_W - 16,
                h: 44,
            },
            chords_grid: Rect {
                x: 8,
                y: HUD_H + 54,
                w: 560,
                h: 248,
            },
            chords_strum: Rect {
                x: 576,
                y: HUD_H + 54,
                w: 216,
                h: 248,
            },
            chords_palette: Rect {
                x: 8,
                y: HUD_H + 310,
                w: SCREEN_W - 16,
                h: 110,
            },
            power_blank_cycle,
            power_shutdown,
            power_reboot,
            power_screen_off,
        }
    }

    pub fn hit_power_menu(&self, px: i32, py: i32) -> Hit {
        if self.power_blank_cycle.contains(px, py) {
            return Hit::PowerBlankCycle;
        }
        if self.power_shutdown.contains(px, py) {
            return Hit::PowerShutdown;
        }
        if self.power_reboot.contains(px, py) {
            return Hit::PowerReboot;
        }
        if self.power_screen_off.contains(px, py) {
            return Hit::PowerScreenOff;
        }
        Hit::None
    }

    /// Performance layout when FULL PAD hides the drum chrome.
    pub fn apply_kaoss_full(&mut self, full: bool) {
        let content_h = SCREEN_H - HUD_H - NAV_H;
        if full {
            // Fill the content area; FULL button hidden (exit via bottom-edge hold).
            self.kaoss = Rect {
                x: 8,
                y: HUD_H + 8,
                w: SCREEN_W - 16,
                h: content_h - 16,
            };
            self.drums = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.divisions = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_prog = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_scale = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_key = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_oct_down = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_oct = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_oct_up = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_hold = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_gate = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_bpm_down = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_bpm_up = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_bpm_down5 = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_bpm_up5 = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_show_all = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_viz = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_wipe_fx = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_channel = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_out = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_full = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_settings_btn = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
        } else {
            *self = Self::new();
        }
    }

    pub fn nav_cell(&self, index: usize) -> Rect {
        // Jam tabs on the right of the top chrome.
        self.nav_jam(index)
    }

    pub fn nav_back(&self) -> Rect {
        Rect {
            x: 96,
            y: 6,
            w: 44,
            h: self.nav.h - 12,
        }
    }

    pub fn nav_home(&self) -> Rect {
        Rect {
            x: 146,
            y: 6,
            w: 72,
            h: self.nav.h - 12,
        }
    }

    pub fn nav_power(&self) -> Rect {
        Rect {
            x: 224,
            y: 6,
            w: 72,
            h: self.nav.h - 12,
        }
    }

    pub fn nav_jam(&self, index: usize) -> Rect {
        let n = JAM_MODES.len() as i32;
        let w = if n > 5 { 62 } else { 70 };
        let gap = 4;
        let total = n * w + (n - 1) * gap;
        let x0 = self.nav.w - total - 8;
        Rect {
            x: x0 + (index as i32) * (w + gap),
            y: self.nav.y + 6,
            w,
            h: self.nav.h - 12,
        }
    }

    pub fn home_tile(&self, index: usize) -> Rect {
        let cols = 5i32;
        let rows = 2i32;
        let gw = (self.content.w - 24) / cols;
        let gh = (self.content.h - 40) / rows;
        let col = (index as i32) % cols;
        let row = (index as i32) / cols;
        Rect {
            x: self.content.x + 12 + col * gw + 4,
            y: self.content.y + 36 + row * gh + 4,
            w: gw - 8,
            h: gh - 8,
        }
    }

    /// Scrollable KAOSS settings panel (Tk ⚙ overlay).
    pub fn kaoss_settings_content_h(&self) -> i32 {
        620
    }

    pub fn kaoss_settings_max_scroll(&self) -> i32 {
        (self.kaoss_settings_content_h() - self.content.h).max(0)
    }

    pub fn kaoss_settings_row(&self, y_off: i32, scroll: i32, h: i32) -> Rect {
        Rect {
            x: self.content.x + 8,
            y: self.content.y + y_off - scroll,
            w: self.content.w - 16,
            h,
        }
    }

    pub fn kaoss_settings_half_row(&self, y_off: i32, scroll: i32, left: bool, h: i32) -> Rect {
        let full = self.kaoss_settings_row(y_off, scroll, h);
        let half_w = (full.w - 8) / 2;
        if left {
            Rect { w: half_w, ..full }
        } else {
            Rect {
                x: full.x + half_w + 8,
                w: half_w,
                y: full.y,
                h: full.h,
            }
        }
    }

    pub fn kaoss_settings_third_row(&self, y_off: i32, scroll: i32, index: usize, h: i32) -> Rect {
        let full = self.kaoss_settings_row(y_off, scroll, h);
        let third = (full.w - 16) / 3;
        let i = (index % 3) as i32;
        Rect {
            x: full.x + i * (third + 8),
            y: full.y,
            w: third,
            h: full.h,
        }
    }

    pub fn kaoss_settings_channel(&self, ch: usize, y_off: i32, scroll: i32) -> Rect {
        let cols = 4i32;
        let cell_w = (self.content.w - 16) / cols;
        let cell_h = 44;
        let col = (ch as i32) % cols;
        let row = (ch as i32) / cols;
        Rect {
            x: self.content.x + 8 + col * cell_w + 2,
            y: self.content.y + y_off - scroll + row * cell_h + 2,
            w: cell_w - 4,
            h: cell_h - 4,
        }
    }

    pub fn kaoss_color_pick_cell(&self, index: usize) -> Rect {
        let cols = 4i32;
        let n = crate::kaoss_viz::pad_color_count() as i32;
        let rows = ((n + cols - 1) / cols).max(1);
        let cell_w = (self.content.w - 16) / cols;
        let cell_h = ((self.content.h - 120) / rows).clamp(48, 72);
        let col = (index as i32) % cols;
        let row = (index as i32) / cols;
        Rect {
            x: self.content.x + 8 + col * cell_w + 2,
            y: self.content.y + 56 + row * cell_h + 2,
            w: cell_w - 4,
            h: cell_h - 4,
        }
    }

    pub fn kaoss_color_done(&self) -> Rect {
        Rect {
            x: self.content.x + 8,
            y: self.content.y + self.content.h - 56,
            w: self.content.w - 16,
            h: 48,
        }
    }

    pub fn hit_kaoss_color_picker(&self, px: i32, py: i32) -> Hit {
        for i in 0..crate::kaoss_viz::pad_color_count() {
            if self.kaoss_color_pick_cell(i).contains(px, py) {
                return Hit::KaossColorPick(i);
            }
        }
        if self.kaoss_color_done().contains(px, py) {
            return Hit::KaossColorDone;
        }
        Hit::KaossColorDone
    }

    pub fn hit_kaoss_settings(&self, px: i32, py: i32, scroll: i32) -> Hit {
        if self.kaoss_settings_row(52, scroll, 48).contains(px, py) {
            return Hit::KaossWipeFx;
        }
        if self.kaoss_settings_row(108, scroll, 48).contains(px, py) {
            return Hit::KaossShowAll;
        }
        if self
            .kaoss_settings_half_row(164, scroll, true, 48)
            .contains(px, py)
        {
            return Hit::KaossAxes;
        }
        if self
            .kaoss_settings_half_row(164, scroll, false, 48)
            .contains(px, py)
        {
            return Hit::KaossGridLines;
        }
        if self
            .kaoss_settings_half_row(232, scroll, true, 48)
            .contains(px, py)
        {
            return Hit::KaossVizCells;
        }
        if self
            .kaoss_settings_half_row(232, scroll, false, 48)
            .contains(px, py)
        {
            return Hit::KaossVizGlow;
        }
        if self.kaoss_settings_row(288, scroll, 48).contains(px, py) {
            return Hit::KaossColor;
        }
        let grid_row = self.kaoss_settings_row(356, scroll, 48);
        let third = (grid_row.w - 16) / 3;
        let minus = Rect {
            x: grid_row.x,
            y: grid_row.y,
            w: third,
            h: grid_row.h,
        };
        let plus = Rect {
            x: grid_row.x + third * 2 + 16,
            y: grid_row.y,
            w: third,
            h: grid_row.h,
        };
        if minus.contains(px, py) {
            return Hit::KaossGridWidthDown;
        }
        if plus.contains(px, py) {
            return Hit::KaossGridWidthUp;
        }
        let out_y = 424;
        for i in 0..3 {
            let cell_w = (self.content.w - 16) / 3;
            let r = Rect {
                x: self.content.x + 8 + i * cell_w + 2,
                y: self.content.y + out_y - scroll + 2,
                w: cell_w - 4,
                h: 44,
            };
            if r.contains(px, py) {
                return Hit::KaossOutPick(i as usize);
            }
        }
        for ch in 0..16 {
            if self
                .kaoss_settings_channel(ch, 496, scroll)
                .contains(px, py)
            {
                return Hit::KaossChannelPick(ch as u8);
            }
        }
        if self.content.contains(px, py) {
            return Hit::ScrollArea(crate::scroll::ScrollKind::KaossSettings);
        }
        Hit::None
    }

    pub fn drum_cell(&self, index: usize) -> Rect {
        let col = (index % 4) as i32;
        let row_from_bottom = (index / 4) as i32;
        let row_from_top = 3 - row_from_bottom;
        let gw = self.drums.w / 4;
        let gh = self.drums.h / 4;
        Rect {
            x: self.drums.x + col * gw + 2,
            y: self.drums.y + row_from_top * gh + 2,
            w: gw - 4,
            h: gh - 4,
        }
    }

    pub fn division_cell(&self, index: usize) -> Rect {
        let n = 4i32;
        let w = self.kit_divisions.w / n;
        Rect {
            x: self.kit_divisions.x + (index as i32) * w + 2,
            y: self.kit_divisions.y + 4,
            w: w - 4,
            h: self.kit_divisions.h - 8,
        }
    }

    pub fn kaoss_cell(&self, col: usize, row_from_bottom: usize) -> Rect {
        let cw = self.kaoss.w / 12;
        let ch = self.kaoss.h / 7;
        let row_from_top = 6 - row_from_bottom as i32;
        Rect {
            x: self.kaoss.x + col as i32 * cw,
            y: self.kaoss.y + row_from_top * ch,
            w: cw,
            h: ch,
        }
    }

    pub fn phrase_cell(&self, index: usize) -> Rect {
        let col = (index % 4) as i32;
        let row = (index / 4) as i32;
        let gw = self.phrase_grid.w / 4;
        let gh = self.phrase_grid.h / 4;
        Rect {
            x: self.phrase_grid.x + col * gw + 3,
            y: self.phrase_grid.y + row * gh + 3,
            w: gw - 6,
            h: gh - 6,
        }
    }

    pub fn synth_slider(&self, index: usize) -> Rect {
        let n = 5i32;
        let w = self.synth_sliders.w / n;
        Rect {
            x: self.synth_sliders.x + (index as i32) * w + 6,
            y: self.synth_sliders.y + 28,
            w: w - 12,
            h: self.synth_sliders.h - 36,
        }
    }

    pub fn fm_recipe_cell(&self, index: usize) -> Rect {
        let n = jambox_core::FM_RECIPE_COUNT as i32;
        let gap = 6;
        let x0 = self.content.x + 12;
        let w_total = self.content.w - 24;
        let w = (w_total - (n - 1) * gap) / n;
        Rect {
            x: x0 + (index as i32) * (w + gap),
            y: self.content.y + 8,
            w,
            h: 44,
        }
    }

    pub fn fm_hint(&self) -> Rect {
        Rect {
            x: self.content.x + 16,
            y: self.content.y + 56,
            w: 520,
            h: 22,
        }
    }

    pub fn fm_oct_down(&self) -> Rect {
        Rect {
            x: self.content.x + self.content.w - 120,
            y: self.content.y + 54,
            w: 52,
            h: 26,
        }
    }

    pub fn fm_oct_up(&self) -> Rect {
        Rect {
            x: self.content.x + self.content.w - 64,
            y: self.content.y + 54,
            w: 52,
            h: 26,
        }
    }

    pub fn fm_diagram(&self) -> Rect {
        Rect {
            x: 16,
            y: self.content.y + 84,
            w: 200,
            h: 128,
        }
    }

    pub fn fm_scope(&self) -> Rect {
        Rect {
            x: 580,
            y: self.content.y + 84,
            w: 204,
            h: 128,
        }
    }

    pub fn fm_slider(&self, index: usize) -> Rect {
        let n = 4i32;
        let area_x = 228;
        let area_w = 340;
        let w = area_w / n;
        Rect {
            x: area_x + (index as i32) * w + 8,
            y: self.content.y + 108,
            w: w - 16,
            h: 104,
        }
    }

    pub fn synth_macro_cell(&self, index: usize) -> Rect {
        let n = 4i32;
        let w = self.kit_macros.w / n;
        Rect {
            x: self.kit_macros.x + (index as i32) * w + 4,
            y: self.kit_macros.y + 2,
            w: w - 8,
            h: self.kit_macros.h - 4,
        }
    }

    pub fn kit_macro_cell(&self, index: usize) -> Rect {
        self.synth_macro_cell(index)
    }

    pub fn kit_division_cell(&self, index: usize) -> Rect {
        self.division_cell(index)
    }

    /// One-octave piano keyboard inside `synth_keys`.
    /// Notes are returned relative to C4 (MIDI 60); the model applies `synth_octave`.
    pub const SYNTH_KEY_BASE: u8 = 60;
    pub const SYNTH_WHITE_COUNT: usize = 7;
    /// Octave shift range relative to C4 (C1 .. C7).
    pub const SYNTH_OCTAVE_MIN: i8 = -3;
    pub const SYNTH_OCTAVE_MAX: i8 = 3;

    pub fn synth_keyboard_white_rect(&self, index: usize) -> Rect {
        let i = index.min(Self::SYNTH_WHITE_COUNT - 1) as i32;
        let w = self.synth_keys.w / Self::SYNTH_WHITE_COUNT as i32;
        Rect {
            x: self.synth_keys.x + i * w + 1,
            y: self.synth_keys.y + 2,
            w: w - 2,
            h: self.synth_keys.h - 4,
        }
    }

    pub fn synth_keyboard_black_rect(&self, index: usize) -> Rect {
        let w = self.synth_keys.w / Self::SYNTH_WHITE_COUNT as i32;
        let bw = (w * 3) / 5;
        let bh = (self.synth_keys.h * 58) / 100;
        let (white_idx, _): (i32, u8) = match index {
            0 => (0, 61),
            1 => (1, 63),
            2 => (3, 66),
            3 => (4, 68),
            _ => (5, 70),
        };
        Rect {
            x: self.synth_keys.x + (white_idx + 1) * w - bw / 2,
            y: self.synth_keys.y + 2,
            w: bw,
            h: bh,
        }
    }

    pub fn synth_keyboard_note_at(&self, px: i32, py: i32) -> Option<u8> {
        if !self.synth_keys.contains(px, py) {
            return None;
        }
        const BLACKS: [(usize, u8); 5] = [(0, 61), (1, 63), (2, 66), (3, 68), (4, 70)];
        for (i, note) in BLACKS {
            if self.synth_keyboard_black_rect(i).contains(px, py) {
                return Some(note);
            }
        }
        let w = self.synth_keys.w / Self::SYNTH_WHITE_COUNT as i32;
        let rel = px - self.synth_keys.x;
        if rel < 0 {
            return None;
        }
        let white = (rel / w.max(1)).clamp(0, Self::SYNTH_WHITE_COUNT as i32 - 1) as usize;
        const WHITE_NOTES: [u8; 7] = [60, 62, 64, 65, 67, 69, 71];
        Some(WHITE_NOTES[white])
    }

    pub fn kit_pad_cell(&self, screen_index: usize) -> Rect {
        let col = (screen_index % 4) as i32;
        let row = (screen_index / 4) as i32;
        let gw = self.kit_grid.w / 4;
        let gh = self.kit_grid.h / 4;
        Rect {
            x: self.kit_grid.x + col * gw + 3,
            y: self.kit_grid.y + row * gh + 3,
            w: gw - 6,
            h: gh - 6,
        }
    }

    pub fn seq_drum_cell(&self, index: usize) -> Rect {
        self.kit_pad_cell(index)
    }

    /// Toolbar: OUT, HOLD, KEY, CHANGES, ARM.
    pub fn chords_tool(&self, index: usize) -> Rect {
        let n = 5i32;
        let w = self.chords_toolbar.w / n;
        Rect {
            x: self.chords_toolbar.x + (index as i32) * w + 3,
            y: self.chords_toolbar.y,
            w: w - 6,
            h: self.chords_toolbar.h,
        }
    }

    pub fn chords_button(&self, col: usize, row: usize) -> Rect {
        let gw = self.chords_grid.w / 12;
        let gh = self.chords_grid.h / 3;
        Rect {
            x: self.chords_grid.x + (col as i32) * gw + 1,
            y: self.chords_grid.y + (row as i32) * gh + 1,
            w: gw - 2,
            h: gh - 2,
        }
    }

    pub fn chords_palette_slot(&self, slot: usize) -> Rect {
        let n = 8i32;
        let w = self.chords_palette.w / n;
        Rect {
            x: self.chords_palette.x + (slot as i32) * w + 3,
            y: self.chords_palette.y + 22,
            w: w - 6,
            h: self.chords_palette.h - 26,
        }
    }

    pub fn chords_overlay_cell(&self, index: usize, count: usize) -> Rect {
        let cols = 4i32;
        let rows = ((count as i32) + cols - 1) / cols;
        let gw = (self.content.w - 24) / cols;
        let gh = ((self.content.h - 80) / rows.max(1)).min(88);
        let col = (index as i32) % cols;
        let row = (index as i32) / cols;
        Rect {
            x: self.content.x + 12 + col * gw + 4,
            y: self.content.y + 52 + row * gh + 4,
            w: gw - 8,
            h: gh - 8,
        }
    }

    pub fn chords_overlay_close(&self) -> Rect {
        Rect {
            x: self.content.x + self.content.w - 120,
            y: self.content.y + 8,
            w: 108,
            h: 40,
        }
    }

    pub fn hit(&self, mode: UiMode, px: i32, py: i32) -> Hit {
        if self.nav.contains(px, py) {
            if self.nav_back().contains(px, py) {
                return Hit::NavBack;
            }
            if self.nav_home().contains(px, py) {
                return Hit::Nav(UiMode::Home);
            }
            if self.nav_power().contains(px, py) {
                return Hit::Power;
            }
            for (i, m) in JAM_MODES.iter().enumerate() {
                if self.nav_jam(i).contains(px, py) {
                    return Hit::Nav(*m);
                }
            }
            return Hit::None;
        }
        match mode {
            UiMode::Kaoss => self.hit_kaoss(px, py),
            UiMode::Pads => self.hit_pads(px, py),
            UiMode::Home => self.hit_home(px, py),
            UiMode::Synth => self.hit_synth(px, py),
            UiMode::Fm => self.hit_fm(px, py),
            UiMode::Drums => self.hit_drums(px, py),
            UiMode::Seq => self.hit_seq(px, py),
            UiMode::Presets => self.hit_presets(px, py),
            UiMode::Songs => self.hit_songs(px, py),
            UiMode::Settings => self.hit_settings(px, py),
            UiMode::Log => self.hit_log(px, py),
            UiMode::Map => self.hit_map(px, py),
            UiMode::Chords => self.hit_chords(px, py),
        }
    }

    fn hit_kaoss(&self, px: i32, py: i32) -> Hit {
        if self.kaoss_prog.contains(px, py) {
            return Hit::KaossProg;
        }
        if self.kaoss_scale.contains(px, py) {
            return Hit::KaossScale;
        }
        if self.kaoss_key.contains(px, py) {
            return Hit::KaossKey;
        }
        if self.kaoss_oct_down.contains(px, py) {
            return Hit::KaossOctDown;
        }
        if self.kaoss_oct.contains(px, py) {
            return Hit::KaossOct;
        }
        if self.kaoss_oct_up.contains(px, py) {
            return Hit::KaossOctUp;
        }
        if self.kaoss_hold.contains(px, py) {
            return Hit::KaossHold;
        }
        if self.kaoss_gate.contains(px, py) {
            return Hit::KaossGate;
        }
        if self.kaoss_bpm_up5.contains(px, py) {
            return Hit::KaossBpmUp5;
        }
        if self.kaoss_bpm_down5.contains(px, py) {
            return Hit::KaossBpmDown5;
        }
        if self.kaoss_bpm_up.contains(px, py) {
            return Hit::KaossBpmUp;
        }
        if self.kaoss_bpm_down.contains(px, py) {
            return Hit::KaossBpmDown;
        }
        if self.kaoss_settings_btn.w > 0 && self.kaoss_settings_btn.contains(px, py) {
            return Hit::KaossSettings;
        }
        if self.kaoss_full.contains(px, py) {
            return Hit::KaossFull;
        }
        if self.kaoss.contains(px, py) {
            let (x, y) = self.kaoss.pad_xy(px, py);
            return Hit::Kaoss { x, y };
        }
        if self.drums.contains(px, py) && self.drums.w > 0 {
            for index in 0..16 {
                if self.drum_cell(index).contains(px, py) {
                    return Hit::Drum {
                        index,
                        note: 36 + index as u8,
                    };
                }
            }
            return Hit::None;
        }
        if self.divisions.contains(px, py) && self.divisions.w > 0 {
            for index in 0..4 {
                if self.division_cell(index).contains(px, py) {
                    return Hit::Division(index);
                }
            }
        }
        Hit::None
    }

    /// When a picker overlay is open, hit-test its cells first.
    pub fn hit_kaoss_picker(
        &self,
        kind: crate::kaoss_ui::KaossPicker,
        px: i32,
        py: i32,
        show_all: bool,
    ) -> Hit {
        let n = crate::kaoss_ui::picker_count(kind, show_all);
        for index in 0..n {
            if crate::kaoss_ui::picker_cell(self.kaoss, kind, index, show_all).contains(px, py) {
                return Hit::KaossPicker(index);
            }
        }
        // Tap outside the pad closes the picker.
        Hit::KaossPickerClose
    }

    fn hit_pads(&self, px: i32, py: i32) -> Hit {
        if self.pads_play.contains(px, py) {
            return Hit::PadsPlayView;
        }
        if self.pads_edit.contains(px, py) {
            return Hit::PadsEditView;
        }
        if self.pads_clear.w > 0 && self.pads_clear.contains(px, py) {
            return Hit::PadsClearArm;
        }
        if self.pads_trig.w > 0 && self.pads_trig.contains(px, py) {
            return Hit::PadsTrig;
        }
        if self.pads_mode.w > 0 && self.pads_mode.contains(px, py) {
            return Hit::PadsModeArm;
        }
        if self.pads_rec.w > 0 && self.pads_rec.contains(px, py) {
            return Hit::PadsRec;
        }
        if self.pads_vol_down.w > 0 && self.pads_vol_down.contains(px, py) {
            return Hit::PadsVolDown;
        }
        if self.pads_vol_up.w > 0 && self.pads_vol_up.contains(px, py) {
            return Hit::PadsVolUp;
        }
        if self.pads_voice.w > 0 && self.pads_voice.contains(px, py) {
            return Hit::PadsVoice;
        }
        if self.pads_channel.w > 0 && self.pads_channel.contains(px, py) {
            return Hit::PadsChannel;
        }
        if self.pads_synth.w > 0 && self.pads_synth.contains(px, py) {
            return Hit::PadsSynth;
        }
        if self.pads_out.w > 0 && self.pads_out.contains(px, py) {
            return Hit::PadsOut;
        }
        if self.stop_all.contains(px, py) {
            return Hit::StopAllClips;
        }
        for index in 0..16 {
            if self.phrase_cell(index).contains(px, py) {
                return Hit::PhrasePad(index);
            }
        }
        Hit::None
    }

    fn hit_home(&self, px: i32, py: i32) -> Hit {
        for (i, (mode, _, _)) in HOME_TILES.iter().enumerate() {
            if self.home_tile(i).contains(px, py) {
                return Hit::HomeTile(*mode);
            }
        }
        Hit::None
    }

    fn hit_synth(&self, px: i32, py: i32) -> Hit {
        if self.synth_wave_a.contains(px, py) {
            return Hit::SynthWaveA;
        }
        if self.synth_wave_b.contains(px, py) {
            return Hit::SynthWaveB;
        }
        if self.synth_swap.contains(px, py) {
            return Hit::SynthSwap;
        }
        if self.synth_save_as.contains(px, py) {
            return Hit::SynthSaveAs;
        }
        if self.synth_vib.contains(px, py) {
            return Hit::SynthVib;
        }
        if self.synth_oct_down.contains(px, py) {
            return Hit::SynthOctDown;
        }
        if self.synth_oct_up.contains(px, py) {
            return Hit::SynthOctUp;
        }
        for index in 0..5 {
            if self.synth_slider(index).contains(px, py) {
                return Hit::SynthSlider(index);
            }
        }
        if let Some(note) = self.synth_keyboard_note_at(px, py) {
            return Hit::SynthKey { note };
        }
        Hit::None
    }

    fn hit_fm(&self, px: i32, py: i32) -> Hit {
        for index in 0..jambox_core::FM_RECIPE_COUNT {
            if self.fm_recipe_cell(index).contains(px, py) {
                return Hit::FmRecipe(index);
            }
        }
        for index in 0..4 {
            if self.fm_slider(index).contains(px, py) {
                return Hit::FmSlider(index);
            }
        }
        if self.fm_oct_down().contains(px, py) {
            return Hit::SynthOctDown;
        }
        if self.fm_oct_up().contains(px, py) {
            return Hit::SynthOctUp;
        }
        if let Some(note) = self.synth_keyboard_note_at(px, py) {
            return Hit::SynthKey { note };
        }
        Hit::None
    }

    fn hit_drums(&self, px: i32, py: i32) -> Hit {
        for index in 0..16 {
            if self.kit_pad_cell(index).contains(px, py) {
                let cell = crate::phrases::PHRASE_GRID_CELLS[index];
                let note = crate::phrases::mpk_note_for_phrase_cell(cell);
                return Hit::Drum { index, note };
            }
        }
        for index in 0..4 {
            if self.kit_macro_cell(index).contains(px, py) {
                return Hit::DrumMacro(index);
            }
        }
        if self.kit_divisions.contains(px, py) {
            for index in 0..4 {
                if self.kit_division_cell(index).contains(px, py) {
                    return Hit::Division(index);
                }
            }
        }
        if self.kit_all.contains(px, py) {
            return Hit::KitAllDrums;
        }
        Hit::None
    }

    pub fn hit_synth_overlay(&self, px: i32, py: i32, vib_open: bool, morph_open: bool) -> Hit {
        if vib_open {
            return self.hit_synth_vib(px, py);
        }
        if self.synth_pick_done.contains(px, py) {
            return Hit::SynthPickDone;
        }
        if self.synth_pick_save_as.contains(px, py) {
            return Hit::SynthSaveAs;
        }
        if morph_open && self.synth_pick_grid.contains(px, py) {
            return Hit::ScrollArea(crate::scroll::ScrollKind::SynthMorphPick);
        }
        Hit::None
    }

    pub fn hit_synth_picker(&self, px: i32, py: i32, vib_open: bool, morph_open: bool) -> Hit {
        self.hit_synth_overlay(px, py, vib_open, morph_open)
    }

    /// Vibrato menu: WHEEL/ON, DEPTH −/+, RATE −/+, DONE.
    pub fn synth_vib_always(&self) -> Rect {
        Rect {
            x: self.content.x + 16,
            y: self.content.y + 16,
            w: self.content.w - 32,
            h: 72,
        }
    }

    fn synth_vib_pair_row(&self, y_off: i32) -> (Rect, Rect, Rect) {
        let full = Rect {
            x: self.content.x + 16,
            y: self.content.y + y_off,
            w: self.content.w - 32,
            h: 72,
        };
        let third = (full.w - 16) / 3;
        let left = Rect {
            x: full.x,
            y: full.y,
            w: third,
            h: full.h,
        };
        let mid = Rect {
            x: full.x + third + 8,
            y: full.y,
            w: third,
            h: full.h,
        };
        let right = Rect {
            x: full.x + 2 * (third + 8),
            y: full.y,
            w: third,
            h: full.h,
        };
        (left, mid, right)
    }

    pub fn synth_vib_depth_down(&self) -> Rect {
        self.synth_vib_pair_row(108).0
    }

    pub fn synth_vib_depth_label(&self) -> Rect {
        self.synth_vib_pair_row(108).1
    }

    pub fn synth_vib_depth_up(&self) -> Rect {
        self.synth_vib_pair_row(108).2
    }

    pub fn synth_vib_rate_down(&self) -> Rect {
        self.synth_vib_pair_row(200).0
    }

    pub fn synth_vib_rate_label(&self) -> Rect {
        self.synth_vib_pair_row(200).1
    }

    pub fn synth_vib_rate_up(&self) -> Rect {
        self.synth_vib_pair_row(200).2
    }

    pub fn hit_synth_vib(&self, px: i32, py: i32) -> Hit {
        if self.synth_vib_always().contains(px, py) {
            return Hit::SynthVibAlways;
        }
        if self.synth_vib_depth_down().contains(px, py) {
            return Hit::SynthVibDepthDown;
        }
        if self.synth_vib_depth_up().contains(px, py) {
            return Hit::SynthVibDepthUp;
        }
        if self.synth_vib_rate_down().contains(px, py) {
            return Hit::SynthVibRateDown;
        }
        if self.synth_vib_rate_up().contains(px, py) {
            return Hit::SynthVibRateUp;
        }
        if self.synth_pick_done.contains(px, py) {
            return Hit::SynthPickDone;
        }
        Hit::None
    }

    pub fn synth_voice_grid(&self, item_count: usize) -> crate::scroll::GridScroll {
        crate::scroll::GridScroll {
            cols: if item_count > 8 { 4 } else { 3 },
            cell_h: 52,
            item_count,
            viewport: self.synth_pick_grid,
        }
    }

    pub fn kaoss_picker_grid(
        &self,
        kind: crate::kaoss_ui::KaossPicker,
        item_count: usize,
        show_all: bool,
    ) -> crate::scroll::GridScroll {
        let cols = match kind {
            crate::kaoss_ui::KaossPicker::Key
            | crate::kaoss_ui::KaossPicker::Octave
            | crate::kaoss_ui::KaossPicker::Gate => 4,
            crate::kaoss_ui::KaossPicker::Program => 3,
            crate::kaoss_ui::KaossPicker::Scale => 4,
        };
        let _ = show_all;
        crate::scroll::GridScroll {
            cols,
            cell_h: 52,
            item_count,
            viewport: self.kaoss,
        }
    }

    pub fn song_list_scroll(&self, item_count: usize) -> crate::scroll::ListScroll {
        crate::scroll::ListScroll {
            row_h: self.song_row(0).h,
            item_count,
            visible_rows: 5,
        }
    }

    pub fn log_list_scroll(&self, item_count: usize) -> crate::scroll::ListScroll {
        crate::scroll::ListScroll {
            row_h: 18,
            item_count,
            visible_rows: 10,
        }
    }

    pub fn synth_pick_cell(&self, index: usize, scroll_y: i32, item_count: usize) -> Rect {
        self.synth_voice_grid(item_count).cell_rect(index, scroll_y)
    }

    pub fn preset_cell(&self, index: usize) -> Rect {
        let col = (index % 4) as i32;
        let row = (index / 4) as i32;
        let gw = self.preset_grid.w / 4;
        let gh = self.preset_grid.h / 2;
        Rect {
            x: self.preset_grid.x + col * gw + 4,
            y: self.preset_grid.y + row * gh + 4,
            w: gw - 8,
            h: gh - 8,
        }
    }

    pub fn song_row(&self, index: usize) -> Rect {
        let h = 56;
        Rect {
            x: self.song_list.x + 4,
            y: self.song_list.y + 4 + (index as i32) * h,
            w: self.song_list.w - 8,
            h: h - 4,
        }
    }

    fn hit_presets(&self, px: i32, py: i32) -> Hit {
        if self.preset_save.contains(px, py) {
            return Hit::PresetSave;
        }
        if self.preset_load.contains(px, py) {
            return Hit::PresetLoad;
        }
        if self.preset_delete.contains(px, py) {
            return Hit::PresetDelete;
        }
        if self.preset_factory.contains(px, py) {
            return Hit::PresetFactory;
        }
        for index in 0..8 {
            if self.preset_cell(index).contains(px, py) {
                return Hit::PresetSlot(index);
            }
        }
        Hit::None
    }

    fn hit_songs(&self, px: i32, py: i32) -> Hit {
        if self.song_play.contains(px, py) {
            return Hit::SongPlay;
        }
        if self.song_stop.contains(px, py) {
            return Hit::SongStop;
        }
        if self.song_loop.contains(px, py) {
            return Hit::SongLoop;
        }
        if self.song_delete.contains(px, py) {
            return Hit::SongDelete;
        }
        if self.song_bpm_up.contains(px, py) {
            return Hit::SongBpmUp;
        }
        if self.song_bpm_down.contains(px, py) {
            return Hit::SongBpmDown;
        }
        if self.song_prev.contains(px, py) {
            return Hit::SongPrev;
        }
        if self.song_next.contains(px, py) {
            return Hit::SongNext;
        }
        if self.song_save_seq.contains(px, py) {
            return Hit::SongSaveSeq;
        }
        if self.song_out.contains(px, py) {
            return Hit::SongOut;
        }
        for index in 0..5 {
            if self.song_row(index).contains(px, py) {
                return Hit::SongRow(index);
            }
        }
        Hit::None
    }

    fn hit_map(&self, px: i32, py: i32) -> Hit {
        if self.map_thru_on.contains(px, py) {
            return Hit::MapThruOn;
        }
        if self.map_thru_off.contains(px, py) {
            return Hit::MapThruOff;
        }
        if self.map_refresh.contains(px, py) {
            return Hit::MapRefresh;
        }
        Hit::None
    }

    fn hit_chords(&self, px: i32, py: i32) -> Hit {
        if self.chords_tool(0).contains(px, py) {
            return Hit::ChordsOut;
        }
        if self.chords_tool(1).contains(px, py) {
            return Hit::ChordsHold;
        }
        if self.chords_tool(2).contains(px, py) {
            return Hit::ChordsKey;
        }
        if self.chords_tool(3).contains(px, py) {
            return Hit::ChordsChanges;
        }
        if self.chords_tool(4).contains(px, py) {
            return Hit::ChordsArm;
        }
        if self.chords_strum.contains(px, py) {
            let (_x, y) = self.chords_strum.pad_xy(px, py);
            return Hit::ChordsStrum { y };
        }
        for slot in 0..8 {
            if self.chords_palette_slot(slot).contains(px, py) {
                return Hit::ChordsPalette { slot };
            }
        }
        for row in 0..3 {
            for col in 0..12 {
                if self.chords_button(col, row).contains(px, py) {
                    return Hit::ChordsButton { col, row };
                }
            }
        }
        Hit::None
    }

    pub fn settings_fx_slider(&self, index: usize) -> Rect {
        let n = 4i32;
        let w = self.settings_fx.w / n;
        Rect {
            x: self.settings_fx.x + (index as i32) * w + 8,
            y: self.settings_fx.y + 28,
            w: w - 16,
            h: self.settings_fx.h - 36,
        }
    }

    fn hit_settings(&self, px: i32, py: i32) -> Hit {
        if self.settings_panic.contains(px, py) {
            return Hit::SettingsPanic;
        }
        if self.settings_all_off.contains(px, py) {
            return Hit::SettingsAllOff;
        }
        if self.settings_fx_target.contains(px, py) {
            return Hit::SettingsFxTarget;
        }
        if self.settings_log.contains(px, py) {
            return Hit::SettingsLog;
        }
        if self.settings_map.contains(px, py) {
            return Hit::SettingsMap;
        }
        if self.settings_wifi.contains(px, py) {
            return Hit::SettingsWifi;
        }
        if self.settings_font.contains(px, py) {
            return Hit::SettingsFont;
        }
        if self.settings_update.contains(px, py) {
            return Hit::SettingsUpdate;
        }
        for index in 0..4 {
            if self.settings_fx_slider(index).contains(px, py) {
                return Hit::SettingsFx(index);
            }
        }
        Hit::None
    }

    fn hit_seq(&self, px: i32, py: i32) -> Hit {
        if self.seq_rec.contains(px, py) {
            return Hit::SeqRec;
        }
        if self.seq_play.contains(px, py) {
            return Hit::SeqPlay;
        }
        if self.seq_keep.contains(px, py) {
            return Hit::SeqKeep;
        }
        if self.seq_drop.contains(px, py) {
            return Hit::SeqDrop;
        }
        if self.seq_undo.contains(px, py) {
            return Hit::SeqUndo;
        }
        if self.seq_len_double.contains(px, py) {
            return Hit::SeqLenDouble;
        }
        if self.seq_len_halve.contains(px, py) {
            return Hit::SeqLenHalve;
        }
        if self.seq_extend.contains(px, py) {
            return Hit::SeqExtend;
        }
        if self.seq_stop.contains(px, py) {
            return Hit::SeqStop;
        }
        if self.seq_clear.contains(px, py) {
            return Hit::SeqClear;
        }
        if self.seq_to_pad.contains(px, py) {
            return Hit::SeqToPad;
        }
        if self.seq_all_off.contains(px, py) {
            return Hit::SeqAllOff;
        }
        if self.seq_bpm_up.contains(px, py) {
            return Hit::SeqBpmUp;
        }
        if self.seq_bpm_down.contains(px, py) {
            return Hit::SeqBpmDown;
        }
        Hit::None
    }

    fn hit_log(&self, px: i32, py: i32) -> Hit {
        if self.log_clear.contains(px, py) {
            return Hit::LogClear;
        }
        if self.log_all_off.contains(px, py) {
            return Hit::LogAllOff;
        }
        Hit::None
    }
}

/// Which performance surface a captured finger owns until lift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Surface {
    Kaoss,
    Drum {
        note: u8,
        repeat: bool,
    },
    Phrase {
        slot: usize,
    },
    SynthKey {
        note: u8,
    },
    SynthSlider {
        index: usize,
    },
    FmSlider {
        index: usize,
    },
    SettingsFx {
        index: usize,
    },
    ChordsButton {
        col: usize,
        row: usize,
    },
    ChordsStrum,
    ChordsPalette {
        slot: usize,
    },
    ScrollDrag {
        kind: crate::scroll::ScrollKind,
        start_py: i32,
        scroll_at_start: i32,
        dragging: bool,
    },
    UiTap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kick_is_a5_screen_cell() {
        let layout = Layout::new();
        let kick_idx = 4usize;
        let cell = layout.kit_pad_cell(kick_idx);
        let phrase_cell = crate::phrases::PHRASE_GRID_CELLS[kick_idx];
        let note = crate::phrases::mpk_note_for_phrase_cell(phrase_cell);
        assert_eq!(note, 36);
        match layout.hit(UiMode::Drums, cell.x + 4, cell.y + 4) {
            Hit::Drum { note: 36, .. } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn kaoss_bottom_is_y_zero() {
        let layout = Layout::new();
        let Hit::Kaoss { x, y } = layout.hit(
            UiMode::Kaoss,
            layout.kaoss.x + 10,
            layout.kaoss.y + layout.kaoss.h - 2,
        ) else {
            panic!("expected kaoss");
        };
        assert!(x < 0.1);
        assert!(y < 0.1);
    }

    #[test]
    fn nav_switches_modes() {
        let layout = Layout::new();
        let cell = layout.nav_jam(3); // Pads
        assert_eq!(
            layout.hit(UiMode::Kaoss, cell.x + 4, cell.y + 4),
            Hit::Nav(UiMode::Pads)
        );
        let home = layout.nav_home();
        assert_eq!(
            layout.hit(UiMode::Kaoss, home.x + 4, home.y + 4),
            Hit::Nav(UiMode::Home)
        );
    }

    #[test]
    fn phrase_pad_a1_is_top_left() {
        let layout = Layout::new();
        let cell = layout.phrase_cell(0);
        assert_eq!(
            layout.hit(UiMode::Pads, cell.x + 4, cell.y + 4),
            Hit::PhrasePad(0)
        );
    }

    #[test]
    fn fm_recipe_and_keyboard_hit() {
        let layout = Layout::new();
        let bell = layout.fm_recipe_cell(0);
        assert_eq!(
            layout.hit(UiMode::Fm, bell.x + 4, bell.y + 4),
            Hit::FmRecipe(0)
        );
        let slider = layout.fm_slider(0);
        assert_eq!(
            layout.hit(UiMode::Fm, slider.x + 4, slider.y + slider.h / 2),
            Hit::FmSlider(0)
        );
        let key = layout.synth_keyboard_white_rect(0);
        match layout.hit(UiMode::Fm, key.x + 4, key.y + 4) {
            Hit::SynthKey { .. } => {}
            other => panic!("{other:?}"),
        }
        let home_fm = layout.home_tile(1);
        assert_eq!(
            layout.hit(UiMode::Home, home_fm.x + 4, home_fm.y + 4),
            Hit::HomeTile(UiMode::Fm)
        );
    }
}
