//! Batched draw list for the 800×480 surface.
//!
//! GLES consumes this as two meshes (color quads + glyph quads). The CPU
//! rasterizer uses the same list so dummy/PPM output matches the GPU path.

use crate::font::{self, GLYPH_H, GLYPH_STRIDE, GLYPH_W};
use crate::kaoss_ui;
use crate::layout::{Rect, HUD_H};
use crate::mode::UiMode;
use crate::model::{NativeModel, RepeatDivisionChoice, LED_COLS, LED_ROWS};
use crate::phrases;
use crate::render::{SCREEN_H, SCREEN_W};
use crate::waves;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub color: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub clear: u32,
    pub color: Vec<ColorQuad>,
    pub glyphs: Vec<GlyphQuad>,
}

impl Scene {
    pub fn fill_rect(&mut self, rect: Rect, color: u32) {
        self.fill(rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32, color);
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.color.push(ColorQuad { x, y, w, h, color });
    }

    pub fn text(&mut self, mut x: i32, y: i32, s: &str, color: u32) {
        for ch in s.chars() {
            let (u0, v0, u1, v1) = font::glyph_uv(ch);
            self.glyphs.push(GlyphQuad {
                x: x as f32,
                y: y as f32,
                w: GLYPH_W as f32,
                h: GLYPH_H as f32,
                u0,
                v0,
                u1,
                v1,
                color,
            });
            x += GLYPH_STRIDE;
        }
    }
}

pub fn build(model: &NativeModel) -> Scene {
    let mut scene = Scene {
        clear: 0x101018,
        color: Vec::with_capacity(200),
        glyphs: Vec::with_capacity(240),
    };
    draw_chrome(&mut scene, model);
    match model.mode {
        UiMode::Kaoss => draw_kaoss(&mut scene, model),
        UiMode::Pads => draw_pads(&mut scene, model),
        UiMode::Home => draw_home(&mut scene, model),
        UiMode::Synth => draw_synth(&mut scene, model),
        UiMode::Seq => draw_seq(&mut scene, model),
        UiMode::Presets => draw_presets(&mut scene, model),
        UiMode::Songs => draw_songs(&mut scene, model),
        UiMode::Settings => draw_settings(&mut scene, model),
        UiMode::Log => draw_log(&mut scene, model),
        other => draw_placeholder(&mut scene, model, other),
    }
    let _ = (SCREEN_W, SCREEN_H);
    scene
}

fn draw_chrome(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.hud, 0x1c1c28);
    let hud = format!(
        "{}  fps {:>4.0}  cb {}/{}us  xrun {}  {}",
        model.mode.title(),
        model.fps,
        model.status.callback_frames,
        model.status.callback_micros,
        model.status.xruns,
        if model.connected { "ENG" } else { "NO ENGINE" },
    );
    scene.text(8, 12, &hud, 0xf2f2f2);
    if !model.status_line.is_empty() {
        scene.text(420, 12, &model.status_line, 0xa0a0b8);
    }

    scene.fill_rect(layout.nav, 0x14141c);
    for (i, mode) in UiMode::ALL.iter().enumerate() {
        let cell = layout.nav_cell(i);
        let active = *mode == model.mode;
        scene.fill_rect(cell, if active { 0x5a3060 } else { 0x242430 });
        scene.text(
            cell.x + 6,
            cell.y + cell.h / 2 - 3,
            mode.label(),
            if active { 0xffffff } else { 0xb0b0c0 },
        );
    }
}

fn draw_kaoss(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;

    if let Some(kind) = model.kaoss_picker {
        scene.fill_rect(layout.kaoss, 0x101018);
        let n = kaoss_ui::picker_count(kind);
        let selected = match kind {
            kaoss_ui::KaossPicker::Program => model.kaoss_program,
            kaoss_ui::KaossPicker::Scale => model.kaoss_scale_index as usize,
            kaoss_ui::KaossPicker::Key => model.kaoss_key as usize,
            kaoss_ui::KaossPicker::Octave => (model.kaoss_octaves as usize).saturating_sub(1),
            kaoss_ui::KaossPicker::Gate => model.kaoss_gate,
        };
        for index in 0..n {
            let cell = kaoss_ui::picker_cell(layout.kaoss, kind, index);
            let on = index == selected;
            scene.fill_rect(cell, if on { 0x458588 } else { 0x2a2a38 });
            let label = kaoss_ui::picker_label(kind, index);
            scene.text(cell.x + 8, cell.y + cell.h / 2 - 3, &label, 0xffffff);
        }
    } else {
        for row in 0..LED_ROWS {
            for col in 0..LED_COLS {
                let cell = layout.kaoss_cell(col, row);
                scene.fill(
                    cell.x as f32,
                    cell.y as f32,
                    (cell.w - 1) as f32,
                    (cell.h - 1) as f32,
                    model.cell(col, row),
                );
            }
        }
    }

    if layout.drums.w > 0 && model.kaoss_picker.is_none() {
        const DRUM_LABELS: [&str; 16] = [
            "KICK", "SNARE", "CLAP", "CHH", "OHH", "TOM L", "TOM M", "RIM", "KIK2", "RIM2", "SHKR",
            "PED", "TOM H", "COW", "CLV", "RIDE",
        ];
        for index in 0..16 {
            let cell = layout.drum_cell(index);
            scene.fill_rect(cell, if index == 0 { 0x3a2030 } else { 0x242436 });
            scene.text(
                cell.x + 6,
                cell.y + cell.h / 2 - 3,
                DRUM_LABELS[index],
                0xd0d0e0,
            );
        }

        for index in 0..4 {
            let cell = layout.division_cell(index);
            let choice = RepeatDivisionChoice::from_index(index);
            scene.fill_rect(
                cell,
                if choice == model.division {
                    0x5a3060
                } else {
                    0x20202c
                },
            );
            scene.text(cell.x + 8, cell.y + 16, choice.label(), 0xffffff);
        }
    }

    let prog = kaoss_ui::program(model.kaoss_program);
    let scale = jambox_core::kaoss_scale(model.kaoss_scale_index as usize);
    let key = jambox_core::NOTE_NAMES[model.kaoss_key as usize];
    let oct = format!("{} OCT", model.kaoss_octaves);
    let gate = kaoss_ui::GATE_LABELS[model.kaoss_gate % kaoss_ui::GATE_LABELS.len()];

    if layout.kaoss_prog.w > 0 {
        scene.fill_rect(layout.kaoss_prog, 0xb16286);
        scene.text(layout.kaoss_prog.x + 36, layout.kaoss_prog.y + 14, prog.label, 0xffffff);
        scene.fill_rect(layout.kaoss_scale, 0x458588);
        scene.text(layout.kaoss_scale.x + 12, layout.kaoss_scale.y + 14, scale.label, 0xffffff);
        scene.fill_rect(layout.kaoss_key, 0x3c3836);
        scene.text(
            layout.kaoss_key.x + 40,
            layout.kaoss_key.y + 14,
            &format!("KEY {key}"),
            0xffffff,
        );
        scene.fill_rect(layout.kaoss_oct, 0x3c3836);
        scene.text(layout.kaoss_oct.x + 36, layout.kaoss_oct.y + 14, &oct, 0xffffff);
        scene.fill_rect(
            layout.kaoss_hold,
            if model.kaoss_hold { 0xd79921 } else { 0x3c3836 },
        );
        scene.text(layout.kaoss_hold.x + 44, layout.kaoss_hold.y + 14, "HOLD", 0xffffff);
    }

    if layout.kaoss_gate.w > 0 {
        scene.fill_rect(layout.kaoss_gate, 0x3c3836);
        scene.text(layout.kaoss_gate.x + 24, layout.kaoss_gate.y + 12, gate, 0xffffff);
        scene.fill_rect(layout.kaoss_bpm_down, 0x3c3836);
        scene.text(
            layout.kaoss_bpm_down.x + 28,
            layout.kaoss_bpm_down.y + 12,
            "BPM -",
            0xffffff,
        );
        scene.fill_rect(layout.kaoss_bpm_up, 0x3c3836);
        scene.text(
            layout.kaoss_bpm_up.x + 28,
            layout.kaoss_bpm_up.y + 12,
            "BPM +",
            0xffffff,
        );
    }

    scene.fill_rect(
        layout.kaoss_full,
        if model.kaoss_full { 0x689d6a } else { 0x458588 },
    );
    scene.text(
        layout.kaoss_full.x + 90,
        layout.kaoss_full.y + 12,
        if model.kaoss_full { "EXIT FULL" } else { "FULL PAD" },
        0xffffff,
    );
}

fn draw_pads(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.text(16, HUD_H + 16, "Phrase Pads", 0xfbf1c7);
    scene.fill_rect(
        layout.pads_play,
        if !model.pads_edit { 0x689d6a } else { 0x3c3836 },
    );
    scene.text(layout.pads_play.x + 32, layout.pads_play.y + 12, "PLAY", 0xffffff);
    scene.fill_rect(
        layout.pads_edit,
        if model.pads_edit { 0xd79921 } else { 0x3c3836 },
    );
    scene.text(layout.pads_edit.x + 36, layout.pads_edit.y + 12, "EDIT", 0xffffff);

    if model.pads_edit {
        scene.fill_rect(
            layout.pads_clear,
            if model.pads_clear_armed {
                0xcc241d
            } else {
                0x9d0006
            },
        );
        scene.text(
            layout.pads_clear.x + 48,
            layout.pads_clear.y + 16,
            if model.pads_clear_armed {
                "CLEAR?"
            } else {
                "CLEAR"
            },
            0xffffff,
        );
        let trig = if model.phrases[model.pads_selected.min(15)].loop_mode {
            "TRIG LOOP"
        } else {
            "TRIG 1SHOT"
        };
        scene.fill_rect(layout.pads_trig, 0x458588);
        scene.text(layout.pads_trig.x + 36, layout.pads_trig.y + 16, trig, 0xffffff);
    }

    scene.fill_rect(layout.stop_all, 0x3c3836);
    scene.text(layout.stop_all.x + 18, layout.stop_all.y + 16, "STOP", 0xffffff);

    for index in 0..16 {
        let cell = layout.phrase_cell(index);
        let pad = &model.phrases[index];
        let selected = model.pads_edit && index == model.pads_selected;
        let color = if model.phrase_playing[index] {
            0x689d6a
        } else if selected {
            0xd79921
        } else if pad.empty {
            0x3c3836
        } else if pad.loop_mode {
            0x458588
        } else {
            0x076678
        };
        scene.fill_rect(cell, color);
        let label = phrases::pad_label(index);
        scene.text(cell.x + 10, cell.y + 16, &label, 0xfbf1c7);
        if !pad.empty {
            scene.text(
                cell.x + 10,
                cell.y + 36,
                if pad.loop_mode { "LOOP" } else { "1SHOT" },
                0xd5c4a1,
            );
        }
    }
}

fn draw_home(scene: &mut Scene, model: &NativeModel) {
    for (i, mode) in UiMode::ALL.iter().enumerate() {
        let cell = model.layout.home_tile(i);
        scene.fill_rect(cell, 0x282838);
        scene.text(
            cell.x + 16,
            cell.y + cell.h / 2 - 4,
            mode.title(),
            0xf2f2f2,
        );
    }
}

fn draw_synth(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;

    if model.synth_pick_a.is_some() {
        let pick_a = model.synth_pick_a.unwrap_or(true);
        scene.text(
            24,
            HUD_H + 4,
            if pick_a {
                "MORPH PAIR · tap a wave for A"
            } else {
                "MORPH PAIR · tap a wave for B"
            },
            0xfbf1c7,
        );
        let start = model.synth_pick_page * 12;
        for local in 0..12 {
            let index = start + local;
            if index >= model.wave_names.len() {
                break;
            }
            let cell = layout.synth_pick_cell(local);
            let color = if index == model.morph_a as usize {
                0xb16286
            } else if index == model.morph_b as usize {
                0x458588
            } else {
                0x3c3836
            };
            scene.fill_rect(cell, color);
            let label = waves::short_label(&model.wave_names[index]);
            scene.text(cell.x + 10, cell.y + cell.h / 2 - 3, &label, 0xffffff);
        }
        scene.fill_rect(layout.synth_pick_prev, 0x504945);
        scene.text(
            layout.synth_pick_prev.x + 48,
            layout.synth_pick_prev.y + 16,
            "PREV",
            0xffffff,
        );
        scene.fill_rect(layout.synth_pick_next, 0x504945);
        scene.text(
            layout.synth_pick_next.x + 48,
            layout.synth_pick_next.y + 16,
            "NEXT",
            0xffffff,
        );
        scene.fill_rect(layout.synth_pick_done, 0x458588);
        scene.text(
            layout.synth_pick_done.x + 160,
            layout.synth_pick_done.y + 16,
            "DONE",
            0xffffff,
        );
        return;
    }

    const LABELS: [&str; 5] = ["MORPH", "TONE", "LEVEL", "ATK", "REL"];
    const KEYS: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];

    let a = waves::short_label(model.wave_label(model.morph_a));
    let b = waves::short_label(model.wave_label(model.morph_b));
    scene.fill_rect(layout.synth_wave_a, 0xb16286);
    scene.text(
        layout.synth_wave_a.x + 12,
        layout.synth_wave_a.y + 16,
        &format!("A · {a}"),
        0xffffff,
    );
    scene.fill_rect(layout.synth_wave_b, 0x458588);
    scene.text(
        layout.synth_wave_b.x + 12,
        layout.synth_wave_b.y + 16,
        &format!("B · {b}"),
        0xffffff,
    );
    scene.fill_rect(layout.synth_swap, 0x504945);
    scene.text(layout.synth_swap.x + 52, layout.synth_swap.y + 16, "SWAP", 0xffffff);

    for index in 0..5 {
        let track = layout.synth_slider(index);
        scene.fill_rect(track, 0x20202c);
        scene.text(track.x + 8, track.y - 18, LABELS[index], 0xc0c0d0);
        let fill_h = (track.h as f32 * model.synth_params[index]) as i32;
        let fill = Rect {
            x: track.x + 4,
            y: track.y + track.h - fill_h,
            w: track.w - 8,
            h: fill_h.max(2),
        };
        scene.fill_rect(fill, if index == 0 { 0xb16286 } else { 0x689d6a });
    }
    for index in 0..12 {
        let key = layout.synth_key(index);
        let black = KEYS[index].contains('#');
        scene.fill_rect(key, if black { 0x1a1a22 } else { 0x3a3a48 });
        scene.text(key.x + 10, key.y + key.h / 2 - 3, KEYS[index], 0xf2f2f2);
    }
}

fn draw_seq(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    let seq = &model.seq;

    scene.text(16, HUD_H + 10, "Sequencer", 0xfbf1c7);
    scene.text(
        160,
        HUD_H + 12,
        &format!("{:.0} BPM · drums on-screen", seq.bpm),
        0xa89984,
    );
    scene.text(16, HUD_H + 30, &seq.status, 0xfabd2f);
    scene.text(16, HUD_H + 44, &seq.layer_line, 0x83a598);

    let (rec_label, rec_bg) = seq.rec_label();
    let (play_label, play_bg) = seq.play_label();
    scene.fill_rect(layout.seq_rec, rec_bg);
    scene.text(layout.seq_rec.x + 48, layout.seq_rec.y + 30, rec_label, 0xffffff);
    scene.fill_rect(layout.seq_play, play_bg);
    scene.text(layout.seq_play.x + 140, layout.seq_play.y + 30, play_label, 0xffffff);

    let pending = seq.has_pending();
    let keep_bg = if pending { 0x458588 } else { 0x3c3836 };
    let drop_bg = if pending { 0x665c54 } else { 0x3c3836 };
    let undo_bg = if seq.layer_count() > 1 {
        0x665c54
    } else {
        0x3c3836
    };
    scene.fill_rect(layout.seq_keep, keep_bg);
    scene.text(layout.seq_keep.x + 88, layout.seq_keep.y + 18, "KEEP", 0xffffff);
    scene.fill_rect(layout.seq_drop, drop_bg);
    scene.text(layout.seq_drop.x + 88, layout.seq_drop.y + 18, "DROP", 0xffffff);
    scene.fill_rect(layout.seq_undo, undo_bg);
    scene.text(layout.seq_undo.x + 88, layout.seq_undo.y + 18, "UNDO", 0xffffff);

    scene.fill_rect(layout.seq_len_double, 0x504945);
    scene.text(
        layout.seq_len_double.x + 44,
        layout.seq_len_double.y + 14,
        "LEN x2",
        0xffffff,
    );
    scene.fill_rect(layout.seq_len_halve, 0x504945);
    scene.text(
        layout.seq_len_halve.x + 44,
        layout.seq_len_halve.y + 14,
        "LEN /2",
        0xffffff,
    );
    let extend_bg = if seq.extend_mode { 0x689d6a } else { 0x3c3836 };
    let extend_label = if seq.extend_mode {
        "OVERDUB: EXTEND"
    } else {
        "OVERDUB: WRAP"
    };
    scene.fill_rect(layout.seq_extend, extend_bg);
    scene.text(
        layout.seq_extend.x + 100,
        layout.seq_extend.y + 14,
        extend_label,
        0xffffff,
    );

    scene.fill_rect(layout.seq_stop, 0x504945);
    scene.text(layout.seq_stop.x + 52, layout.seq_stop.y + 14, "STOP ALL", 0xffffff);
    scene.fill_rect(layout.seq_clear, 0x3c3836);
    scene.text(layout.seq_clear.x + 64, layout.seq_clear.y + 14, "CLEAR", 0xffffff);
    scene.fill_rect(layout.seq_bpm_down, 0x282828);
    scene.text(layout.seq_bpm_down.x + 58, layout.seq_bpm_down.y + 14, "- BPM", 0xffffff);
    scene.fill_rect(layout.seq_bpm_up, 0x282828);
    scene.text(layout.seq_bpm_up.x + 58, layout.seq_bpm_up.y + 14, "+ BPM", 0xffffff);

    const DRUM_LABELS: [&str; 8] = [
        "KICK", "SNARE", "CLAP", "CHH", "OHH", "TOM L", "TOM M", "RIM",
    ];
    for index in 0..8 {
        let cell = layout.seq_drum_cell(index);
        let bg = if seq.is_recording() {
            0x3a2828
        } else {
            0x242436
        };
        scene.fill_rect(cell, bg);
        scene.text(cell.x + 16, cell.y + cell.h / 2 - 3, DRUM_LABELS[index], 0xd0d0e0);
    }
}

fn draw_presets(scene: &mut Scene, model: &NativeModel) {
    for index in 0..8 {
        let cell = model.layout.preset_cell(index);
        let selected = index == model.preset_selected;
        let color = if selected {
            0x5a3060
        } else if model.preset_occupied[index] {
            0x458588
        } else {
            0x3c3836
        };
        scene.fill_rect(cell, color);
        scene.text(
            cell.x + 16,
            cell.y + cell.h / 2 - 4,
            &format!("SLOT {}", index + 1),
            0xfbf1c7,
        );
    }
    scene.fill_rect(model.layout.preset_save, 0x689d6a);
    scene.text(
        model.layout.preset_save.x + 40,
        model.layout.preset_save.y + 24,
        "SAVE",
        0xffffff,
    );
}

fn draw_songs(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.song_list, 0x1c1c28);
    for row in 0..5 {
        let idx = model.song_scroll + row;
        let cell = layout.song_row(row);
        if idx >= model.song_files.len() {
            scene.fill_rect(cell, 0x14141c);
            continue;
        }
        let selected = idx == model.song_selected;
        scene.fill_rect(cell, if selected { 0x458588 } else { 0x282838 });
        let name = model.song_files[idx]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        let label: String = name.chars().take(28).collect();
        scene.text(cell.x + 12, cell.y + 20, &label, 0xfbf1c7);
    }
    scene.fill_rect(
        layout.song_play,
        if model.song_playing { 0x689d6a } else { 0x3a5040 },
    );
    scene.text(layout.song_play.x + 60, layout.song_play.y + 32, "PLAY", 0xffffff);
    scene.fill_rect(layout.song_stop, 0x3c3836);
    scene.text(layout.song_stop.x + 60, layout.song_stop.y + 32, "STOP", 0xffffff);
    scene.fill_rect(layout.song_prev, 0x282838);
    scene.text(layout.song_prev.x + 28, layout.song_prev.y + 24, "UP", 0xffffff);
    scene.fill_rect(layout.song_next, 0x282838);
    scene.text(layout.song_next.x + 16, layout.song_next.y + 24, "DOWN", 0xffffff);
}

fn draw_settings(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.settings_panic, 0x9d0006);
    scene.text(layout.settings_panic.x + 60, layout.settings_panic.y + 32, "PANIC", 0xffffff);
    scene.fill_rect(layout.settings_all_off, 0x504945);
    scene.text(
        layout.settings_all_off.x + 40,
        layout.settings_all_off.y + 32,
        "NOTES OFF",
        0xffffff,
    );
    const LABELS: [&str; 3] = ["DRIVE", "DELAY", "REVERB"];
    for index in 0..3 {
        let track = layout.settings_fx_slider(index);
        scene.fill_rect(track, 0x20202c);
        scene.text(track.x + 8, track.y - 18, LABELS[index], 0xc0c0d0);
        let fill_h = (track.h as f32 * model.fx_bus[index]) as i32;
        let fill = Rect {
            x: track.x + 4,
            y: track.y + track.h - fill_h,
            w: track.w - 8,
            h: fill_h,
        };
        scene.fill_rect(fill, 0x458588);
    }
}

fn draw_log(scene: &mut Scene, model: &NativeModel) {
    let c = model.layout.content;
    scene.text(c.x + 16, c.y + 16, "ENGINE LOG", 0xfbf1c7);
    scene.text(
        c.x + 16,
        c.y + 40,
        &format!(
            "cb {}/{}us  xrun {}  drop {}  rel {}  rpt {}",
            model.status.callback_frames,
            model.status.callback_micros,
            model.status.xruns,
            model.status.command_drops,
            model.status.emergency_releases,
            model.status.active_repeats,
        ),
        0xa0a0b8,
    );
    for (i, line) in model.log_lines.iter().rev().take(12).enumerate() {
        scene.text(c.x + 16, c.y + 70 + (i as i32) * 18, line, 0xd5c4a1);
    }
}

fn draw_placeholder(scene: &mut Scene, model: &NativeModel, mode: UiMode) {
    let c = model.layout.content;
    scene.text(
        c.x + 24,
        c.y + 48,
        &format!("{} — coming on this branch", mode.title()),
        0xc0c0d0,
    );
    scene.text(
        c.x + 24,
        c.y + 72,
        "Native kiosk port; engine owns musical time.",
        0x808098,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Outbox;
    use crate::model::NativeModel;

    #[test]
    fn cells_are_one_batched_field() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        model.finger_down(1, k.x + 40, k.y + 40, &mut out);
        model.tick(1.0 / 60.0);
        let scene = build(&model);
        let cells = scene
            .color
            .iter()
            .filter(|q| {
                q.x >= model.layout.kaoss.x as f32
                    && q.y >= model.layout.kaoss.y as f32
                    && q.x < (model.layout.kaoss.x + model.layout.kaoss.w) as f32
                    && q.y < (model.layout.kaoss.y + model.layout.kaoss.h) as f32
                    && q.w < 80.0
                    && q.h < 80.0
            })
            .count();
        assert_eq!(cells, LED_COLS * LED_ROWS);
        assert!(!scene.glyphs.is_empty());
    }

    #[test]
    fn pads_mode_draws_sixteen_tiles() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Pads);
        let scene = build(&model);
        let tiles = scene
            .color
            .iter()
            .filter(|q| {
                q.w > 100.0
                    && q.h > 50.0
                    && q.x >= model.layout.phrase_grid.x as f32
                    && q.y >= model.layout.phrase_grid.y as f32
            })
            .count();
        assert!(tiles >= 16);
    }
}
