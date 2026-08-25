//! 800×480 hit-testing. Coordinates are pixels unless noted as 0..1 pad space.

use crate::mode::UiMode;

pub const SCREEN_W: i32 = 800;
pub const SCREEN_H: i32 = 480;
pub const NAV_H: i32 = 48;
pub const HUD_H: i32 = 36;

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
    SynthSlider(usize),
    SynthKey(usize),
    SynthWaveA,
    SynthWaveB,
    SynthSwap,
    SynthVibUp,
    SynthVibDown,
    SynthPickDone,
    SynthPickPrev,
    SynthPickNext,
    SynthPickWave(usize),
    SynthSaveAs,
    DrumMacro(usize),
    MapThruOn,
    MapThruOff,
    MapRefresh,
    KaossProg,
    KaossScale,
    KaossKey,
    KaossOct,
    KaossHold,
    KaossGate,
    KaossFull,
    KaossBpmUp,
    KaossBpmDown,
    KaossShowAll,
    KaossChannel,
    KaossSettings,
    KaossWipeFx,
    KaossViz,
    KaossOut,
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
    LogClear,
    LogAllOff,
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
    pub synth_macros: Rect,
    pub synth_wave_a: Rect,
    pub synth_wave_b: Rect,
    pub synth_swap: Rect,
    pub synth_vib_down: Rect,
    pub synth_vib_up: Rect,
    pub synth_pick_done: Rect,
    pub synth_pick_prev: Rect,
    pub synth_pick_next: Rect,
    pub synth_pick_grid: Rect,
    pub synth_save_as: Rect,
    pub synth_pick_save_as: Rect,
    pub map_thru_on: Rect,
    pub map_thru_off: Rect,
    pub map_refresh: Rect,
    pub kaoss_prog: Rect,
    pub kaoss_scale: Rect,
    pub kaoss_key: Rect,
    pub kaoss_oct: Rect,
    pub kaoss_hold: Rect,
    pub kaoss_gate: Rect,
    pub kaoss_full: Rect,
    pub kaoss_bpm_up: Rect,
    pub kaoss_bpm_down: Rect,
    pub kaoss_show_all: Rect,
    pub kaoss_channel: Rect,
    pub kaoss_wipe_fx: Rect,
    pub kaoss_viz: Rect,
    pub kaoss_out: Rect,
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
    pub settings_update: Rect,
    pub log_clear: Rect,
    pub log_all_off: Rect,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

impl Layout {
    pub fn new() -> Self {
        let content_h = SCREEN_H - HUD_H - NAV_H;
        Self {
            hud: Rect {
                x: 0,
                y: 0,
                w: SCREEN_W,
                h: HUD_H,
            },
            content: Rect {
                x: 0,
                y: HUD_H,
                w: SCREEN_W,
                h: content_h,
            },
            nav: Rect {
                x: 0,
                y: SCREEN_H - NAV_H,
                w: SCREEN_W,
                h: NAV_H,
            },
            kaoss: Rect {
                x: 8,
                y: HUD_H + 8,
                w: 520,
                h: content_h - 112,
            },
            drums: Rect {
                x: 540,
                y: HUD_H + 8,
                w: 252,
                h: content_h - 172,
            },
            divisions: Rect {
                x: 540,
                y: HUD_H + content_h - 156,
                w: 252,
                h: 44,
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
            kaoss_oct: Rect {
                x: 488,
                y: HUD_H + content_h - 100,
                w: 148,
                h: 44,
            },
            kaoss_hold: Rect {
                x: 644,
                y: HUD_H + content_h - 100,
                w: 148,
                h: 44,
            },
            kaoss_gate: Rect {
                x: 8,
                y: HUD_H + content_h - 48,
                w: 120,
                h: 40,
            },
            kaoss_bpm_down: Rect {
                x: 136,
                y: HUD_H + content_h - 48,
                w: 72,
                h: 40,
            },
            kaoss_bpm_up: Rect {
                x: 216,
                y: HUD_H + content_h - 48,
                w: 72,
                h: 40,
            },
            kaoss_show_all: Rect {
                x: 296,
                y: HUD_H + content_h - 48,
                w: 72,
                h: 40,
            },
            kaoss_viz: Rect {
                x: 376,
                y: HUD_H + content_h - 48,
                w: 64,
                h: 40,
            },
            kaoss_wipe_fx: Rect {
                x: 448,
                y: HUD_H + content_h - 48,
                w: 72,
                h: 40,
            },
            kaoss_channel: Rect {
                x: 528,
                y: HUD_H + content_h - 48,
                w: 48,
                h: 40,
            },
            kaoss_out: Rect {
                x: 584,
                y: HUD_H + content_h - 48,
                w: 100,
                h: 40,
            },
            kaoss_full: Rect {
                x: 692,
                y: HUD_H + content_h - 48,
                w: 100,
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
            pads_clear: Rect {
                x: 16,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
            },
            pads_trig: Rect {
                x: 112,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
            },
            pads_mode: Rect {
                x: 232,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
            },
            pads_rec: Rect {
                x: 340,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
            },
            pads_vol_down: Rect {
                x: 448,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
            },
            pads_vol_up: Rect {
                x: 536,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
            },
            pads_voice: Rect {
                x: 0,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
            },
            pads_channel: Rect {
                x: 0,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
            },
            pads_synth: Rect {
                x: 0,
                y: HUD_H + content_h - 56,
                w: 0,
                h: 48,
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
            synth_macros: Rect {
                x: 24,
                y: HUD_H + 240,
                w: 752,
                h: 40,
            },
            synth_keys: Rect {
                x: 24,
                y: HUD_H + 288,
                w: 752,
                h: content_h - 296,
            },
            synth_wave_a: Rect {
                x: 16,
                y: HUD_H + 12,
                w: 200,
                h: 48,
            },
            synth_wave_b: Rect {
                x: 224,
                y: HUD_H + 12,
                w: 200,
                h: 48,
            },
            synth_swap: Rect {
                x: 432,
                y: HUD_H + 12,
                w: 88,
                h: 48,
            },
            synth_save_as: Rect {
                x: 528,
                y: HUD_H + 12,
                w: 120,
                h: 48,
            },
            synth_vib_down: Rect {
                x: 656,
                y: HUD_H + 12,
                w: 64,
                h: 48,
            },
            synth_vib_up: Rect {
                x: 728,
                y: HUD_H + 12,
                w: 56,
                h: 48,
            },
            synth_pick_grid: Rect {
                x: 16,
                y: HUD_H + 16,
                w: 768,
                h: content_h - 80,
            },
            synth_pick_prev: Rect {
                x: 16,
                y: HUD_H + content_h - 56,
                w: 140,
                h: 48,
            },
            synth_pick_next: Rect {
                x: 164,
                y: HUD_H + content_h - 56,
                w: 140,
                h: 48,
            },
            synth_pick_save_as: Rect {
                x: 312,
                y: HUD_H + content_h - 56,
                w: 140,
                h: 48,
            },
            synth_pick_done: Rect {
                x: 460,
                y: HUD_H + content_h - 56,
                w: 324,
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
            // Tk-like transport chrome, plus a compact drum strip for live capture
            // (improvement vs Tk — no need to leave SEQ to hit drums).
            // Content top: status/layer (~52px), then rows, then drums.
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
                x: 12,
                y: HUD_H + 306,
                w: 776,
                h: content_h - 334,
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
                x: 24,
                y: HUD_H + 336,
                w: 368,
                h: 56,
            },
            settings_update: Rect {
                x: 408,
                y: HUD_H + 336,
                w: 368,
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
        }
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
            self.kaoss_oct = Rect {
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
        } else {
            *self = Self::new();
        }
    }

    pub fn nav_cell(&self, index: usize) -> Rect {
        let n = UiMode::ALL.len() as i32;
        let w = self.nav.w / n;
        Rect {
            x: self.nav.x + (index as i32) * w + 1,
            y: self.nav.y + 4,
            w: w - 2,
            h: self.nav.h - 8,
        }
    }

    pub fn home_tile(&self, index: usize) -> Rect {
        let n = UiMode::ALL.len() as i32;
        let cols = 5i32;
        let rows = (n + cols - 1) / cols;
        let gw = (self.content.w - 24) / cols;
        let gh = (self.content.h - 24) / rows.max(1);
        let col = (index as i32) % cols;
        let row = (index as i32) / cols;
        Rect {
            x: self.content.x + 12 + col * gw + 4,
            y: self.content.y + 12 + row * gh + 4,
            w: gw - 8,
            h: gh - 8,
        }
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
        let w = self.divisions.w / n;
        Rect {
            x: self.divisions.x + (index as i32) * w + 2,
            y: self.divisions.y + 4,
            w: w - 4,
            h: self.divisions.h - 8,
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

    pub fn synth_macro_cell(&self, index: usize) -> Rect {
        let n = 4i32;
        let w = self.synth_macros.w / n;
        Rect {
            x: self.synth_macros.x + (index as i32) * w + 4,
            y: self.synth_macros.y + 2,
            w: w - 8,
            h: self.synth_macros.h - 4,
        }
    }

    pub fn synth_key(&self, index: usize) -> Rect {
        let n = 12i32;
        let w = self.synth_keys.w / n;
        Rect {
            x: self.synth_keys.x + (index as i32) * w + 2,
            y: self.synth_keys.y + 4,
            w: w - 4,
            h: self.synth_keys.h - 8,
        }
    }

    pub fn seq_drum_cell(&self, index: usize) -> Rect {
        // Eight fat pads (kick→ohh) — easier on 800×480 than a cramped 4×4.
        let col = (index % 4) as i32;
        let row = (index / 4) as i32;
        let gw = self.seq_drums.w / 4;
        let gh = self.seq_drums.h / 2;
        Rect {
            x: self.seq_drums.x + col * gw + 3,
            y: self.seq_drums.y + row * gh + 3,
            w: gw - 6,
            h: gh - 6,
        }
    }

    pub fn hit(&self, mode: UiMode, px: i32, py: i32) -> Hit {
        if self.nav.contains(px, py) {
            for (i, m) in UiMode::ALL.iter().enumerate() {
                if self.nav_cell(i).contains(px, py) {
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
            UiMode::Seq => self.hit_seq(px, py),
            UiMode::Presets => self.hit_presets(px, py),
            UiMode::Songs => self.hit_songs(px, py),
            UiMode::Settings => self.hit_settings(px, py),
            UiMode::Log => self.hit_log(px, py),
            UiMode::Map => self.hit_map(px, py),
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
        if self.kaoss_oct.contains(px, py) {
            return Hit::KaossOct;
        }
        if self.kaoss_hold.contains(px, py) {
            return Hit::KaossHold;
        }
        if self.kaoss_gate.contains(px, py) {
            return Hit::KaossGate;
        }
        if self.kaoss_bpm_up.contains(px, py) {
            return Hit::KaossBpmUp;
        }
        if self.kaoss_bpm_down.contains(px, py) {
            return Hit::KaossBpmDown;
        }
        if self.kaoss_show_all.w > 0 && self.kaoss_show_all.contains(px, py) {
            return Hit::KaossShowAll;
        }
        if self.kaoss_viz.w > 0 && self.kaoss_viz.contains(px, py) {
            return Hit::KaossViz;
        }
        if self.kaoss_wipe_fx.w > 0 && self.kaoss_wipe_fx.contains(px, py) {
            return Hit::KaossWipeFx;
        }
        if self.kaoss_channel.w > 0 && self.kaoss_channel.contains(px, py) {
            return Hit::KaossChannel;
        }
        if self.kaoss_out.w > 0 && self.kaoss_out.contains(px, py) {
            return Hit::KaossOut;
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
        for (i, mode) in UiMode::ALL.iter().enumerate() {
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
        if self.synth_vib_down.contains(px, py) {
            return Hit::SynthVibDown;
        }
        if self.synth_vib_up.contains(px, py) {
            return Hit::SynthVibUp;
        }
        for index in 0..4 {
            if self.synth_macro_cell(index).contains(px, py) {
                return Hit::DrumMacro(index);
            }
        }
        for index in 0..5 {
            if self.synth_slider(index).contains(px, py) {
                return Hit::SynthSlider(index);
            }
        }
        for index in 0..12 {
            if self.synth_key(index).contains(px, py) {
                return Hit::SynthKey(index);
            }
        }
        Hit::None
    }

    pub fn hit_synth_picker(&self, px: i32, py: i32, page: usize, page_len: usize) -> Hit {
        if self.synth_pick_done.contains(px, py) {
            return Hit::SynthPickDone;
        }
        if self.synth_pick_save_as.contains(px, py) {
            return Hit::SynthSaveAs;
        }
        if self.synth_pick_prev.contains(px, py) {
            return Hit::SynthPickPrev;
        }
        if self.synth_pick_next.contains(px, py) {
            return Hit::SynthPickNext;
        }
        for local in 0..page_len.min(12) {
            if self.synth_pick_cell(local).contains(px, py) {
                return Hit::SynthPickWave(page * 12 + local);
            }
        }
        Hit::None
    }

    pub fn synth_pick_cell(&self, index: usize) -> Rect {
        let col = (index % 4) as i32;
        let row = (index / 4) as i32;
        let gw = self.synth_pick_grid.w / 4;
        let gh = self.synth_pick_grid.h / 3;
        Rect {
            x: self.synth_pick_grid.x + col * gw + 4,
            y: self.synth_pick_grid.y + row * gh + 4,
            w: gw - 8,
            h: gh - 8,
        }
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

    pub fn settings_fx_slider(&self, index: usize) -> Rect {
        let n = 3i32;
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
        if self.settings_wifi.contains(px, py) {
            return Hit::SettingsWifi;
        }
        if self.settings_update.contains(px, py) {
            return Hit::SettingsUpdate;
        }
        for index in 0..3 {
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
        for index in 0..8 {
            if self.seq_drum_cell(index).contains(px, py) {
                return Hit::Drum {
                    index,
                    note: 36 + index as u8,
                };
            }
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
    Drum { note: u8, repeat: bool },
    Phrase { slot: usize },
    SynthKey { note: u8 },
    SynthSlider { index: usize },
    SettingsFx { index: usize },
    UiTap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kick_is_bottom_left() {
        let layout = Layout::new();
        let cell = layout.drum_cell(0);
        match layout.hit(UiMode::Kaoss, cell.x + 4, cell.y + 4) {
            Hit::Drum { note: 36, .. } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn kaoss_bottom_is_y_zero() {
        let layout = Layout::new();
        let Hit::Kaoss { x, y } =
            layout.hit(UiMode::Kaoss, layout.kaoss.x + 10, layout.kaoss.y + layout.kaoss.h - 2)
        else {
            panic!("expected kaoss");
        };
        assert!(x < 0.1);
        assert!(y < 0.1);
    }

    #[test]
    fn nav_switches_modes() {
        let layout = Layout::new();
        let cell = layout.nav_cell(UiMode::Pads.index());
        assert_eq!(
            layout.hit(UiMode::Kaoss, cell.x + 4, cell.y + 4),
            Hit::Nav(UiMode::Pads)
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
}
