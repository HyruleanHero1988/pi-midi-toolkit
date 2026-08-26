//! Batched draw list for the 800×480 surface.
//!
//! GLES consumes this as two meshes (color quads + glyph quads). The CPU
//! rasterizer uses the same list so dummy/PPM output matches the GPU path.

use crate::font::{self, FontStyle, GLYPH_H, GLYPH_STRIDE, GLYPH_W};
use crate::kaoss_ui;
use crate::kaoss_viz;
use crate::screensaver;
use crate::layout::{Layout, Rect, HUD_H, NAV_H};
use crate::mode::UiMode;
use crate::model::{NativeModel, RepeatDivisionChoice, LED_COLS, LED_ROWS};
use crate::phrases;
use crate::render::{SCREEN_H, SCREEN_W};
use crate::waves;
use jambox_core::drum_model_for_note;

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
    pub font_style: FontStyle,
    pub color: Vec<ColorQuad>,
    pub glyphs: Vec<GlyphQuad>,
}

/// Unpack 0xRRGGBB (or 0xAARRGGBB) into RGBA floats for GLES.
pub fn unpack_rgb(color: u32) -> [f32; 4] {
    let r = ((color >> 16) & 0xff) as f32 / 255.0;
    let g = ((color >> 8) & 0xff) as f32 / 255.0;
    let b = (color & 0xff) as f32 / 255.0;
    let a = if color > 0x00ff_ffff {
        ((color >> 24) & 0xff) as f32 / 255.0
    } else {
        1.0
    };
    [r, g, b, a]
}

impl Scene {
    pub fn fill_rect(&mut self, rect: Rect, color: u32) {
        self.fill(rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32, color);
    }

    /// Tk-style touch button: light border around a solid fill.
    pub fn button(&mut self, rect: Rect, fill: u32) {
        const BORDER: u32 = 0xa89984;
        self.fill_rect(rect, BORDER);
        if rect.w > 2 && rect.h > 2 {
            self.fill(
                (rect.x + 1) as f32,
                (rect.y + 1) as f32,
                (rect.w - 2) as f32,
                (rect.h - 2) as f32,
                fill,
            );
        }
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.color.push(ColorQuad { x, y, w, h, color });
    }

    pub fn fill_disc(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
        if radius < 1.0 {
            return;
        }
        let r = radius.ceil() as i32;
        let r2 = radius * radius;
        for dy in -r..=r {
            let y = cy + dy as f32;
            let dx_max_sq = r2 - (dy as f32 * dy as f32);
            if dx_max_sq <= 0.0 {
                continue;
            }
            let half_w = dx_max_sq.sqrt();
            self.fill(cx - half_w, y, half_w * 2.0, 1.0, color);
        }
    }

    pub fn stroke_disc(&mut self, cx: f32, cy: f32, radius: f32, width: f32, color: u32, bg: u32) {
        if radius < width + 1.0 {
            return;
        }
        self.fill_disc(cx, cy, radius, color);
        self.fill_disc(cx, cy, (radius - width).max(0.0), bg);
    }

    /// Default UI text scale depends on font style.
    pub fn text(&mut self, x: i32, y: i32, s: &str, color: u32) {
        let scale = self.font_style.resolved().default_text_scale();
        self.text_scaled(x, y, s, color, scale);
    }

    pub fn text_scaled(&mut self, mut x: i32, y: i32, s: &str, color: u32, scale: i32) {
        let style = self.font_style.resolved();
        match style {
            FontStyle::Retro => self.text_scaled_retro(&mut x, y, s, color, scale.max(1)),
            FontStyle::Smooth => self.text_scaled_smooth(&mut x, y, s, color, scale.max(1)),
        }
    }

    fn text_scaled_retro(&mut self, x: &mut i32, y: i32, s: &str, color: u32, scale: i32) {
        let gw = GLYPH_W * scale;
        let gh = GLYPH_H * scale;
        let stride = GLYPH_STRIDE * scale;
        for ch in s.chars() {
            let (u0, v0, u1, v1) = font::glyph_uv(ch);
            self.glyphs.push(GlyphQuad {
                x: *x as f32,
                y: y as f32,
                w: gw as f32,
                h: gh as f32,
                u0,
                v0,
                u1,
                v1,
                color,
            });
            *x += stride;
        }
    }

    fn text_scaled_smooth(&mut self, x: &mut i32, y: i32, s: &str, color: u32, scale: i32) {
        let Some(atlas) = font::smooth_atlas() else {
            self.font_style = FontStyle::Retro;
            self.text_scaled_retro(x, y, s, color, scale.max(2));
            return;
        };
        let scale_f = scale as f32;
        let baseline = y as f32 + atlas.line_height * 0.78 * scale_f;
        for ch in s.chars() {
            let g = atlas.glyph(ch);
            let w = g.width * scale_f;
            let h = g.height * scale_f;
            self.glyphs.push(GlyphQuad {
                x: *x as f32 + g.x_offset * scale_f,
                y: baseline + g.y_offset * scale_f,
                w: w.max(1.0),
                h: h.max(1.0),
                u0: g.u0,
                v0: g.v0,
                u1: g.u1,
                v1: g.v1,
                color,
            });
            *x += (g.advance * scale_f).round() as i32;
        }
    }

    pub fn text_centered(&mut self, rect: Rect, s: &str, color: u32, scale: i32) {
        let style = self.font_style.resolved();
        let scale = scale.max(1);
        let (w, h) = match style {
            FontStyle::Retro => (
                (s.chars().count() as i32) * GLYPH_STRIDE * scale,
                GLYPH_H * scale,
            ),
            FontStyle::Smooth => match font::smooth_atlas() {
                Some(atlas) => {
                    let width = s
                        .chars()
                        .map(|ch| (atlas.glyph(ch).advance * scale as f32).round() as i32)
                        .sum();
                    let height = (atlas.line_height * scale as f32).round() as i32;
                    (width, height)
                }
                None => (
                    (s.chars().count() as i32) * GLYPH_STRIDE * scale,
                    GLYPH_H * scale,
                ),
            },
        };
        let x = rect.x + (rect.w - w).max(0) / 2;
        let y = rect.y + (rect.h - h).max(0) / 2;
        self.text_scaled(x, y, s, color, scale);
    }
}

pub fn build(model: &NativeModel) -> Scene {
    let mut scene = Scene {
        clear: 0x111111,
        font_style: model.font_style.resolved(),
        color: Vec::with_capacity(200),
        glyphs: Vec::with_capacity(240),
    };
    draw_chrome(&mut scene, model);
    if model.screensaver_active() {
        draw_screensaver(&mut scene, model);
    } else if model.power_menu_open {
        draw_power_menu(&mut scene, model);
    } else {
        match model.mode {
            UiMode::Kaoss => draw_kaoss(&mut scene, model),
            UiMode::Pads => draw_pads(&mut scene, model),
            UiMode::Home => draw_home(&mut scene, model),
            UiMode::Synth => draw_synth(&mut scene, model),
            UiMode::Drums => draw_drums(&mut scene, model),
            UiMode::Seq => draw_seq(&mut scene, model),
            UiMode::Presets => draw_presets(&mut scene, model),
            UiMode::Songs => draw_songs(&mut scene, model),
            UiMode::Map => draw_map(&mut scene, model),
            UiMode::Settings => draw_settings(&mut scene, model),
            UiMode::Log => draw_log(&mut scene, model),
        }
        apply_content_shift(&mut scene, model.ui_shift, crate::layout::HUD_H);
    }
    let _ = (SCREEN_W, SCREEN_H);
    scene
}

fn apply_content_shift(scene: &mut Scene, shift: (i32, i32), hud_h: i32) {
    if shift == (0, 0) {
        return;
    }
    let (dx, dy) = shift;
    let hud = hud_h as f32;
    for q in &mut scene.color {
        if q.y >= hud {
            q.x += dx as f32;
            q.y += dy as f32;
        }
    }
    for g in &mut scene.glyphs {
        if g.y >= hud {
            g.x += dx as f32;
            g.y += dy as f32;
        }
    }
}

fn draw_power_menu(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.content, 0x111111);
    scene.text_scaled(layout.content.x + 12, layout.content.y + 12, "POWER", 0xfbf1c7, 3);
    let blank_label = screensaver::timeout_label_dynamic(model.screensaver.timeout_sec);
    scene.button(layout.power_blank_cycle, 0x3c3836);
    scene.text_centered(layout.power_blank_cycle, &blank_label, 0xffffff, 2);
    scene.text(
        layout.content.x + 12,
        layout.content.y + 64,
        "Shut down cleanly before unplugging.",
        0xebdbb2,
    );
    scene.text(
        layout.content.x + 12,
        layout.content.y + 88,
        "Reboot restarts into kiosk. SCREEN OFF blanks the TFT.",
        0xa89984,
    );
    scene.button(layout.power_shutdown, 0x9d0006);
    scene.text_centered(layout.power_shutdown, "SHUT DOWN", 0xfbf1c7, 3);
    scene.button(layout.power_reboot, 0xd79921);
    scene.text_centered(layout.power_reboot, "REBOOT", 0x1d2021, 3);
    scene.button(layout.power_screen_off, 0x1d2021);
    scene.text_centered(layout.power_screen_off, "SCREEN OFF", 0xfbf1c7, 2);
}

fn draw_screensaver(scene: &mut Scene, model: &NativeModel) {
    scene.fill(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32, 0x000000);
    let hint = "TAP TO WAKE";
    let gw = (crate::font::GLYPH_W * 3) as f32;
    let gh = (crate::font::GLYPH_H * 3) as f32;
    let span_x = (SCREEN_W as f32 - gw * hint.len() as f32 - 32.0).max(0.0);
    let span_y = (SCREEN_H as f32 - gh - 32.0).max(0.0);
    let t = model.screensaver_orbit();
    let x = 16.0 + ((t * 2.0 * std::f32::consts::PI / 47.0).sin() * 0.5 + 0.5) * span_x;
    let y = 16.0 + ((t * 2.0 * std::f32::consts::PI / 31.0 + 1.2).sin() * 0.5 + 0.5) * span_y;
    scene.text_scaled(x as i32, y as i32, hint, 0x665c54, 3);
}

fn draw_chrome(scene: &mut Scene, model: &NativeModel) {
    use crate::layout::JAM_MODES;

    let layout = model.layout;
    // Tk top bar: warm charcoal, cyan brand, ← BACK / HOME / POWER, jam tabs.
    scene.fill_rect(layout.nav, 0x1d2021);
    scene.text_scaled(10, 16, "PiDI", 0x00d4ff, 3);

    let back = layout.nav_back();
    scene.button(
        back,
        if model.can_nav_back() { 0x3c3836 } else { 0x1d2021 },
    );
    scene.text_centered(back, "<", 0xfbf1c7, 2);

    let home = layout.nav_home();
    scene.button(home, if model.mode == UiMode::Home { 0x458588 } else { 0x3c3836 });
    scene.text_centered(home, "HOME", 0xfbf1c7, 2);

    let power = layout.nav_power();
    scene.button(
        power,
        if model.power_menu_open { 0xb16286 } else { 0x9d0006 },
    );
    scene.text_centered(power, "POWER", 0xfbf1c7, 2);

    for (i, mode) in JAM_MODES.iter().enumerate() {
        let cell = layout.nav_jam(i);
        let active = *mode == model.mode;
        scene.button(cell, if active { 0x458588 } else { 0x3c3836 });
        scene.text_centered(cell, mode.title(), 0xfbf1c7, 2);
    }

    let status = chrome_status(model);
    if !status.is_empty() {
        scene.text_scaled(286, 18, &status, 0xfe8019, 2);
    }
}

fn chrome_status(model: &NativeModel) -> String {
    if !model.status_line.is_empty() {
        return model.status_line.clone();
    }
    match model.mode {
        UiMode::Kaoss => {
            let gate = kaoss_ui::gate(model.kaoss_gate).label;
            format!(
                "{:.0} BPM  {}  {}",
                model.bpm,
                gate,
                model.kaoss_out.label(),
            )
        }
        UiMode::Seq => model.seq.status.clone(),
        UiMode::Presets => format!("slot {} — tap LOAD", model.preset_selected + 1),
        _ => String::new(),
    }
}

fn draw_kaoss(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;

    if model.kaoss_settings_open {
        draw_kaoss_settings(scene, model);
        return;
    }

    if let Some(kind) = model.kaoss_picker {
        scene.fill_rect(layout.kaoss, 0x08040a);
        let n = kaoss_ui::picker_count(kind, model.kaoss_show_all);
        let grid = layout.kaoss_picker_grid(kind, n, model.kaoss_show_all);
        let selected = match kind {
            kaoss_ui::KaossPicker::Program => {
                if model.kaoss_show_all {
                    model.kaoss_program
                } else {
                    kaoss_ui::KAOSS_PROGRAMS
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.curated)
                        .position(|(i, _)| i == model.kaoss_program)
                        .unwrap_or(0)
                }
            }
            kaoss_ui::KaossPicker::Scale => {
                kaoss_ui::scale_picker_index(model.kaoss_show_all, model.kaoss_scale_index)
            }
            kaoss_ui::KaossPicker::Key => model.kaoss_key as usize,
            kaoss_ui::KaossPicker::Octave => (model.kaoss_octaves as usize).saturating_sub(1),
            kaoss_ui::KaossPicker::Gate => model.kaoss_gate,
        };
        scene.text(
            layout.kaoss.x + 8,
            layout.kaoss.y + 4,
            "drag to scroll",
            0xa89984,
        );
        for index in grid.visible_range(model.kaoss_picker_scroll) {
            let cell = grid.cell_rect(index, model.kaoss_picker_scroll);
            if cell.y + cell.h < grid.viewport.y
                || cell.y > grid.viewport.y + grid.viewport.h
            {
                continue;
            }
            let on = index == selected;
            scene.button(cell, if on { 0x458588 } else { 0x2a2a38 });
            let label = kaoss_ui::picker_label(kind, index, model.kaoss_show_all);
            scene.text_centered(cell, &label, 0xffffff, 2);
        }
    } else {
        scene.fill_rect(layout.kaoss, 0x08040a);
        if model.kaoss_viz_glow {
            draw_kaoss_glow(scene, layout.kaoss, model);
        } else {
            for row in 0..LED_ROWS {
                for col in 0..LED_COLS {
                    let cell = layout.kaoss_cell(col, row);
                    let color = model.cell(col, row);
                    if color != 0 {
                        scene.fill(
                            cell.x as f32,
                            cell.y as f32,
                            (cell.w - 1) as f32,
                            (cell.h - 1) as f32,
                            color,
                        );
                    }
                }
            }
        }
        draw_kaoss_axes(scene, layout.kaoss, model);
        draw_kaoss_ripples(scene, layout.kaoss, model);
        if let Some((fx, fy)) = model.kaoss_finger() {
            draw_kaoss_cursor(scene, layout.kaoss, fx, fy, model);
        }
    }

    let prog = kaoss_ui::program(model.kaoss_program);
    let scale = jambox_core::kaoss_scale(model.kaoss_scale_index as usize);
    let key = jambox_core::NOTE_NAMES[model.kaoss_key as usize];
    let oct = format!("{} OCT", model.kaoss_octaves);
    let gate = kaoss_ui::gate(model.kaoss_gate).label;

    if layout.kaoss_prog.w > 0 {
        scene.button(layout.kaoss_prog, 0xb16286);
        scene.text_centered(layout.kaoss_prog, prog.label, 0xffffff, 2);
        scene.button(layout.kaoss_scale, 0x458588);
        scene.text_centered(layout.kaoss_scale, scale.label, 0xffffff, 2);
        scene.button(layout.kaoss_key, 0x3c3836);
        scene.text_centered(layout.kaoss_key, &format!("KEY {key}"), 0xffffff, 2);
        scene.button(layout.kaoss_oct, 0x3c3836);
        scene.text_centered(layout.kaoss_oct, &oct, 0xffffff, 2);
        scene.button(
            layout.kaoss_hold,
            if model.kaoss_hold { 0xd79921 } else { 0x3c3836 },
        );
        scene.text_centered(layout.kaoss_hold, "HOLD", 0xffffff, 2);
    }

    if layout.kaoss_gate.w > 0 {
        scene.button(layout.kaoss_gate, 0x3c3836);
        scene.text_centered(layout.kaoss_gate, gate, 0xffffff, 2);
        scene.button(layout.kaoss_bpm_down, 0x3c3836);
        scene.text_centered(layout.kaoss_bpm_down, "BPM -", 0xffffff, 2);
        scene.button(layout.kaoss_bpm_up, 0x3c3836);
        scene.text_centered(layout.kaoss_bpm_up, "BPM +", 0xffffff, 2);
        scene.button(
            layout.kaoss_full,
            if model.kaoss_full { 0x689d6a } else { 0x458588 },
        );
        scene.text_centered(
            layout.kaoss_full,
            if model.kaoss_full { "EXIT" } else { "FULL" },
            0xffffff,
            2,
        );
        scene.button(layout.kaoss_settings_btn, 0x504945);
        scene.text_centered(layout.kaoss_settings_btn, "SET", 0xffffff, 2);
    }

    if layout.kaoss_full.w > 0 && layout.kaoss_gate.w == 0 {
        // FULL PAD mode — exit control only via bottom-edge hold.
    }
}

fn draw_kaoss_settings(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    let scroll = model.kaoss_settings_scroll;
    let c = layout.content;
    scene.fill_rect(c, 0x111111);
    scene.text(c.x + 8, c.y + 8 - scroll, "KAOSS settings", 0xfbf1c7);
    scene.text(c.x + c.w - 120, c.y + 8 - scroll, "drag to scroll", 0xa89984);

    let wipe = layout.kaoss_settings_row(52, scroll, 48);
    scene.button(wipe, 0x9d0006);
    scene.text_centered(wipe, "WIPE FX", 0xffffff, 2);

    let show_all = layout.kaoss_settings_row(108, scroll, 48);
    scene.button(
        show_all,
        if model.kaoss_show_all { 0xd79921 } else { 0x3c3836 },
    );
    scene.text_centered(
        show_all,
        if model.kaoss_show_all {
            "SHOW ALL: ON"
        } else {
            "SHOW ALL: OFF"
        },
        0xffffff,
        2,
    );

    let axes = layout.kaoss_settings_half_row(164, scroll, true, 48);
    scene.button(
        axes,
        if model.kaoss_show_axis_labels { 0x458588 } else { 0x3c3836 },
    );
    scene.text_centered(
        axes,
        if model.kaoss_show_axis_labels {
            "AXES: ON"
        } else {
            "AXES: OFF"
        },
        0xffffff,
        2,
    );

    let grid = layout.kaoss_settings_half_row(164, scroll, false, 48);
    scene.button(
        grid,
        if model.kaoss_show_grid_lines { 0x458588 } else { 0x3c3836 },
    );
    scene.text_centered(
        grid,
        if model.kaoss_show_grid_lines {
            "GRID: ON"
        } else {
            "GRID: OFF"
        },
        0xffffff,
        2,
    );

    scene.text(c.x + 8, c.y + 216 - scroll, "PAD VIZ", 0xa89984);
    let cells = layout.kaoss_settings_half_row(232, scroll, true, 48);
    scene.button(
        cells,
        if !model.kaoss_viz_glow { 0x458588 } else { 0x3c3836 },
    );
    scene.text_centered(cells, "CELLS", 0xffffff, 2);
    let glow = layout.kaoss_settings_half_row(232, scroll, false, 48);
    scene.button(
        glow,
        if model.kaoss_viz_glow { 0xb16286 } else { 0x3c3836 },
    );
    scene.text_centered(glow, "GLOW", 0xffffff, 2);

    scene.text(c.x + 8, c.y + 284 - scroll, "GRID LINES", 0xa89984);
    let grid_row = layout.kaoss_settings_row(300, scroll, 48);
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
    let label = Rect {
        x: grid_row.x + third + 8,
        y: grid_row.y,
        w: third,
        h: grid_row.h,
    };
    scene.button(minus, 0x3c3836);
    scene.text_centered(minus, "-", 0xffffff, 2);
    scene.fill_rect(label, 0x1d2021);
    scene.text_centered(
        label,
        &format!("{} px", model.kaoss_grid_width),
        0xfbf1c7,
        2,
    );
    scene.button(plus, 0x3c3836);
    scene.text_centered(plus, "+", 0xffffff, 2);

    scene.text(c.x + 8, c.y + 352 - scroll, "OUT", 0xa89984);
    for (i, mode) in [
        crate::session::OutMode::Local,
        crate::session::OutMode::Usb,
        crate::session::OutMode::Both,
    ]
    .iter()
    .enumerate()
    {
        let cell_w = (c.w - 16) / 3;
        let r = Rect {
            x: c.x + 8 + i as i32 * cell_w + 2,
            y: c.y + 368 - scroll + 2,
            w: cell_w - 4,
            h: 44,
        };
        let on = *mode == model.kaoss_out;
        scene.button(r, if on { mode.color() } else { 0x3c3836 });
        scene.text_centered(r, mode.label(), 0xffffff, 2);
    }

    scene.text(c.x + 8, c.y + 424 - scroll, "MIDI channel", 0xa89984);
    for ch in 0..16 {
        let cell = layout.kaoss_settings_channel(ch, 440, scroll);
        let on = ch as u8 == model.kaoss_channel;
        scene.button(cell, if on { 0x458588 } else { 0x3c3836 });
        scene.text_centered(cell, &format!("{}", ch + 1), 0xffffff, 2);
    }
}

fn draw_kaoss_glow(scene: &mut Scene, pad: crate::layout::Rect, model: &NativeModel) {
    let prog = kaoss_ui::program(model.kaoss_program);
    let hue = kaoss_viz::program_hue(prog.id);
    let t = model.kaoss_viz_time();
    let pulse = kaoss_viz::viz_pulse(t, model.bpm, model.kaoss_gate_flash());
    let hold = model.kaoss_hold && model.kaoss_touching;
    let amp = model.kaoss_glow_amp;

    let wash_v = (0.05 + 0.07 * pulse + if hold { 0.04 } else { 0.0 }) * (0.35 + 0.65 * amp);
    scene.fill_rect(pad, kaoss_viz::hsv_color(hue, 0.55, wash_v));

    let (fx, fy) = model.kaoss_glow_xy;
    let px = pad.x as f32 + fx.clamp(0.0, 1.0) * pad.w as f32;
    let py = pad.y as f32 + (1.0 - fy.clamp(0.0, 1.0)) * pad.h as f32;
    let span = (pad.w.min(pad.h)) as f32;

    if amp >= 0.02 {
        let (outer, mid, core) = kaoss_viz::glow_radii(span, amp);
        let fills = [
            kaoss_viz::hsv_color(hue, 0.85, 0.34 * amp),
            kaoss_viz::hsv_color(hue, 0.70, 0.68 * amp),
            kaoss_viz::hsv_color(hue, 0.18, 0.55 + 0.45 * amp),
        ];
        for (radius, color) in [(outer, fills[0]), (mid, fills[1]), (core, fills[2])] {
            if radius >= 1.5 {
                scene.fill_disc(px, py, radius, color);
            }
        }
    }

    for &(tx, ty, age) in model.kaoss_trail_points() {
        let tpx = pad.x as f32 + tx.clamp(0.0, 1.0) * pad.w as f32;
        let tpy = pad.y as f32 + (1.0 - ty.clamp(0.0, 1.0)) * pad.h as f32;
        let radius = (8.0 + 18.0 * age) * amp.max(0.25);
        let color = kaoss_viz::hsv_color(hue, 0.45, 0.50 * age * amp.max(0.2));
        scene.fill_disc(tpx, tpy, radius, color);
    }
}

fn draw_kaoss_ripples(scene: &mut Scene, pad: crate::layout::Rect, model: &NativeModel) {
    let hue = kaoss_viz::program_hue(kaoss_ui::program(model.kaoss_program).id);
    let pad_bg = 0x08040a;
    let span = (pad.w.min(pad.h)) as f32;
    for &(x, y, age) in model.kaoss_ripple_points() {
        let px = pad.x as f32 + x.clamp(0.0, 1.0) * pad.w as f32;
        let py = pad.y as f32 + (1.0 - y.clamp(0.0, 1.0)) * pad.h as f32;
        let radius = 10.0 + age * span * 0.42;
        let color = kaoss_viz::hsv_color(hue, 0.35, 0.95 * (1.0 - age));
        scene.stroke_disc(px, py, radius, 2.0, color, pad_bg);
    }
}

fn draw_kaoss_axes(scene: &mut Scene, pad: crate::layout::Rect, model: &NativeModel) {
    let prog = kaoss_ui::program(model.kaoss_program);
    let cx = pad.x as f32 + pad.w as f32 * 0.5;
    let cy = pad.y as f32 + pad.h as f32 * 0.5;
    if model.kaoss_show_grid_lines {
        let w = model.kaoss_grid_width.max(1) as f32;
        scene.fill(cx - w * 0.5, pad.y as f32, w, pad.h as f32, 0x9d2449);
        scene.fill(pad.x as f32, cy - w * 0.5, pad.w as f32, w, 0x9d2449);
    }
    if model.kaoss_show_axis_labels {
        let y_label = match prog.y_param {
            "tone" => "Y TONE",
            "morph" => "Y MORPH",
            "vibrato_always" => "Y VIB",
            "level" => "Y LEVEL",
            "release" => "Y DECAY",
            "attack" => "Y ATTACK",
            "delay_mix" => "Y DLY MIX",
            "reverb_mix" => "Y REVERB",
            _ => "Y",
        };
        let x_label = if prog.note {
            "X PITCH"
        } else {
            match prog.x_param {
                Some("tone") => "X TONE",
                Some("delay_time") => "X DLY T",
                Some("drive") => "X DRIVE",
                Some("attack") => "X ATTACK",
                Some("delay_mix") => "X ECHO",
                Some("reverb_size") => "X SIZE",
                _ => "X",
            }
        };
        scene.text_scaled(pad.x + 6, pad.y + pad.h / 2 - 20, y_label, 0xd3869b, 2);
        scene.text_scaled(
            pad.x + pad.w / 2 - 40,
            pad.y + pad.h - 22,
            x_label,
            0xd3869b,
            2,
        );
    }
}

fn draw_kaoss_cursor(
    scene: &mut Scene,
    pad: crate::layout::Rect,
    fx: f32,
    fy: f32,
    model: &NativeModel,
) {
    let cx = pad.x as f32 + fx.clamp(0.0, 1.0) * pad.w as f32;
    let cy = pad.y as f32 + (1.0 - fy.clamp(0.0, 1.0)) * pad.h as f32;
    // GLOW draws the finger blob; skip hard rings (Tk parity).
    if model.kaoss_viz_glow {
        if kaoss_ui::program(model.kaoss_program).note {
            let note = jambox_core::NOTE_NAMES[model.kaoss_key as usize];
            scene.text_scaled((cx as i32) - 8, (cy as i32) - 28, note, 0xfbf1c7, 2);
        }
        return;
    }
    let hue =
        (fx.clamp(0.0, 1.0) * 0.70 + kaoss_viz::program_hue(kaoss_ui::program(model.kaoss_program).id))
            .rem_euclid(1.0);
    let outer = kaoss_viz::hsv_color(hue, 0.90, 0.55);
    let mid = kaoss_viz::hsv_color(hue, 0.85, 1.0);
    let core = kaoss_viz::hsv_color(hue, 0.12, 1.0);
    scene.stroke_disc(cx, cy, 34.0, 2.0, outer, 0x08040a);
    scene.stroke_disc(cx, cy, 20.0, 3.0, mid, 0x08040a);
    scene.fill_disc(cx, cy, 7.0, core);
    scene.fill(cx - 28.0, cy - 1.0, 56.0, 2.0, mid);
    scene.fill(cx - 1.0, cy - 28.0, 2.0, 56.0, mid);
    if kaoss_ui::program(model.kaoss_program).note {
        let note = jambox_core::NOTE_NAMES[model.kaoss_key as usize];
        scene.text_scaled((cx as i32) - 8, (cy as i32) - 40, note, 0xfbf1c7, 2);
    }
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
            layout.pads_clear.x + 16,
            layout.pads_clear.y + 16,
            if model.pads_clear_armed {
                "CLR?"
            } else {
                "CLEAR"
            },
            0xffffff,
        );
        let trig = if model.phrases[model.pads_selected.min(15)].loop_mode {
            "LOOP"
        } else {
            "1SHOT"
        };
        scene.fill_rect(layout.pads_trig, 0x458588);
        scene.text(layout.pads_trig.x + 24, layout.pads_trig.y + 16, trig, 0xffffff);
        scene.fill_rect(
            layout.pads_mode,
            if model.pads_mode_armed {
                0xd79921
            } else {
                0x504945
            },
        );
        scene.text(layout.pads_mode.x + 16, layout.pads_mode.y + 16, "MODE", 0xffffff);
        let pad = &model.phrases[model.pads_selected.min(15)];
        let voice_bg = if pad.voice_locked { 0xb16286 } else { 0x689d6a };
        let voice_label = if pad.voice_locked { "LOCK" } else { "FOLLOW" };
        scene.fill_rect(layout.pads_voice, voice_bg);
        scene.text(
            layout.pads_voice.x + 8,
            layout.pads_voice.y + 16,
            voice_label,
            0xffffff,
        );
        let synth_bg = if pad.local_synth { 0xd65d0e } else { 0x504945 };
        let synth_label = if pad.local_synth { "SYNTH" } else { "MIDI" };
        scene.fill_rect(layout.pads_synth, synth_bg);
        scene.text(
            layout.pads_synth.x + 10,
            layout.pads_synth.y + 16,
            synth_label,
            0xffffff,
        );
        let ch_label = if pad.out_channel < 0 {
            "CH:rec".to_string()
        } else {
            format!("CH:{}", pad.out_channel + 1)
        };
        scene.fill_rect(layout.pads_channel, 0x504945);
        scene.text(
            layout.pads_channel.x + 6,
            layout.pads_channel.y + 16,
            &ch_label,
            0xffffff,
        );
        let rec_on = model.pads_recording.is_some();
        scene.fill_rect(layout.pads_rec, if rec_on { 0xcc241d } else { 0x9d0006 });
        scene.text(
            layout.pads_rec.x + 12,
            layout.pads_rec.y + 16,
            if rec_on { "STOP" } else { "REC" },
            0xffffff,
        );
        scene.fill_rect(layout.pads_vol_down, 0x3c3836);
        scene.text(
            layout.pads_vol_down.x + 8,
            layout.pads_vol_down.y + 16,
            "VOL-",
            0xffffff,
        );
        scene.fill_rect(layout.pads_vol_up, 0x3c3836);
        scene.text(
            layout.pads_vol_up.x + 8,
            layout.pads_vol_up.y + 16,
            "VOL+",
            0xffffff,
        );
    }

    if model.seq_to_pad_armed {
        scene.text(16, HUD_H + 40, "→PAD armed — tap a slot", 0xfabd2f);
    }

    if layout.pads_out.w > 0 {
        scene.fill_rect(layout.pads_out, model.pads_out.color());
        scene.text(
            layout.pads_out.x + 10,
            layout.pads_out.y + 16,
            model.pads_out.label(),
            0xffffff,
        );
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
    use crate::layout::HOME_TILES;

    let layout = model.layout;
    scene.text_scaled(layout.content.x + 12, layout.content.y + 10, "Home", 0xfbf1c7, 3);
    scene.text_scaled(
        layout.content.x + layout.content.w - 160,
        layout.content.y + 14,
        "tap a mode",
        0x928374,
        2,
    );
    for (i, (_mode, title, color)) in HOME_TILES.iter().enumerate() {
        let cell = layout.home_tile(i);
        scene.button(cell, *color);
        scene.text_centered(cell, title, 0xfbf1c7, 2);
    }
}

fn draw_scroll_wave_grid(
    scene: &mut Scene,
    layout: &Layout,
    model: &NativeModel,
    scroll_y: i32,
    morph_a: u16,
    morph_b: u16,
    current_voice: Option<u16>,
) {
    let n = model.wave_names.len();
    let grid = layout.synth_voice_grid(n);
    scene.fill_rect(grid.viewport, 0x141418);
    for index in grid.visible_range(scroll_y) {
        let cell = grid.cell_rect(index, scroll_y);
        if cell.y + cell.h < grid.viewport.y || cell.y > grid.viewport.y + grid.viewport.h {
            continue;
        }
        let color = if Some(index as u16) == current_voice {
            0x689d6a
        } else if index as u16 == morph_a {
            0xb16286
        } else if index as u16 == morph_b {
            0x458588
        } else {
            0x3c3836
        };
        scene.fill_rect(cell, color);
        let label = waves::short_label(&model.wave_names[index]);
        scene.text(cell.x + 10, cell.y + cell.h / 2 - 3, &label, 0xffffff);
    }
}

fn draw_synth(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;

    if model.synth_voice_open {
        scene.text(24, HUD_H + 4, "VOICES — tap · drag to scroll", 0xfbf1c7);
        scene.text(
            560,
            HUD_H + 6,
            &format!("{} loaded", model.wave_names.len()),
            0xa89984,
        );
        draw_scroll_wave_grid(
            scene,
            &layout,
            model,
            model.synth_voice_scroll,
            model.morph_a,
            model.morph_b,
            Some(model.morph_a),
        );
        scene.fill_rect(layout.synth_pick_save_as, 0x689d6a);
        scene.text_centered(layout.synth_pick_save_as, "SAVE AS", 0xffffff, 2);
        scene.fill_rect(layout.synth_pick_done, 0x458588);
        scene.text_centered(layout.synth_pick_done, "DONE", 0xffffff, 2);
        return;
    }

    if model.synth_pick_a.is_some() {
        let pick_a = model.synth_pick_a.unwrap_or(true);
        scene.text(
            24,
            HUD_H + 4,
            if pick_a {
                "MORPH PAIR · tap A · drag to scroll"
            } else {
                "MORPH PAIR · tap B · drag to scroll"
            },
            0xfbf1c7,
        );
        draw_scroll_wave_grid(
            scene,
            &layout,
            model,
            model.synth_pick_scroll,
            model.morph_a,
            model.morph_b,
            None,
        );
        scene.fill_rect(layout.synth_pick_save_as, 0x689d6a);
        scene.text_centered(layout.synth_pick_save_as, "SAVE AS", 0xffffff, 2);
        scene.fill_rect(layout.synth_pick_done, 0x458588);
        scene.text_centered(layout.synth_pick_done, "DONE", 0xffffff, 2);
        return;
    }

    const LABELS: [&str; 5] = ["MORPH", "TONE", "LEVEL", "ATK", "REL"];

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
    scene.text(layout.synth_swap.x + 16, layout.synth_swap.y + 16, "SWAP", 0xffffff);
    scene.fill_rect(layout.synth_voices, 0x458588);
    scene.text_centered(layout.synth_voices, "VOICES", 0xffffff, 2);
    scene.fill_rect(layout.synth_save_as, 0x689d6a);
    scene.text(
        layout.synth_save_as.x + 20,
        layout.synth_save_as.y + 16,
        "SAVE AS",
        0xffffff,
    );
    scene.fill_rect(layout.synth_vib_down, 0x3c3836);
    scene.text(
        layout.synth_vib_down.x + 8,
        layout.synth_vib_down.y + 16,
        "VIB-",
        0xffffff,
    );
    scene.fill_rect(layout.synth_vib_up, 0x3c3836);
    scene.text(
        layout.synth_vib_up.x + 8,
        layout.synth_vib_up.y + 16,
        "VIB+",
        0xffffff,
    );
    scene.text(
        24,
        HUD_H + 64,
        &format!("vib {:.2}", model.vibrato_always),
        0xa89984,
    );

    // CRT-ish morph scope (builtins WaveBank).
    draw_synth_scope(scene, model);

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

    for index in 0..Layout::SYNTH_WHITE_COUNT {
        let key = layout.synth_keyboard_white_rect(index);
        scene.fill_rect(key, 0xf2f2ea);
        scene.fill_rect(
            Rect {
                x: key.x,
                y: key.y + key.h - 2,
                w: key.w,
                h: 2,
            },
            0xc0c0b8,
        );
    }
    const BLACK_LABELS: [&str; 5] = ["C#", "D#", "F#", "G#", "A#"];
    for index in 0..5 {
        let key = layout.synth_keyboard_black_rect(index);
        scene.fill_rect(key, 0x1a1a22);
        scene.text(key.x + 6, key.y + key.h - 14, BLACK_LABELS[index], 0xd0d0d8);
    }
}

fn draw_synth_scope(scene: &mut Scene, model: &NativeModel) {
    let rect = model.layout.synth_scope;
    scene.fill_rect(rect, 0x1a1a12);
    // Grid
    for i in 1..4 {
        let y = rect.y + (rect.h * i) / 4;
        scene.fill(
            rect.x as f32,
            y as f32,
            rect.w as f32,
            1.0,
            0x2a2a18,
        );
    }
    let Some(bank) = model.wave_bank.as_ref() else {
        scene.text(rect.x + 24, rect.y + rect.h / 2 - 4, "NO WAVE", 0x504945);
        return;
    };
    let table = bank.morph_table();
    let n = (rect.w / 2).max(48) as usize;
    let mid_y = rect.y as f32 + rect.h as f32 * 0.5;
    let amp = (rect.h as f32 * 0.42).max(4.0);
    let mut prev_x = rect.x as f32 + 2.0;
    let mut prev_y = mid_y;
    for i in 0..n {
        let sample_i = (i * jambox_core::TABLE_SIZE) / n.max(1);
        let s = table[sample_i.min(jambox_core::TABLE_SIZE - 1)];
        let x = rect.x as f32 + 2.0 + (i as f32) * ((rect.w - 4) as f32) / n as f32;
        let y = mid_y - s * amp;
        stroke_scope_seg(scene, prev_x, prev_y, x, y, 0xb8bb26);
        prev_x = x;
        prev_y = y;
    }
}

fn stroke_scope_seg(scene: &mut Scene, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let steps = dx.abs().max(dy.abs()).max(1.0) as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = x0 + dx * t;
        let y = y0 + dy * t;
        scene.fill(x, y, 2.0, 2.0, color);
    }
}

fn draw_drums(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.text(24, HUD_H + 8, "DRUM KIT", 0xfbf1c7);
    scene.text(560, HUD_H + 10, "tap a pad · knobs reshape the wave", 0xa89984);

    let sel_cell = phrases::PHRASE_GRID_CELLS[model.kit_selected];
    let sel_note = phrases::mpk_note_for_phrase_cell(sel_cell);
    let model_name = drum_model_for_note(sel_note).name().replace('_', " ");
    scene.text(
        24,
        HUD_H + 24,
        &format!(
            "{} · {} · tone {:.0} · snap {:.0} · pitch {:.0} · decay {:.0}",
            phrases::pad_label(sel_cell),
            model_name,
            model.drum_macros[0] * 100.0,
            model.drum_macros[1] * 100.0,
            model.drum_macros[2] * 100.0,
            model.drum_macros[3] * 100.0,
        ),
        0xfabd2f,
    );

    let scope = layout.kit_scope;
    scene.fill_rect(scope, 0x1a1a12);
    for i in 1..4 {
        let y = scope.y + (scope.h * i) / 4;
        scene.fill(scope.x as f32, y as f32, scope.w as f32, 1.0, 0x2a2a18);
    }
    scene.text(
        scope.x + 24,
        scope.y + scope.h / 2 - 4,
        &format!("{} waveform", model_name),
        0x504945,
    );

    for screen_index in 0..16 {
        let cell = layout.kit_pad_cell(screen_index);
        let phrase_cell = phrases::PHRASE_GRID_CELLS[screen_index];
        let note = phrases::mpk_note_for_phrase_cell(phrase_cell);
        let voice = drum_model_for_note(note).name().replace('_', " ");
        let selected = !model.kit_all_drums && screen_index == model.kit_selected;
        let bg = if selected { 0xd79921 } else { 0x3c3836 };
        scene.fill_rect(cell, bg);
        let label = format!("{}\n{}", phrases::pad_label(phrase_cell), voice);
        let lines: Vec<&str> = label.lines().collect();
        scene.text(cell.x + 8, cell.y + cell.h / 2 - 10, lines[0], 0xfbf1c7);
        if lines.len() > 1 {
            scene.text(cell.x + 8, cell.y + cell.h / 2 + 6, lines[1], 0xa89984);
        }
    }

    const MACROS: [&str; 4] = ["TONE", "SNAP", "PITCH", "DECAY"];
    for index in 0..4 {
        let cell = layout.kit_macro_cell(index);
        scene.fill_rect(cell, 0x3c3836);
        scene.text(
            cell.x + 16,
            cell.y + 12,
            &format!("{} {:.0}", MACROS[index], model.drum_macros[index] * 100.0),
            0xfbf1c7,
        );
    }

    const DIV_LABELS: [&str; 4] = ["1/4", "1/8", "1/8T", "1/16"];
    for index in 0..4 {
        let cell = layout.kit_division_cell(index);
        let choice = RepeatDivisionChoice::from_index(index);
        let active = model.division == choice;
        scene.fill_rect(cell, if active { 0x458588 } else { 0x282828 });
        scene.text(cell.x + 16, cell.y + 8, DIV_LABELS[index], 0xffffff);
    }

    let all_bg = if model.kit_all_drums { 0xb16286 } else { 0x504945 };
    scene.fill_rect(layout.kit_all, all_bg);
    scene.text_centered(layout.kit_all, "ALL DRUMS", 0xfbf1c7, 2);
}

fn draw_seq(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    let seq = &model.seq;

    scene.text(16, HUD_H + 10, "Sequencer", 0xfbf1c7);
    scene.text(
        160,
        HUD_H + 12,
        &format!("{:.0} BPM", seq.bpm),
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
    scene.text(layout.seq_stop.x + 24, layout.seq_stop.y + 14, "STOP", 0xffffff);
    scene.fill_rect(layout.seq_clear, 0x3c3836);
    scene.text(layout.seq_clear.x + 20, layout.seq_clear.y + 14, "CLEAR", 0xffffff);
    scene.fill_rect(
        layout.seq_to_pad,
        if model.seq_to_pad_armed {
            0xd79921
        } else {
            0x458588
        },
    );
    scene.text(layout.seq_to_pad.x + 28, layout.seq_to_pad.y + 14, "→PAD", 0xffffff);
    scene.fill_rect(layout.seq_all_off, 0x665c54);
    scene.text(
        layout.seq_all_off.x + 20,
        layout.seq_all_off.y + 14,
        "ALL OFF",
        0xffffff,
    );
    scene.fill_rect(layout.seq_bpm_down, 0x282828);
    scene.text(layout.seq_bpm_down.x + 36, layout.seq_bpm_down.y + 14, "- BPM", 0xffffff);
    scene.fill_rect(layout.seq_bpm_up, 0x282828);
    scene.text(layout.seq_bpm_up.x + 36, layout.seq_bpm_up.y + 14, "+ BPM", 0xffffff);
    if model.seq_to_pad_armed {
        scene.text(400, HUD_H + 30, "tap a PAD slot", 0xfabd2f);
    } else {
        scene.text(400, HUD_H + 30, "→PAD assigns loop to a pad", 0x83a598);
    }

    // Tk howto abbreviated to one line.
    scene.text(
        12,
        HUD_H + layout.content.h - 18,
        "REC locks loop · overdub · KEEP/DROP/UNDO · →PAD assigns clip",
        0x83a598,
    );
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
        model.layout.preset_save.x + 48,
        model.layout.preset_save.y + 24,
        "SAVE",
        0xffffff,
    );
    scene.fill_rect(model.layout.preset_load, 0x458588);
    scene.text(
        model.layout.preset_load.x + 48,
        model.layout.preset_load.y + 24,
        "LOAD",
        0xffffff,
    );
    scene.fill_rect(model.layout.preset_delete, 0x9d0006);
    scene.text(
        model.layout.preset_delete.x + 36,
        model.layout.preset_delete.y + 24,
        "DELETE",
        0xffffff,
    );
    scene.fill_rect(model.layout.preset_factory, 0x504945);
    scene.text(
        model.layout.preset_factory.x + 28,
        model.layout.preset_factory.y + 24,
        "FACTORY RESET",
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
    scene.text(layout.song_play.x + 60, layout.song_play.y + 20, "PLAY", 0xffffff);
    scene.fill_rect(layout.song_stop, 0x3c3836);
    scene.text(layout.song_stop.x + 60, layout.song_stop.y + 20, "STOP", 0xffffff);
    scene.fill_rect(
        layout.song_loop,
        if model.song_loop { 0x458588 } else { 0x282838 },
    );
    scene.text(layout.song_loop.x + 20, layout.song_loop.y + 16, "LOOP", 0xffffff);
    scene.fill_rect(layout.song_delete, 0x9d0006);
    scene.text(layout.song_delete.x + 24, layout.song_delete.y + 16, "DEL", 0xffffff);
    scene.fill_rect(layout.song_bpm_down, 0x282838);
    scene.text(
        layout.song_bpm_down.x + 16,
        layout.song_bpm_down.y + 16,
        "BPM-",
        0xffffff,
    );
    scene.fill_rect(layout.song_bpm_up, 0x282838);
    scene.text(layout.song_bpm_up.x + 16, layout.song_bpm_up.y + 16, "BPM+", 0xffffff);
    scene.fill_rect(layout.song_prev, 0x282838);
    scene.text(layout.song_prev.x + 28, layout.song_prev.y + 20, "UP", 0xffffff);
    scene.fill_rect(layout.song_next, 0x282838);
    scene.text(layout.song_next.x + 16, layout.song_next.y + 20, "DOWN", 0xffffff);
    scene.fill_rect(layout.song_save_seq, 0x458588);
    scene.text(
        layout.song_save_seq.x + 48,
        layout.song_save_seq.y + 14,
        "SAVE SEQ",
        0xffffff,
    );
    scene.fill_rect(layout.song_out, model.song_out.color());
    scene.text(
        layout.song_out.x + 48,
        layout.song_out.y + 14,
        model.song_out.label(),
        0xffffff,
    );
}

fn draw_map(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    let c = layout.content;
    scene.text(c.x + 16, c.y + 16, "MAP / THRU", 0xfbf1c7);
    scene.text(
        c.x + 16,
        c.y + 44,
        &format!("status: {}", crate::host::map_status_line()),
        0xa0a0b8,
    );
    scene.text(
        c.x + 16,
        c.y + 68,
        "USB MIDI in → remap → out (midi-engine on appliance)",
        0x83a598,
    );
    scene.fill_rect(layout.map_thru_on, 0x689d6a);
    scene.text(
        layout.map_thru_on.x + 56,
        layout.map_thru_on.y + 28,
        "THRU ON",
        0xffffff,
    );
    scene.fill_rect(layout.map_thru_off, 0x9d0006);
    scene.text(
        layout.map_thru_off.x + 48,
        layout.map_thru_off.y + 28,
        "THRU OFF",
        0xffffff,
    );
    scene.fill_rect(layout.map_refresh, 0x458588);
    scene.text(
        layout.map_refresh.x + 28,
        layout.map_refresh.y + 28,
        "REFRESH PORTS",
        0xffffff,
    );
    // Recent log peek for list output
    let mut y = layout.map_thru_on.y + 96;
    for line in model.log_lines.iter().rev().take(8) {
        scene.text(c.x + 16, y, line, 0xc0c0d0);
        y += 18;
        if y > crate::layout::SCREEN_H - NAV_H - 20 {
            break;
        }
    }
}

fn draw_settings(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.settings_panic, 0x9d0006);
    scene.text(layout.settings_panic.x + 48, layout.settings_panic.y + 28, "PANIC", 0xffffff);
    scene.fill_rect(layout.settings_all_off, 0x504945);
    scene.text(
        layout.settings_all_off.x + 28,
        layout.settings_all_off.y + 28,
        "NOTES OFF",
        0xffffff,
    );
    scene.fill_rect(
        layout.settings_fx_target,
        match model.fx_target {
            crate::model::FxEditTarget::Bus => 0x458588,
            crate::model::FxEditTarget::Voice => 0xb16286,
            crate::model::FxEditTarget::DrumGroup => 0xd79921,
        },
    );
    scene.text(
        layout.settings_fx_target.x + 28,
        layout.settings_fx_target.y + 28,
        match model.fx_target {
            crate::model::FxEditTarget::Bus => "FX: BUS",
            crate::model::FxEditTarget::Voice => "FX: VOICE 0",
            crate::model::FxEditTarget::DrumGroup => "FX: DRUMS",
        },
        0xffffff,
    );
    const LABELS: [&str; 3] = ["DRIVE", "DELAY", "REVERB"];
    let values = match model.fx_target {
        crate::model::FxEditTarget::Bus => &model.fx_bus,
        crate::model::FxEditTarget::Voice => &model.fx_voice,
        crate::model::FxEditTarget::DrumGroup => &model.fx_drum,
    };
    let fill_color = match model.fx_target {
        crate::model::FxEditTarget::Bus => 0x458588,
        crate::model::FxEditTarget::Voice => 0xb16286,
        crate::model::FxEditTarget::DrumGroup => 0xd79921,
    };
    for index in 0..3 {
        let track = layout.settings_fx_slider(index);
        scene.fill_rect(track, 0x20202c);
        scene.text(track.x + 8, track.y - 18, LABELS[index], 0xc0c0d0);
        let fill_h = (track.h as f32 * values[index]) as i32;
        let fill = Rect {
            x: track.x + 4,
            y: track.y + track.h - fill_h,
            w: track.w - 8,
            h: fill_h,
        };
        scene.fill_rect(fill, fill_color);
    }
    scene.fill_rect(layout.settings_drum_kit, 0x98971a);
    scene.text_centered(layout.settings_drum_kit, "DRUM KIT", 0xffffff, 2);
    scene.fill_rect(layout.settings_map, 0x83a598);
    scene.text_centered(layout.settings_map, "MAP", 0xffffff, 2);
    scene.fill_rect(layout.settings_wifi, 0xd79921);
    scene.text_centered(layout.settings_wifi, "WIFI", 0xffffff, 2);
    scene.fill_rect(
        layout.settings_font,
        model.font_style.resolved().settings_color(),
    );
    scene.text_centered(
        layout.settings_font,
        model.font_style.label(),
        0xffffff,
        2,
    );
    scene.fill_rect(layout.settings_update, 0x689d6a);
    scene.text_centered(layout.settings_update, "UPDATE", 0xffffff, 2);
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
    for (i, line) in model
        .log_lines
        .iter()
        .rev()
        .skip(model.log_scroll)
        .take(10)
        .enumerate()
    {
        scene.text(c.x + 16, c.y + 70 + (i as i32) * 18, line, 0xd5c4a1);
    }
    scene.fill_rect(model.layout.log_clear, 0x504945);
    scene.text(
        model.layout.log_clear.x + 48,
        model.layout.log_clear.y + 16,
        "CLEAR",
        0xffffff,
    );
    scene.fill_rect(model.layout.log_all_off, 0x9d0006);
    scene.text(
        model.layout.log_all_off.x + 40,
        model.layout.log_all_off.y + 16,
        "ALL OFF",
        0xffffff,
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
        assert!(!model.kaoss_viz_glow);
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        model.finger_down(1, k.x + 40, k.y + 40, &mut out);
        model.tick(1.0 / 60.0, &mut out);
        let scene = build(&model);
        let cells = scene
            .color
            .iter()
            .filter(|q| {
                q.x >= model.layout.kaoss.x as f32
                    && q.y >= model.layout.kaoss.y as f32
                    && q.x < (model.layout.kaoss.x + model.layout.kaoss.w) as f32
                    && q.y < (model.layout.kaoss.y + model.layout.kaoss.h) as f32
                    && q.w >= 20.0
                    && q.h >= 20.0
                    && q.w < 80.0
                    && q.h < 80.0
            })
            .count();
        assert_eq!(cells, LED_COLS * LED_ROWS);
        assert!(!scene.glyphs.is_empty());
    }

    #[test]
    fn glow_mode_skips_led_grid() {
        let mut model = NativeModel::new();
        model.kaoss_viz_glow = true;
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        model.finger_down(1, k.x + 40, k.y + 40, &mut out);
        for _ in 0..10 {
            model.tick(1.0 / 60.0, &mut out);
        }
        let scene = build(&model);
        let grid_cells = scene
            .color
            .iter()
            .filter(|q| {
                q.x >= model.layout.kaoss.x as f32
                    && q.y >= model.layout.kaoss.y as f32
                    && q.x < (model.layout.kaoss.x + model.layout.kaoss.w) as f32
                    && q.y < (model.layout.kaoss.y + model.layout.kaoss.h) as f32
                    && q.w >= 20.0
                    && q.h >= 20.0
                    && q.w < 80.0
                    && q.h < 80.0
            })
            .count();
        assert_eq!(grid_cells, 0, "GLOW should not paint the 12×7 LED grid");
        assert!(
            model.kaoss_glow_amp > 0.1,
            "touch should ramp the glow envelope"
        );
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
