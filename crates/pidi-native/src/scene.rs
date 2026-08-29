//! Batched draw list for the 800×480 surface.
//!
//! GLES consumes this as two meshes (color quads + glyph quads). The CPU
//! rasterizer uses the same list so dummy/PPM output matches the GPU path.

use crate::font::{self, FontStyle, GLYPH_H, GLYPH_STRIDE, GLYPH_W};
use crate::kaoss_ui;
use crate::kaoss_viz;
use crate::layout::{Layout, Rect, HUD_H, NAV_H};
use crate::mode::UiMode;
use crate::model::{NativeModel, RepeatDivisionChoice, LED_COLS, LED_ROWS};
use crate::phrases;
use crate::render::{SCREEN_H, SCREEN_W};
use crate::screensaver;
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
        self.fill(
            rect.x as f32,
            rect.y as f32,
            rect.w as f32,
            rect.h as f32,
            color,
        );
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
        self.fill_disc_clipped(cx, cy, radius, color, None);
    }

    /// Filled disc, optionally clipped to a rect (for pad-local glows).
    pub fn fill_disc_clipped(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        color: u32,
        clip: Option<Rect>,
    ) {
        if radius < 1.0 {
            return;
        }
        let r = radius.ceil() as i32;
        let r2 = radius * radius;
        for dy in -r..=r {
            let y = cy + dy as f32;
            if let Some(c) = clip {
                if y < c.y as f32 || y >= (c.y + c.h) as f32 {
                    continue;
                }
            }
            let dx_max_sq = r2 - (dy as f32 * dy as f32);
            if dx_max_sq <= 0.0 {
                continue;
            }
            let half_w = dx_max_sq.sqrt();
            let mut x0 = cx - half_w;
            let mut x1 = cx + half_w;
            if let Some(c) = clip {
                x0 = x0.max(c.x as f32);
                x1 = x1.min((c.x + c.w) as f32);
            }
            let w = x1 - x0;
            if w > 0.0 {
                self.fill(x0, y, w, 1.0, color);
            }
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

    fn text_scaled_smooth(&mut self, x: &mut i32, y: i32, text: &str, color: u32, scale: i32) {
        let Some(atlas) = font::smooth_atlas() else {
            self.font_style = FontStyle::Retro;
            self.text_scaled_retro(x, y, text, color, scale.max(2));
            return;
        };
        // `y` is the top of the line box (same contract as retro). Baseline sits
        // ascent below that; glyph tops use fontdue ymin (bottom vs baseline).
        let ds = atlas.draw_scale(scale);
        // Snap baseline once so every glyph shares the same pixel row.
        let baseline = (y as f32 + atlas.ascent * ds).round();
        let mut pen_x = *x as f32;
        for ch in text.chars() {
            let g = atlas.glyph(ch);
            if g.width < 0.5 || g.height < 0.5 {
                pen_x += g.advance * ds;
                continue;
            }
            // Snap the baseline edge (ymin) first, then height — capitals with
            // ymin≈0 then share one bottom pixel row instead of drifting ±1.
            let bottom = (baseline - g.ymin * ds).round();
            let gh = (g.height * ds).round().max(1.0);
            let gy = bottom - gh;
            let gx = (pen_x + g.xmin * ds).round();
            let gw = (g.width * ds).round().max(1.0);
            self.glyphs.push(GlyphQuad {
                x: gx,
                y: gy,
                w: gw,
                h: gh,
                u0: g.u0,
                v0: g.v0,
                u1: g.u1,
                v1: g.v1,
                color,
            });
            pen_x += g.advance * ds;
        }
        *x = pen_x.round() as i32;
    }

    pub fn text_centered(&mut self, rect: Rect, s: &str, color: u32, scale: i32) {
        let style = self.font_style.resolved();
        let max_w = (rect.w - 4).max(1);
        let max_h = (rect.h - 4).max(1);
        let measure = |text: &str, scale: i32| -> (i32, i32) {
            match style {
                FontStyle::Retro => (
                    (text.chars().count() as i32) * GLYPH_STRIDE * scale,
                    GLYPH_H * scale,
                ),
                FontStyle::Smooth => match font::smooth_atlas() {
                    Some(atlas) => atlas.measure(text, scale),
                    None => (
                        (text.chars().count() as i32) * GLYPH_STRIDE * scale,
                        GLYPH_H * scale,
                    ),
                },
            }
        };

        let mut scale = scale.max(1);
        let mut draw = s.to_string();
        let (mut w, mut h) = measure(&draw, scale);
        while scale > 1 && (w > max_w || h > max_h) {
            scale -= 1;
            let m = measure(&draw, scale);
            w = m.0;
            h = m.1;
        }
        while w > max_w && draw.chars().count() > 1 {
            draw.pop();
            let m = measure(&draw, scale);
            w = m.0;
            h = m.1;
        }

        let x = rect.x + (rect.w - w).max(0) / 2;
        let y = rect.y + (rect.h - h).max(0) / 2;
        self.text_scaled(x, y, &draw, color, scale);
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
    } else if model.update_panel_open {
        draw_update_panel(&mut scene, model);
    } else if model.wifi_kb_open {
        draw_wifi_keyboard(&mut scene, model);
    } else if model.wifi_panel_open {
        draw_wifi_panel(&mut scene, model);
    } else {
        match model.mode {
            UiMode::Kaoss => draw_kaoss(&mut scene, model),
            UiMode::Pads => draw_pads(&mut scene, model),
            UiMode::Home => draw_home(&mut scene, model),
            UiMode::Synth => draw_synth(&mut scene, model),
            UiMode::Fm => draw_fm(&mut scene, model),
            UiMode::Drums => draw_drums(&mut scene, model),
            UiMode::Seq => draw_seq(&mut scene, model),
            UiMode::Presets => draw_presets(&mut scene, model),
            UiMode::Songs => draw_songs(&mut scene, model),
            UiMode::Map => draw_map(&mut scene, model),
            UiMode::Settings => draw_settings(&mut scene, model),
            UiMode::Fx => draw_fx(&mut scene, model),
            UiMode::Log => draw_log(&mut scene, model),
            UiMode::Chords => draw_chords(&mut scene, model),
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
    scene.text_scaled(
        layout.content.x + 12,
        layout.content.y + 12,
        "POWER",
        0xfbf1c7,
        3,
    );
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

fn draw_update_panel(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.content, 0x111111);
    scene.text_scaled(layout.content.x + 12, layout.content.y + 12, "UPDATE", 0xfbf1c7, 3);
    scene.button(layout.update_close, 0x504945);
    scene.text_centered(layout.update_close, "CLOSE", 0xffffff, 2);

    // Status body — wrap by drawing lines.
    let mut y = layout.content.y + 72;
    for line in model.update_status.lines().take(12) {
        let color = if line.contains("available") || line.contains("UPDATE available") {
            0xb8bb26
        } else if line.starts_with("Running:") {
            0xfabd2f
        } else {
            0xebdbb2
        };
        scene.text(layout.content.x + 16, y, line, color);
        y += 22;
    }

    let check_label = if model.update_confirming {
        "CANCEL"
    } else if model.update_busy {
        "…"
    } else {
        "CHECK"
    };
    let check_bg = if model.update_confirming {
        0x504945
    } else if model.update_busy {
        0x3c3836
    } else {
        0x458588
    };
    scene.button(layout.update_check, check_bg);
    scene.text_centered(layout.update_check, check_label, 0xffffff, 2);

    let (apply_label, apply_bg) = if model.update_busy {
        ("…", 0x3c3836)
    } else if model.update_confirming {
        ("INSTALL NOW", 0x9d0006)
    } else if model.update_available {
        ("UPDATE", 0x689d6a)
    } else {
        ("UPDATE", 0x3c3836)
    };
    scene.button(layout.update_apply, apply_bg);
    scene.text_centered(layout.update_apply, apply_label, 0xffffff, 2);
}

fn draw_wifi_panel(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.content, 0x111111);
    scene.text_scaled(layout.content.x + 12, layout.content.y + 12, "WIFI", 0xfbf1c7, 3);
    scene.button(layout.wifi_close(), 0x504945);
    scene.text_centered(layout.wifi_close(), "CLOSE", 0xffffff, 2);

    scene.text(
        layout.content.x + 16,
        layout.wifi_status_y(),
        &model.wifi_status,
        0xfabd2f,
    );

    for i in 0..crate::wifi::LIST_VISIBLE {
        let cell = layout.wifi_row(i);
        let idx = model.wifi_scroll + i;
        match model.wifi_networks.get(idx) {
            Some(net) => {
                let bg = if net.in_use { 0x458588 } else { 0x3c3836 };
                scene.button(cell, bg);
                let label = net.label();
                let short = if label.len() > 36 {
                    format!("{}…", &label[..35])
                } else {
                    label
                };
                scene.text(cell.x + 10, cell.y + 16, &short, 0xfbf1c7);
            }
            None => {
                scene.button(cell, 0x1d2021);
            }
        }
    }

    scene.button(layout.wifi_scroll_up(), 0x504945);
    scene.text_centered(layout.wifi_scroll_up(), "^", 0xffffff, 2);
    scene.button(layout.wifi_scroll_down(), 0x504945);
    scene.text_centered(layout.wifi_scroll_down(), "v", 0xffffff, 2);

    let scan_bg = if model.wifi_busy { 0x3c3836 } else { 0x458588 };
    let rejoin_bg = if model.wifi_busy { 0x3c3836 } else { 0xd79921 };
    scene.button(layout.wifi_scan_btn(), scan_bg);
    scene.text_centered(
        layout.wifi_scan_btn(),
        if model.wifi_busy { "…" } else { "SCAN" },
        0xffffff,
        2,
    );
    scene.button(layout.wifi_rejoin_btn(), rejoin_bg);
    scene.text_centered(
        layout.wifi_rejoin_btn(),
        if model.wifi_busy { "…" } else { "REJOIN" },
        0xffffff,
        2,
    );
}

fn draw_wifi_keyboard(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.content, 0x111111);
    scene.text_scaled(layout.content.x + 12, layout.content.y + 10, "PASSWORD", 0xfbf1c7, 2);
    let sub = if model.wifi_kb_ssid.len() > 28 {
        format!("{}…", &model.wifi_kb_ssid[..27])
    } else {
        model.wifi_kb_ssid.clone()
    };
    scene.text(layout.content.x + 200, layout.content.y + 14, &sub, 0xa89984);
    scene.button(layout.wifi_kb_cancel(), 0x9d0006);
    scene.text_centered(layout.wifi_kb_cancel(), "CANCEL", 0xffffff, 1);

    let entry = layout.wifi_kb_entry();
    scene.fill_rect(entry, 0x1d2021);
    let shown = if model.wifi_kb_show {
        model.wifi_kb_text.clone()
    } else {
        "*".repeat(model.wifi_kb_text.chars().count())
    };
    let shown = if shown.len() > 40 {
        format!("…{}", &shown[shown.len().saturating_sub(39)..])
    } else {
        shown
    };
    scene.text(entry.x + 8, entry.y + 12, &shown, 0xfbf1c7);
    scene.button(layout.wifi_kb_show(), 0x504945);
    scene.text_centered(
        layout.wifi_kb_show(),
        if model.wifi_kb_show { "HIDE" } else { "SHOW" },
        0xffffff,
        1,
    );

    let rows = if model.wifi_kb_sym {
        crate::wifi::keyboard_sym_rows()
    } else {
        crate::wifi::keyboard_abc_rows(model.wifi_kb_shift)
    };
    for (ri, row) in rows.iter().enumerate() {
        let mut col = 0i32;
        for (label, action, span) in row {
            if *action == "pad" {
                col += i32::from(*span);
                continue;
            }
            let cell = layout.wifi_kb_key(ri, col, *span);
            let bg = match *action {
                "ok" => 0x689d6a,
                "shift" if model.wifi_kb_shift => 0xd79921,
                "sym" | "abc" | "shift" | "back" => 0x504945,
                _ => 0x3c3836,
            };
            scene.button(cell, bg);
            scene.text_centered(cell, label, 0xffffff, 1);
            col += i32::from(*span);
        }
    }
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
        if model.can_nav_back() {
            0x3c3836
        } else {
            0x1d2021
        },
    );
    scene.text_centered(back, "<", 0xfbf1c7, 2);

    let home = layout.nav_home();
    scene.button(
        home,
        if model.mode == UiMode::Home {
            0x458588
        } else {
            0x3c3836
        },
    );
    scene.text_centered(home, "HOME", 0xfbf1c7, 2);

    let power = layout.nav_power();
    scene.button(
        power,
        if model.power_menu_open {
            0xb16286
        } else {
            0x9d0006
        },
    );
    scene.text_centered(power, "POWER", 0xfbf1c7, 2);

    let rec = layout.nav_chrome_rec();
    let (rec_label, rec_bg) = chrome_rec_label(model);
    scene.fill_rect(rec, rec_bg);
    scene.text_centered(rec, rec_label, 0xffffff, 2);

    for (i, mode) in JAM_MODES.iter().enumerate() {
        let cell = layout.nav_jam(i);
        let active = *mode == model.mode;
        let recording_seq = *mode == UiMode::Seq && model.seq.is_recording();
        scene.button(
            cell,
            if active {
                0x458588
            } else if recording_seq {
                0x9d0006
            } else {
                0x3c3836
            },
        );
        scene.text_centered(cell, mode.label(), 0xfbf1c7, 2);
    }

    let status = chrome_status(model);
    if !status.is_empty() {
        let status_x = layout.nav_chrome_rec().x - 8;
        // Right-align into the gap left of REC when possible.
        let approx_w = status.chars().count() as i32 * 10;
        let x = (status_x - approx_w).max(layout.nav_power().x + layout.nav_power().w + 8);
        scene.text(x, 18, &status, 0xfe8019);
    }
}

fn chrome_rec_label(model: &NativeModel) -> (&'static str, u32) {
    match model.seq.state {
        crate::seq::SeqState::RecBackbone | crate::seq::SeqState::Overdub => ("STOP", 0xcc241d),
        crate::seq::SeqState::Empty => ("REC", 0x9d0006),
        crate::seq::SeqState::Review => ("REC", 0xd79921),
        _ => ("REC", 0x504945),
    }
}

fn chrome_status(model: &NativeModel) -> String {
    // Prefer a short live readout — long status_line belongs in the content area.
    match model.mode {
        UiMode::Kaoss => {
            if model.kaoss_settings_open {
                return "SETTINGS".into();
            }
            format!(
                "{:.0} BPM {}",
                model.bpm,
                kaoss_ui::gate(model.kaoss_gate).label
            )
        }
        UiMode::Seq => format!("{:.0} BPM", model.bpm),
        UiMode::Chords => {
            let name = model
                .chords_current
                .map(|c| c.name())
                .unwrap_or_else(|| "CHORDS".into());
            if model.seq.is_recording() {
                format!("REC {name}")
            } else if model.pads_recording.is_some() {
                format!("PAD {name}")
            } else {
                name
            }
        }
        UiMode::Presets => format!("SLOT {}", model.preset_selected + 1),
        UiMode::Fm => format!(
            "{} · {}",
            jambox_core::fm_recipe(model.fm_recipe).label,
            jambox_core::OP_NAMES[model.fm_selected]
        ),
        UiMode::Synth if model.synth_vib_open => "VIB".into(),
        UiMode::Drums if model.kit_edit_open => "WAVE".into(),
        UiMode::Drums if model.kit_repeat_open => "REPEAT".into(),
        UiMode::Fx => {
            if model.status_line.is_empty() {
                "FX".into()
            } else {
                model.status_line.chars().take(18).collect()
            }
        }
        UiMode::Settings => {
            if model.status_line.is_empty() {
                "SETTINGS".into()
            } else {
                model.status_line.chars().take(18).collect()
            }
        }
        _ => {
            if !model.status_line.is_empty() && model.status_line.chars().count() <= 12 {
                model.status_line.clone()
            } else {
                String::new()
            }
        }
    }
}

fn draw_kaoss(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;

    if model.kaoss_color_picker_open {
        draw_kaoss_color_picker(scene, model);
        return;
    }

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
            kaoss_ui::KaossPicker::Octave => usize::MAX, // dual-select below
            kaoss_ui::KaossPicker::Gate => model.kaoss_gate,
        };
        scene.text(
            layout.kaoss.x + 8,
            layout.kaoss.y + 4,
            if matches!(kind, kaoss_ui::KaossPicker::Octave) {
                "start C + width · tap outside to close"
            } else {
                "drag to scroll"
            },
            0xa89984,
        );
        let oct_start = kaoss_ui::root_octave_index(model.kaoss_root_midi);
        let oct_wide =
            kaoss_ui::ROOT_OCTAVE_MIDI.len() + (model.kaoss_octaves as usize).saturating_sub(1);
        for index in grid.visible_range(model.kaoss_picker_scroll) {
            let cell = grid.cell_rect(index, model.kaoss_picker_scroll);
            if cell.y + cell.h < grid.viewport.y || cell.y > grid.viewport.y + grid.viewport.h {
                continue;
            }
            let on = if matches!(kind, kaoss_ui::KaossPicker::Octave) {
                index == oct_start || index == oct_wide
            } else {
                index == selected
            };
            scene.button(cell, if on { 0x458588 } else { 0x2a2a38 });
            let label = kaoss_ui::picker_label(kind, index, model.kaoss_show_all);
            scene.text_centered(cell, &label, 0xffffff, 2);
        }
    } else {
        scene.fill_rect(layout.kaoss, 0x08040a);
        if model.kaoss_viz_style.is_glow() {
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
        draw_kaoss_note_readout(scene, layout.kaoss, model);
    }

    let prog = kaoss_ui::program(model.kaoss_program);
    let scale = jambox_core::kaoss_scale(model.kaoss_scale_index as usize);
    let key = jambox_core::NOTE_NAMES[model.kaoss_key as usize];
    let oct = format!(
        "{}·{}",
        kaoss_ui::midi_note_label(model.kaoss_root_midi),
        model.kaoss_octaves
    );
    let gate = kaoss_ui::gate(model.kaoss_gate).label;

    if layout.kaoss_prog.w > 0 {
        scene.button(layout.kaoss_prog, 0xb16286);
        scene.text_centered(layout.kaoss_prog, prog.label, 0xffffff, 2);
        scene.button(layout.kaoss_scale, 0x458588);
        scene.text_centered(layout.kaoss_scale, scale.label, 0xffffff, 2);
        scene.button(layout.kaoss_key, 0x3c3836);
        scene.text_centered(layout.kaoss_key, &format!("KEY {key}"), 0xffffff, 2);
        if layout.kaoss_oct_down.w > 0 {
            scene.button(layout.kaoss_oct_down, 0x504945);
            scene.text_centered(layout.kaoss_oct_down, "-", 0xffffff, 2);
        }
        scene.button(layout.kaoss_oct, 0x3c3836);
        scene.text_centered(layout.kaoss_oct, &oct, 0xffffff, 2);
        if layout.kaoss_oct_up.w > 0 {
            scene.button(layout.kaoss_oct_up, 0x504945);
            scene.text_centered(layout.kaoss_oct_up, "+", 0xffffff, 2);
        }
        scene.button(
            layout.kaoss_hold,
            if model.kaoss_hold { 0xd79921 } else { 0x3c3836 },
        );
        scene.text_centered(layout.kaoss_hold, "HOLD", 0xffffff, 2);
    }

    if layout.kaoss_gate.w > 0 {
        scene.button(layout.kaoss_gate, 0x3c3836);
        scene.text_centered(layout.kaoss_gate, gate, 0xffffff, 2);
        scene.button(layout.kaoss_bpm_down5, 0x3c3836);
        scene.text_centered(layout.kaoss_bpm_down5, "-5", 0xffffff, 2);
        scene.button(layout.kaoss_bpm_down, 0x3c3836);
        scene.text_centered(layout.kaoss_bpm_down, "-1", 0xffffff, 2);
        scene.button(layout.kaoss_bpm_up, 0x3c3836);
        scene.text_centered(layout.kaoss_bpm_up, "+1", 0xffffff, 2);
        scene.button(layout.kaoss_bpm_up5, 0x3c3836);
        scene.text_centered(layout.kaoss_bpm_up5, "+5", 0xffffff, 2);
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
    scene.text(c.x + c.w - 80, c.y + 8 - scroll, "SCROLL", 0xa89984);

    let wipe = layout.kaoss_settings_row(52, scroll, 48);
    scene.button(wipe, 0x9d0006);
    scene.text_centered(wipe, "WIPE FX", 0xffffff, 2);

    let show_all = layout.kaoss_settings_row(108, scroll, 48);
    scene.button(
        show_all,
        if model.kaoss_show_all {
            0xd79921
        } else {
            0x3c3836
        },
    );
    scene.text_centered(
        show_all,
        if model.kaoss_show_all {
            "ALL SCALES"
        } else {
            "CURATED"
        },
        0xffffff,
        2,
    );

    let axes = layout.kaoss_settings_half_row(164, scroll, true, 48);
    scene.button(
        axes,
        if model.kaoss_show_axis_labels {
            0x458588
        } else {
            0x3c3836
        },
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
        if model.kaoss_show_grid_lines {
            0x458588
        } else {
            0x3c3836
        },
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
        if model.kaoss_viz_style.is_cells() {
            0x458588
        } else {
            0x3c3836
        },
    );
    scene.text_centered(cells, "CELLS", 0xffffff, 2);
    let glow = layout.kaoss_settings_half_row(232, scroll, false, 48);
    scene.button(
        glow,
        if model.kaoss_viz_style.is_glow() {
            0xb16286
        } else {
            0x3c3836
        },
    );
    scene.text_centered(glow, "GLOW", 0xffffff, 2);

    let color_btn = layout.kaoss_settings_row(288, scroll, 48);
    let color_label = kaoss_viz::pad_color_label(model.kaoss_mono_color);
    let swatch = if kaoss_viz::pad_color_is_rainbow(model.kaoss_mono_color) {
        0xb16286
    } else {
        let (mh, ms) = kaoss_viz::pad_color_hs(model.kaoss_mono_color).unwrap_or((0.93, 0.88));
        kaoss_viz::hsv_color(mh, ms.max(0.35), 0.85)
    };
    scene.button(color_btn, swatch);
    scene.text_centered(color_btn, &format!("COLOR · {color_label}"), 0xffffff, 2);

    scene.text(c.x + 8, c.y + 340 - scroll, "GRID LINES", 0xa89984);
    let grid_row = layout.kaoss_settings_row(356, scroll, 48);
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

    scene.text(c.x + 8, c.y + 408 - scroll, "OUT", 0xa89984);
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
            y: c.y + 424 - scroll + 2,
            w: cell_w - 4,
            h: 44,
        };
        let on = *mode == model.kaoss_out;
        scene.button(r, if on { mode.color() } else { 0x3c3836 });
        scene.text_centered(r, mode.short_label(), 0xffffff, 2);
    }

    scene.text(c.x + 8, c.y + 480 - scroll, "MIDI channel", 0xa89984);
    for ch in 0..16 {
        let cell = layout.kaoss_settings_channel(ch, 496, scroll);
        let on = ch as u8 == model.kaoss_channel;
        scene.button(cell, if on { 0x458588 } else { 0x3c3836 });
        scene.text_centered(cell, &format!("{}", ch + 1), 0xffffff, 2);
    }
}

fn draw_kaoss_color_picker(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    let c = layout.content;
    scene.fill_rect(c, 0x111111);
    scene.text(c.x + 8, c.y + 12, "PAD COLOR", 0xfbf1c7);
    scene.text(c.x + 8, c.y + 32, "applies to CELLS and GLOW", 0xa89984);
    for i in 0..kaoss_viz::pad_color_count() {
        let cell = layout.kaoss_color_pick_cell(i);
        let on = i == model.kaoss_mono_color;
        let fill = if kaoss_viz::pad_color_is_rainbow(i) {
            if on {
                0xb16286
            } else {
                0x504945
            }
        } else {
            let (h, s) = kaoss_viz::pad_color_hs(i).unwrap_or((0.93, 0.88));
            let v = if on { 0.92 } else { 0.55 };
            kaoss_viz::hsv_color(h, s.max(0.35), v)
        };
        scene.button(cell, fill);
        let label = kaoss_viz::pad_color_label(i);
        scene.text_centered(cell, label, 0xffffff, 2);
    }
    let done = layout.kaoss_color_done();
    scene.button(done, 0x458588);
    scene.text_centered(done, "DONE", 0xffffff, 2);
}

fn draw_kaoss_glow(scene: &mut Scene, pad: crate::layout::Rect, model: &NativeModel) {
    let t = model.kaoss_viz_time();
    let pulse = kaoss_viz::viz_pulse(t, model.bpm, model.kaoss_gate_flash());
    let hold = model.kaoss_hold && model.kaoss_touching;
    let amp_max = model.kaoss_glow_amp();
    let color_idx = model.kaoss_mono_color;
    let wash_hue = kaoss_viz::glow_ring_hue(color_idx, 0.35, t);
    let wash_sat = kaoss_viz::glow_ring_sat(color_idx) * 0.6;

    // Mostly dark membrane with a faint under-wash (idle or held).
    let wash_v = (0.025 + 0.04 * pulse + if hold { 0.02 } else { 0.0 })
        * (0.55 + 0.45 * amp_max);
    scene.fill_rect(pad, kaoss_viz::hsv_color(wash_hue, wash_sat * 0.55, wash_v));

    let pad_w = pad.w as f32;
    let pad_h = pad.h as f32;
    let cols = kaoss_viz::GLOW_FIELD_COLS;
    let rows = kaoss_viz::GLOW_FIELD_ROWS;
    let cell_w = pad_w / cols as f32;
    let cell_h = pad_h / rows as f32;
    let touches = &model.kaoss_glow;

    for row in 0..rows {
        // row 0 at bottom of pad (matches CELLS / finger Y = 0 at bottom).
        let ny = (row as f32 + 0.5) / rows as f32;
        let py = pad.y as f32 + pad_h - (row as f32 + 1.0) * cell_h;
        for col in 0..cols {
            let nx = (col as f32 + 0.5) / cols as f32;
            let field = kaoss_viz::glow_field_at(nx, ny, pad_w, pad_h, touches);
            if field < 0.04 {
                continue;
            }
            let hue = kaoss_viz::glow_ring_hue(color_idx, field, t);
            let color = kaoss_viz::glow_sample(hue, 1.0, field, pulse);
            let px = pad.x as f32 + col as f32 * cell_w;
            scene.fill(px, py, cell_w.max(1.0), cell_h.max(1.0), color);
        }
    }
}

fn draw_kaoss_axes(scene: &mut Scene, pad: crate::layout::Rect, model: &NativeModel) {
    let prog = kaoss_ui::program(model.kaoss_program);
    if model.kaoss_show_grid_lines {
        draw_kaoss_grid(scene, pad, model, prog.note);
    }
    if model.kaoss_show_axis_labels {
        let bend = prog.y_param == "pitch_bend";
        let y_label = match prog.y_param {
            "tone" => "Y TONE",
            "morph" => "Y MORPH",
            "vib" | "vibrato_always" => "Y VIB",
            "tone_lfo" => "Y RATE",
            "level" => "Y LEVEL",
            "release" => "Y DECAY",
            "attack" => "Y ATTACK",
            "delay_mix" => "Y DLY MIX",
            "reverb_mix" => "Y REVERB",
            "octave" => "Y OCTAVE",
            "drive" => "Y DRIVE",
            "delay_fb" => "Y FB",
            "reverb_size" => "Y SIZE",
            "delay_time" => "Y DLY T",
            "pitch_bend" => "Y BEND",
            "flanger_mix" => "Y FLANGE",
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
                Some("flanger_rate") => "X RATE",
                _ => "X",
            }
        };
        scene.text_scaled(pad.x + 6, pad.y + pad.h / 2 - 20, y_label, 0xd3869b, 2);
        // BEND: pitch lives on the horizontal midline (not the bottom edge).
        // Keep X label well inside the pad — glyphs render after button fills.
        let x_label_y = if bend {
            pad.y + pad.h / 2 - 10
        } else {
            pad.y + pad.h - 52
        };
        scene.text_scaled(pad.x + pad.w / 2 - 40, x_label_y, x_label, 0xd3869b, 2);
    }
}

/// Scale / Y play-grid overlay (Tk `_kaoss_draw_grid` parity).
fn draw_kaoss_grid(
    scene: &mut Scene,
    pad: crate::layout::Rect,
    model: &NativeModel,
    note_program: bool,
) {
    let regular = model.kaoss_grid_width.clamp(1, 6) as f32;
    let octave_w = regular + 1.0;

    // Horizontal guides at 25 / 50 / 75% of pad Y (brighter midline).
    for &(frac, color) in &[(0.25_f32, 0x3a1528_u32), (0.50, 0x5b203c), (0.75, 0x3a1528)] {
        let stroke = if (frac - 0.5).abs() < 0.001 {
            octave_w
        } else {
            regular
        };
        let y = pad.y as f32 + pad.h as f32 * (1.0 - frac);
        scene.fill(pad.x as f32, y - stroke * 0.5, pad.w as f32, stroke, color);
    }

    if note_program {
        let scale = jambox_core::kaoss_scale(model.kaoss_scale_index as usize);
        let notes = jambox_core::scale_notes(
            scale.degrees,
            model.kaoss_key,
            match model.kaoss_octaves {
                1 | 2 => 48,
                3 => 36,
                _ => 24,
            },
            model.kaoss_octaves,
        );
        let n = notes
            .iter()
            .rposition(|&n| n != 0)
            .map(|i| i + 1)
            .unwrap_or(1)
            .max(1);
        let key = model.kaoss_key % 12;
        // Equal-width note cell edges (N notes → N+1 lines), roots brighter.
        for i in 0..=n {
            let frac = i as f32 / n as f32;
            let x = pad.x as f32 + pad.w as f32 * frac;
            let octave = i < n && (notes[i] % 12) == key;
            let (color, stroke) = if octave {
                (0xfb4934_u32, octave_w)
            } else {
                (0x4a2040_u32, regular)
            };
            scene.fill(x - stroke * 0.5, pad.y as f32, stroke, pad.h as f32, color);
        }
    } else {
        // FX programs: same 25/50/75% vertical guides as the Y axis.
        for &frac in &[0.25_f32, 0.50, 0.75] {
            let stroke = if (frac - 0.5).abs() < 0.001 {
                octave_w
            } else {
                regular
            };
            let x = pad.x as f32 + pad.w as f32 * frac;
            scene.fill(
                x - stroke * 0.5,
                pad.y as f32,
                stroke,
                pad.h as f32,
                0x3a1528,
            );
        }
    }
}

/// Fixed note HUD (chord-mode style) — never sits under the finger.
fn draw_kaoss_note_readout(
    scene: &mut Scene,
    pad: crate::layout::Rect,
    model: &NativeModel,
) {
    let Some(fx) = model.kaoss_sounding_x() else {
        return;
    };
    let label = kaoss_ui::midi_note_label(model.kaoss_note_at(fx));
    // Top-right of the pad: clear of Y-axis labels (left) and typical play area.
    let w = 104;
    let h = 40;
    let r = Rect {
        x: pad.x + pad.w - w - 8,
        y: pad.y + 8,
        w,
        h,
    };
    scene.fill_rect(r, 0x1d2021);
    scene.text_centered(r, &label, 0xfbf1c7, 3);
}

fn draw_pads(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.text(16, HUD_H + 16, "Phrase Pads", 0xfbf1c7);
    scene.fill_rect(
        layout.pads_play,
        if !model.pads_edit { 0x689d6a } else { 0x3c3836 },
    );
    scene.text_centered(layout.pads_play, "PLAY", 0xffffff, 2);
    scene.fill_rect(
        layout.pads_edit,
        if model.pads_edit { 0xd79921 } else { 0x3c3836 },
    );
    scene.text_centered(layout.pads_edit, "EDIT", 0xffffff, 2);

    if model.pads_edit {
        scene.fill_rect(
            layout.pads_clear,
            if model.pads_clear_armed {
                0xcc241d
            } else {
                0x9d0006
            },
        );
        scene.text_centered(
            layout.pads_clear,
            if model.pads_clear_armed {
                "CLR?"
            } else {
                "CLEAR"
            },
            0xffffff,
            2,
        );
        let trig = if model.phrases[model.pads_selected.min(15)].loop_mode {
            "LOOP"
        } else {
            "ONE"
        };
        scene.fill_rect(layout.pads_trig, 0x458588);
        scene.text_centered(layout.pads_trig, trig, 0xffffff, 2);
        scene.fill_rect(
            layout.pads_mode,
            if model.pads_mode_armed {
                0xd79921
            } else {
                0x504945
            },
        );
        scene.text_centered(layout.pads_mode, "MODE", 0xffffff, 2);
        let pad = &model.phrases[model.pads_selected.min(15)];
        let voice_bg = if pad.voice_locked { 0xb16286 } else { 0x689d6a };
        let voice_label = if pad.voice_locked { "LOCK" } else { "FLW" };
        scene.fill_rect(layout.pads_voice, voice_bg);
        scene.text_centered(layout.pads_voice, voice_label, 0xffffff, 2);
        let synth_bg = if pad.local_synth { 0xd65d0e } else { 0x504945 };
        let synth_label = if pad.local_synth { "SYN" } else { "MIDI" };
        scene.fill_rect(layout.pads_synth, synth_bg);
        scene.text_centered(layout.pads_synth, synth_label, 0xffffff, 2);
        let ch_label = if pad.out_channel < 0 {
            "CH*".to_string()
        } else {
            format!("CH{}", pad.out_channel + 1)
        };
        scene.fill_rect(layout.pads_channel, 0x504945);
        scene.text_centered(layout.pads_channel, &ch_label, 0xffffff, 2);
        let rec_on = model.pads_recording.is_some();
        scene.fill_rect(layout.pads_rec, if rec_on { 0xcc241d } else { 0x9d0006 });
        scene.text_centered(
            layout.pads_rec,
            if rec_on { "STOP" } else { "REC" },
            0xffffff,
            2,
        );
        scene.fill_rect(layout.pads_vol_down, 0x3c3836);
        scene.text_centered(layout.pads_vol_down, "V-", 0xffffff, 2);
        scene.fill_rect(layout.pads_vol_up, 0x3c3836);
        scene.text_centered(layout.pads_vol_up, "V+", 0xffffff, 2);
    }

    if model.seq_to_pad_armed {
        scene.text(16, HUD_H + 40, "PAD armed - tap a slot", 0xfabd2f);
    }

    if layout.pads_out.w > 0 {
        scene.fill_rect(layout.pads_out, model.pads_out.color());
        scene.text_centered(layout.pads_out, model.pads_out.short_label(), 0xffffff, 2);
    }

    scene.fill_rect(layout.stop_all, 0x3c3836);
    scene.text_centered(layout.stop_all, "STOP", 0xffffff, 2);

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
        scene.text_centered(
            Rect {
                x: cell.x,
                y: cell.y + 8,
                w: cell.w,
                h: 20,
            },
            &label,
            0xfbf1c7,
            2,
        );
        if !pad.empty {
            scene.text_centered(
                Rect {
                    x: cell.x,
                    y: cell.y + 30,
                    w: cell.w,
                    h: 18,
                },
                if pad.loop_mode { "LOOP" } else { "ONE" },
                0xd5c4a1,
                1,
            );
        }
    }
}

fn draw_home(scene: &mut Scene, model: &NativeModel) {
    use crate::layout::HOME_TILES;

    let layout = model.layout;
    scene.text_scaled(
        layout.content.x + 12,
        layout.content.y + 10,
        "Home",
        0xfbf1c7,
        3,
    );
    scene.text_scaled(
        layout.content.x + layout.content.w - 160,
        layout.content.y + 14,
        "tap a mode · drag",
        0x928374,
        2,
    );
    for (i, (_mode, title, color)) in HOME_TILES.iter().enumerate() {
        let cell = layout.home_tile(i, model.home_scroll);
        scene.button(cell, *color);
        scene.text_centered(cell, title, 0xfbf1c7, 2);
    }
}

fn draw_chords(scene: &mut Scene, model: &NativeModel) {
    use crate::chords::{self, Overlay, QualityRow, KEY_NAMES, PALETTE_SLOTS, ROOT_NAMES};

    let layout = model.layout;
    if let Some(overlay) = model.chords_overlay {
        let title = match overlay {
            Overlay::Key => "KEY",
            Overlay::Changes => "CHANGES",
        };
        scene.text_scaled(
            layout.content.x + 12,
            layout.content.y + 14,
            title,
            0xfbf1c7,
            3,
        );
        scene.button(layout.chords_overlay_close(), 0x9d0006);
        scene.text_centered(layout.chords_overlay_close(), "CLOSE", 0xfbf1c7, 2);
        match overlay {
            Overlay::Key => {
                for pc in 0..12u8 {
                    let cell = layout.chords_overlay_cell(pc as usize, 12);
                    let on = model.chords_key == pc;
                    scene.button(cell, if on { 0xcc241d } else { 0x3c3836 });
                    scene.text_centered(cell, KEY_NAMES[pc as usize], 0xfbf1c7, 2);
                }
            }
            Overlay::Changes => {
                let n = chords::PROGRESSIONS.len();
                for (i, prog) in chords::PROGRESSIONS.iter().enumerate() {
                    let cell = layout.chords_overlay_cell(i, n);
                    scene.button(cell, 0x504945);
                    scene.text(cell.x + 8, cell.y + 10, prog.name, 0xfbf1c7);
                    scene.text(cell.x + 8, cell.y + 32, prog.label, 0xa89984);
                }
            }
        }
        return;
    }

    let tools = [
        model.chords_out.short_label(),
        if model.chords_hold { "HOLD" } else { "MOM" },
        KEY_NAMES[model.chords_key as usize],
        "CHANGES",
        if model.chords_arm { "ARM*" } else { "ARM" },
    ];
    let tool_colors = [
        model.chords_out.color(),
        if model.chords_hold {
            0xb16286
        } else {
            0x3c3836
        },
        0x458588,
        0xd79921,
        if model.chords_arm { 0xcc241d } else { 0x3c3836 },
    ];
    for i in 0..5 {
        let cell = layout.chords_tool(i);
        scene.button(cell, tool_colors[i]);
        scene.text_centered(cell, tools[i], 0xfbf1c7, 2);
    }

    scene.fill_rect(layout.chords_root_strip, 0x111111);
    for col in 0..12 {
        let label = layout.chords_root_label(col);
        scene.text_centered(label, ROOT_NAMES[col], 0xa89984, 1);
    }
    let held = model.chords_held_buttons();
    for row in 0..3 {
        let qrow = QualityRow::from_index(row).unwrap();
        for col in 0..12 {
            let cell = layout.chords_button(col, row);
            let lit = if !held.is_empty() {
                held.iter().any(|&(c, r)| c == col && r == qrow)
            } else {
                model
                    .chords_current
                    .map(|spec| {
                        chords::lit_buttons_for_chord(spec)
                            .iter()
                            .any(|&(c, r)| c == col && r == qrow)
                    })
                    .unwrap_or(false)
            };
            let color = match (qrow, lit) {
                (QualityRow::Maj, true) => 0xcc241d,
                (QualityRow::Maj, false) => 0x9d0006,
                (QualityRow::Min, true) => 0x458588,
                (QualityRow::Min, false) => 0x076678,
                (QualityRow::Seven, true) => 0xd79921,
                (QualityRow::Seven, false) => 0xb57614,
            };
            scene.button(cell, color);
            scene.text_centered(cell, qrow.label(), 0xfbf1c7, 1);
        }
    }

    scene.fill_rect(layout.chords_strum, 0x1d2021);
    scene.button(layout.chords_oct_down(), 0x504945);
    scene.text_centered(layout.chords_oct_down(), "OCT-", 0xffffff, 1);
    scene.button(layout.chords_oct_up(), 0x504945);
    scene.text_centered(layout.chords_oct_up(), "OCT+", 0xffffff, 1);
    let oct_label = {
        let midi = chords::block_base_for_octave(model.chords_octave);
        let octave = (midi as i32 / 12) - 1;
        format!("C{octave}")
    };
    scene.text_centered(layout.chords_oct_label(), &oct_label, 0xfbf1c7, 2);
    let play = layout.chords_strum_play();
    if let Some(spec) = model.chords_current {
        scene.text(play.x + 8, play.y + 2, &spec.name(), 0xfe8019);
    }
    let strings = chords::STRUM_STRINGS;
    let strum_notes = model.chords_current.map(|spec| {
        spec.strum_strings_at(chords::strum_base_for_octave(model.chords_octave))
    });
    let band_h = (play.h - chords::STRUM_BAND_TOP_INSET - chords::STRUM_BAND_BOTTOM_INSET).max(1)
        / strings as i32;
    for i in 0..strings {
        let i = i as i32;
        let y = play.y + chords::STRUM_BAND_TOP_INSET + i * band_h;
        scene.fill_rect(
            Rect {
                x: play.x + 12,
                y,
                w: play.w - 24,
                h: 3,
            },
            if i % 4 == 0 { 0xebdbb2 } else { 0x504945 },
        );
        if let Some(ref notes) = strum_notes {
            // Band 0 is top (highest note); band strings-1 is bottom (lowest).
            let idx = strings - 1 - i as usize;
            let label = crate::kaoss_ui::midi_note_label(notes[idx]);
            scene.text(play.x + play.w - 44, y - 2, &label, 0xa89984);
        }
    }

    scene.text(
        layout.chords_palette.x + 4,
        layout.chords_palette.y + 2,
        "PALETTE",
        0xa89984,
    );
    for slot in 0..PALETTE_SLOTS {
        let cell = layout.chords_palette_slot(slot);
        match model.chords_palette[slot] {
            Some(spec) => {
                let on = model.chords_current == Some(spec);
                scene.button(cell, if on { 0xcc241d } else { 0x689d6a });
                scene.text_centered(cell, &spec.name(), 0xfbf1c7, 1);
            }
            None => {
                scene.button(cell, 0x3c3836);
                scene.text_centered(cell, "+", 0x665c54, 2);
            }
        }
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

fn draw_synth_vib(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.content, 0x111111);
    scene.text(24, HUD_H + 8, "VIBRATO", 0xfbf1c7);
    scene.text(
        200,
        HUD_H + 10,
        "depth · rate · wheel / always-on",
        0xa89984,
    );

    let always = layout.synth_vib_always();
    let on = model.vibrato_always > 0.01;
    scene.button(always, if on { 0xb16286 } else { 0x3c3836 });
    scene.text_centered(
        always,
        if on { "ON - ALWAYS" } else { "WHEEL - MOD" },
        0xffffff,
        2,
    );

    let depth_down = layout.synth_vib_depth_down();
    let depth_label = layout.synth_vib_depth_label();
    let depth_up = layout.synth_vib_depth_up();
    scene.button(depth_down, 0x504945);
    scene.text_centered(depth_down, "DEPTH -", 0xffffff, 2);
    scene.fill_rect(depth_label, 0x1d2021);
    scene.text_centered(
        depth_label,
        &format!("{:.2} st", model.vibrato_depth),
        0xfbf1c7,
        2,
    );
    scene.button(depth_up, 0x504945);
    scene.text_centered(depth_up, "DEPTH +", 0xffffff, 2);

    let rate_down = layout.synth_vib_rate_down();
    let rate_label = layout.synth_vib_rate_label();
    let rate_up = layout.synth_vib_rate_up();
    scene.button(rate_down, 0x504945);
    scene.text_centered(rate_down, "RATE -", 0xffffff, 2);
    scene.fill_rect(rate_label, 0x1d2021);
    scene.text_centered(
        rate_label,
        &format!("{:.1} Hz", model.vibrato_rate),
        0xfbf1c7,
        2,
    );
    scene.button(rate_up, 0x504945);
    scene.text_centered(rate_up, "RATE +", 0xffffff, 2);

    scene.fill_rect(layout.synth_pick_done, 0x458588);
    scene.text_centered(layout.synth_pick_done, "DONE", 0xffffff, 2);
}

fn draw_synth(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;

    if model.synth_vib_open {
        draw_synth_vib(scene, model);
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

    const LABELS: [&str; 6] = ["MORPH", "TONE", "LEVEL", "ATK", "REL", "FLANGE"];

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
    scene.text(
        layout.synth_swap.x + 16,
        layout.synth_swap.y + 16,
        "SWAP",
        0xffffff,
    );
    let vib_on = model.vibrato_always > 0.01;
    scene.fill_rect(layout.synth_vib, if vib_on { 0xb16286 } else { 0x458588 });
    scene.text_centered(layout.synth_vib, "VIB", 0xffffff, 2);
    scene.fill_rect(layout.synth_save_as, 0x689d6a);
    scene.text_centered(layout.synth_save_as, "SAVE", 0xffffff, 2);
    scene.fill_rect(layout.synth_oct_down, 0x504945);
    scene.text_centered(layout.synth_oct_down, "OCT-", 0xffffff, 2);
    scene.fill_rect(layout.synth_oct_up, 0x504945);
    scene.text_centered(layout.synth_oct_up, "OCT+", 0xffffff, 2);
    let oct_label = {
        let midi = (60i16 + model.synth_octave as i16 * 12).clamp(24, 108) as u8;
        let octave = (midi as i32 / 12) - 1;
        format!("C{octave}")
    };
    scene.text(
        24,
        HUD_H + 64,
        &format!(
            "{} · vib {:.2}st · {:.1}Hz · {}",
            oct_label,
            model.vibrato_depth,
            model.vibrato_rate,
            if vib_on { "ON" } else { "WHEEL" }
        ),
        0xa89984,
    );

    // CRT-ish morph scope (live A/B blend from the loaded wave bank).
    draw_synth_scope(scene, model);

    for index in 0..6 {
        let track = layout.synth_slider(index);
        scene.fill_rect(track, 0x20202c);
        scene.text(track.x + 8, track.y - 18, LABELS[index], 0xc0c0d0);
        let value = if index < 5 {
            model.synth_params[index]
        } else {
            model.fx_voice[3]
        };
        let fill_h = (track.h as f32 * value) as i32;
        let fill = Rect {
            x: track.x + 4,
            y: track.y + track.h - fill_h,
            w: track.w - 8,
            h: fill_h.max(2),
        };
        scene.fill_rect(
            fill,
            if index == 0 {
                0xb16286
            } else if index == 5 {
                0xd79921
            } else {
                0x689d6a
            },
        );
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
        scene.fill(rect.x as f32, y as f32, rect.w as f32, 1.0, 0x2a2a18);
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

fn draw_fm(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    for index in 0..jambox_core::FM_RECIPE_COUNT {
        let rec = jambox_core::fm_recipe(index);
        let cell = layout.fm_recipe_cell(index);
        let on = index == model.fm_recipe;
        scene.fill_rect(cell, if on { 0x8ec07c } else { 0x3c3836 });
        scene.text_centered(cell, rec.label, if on { 0x1d2021 } else { 0xfbf1c7 }, 1);
    }

    let hint = layout.fm_hint();
    scene.text(
        hint.x,
        hint.y + 4,
        jambox_core::fm_recipe(model.fm_recipe).hint,
        0xa89984,
    );
    scene.fill_rect(layout.fm_clear(), 0x9d0006);
    scene.text_centered(layout.fm_clear(), "CLEAR", 0xfbf1c7, 1);
    scene.fill_rect(layout.fm_oct_down(), 0x504945);
    scene.text_centered(layout.fm_oct_down(), "OCT-", 0xfbf1c7, 1);
    scene.fill_rect(layout.fm_oct_up(), 0x504945);
    scene.text_centered(layout.fm_oct_up(), "OCT+", 0xfbf1c7, 1);

    draw_fm_graph(scene, model);
    draw_fm_scope(scene, model);

    const LABELS: [&str; 4] = ["RATIO", "OUT", "FOLD", "ENV"];
    for index in 0..4 {
        let track = layout.fm_slider(index);
        scene.fill_rect(track, 0x20202c);
        scene.text(track.x, track.y - 16, LABELS[index], 0xc0c0d0);
        let fill_h = (track.h as f32 * model.fm_params[index]) as i32;
        let fill = Rect {
            x: track.x + 3,
            y: track.y + track.h - fill_h,
            w: track.w - 6,
            h: fill_h.max(2),
        };
        scene.fill_rect(fill, if index == 2 { 0xfe8019 } else { 0x8ec07c });
    }
    scene.text(
        layout.fm_slider(0).x,
        layout.fm_slider(0).y + layout.fm_slider(0).h + 4,
        jambox_core::clang_label(model.fm_params[0]),
        0xa89984,
    );

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

fn draw_fm_graph(scene: &mut Scene, model: &NativeModel) {
    let rect = model.layout.fm_graph();
    scene.fill_rect(rect, 0x1a1a12);
    scene.text(rect.x + 8, rect.y + 6, "draw one into another", 0xa89984);

    let r = Layout::FM_OP_RADIUS as f32;
    for src in 0..jambox_core::FM_OP_COUNT {
        for dst in 0..jambox_core::FM_OP_COUNT {
            let amount = model.fm_matrix[src][dst];
            if amount < 0.04 {
                continue;
            }
            let color = jambox_core::OP_COLORS[src];
            if src == dst {
                let (cx, cy) = model.layout.fm_op_center(src);
                scene.stroke_disc(
                    cx as f32,
                    cy as f32,
                    r + 6.0 + amount * 10.0,
                    3.0,
                    color,
                    0x1a1a12,
                );
            } else {
                let (x0, y0) = model.layout.fm_op_center(src);
                let (x1, y1) = model.layout.fm_op_center(dst);
                stroke_fm_link(
                    scene, x0 as f32, y0 as f32, x1 as f32, y1 as f32, r, amount, color,
                );
            }
        }
    }

    if let Some((from, px, py)) = model.fm_drag() {
        let (x0, y0) = model.layout.fm_op_center(from);
        stroke_scope_seg(
            scene,
            x0 as f32,
            y0 as f32,
            px as f32,
            py as f32,
            jambox_core::OP_COLORS[from],
        );
        scene.fill_disc(px as f32, py as f32, 5.0, jambox_core::OP_COLORS[from]);
    }

    for index in 0..jambox_core::FM_OP_COUNT {
        let (cx, cy) = model.layout.fm_op_center(index);
        let op = model.fm_ops[index];
        let heard = op.audio > 0.05;
        let selected = index == model.fm_selected;
        let radius = r + if selected { 4.0 } else { 0.0 } + op.fold * 4.0;
        let color = jambox_core::OP_COLORS[index];
        if selected {
            scene.fill_disc(cx as f32, cy as f32, radius + 5.0, 0xfbf1c7);
        }
        scene.fill_disc(cx as f32, cy as f32, radius, color);
        if heard {
            scene.fill_disc(cx as f32, cy as f32, radius * 0.42, 0x1d2021);
        }
        let label = Rect {
            x: cx - 10,
            y: cy - 8,
            w: 20,
            h: 16,
        };
        scene.text_centered(
            label,
            jambox_core::OP_NAMES[index],
            if heard { 0xfbf1c7 } else { 0x1d2021 },
            1,
        );
    }
}

fn stroke_fm_link(
    scene: &mut Scene,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    radius: f32,
    amount: f32,
    color: u32,
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = dx.hypot(dy).max(1.0);
    let ux = dx / len;
    let uy = dy / len;
    let sx = x0 + ux * (radius + 2.0);
    let sy = y0 + uy * (radius + 2.0);
    let ex = x1 - ux * (radius + 4.0);
    let ey = y1 - uy * (radius + 4.0);
    let mx = (sx + ex) * 0.5 - uy * 28.0;
    let my = (sy + ey) * 0.5 + ux * 28.0;
    let steps = ((len * 0.7) as i32).clamp(12, 48);
    let mut prev_x = sx;
    let mut prev_y = sy;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let u = 1.0 - t;
        let x = u * u * sx + 2.0 * u * t * mx + t * t * ex;
        let y = u * u * sy + 2.0 * u * t * my + t * t * ey;
        let w = 2.0 + amount * 3.0;
        scene.fill(x - w * 0.5, y - w * 0.5, w, w, color);
        stroke_scope_seg(scene, prev_x, prev_y, x, y, color);
        prev_x = x;
        prev_y = y;
    }
    scene.fill(ex - 3.0, ey - 3.0, 7.0, 7.0, color);
}

fn draw_fm_scope(scene: &mut Scene, model: &NativeModel) {
    let rect = model.layout.fm_scope();
    scene.fill_rect(rect, 0x1a1a12);
    for i in 1..4 {
        let y = rect.y + (rect.h * i) / 4;
        scene.fill(rect.x as f32, y as f32, rect.w as f32, 1.0, 0x2a2a18);
    }
    let patch = model.fm_patch();
    let n = (rect.w / 2).max(48) as usize;
    let mut cycle = vec![0.0f32; n];
    jambox_core::FmSynth::preview_cycle(patch, &mut cycle);
    let mid_y = rect.y as f32 + rect.h as f32 * 0.5;
    let amp = (rect.h as f32 * 0.42).max(4.0);
    let mut prev_x = rect.x as f32 + 2.0;
    let mut prev_y = mid_y;
    for (i, sample) in cycle.iter().enumerate() {
        let x = rect.x as f32 + 2.0 + (i as f32) * ((rect.w - 4) as f32) / n as f32;
        let y = mid_y - sample * amp;
        stroke_scope_seg(scene, prev_x, prev_y, x, y, 0x8ec07c);
        prev_x = x;
        prev_y = y;
    }
}

fn draw_drums(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    if model.kit_edit_open {
        draw_kit_edit(scene, model);
        return;
    }
    if model.kit_repeat_open {
        draw_kit_repeat(scene, model);
        return;
    }

    scene.text(24, HUD_H + 8, "DRUM KIT", 0xfbf1c7);

    let sel_cell = phrases::PHRASE_GRID_CELLS[model.kit_selected];
    let sel_note = phrases::mpk_note_for_phrase_cell(sel_cell);
    let model_name = drum_model_for_note(sel_note).name().replace('_', " ");
    let macros = model.edit_drum_macros();
    let target = if model.kit_all_drums {
        "ALL DRUMS".to_string()
    } else {
        format!("{} {}", phrases::pad_label(sel_cell), model_name)
    };
    let status = format!(
        "{}  T{:.0} S{:.0} P{:.0} D{:.0}",
        target,
        macros[0] * 100.0,
        macros[1] * 100.0,
        macros[2] * 100.0,
        macros[3] * 100.0,
    );
    scene.text_centered(
        Rect {
            x: 160,
            y: HUD_H + 6,
            w: 480,
            h: 24,
        },
        &status,
        0xfabd2f,
        2,
    );

    for screen_index in 0..16 {
        let cell = layout.kit_pad_cell(screen_index);
        let phrase_cell = phrases::PHRASE_GRID_CELLS[screen_index];
        let note = phrases::mpk_note_for_phrase_cell(phrase_cell);
        let voice = drum_model_for_note(note).name().replace('_', " ");
        let repeat = model.drum_repeat_for_note(note);
        let selected = !model.kit_all_drums && screen_index == model.kit_selected;
        let bg = if selected {
            0xd79921
        } else if repeat.is_on() {
            0x365a5c
        } else {
            0x3c3836
        };
        scene.fill_rect(cell, bg);
        scene.text_centered(
            Rect {
                x: cell.x,
                y: cell.y + 4,
                w: cell.w,
                h: 18,
            },
            &phrases::pad_label(phrase_cell),
            0xfbf1c7,
            2,
        );
        scene.text_centered(
            Rect {
                x: cell.x + 2,
                y: cell.y + cell.h / 2 - 2,
                w: cell.w - 4,
                h: 18,
            },
            &voice,
            0xa89984,
            1,
        );
        if let Some(label) = repeat.pad_label() {
            scene.text_centered(
                Rect {
                    x: cell.x + 2,
                    y: cell.y + cell.h - 20,
                    w: cell.w - 4,
                    h: 16,
                },
                label,
                if selected { 0xfbf1c7 } else { 0x83c07c },
                1,
            );
        }
    }

    let repeat_bg = if model.edit_drum_repeat().is_on() && !model.edit_drum_repeat_mixed() {
        0x458588
    } else if model.edit_drum_repeat_mixed() {
        0xb16286
    } else {
        0x504945
    };
    scene.fill_rect(layout.kit_note_repeat, repeat_bg);
    scene.text_centered(
        layout.kit_note_repeat,
        &model.note_repeat_button_label(),
        0xfbf1c7,
        2,
    );

    scene.fill_rect(layout.kit_wave, 0x689d6a);
    scene.text_centered(layout.kit_wave, "WAVE", 0xfbf1c7, 2);

    let all_bg = if model.kit_all_drums {
        0xb16286
    } else {
        0x504945
    };
    scene.fill_rect(layout.kit_all, all_bg);
    scene.text_centered(layout.kit_all, "ALL DRUMS", 0xfbf1c7, 2);
}

fn draw_kit_edit(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    let macros = model.edit_drum_macros();
    let title = if model.kit_all_drums {
        "ALL DRUMS".to_string()
    } else {
        let cell = phrases::PHRASE_GRID_CELLS[model.kit_selected];
        format!(
            "{} {}",
            phrases::pad_label(cell),
            model.selected_drum_model().name().replace('_', " ")
        )
    };
    scene.text(24, HUD_H + 8, "DRUM WAVE", 0xfbf1c7);
    scene.text_centered(
        Rect {
            x: 160,
            y: HUD_H + 6,
            w: 480,
            h: 24,
        },
        &title,
        0xfabd2f,
        2,
    );

    draw_scope_wave(scene, layout.kit_edit_scope(), &model.kit_wave);

    const LABELS: [&str; 4] = ["TONE", "SNAP", "PITCH", "DECAY"];
    for index in 0..4 {
        let track = layout.kit_edit_slider(index);
        scene.fill_rect(track, 0x20202c);
        scene.text(track.x + 8, track.y - 18, LABELS[index], 0xc0c0d0);
        let fill_h = (track.h as f32 * macros[index]) as i32;
        let fill = Rect {
            x: track.x + 4,
            y: track.y + track.h - fill_h,
            w: track.w - 8,
            h: fill_h.max(2),
        };
        scene.fill_rect(fill, 0x689d6a);
    }

    scene.fill_rect(layout.kit_edit_play(), 0x458588);
    scene.text_centered(layout.kit_edit_play(), "PLAY", 0xffffff, 2);
    scene.text(
        layout.kit_edit_play().x + 256,
        layout.kit_edit_play().y + 18,
        "BACK returns to pads",
        0xa89984,
    );
}

fn draw_kit_repeat(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    let title = if model.kit_all_drums {
        "ALL DRUMS".to_string()
    } else {
        let cell = phrases::PHRASE_GRID_CELLS[model.kit_selected];
        format!(
            "{} {}",
            phrases::pad_label(cell),
            model.selected_drum_model().name().replace('_', " ")
        )
    };
    scene.text(24, HUD_H + 8, "NOTE REPEAT", 0xfbf1c7);
    scene.text_centered(
        Rect {
            x: 200,
            y: HUD_H + 6,
            w: 440,
            h: 24,
        },
        &title,
        0xfabd2f,
        2,
    );

    let current = model.edit_drum_repeat();
    let mixed = model.edit_drum_repeat_mixed();
    for (index, choice) in RepeatDivisionChoice::ALL.iter().enumerate() {
        let cell = layout.kit_repeat_choice_cell(index);
        let active = !mixed && *choice == current;
        scene.fill_rect(cell, if active { 0x458588 } else { 0x3c3836 });
        scene.text_centered(cell, choice.label(), 0xffffff, 2);
    }

    scene.text(
        24,
        HUD_H + 368,
        "OFF is one-shot · hold a pad to repeat",
        0xa89984,
    );
    scene.text(24, HUD_H + 392, "BACK returns to pads", 0xa89984);
}

fn draw_scope_wave(scene: &mut Scene, rect: Rect, samples: &[f32]) {
    scene.fill_rect(rect, 0x1a1a12);
    for i in 1..4 {
        let y = rect.y + (rect.h * i) / 4;
        scene.fill(rect.x as f32, y as f32, rect.w as f32, 1.0, 0x2a2a18);
    }
    if samples.iter().all(|s| *s == 0.0) {
        scene.text(rect.x + 24, rect.y + rect.h / 2 - 4, "NO WAVE", 0x504945);
        return;
    }
    let n = samples.len().max(2);
    let mid_y = rect.y as f32 + rect.h as f32 * 0.5;
    let amp = (rect.h as f32 * 0.42).max(4.0);
    let mut prev_x = rect.x as f32 + 2.0;
    let mut prev_y = mid_y;
    for (i, s) in samples.iter().enumerate() {
        let x = rect.x as f32 + 2.0 + (i as f32) * ((rect.w - 4) as f32) / (n - 1) as f32;
        let y = mid_y - *s * amp;
        if i > 0 {
            stroke_scope_seg(scene, prev_x, prev_y, x, y, 0xb8bb26);
        }
        prev_x = x;
        prev_y = y;
    }
}

fn draw_seq(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    let seq = &model.seq;

    scene.text(16, HUD_H + 10, "Sequencer", 0xfbf1c7);
    scene.text(160, HUD_H + 12, &format!("{:.0} BPM", seq.bpm), 0xa89984);
    scene.text(16, HUD_H + 30, &seq.status, 0xfabd2f);
    scene.text(16, HUD_H + 44, &seq.layer_line, 0x83a598);

    let (rec_label, rec_bg) = seq.rec_label();
    let (play_label, play_bg) = seq.play_label();
    scene.fill_rect(layout.seq_rec, rec_bg);
    scene.text_centered(layout.seq_rec, rec_label, 0xffffff, 2);
    scene.fill_rect(layout.seq_play, play_bg);
    scene.text_centered(layout.seq_play, play_label, 0xffffff, 2);

    let pending = seq.has_pending();
    let keep_bg = if pending { 0x458588 } else { 0x3c3836 };
    let drop_bg = if pending { 0x665c54 } else { 0x3c3836 };
    let undo_bg = if seq.layer_count() > 1 {
        0x665c54
    } else {
        0x3c3836
    };
    scene.fill_rect(layout.seq_keep, keep_bg);
    scene.text_centered(layout.seq_keep, "KEEP", 0xffffff, 2);
    scene.fill_rect(layout.seq_drop, drop_bg);
    scene.text_centered(layout.seq_drop, "DROP", 0xffffff, 2);
    scene.fill_rect(layout.seq_undo, undo_bg);
    scene.text_centered(layout.seq_undo, "UNDO", 0xffffff, 2);

    scene.fill_rect(layout.seq_len_double, 0x504945);
    scene.text_centered(layout.seq_len_double, "LEN x2", 0xffffff, 2);
    scene.fill_rect(layout.seq_len_halve, 0x504945);
    scene.text_centered(layout.seq_len_halve, "LEN /2", 0xffffff, 2);
    let extend_bg = if seq.extend_mode { 0x689d6a } else { 0x3c3836 };
    let extend_label = if seq.extend_mode { "EXTEND" } else { "WRAP" };
    scene.fill_rect(layout.seq_extend, extend_bg);
    scene.text_centered(layout.seq_extend, extend_label, 0xffffff, 2);

    scene.fill_rect(layout.seq_stop, 0x504945);
    scene.text_centered(layout.seq_stop, "STOP", 0xffffff, 2);
    scene.fill_rect(layout.seq_clear, 0x3c3836);
    scene.text_centered(layout.seq_clear, "CLEAR", 0xffffff, 2);
    scene.fill_rect(
        layout.seq_to_pad,
        if model.seq_to_pad_armed {
            0xd79921
        } else {
            0x458588
        },
    );
    scene.text_centered(layout.seq_to_pad, ">PAD", 0xffffff, 2);
    scene.fill_rect(layout.seq_all_off, 0x665c54);
    scene.text_centered(layout.seq_all_off, "ALL OFF", 0xffffff, 2);
    scene.fill_rect(layout.seq_bpm_down, 0x282828);
    scene.text_centered(layout.seq_bpm_down, "- BPM", 0xffffff, 2);
    scene.fill_rect(layout.seq_bpm_up, 0x282828);
    scene.text_centered(layout.seq_bpm_up, "+ BPM", 0xffffff, 2);
    if model.seq_to_pad_armed {
        scene.text(400, HUD_H + 30, "tap a PAD slot", 0xfabd2f);
    } else {
        scene.text(400, HUD_H + 30, ">PAD assigns loop", 0x83a598);
    }

    scene.text(
        12,
        HUD_H + layout.content.h - 18,
        "REC locks loop · overdub · KEEP/DROP/UNDO · >PAD assigns",
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
        if model.song_playing {
            0x689d6a
        } else {
            0x3a5040
        },
    );
    scene.text(
        layout.song_play.x + 60,
        layout.song_play.y + 20,
        "PLAY",
        0xffffff,
    );
    scene.fill_rect(layout.song_stop, 0x3c3836);
    scene.text(
        layout.song_stop.x + 60,
        layout.song_stop.y + 20,
        "STOP",
        0xffffff,
    );
    scene.fill_rect(
        layout.song_loop,
        if model.song_loop { 0x458588 } else { 0x282838 },
    );
    scene.text(
        layout.song_loop.x + 20,
        layout.song_loop.y + 16,
        "LOOP",
        0xffffff,
    );
    scene.fill_rect(layout.song_delete, 0x9d0006);
    scene.text(
        layout.song_delete.x + 24,
        layout.song_delete.y + 16,
        "DEL",
        0xffffff,
    );
    scene.fill_rect(layout.song_bpm_down, 0x282838);
    scene.text(
        layout.song_bpm_down.x + 16,
        layout.song_bpm_down.y + 16,
        "BPM-",
        0xffffff,
    );
    scene.fill_rect(layout.song_bpm_up, 0x282838);
    scene.text(
        layout.song_bpm_up.x + 16,
        layout.song_bpm_up.y + 16,
        "BPM+",
        0xffffff,
    );
    scene.fill_rect(layout.song_prev, 0x282838);
    scene.text(
        layout.song_prev.x + 28,
        layout.song_prev.y + 20,
        "UP",
        0xffffff,
    );
    scene.fill_rect(layout.song_next, 0x282838);
    scene.text(
        layout.song_next.x + 16,
        layout.song_next.y + 20,
        "DOWN",
        0xffffff,
    );
    scene.fill_rect(layout.song_save_seq, 0x458588);
    scene.text(
        layout.song_save_seq.x + 48,
        layout.song_save_seq.y + 14,
        "SAVE SEQ",
        0xffffff,
    );
    scene.fill_rect(layout.song_out, model.song_out.color());
    scene.text_centered(layout.song_out, model.song_out.short_label(), 0xffffff, 2);
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
        if model.host_busy() == Some(crate::host::HostTask::MapThruOn) {
            "WAIT"
        } else {
            "THRU ON"
        },
        0xffffff,
    );
    scene.fill_rect(layout.map_thru_off, 0x9d0006);
    scene.text(
        layout.map_thru_off.x + 48,
        layout.map_thru_off.y + 28,
        if model.host_busy() == Some(crate::host::HostTask::MapThruOff) {
            "WAIT"
        } else {
            "THRU OFF"
        },
        0xffffff,
    );
    scene.fill_rect(layout.map_refresh, 0x458588);
    scene.text(
        layout.map_refresh.x + 28,
        layout.map_refresh.y + 28,
        if model.host_busy() == Some(crate::host::HostTask::MapList) {
            "WAIT"
        } else {
            "REFRESH PORTS"
        },
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
    scene.text_scaled(layout.content.x + 12, layout.content.y + 8, "SETTINGS", 0xfbf1c7, 2);
    scene.fill_rect(layout.settings_panic, 0x9d0006);
    scene.text_centered(layout.settings_panic, "PANIC", 0xffffff, 2);
    scene.fill_rect(layout.settings_all_off, 0x504945);
    scene.text_centered(layout.settings_all_off, "NOTES OFF", 0xffffff, 2);
    scene.fill_rect(layout.settings_audio, 0x458588);
    scene.text_centered(layout.settings_audio, "AUDIO", 0xffffff, 2);
    scene.fill_rect(layout.settings_log, 0x504945);
    scene.text_centered(layout.settings_log, "LOG", 0xffffff, 2);
    scene.fill_rect(layout.settings_map, 0x83a598);
    scene.text_centered(layout.settings_map, "MAP", 0xffffff, 2);
    let wifi_busy = model.host_busy() == Some(crate::host::HostTask::Wifi);
    scene.fill_rect(
        layout.settings_wifi,
        if wifi_busy { 0x504945 } else { 0xd79921 },
    );
    scene.text_centered(
        layout.settings_wifi,
        if wifi_busy { "WAIT" } else { "WIFI" },
        0xffffff,
        2,
    );
    scene.fill_rect(
        layout.settings_font,
        model.font_style.resolved().settings_color(),
    );
    scene.text_centered(
        layout.settings_font,
        &format!("FONT {}", model.font_style.label()),
        0xffffff,
        2,
    );
    let upd_busy = model.host_busy() == Some(crate::host::HostTask::UpdateCheck);
    scene.fill_rect(
        layout.settings_update,
        if upd_busy { 0x504945 } else { 0x689d6a },
    );
    scene.text_centered(
        layout.settings_update,
        if upd_busy { "WAIT" } else { "UPDATE" },
        0xffffff,
        2,
    );
    // WIFI / UPDATE / MAP write status_line — show it here (chrome truncates >12 chars).
    if !model.status_line.is_empty() && !model.update_panel_open && !model.wifi_panel_open {
        let c = layout.content;
        scene.text(c.x + 16, c.y + c.h - 28, &model.status_line, 0xfabd2f);
    }
}

fn draw_fx(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.text_scaled(layout.content.x + 12, layout.content.y + 8, "FX", 0xfbf1c7, 2);
    scene.fill_rect(
        layout.settings_fx_target,
        match model.fx_target {
            crate::model::FxEditTarget::Bus => 0x458588,
            crate::model::FxEditTarget::Voice => 0xb16286,
            crate::model::FxEditTarget::DrumGroup => 0xd79921,
        },
    );
    scene.text_centered(
        layout.settings_fx_target,
        match model.fx_target {
            crate::model::FxEditTarget::Bus => "TARGET: BUS (global)",
            crate::model::FxEditTarget::Voice => "TARGET: VOICE",
            crate::model::FxEditTarget::DrumGroup => "TARGET: DRUMS",
        },
        0xffffff,
        2,
    );
    const LABELS: [&str; 4] = ["DRIVE", "DELAY", "REVERB", "FLANGE"];
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
    for index in 0..4 {
        let track = layout.settings_fx_slider(index);
        scene.fill_rect(track, 0x20202c);
        scene.text(track.x + 4, track.y - 18, LABELS[index], 0xc0c0d0);
        let fill_h = (track.h as f32 * values[index]) as i32;
        let fill = Rect {
            x: track.x + 4,
            y: track.y + track.h - fill_h,
            w: track.w - 8,
            h: fill_h,
        };
        scene.fill_rect(fill, fill_color);
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
        assert!(model.kaoss_viz_style.is_cells());
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
        model.kaoss_viz_style = kaoss_viz::KaossVizStyle::Glow;
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
            model.kaoss_glow_amp() > 0.1,
            "touch should ramp the glow envelope"
        );
        let field_cells = scene
            .color
            .iter()
            .filter(|q| {
                q.x >= model.layout.kaoss.x as f32
                    && q.y >= model.layout.kaoss.y as f32
                    && q.w > 0.0
                    && q.h > 0.0
                    && q.w < 40.0
                    && q.h < 40.0
            })
            .count();
        assert!(
            field_cells > 20,
            "GLOW should paint a soft light-field, got {field_cells}"
        );
    }

    #[test]
    fn note_program_draws_scale_grid_lines() {
        let model = NativeModel::new();
        assert!(model.kaoss_show_grid_lines);
        assert!(kaoss_ui::program(model.kaoss_program).note);
        let scene = build(&model);
        let pad = model.layout.kaoss;
        let vert = scene
            .color
            .iter()
            .filter(|q| {
                q.w <= 8.0
                    && q.h >= (pad.h as f32) * 0.9
                    && q.x >= pad.x as f32 - 4.0
                    && q.x <= (pad.x + pad.w) as f32 + 4.0
                    && q.y >= pad.y as f32 - 2.0
            })
            .count();
        let horiz = scene
            .color
            .iter()
            .filter(|q| {
                q.h <= 8.0
                    && q.w >= (pad.w as f32) * 0.9
                    && q.y >= pad.y as f32 - 4.0
                    && q.y <= (pad.y + pad.h) as f32 + 4.0
                    && q.x >= pad.x as f32 - 2.0
            })
            .count();
        assert!(vert >= 8, "expected scale-note vertical lines, got {vert}");
        assert_eq!(horiz, 3, "expected Y guides at 25/50/75%, got {horiz}");
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

    #[test]
    fn fm_mode_draws_recipe_chips_and_keyboard() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Fm);
        let scene = build(&model);
        let chips = scene
            .color
            .iter()
            .filter(|q| {
                let cell = model.layout.fm_recipe_cell(0);
                q.w > 40.0 && q.h > 20.0 && q.y >= cell.y as f32 - 2.0 && q.y <= cell.y as f32 + 8.0
            })
            .count();
        assert!(
            chips >= jambox_core::FM_RECIPE_COUNT,
            "expected 8 recipe chips, got {chips}"
        );
        let whites = scene
            .color
            .iter()
            .filter(|q| q.color == 0xf2f2ea && q.h > 40.0)
            .count();
        assert!(whites >= Layout::SYNTH_WHITE_COUNT);
        let clear = model.layout.fm_clear();
        let clear_btn = scene
            .color
            .iter()
            .filter(|q| {
                q.color == 0x9d0006
                    && (q.x - clear.x as f32).abs() < 2.0
                    && (q.y - clear.y as f32).abs() < 2.0
            })
            .count();
        assert!(clear_btn >= 1, "CLEAR button missing");
    }
}
