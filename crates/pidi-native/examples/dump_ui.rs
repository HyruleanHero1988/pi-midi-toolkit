//! Dump one PPM per mode for host UI review.
//! Official docs captures: `./scripts/capture-pidi-docs.sh` (example dump_docs).
//!
//! `cargo run -p pidi-native --example dump_ui --no-default-features`

use std::path::PathBuf;

use pidi_native::client::Outbox;
use pidi_native::font::FontStyle;
use pidi_native::kaoss_ui::{self, KaossPicker};
use pidi_native::mode::UiMode;
use pidi_native::model::NativeModel;
use pidi_native::render::{self, Frame};

fn dump(model: &NativeModel, path: PathBuf) {
    let mut frame = Frame::new();
    render::draw(&mut frame, model);
    frame.write_ppm(&path).expect("write ppm");
    println!("wrote {}", path.display());
}

fn main() {
    let out = PathBuf::from("tmp_ui_shots");
    let _ = std::fs::create_dir_all(&out);

    let modes = [
        ("home", UiMode::Home),
        ("synth", UiMode::Synth),
        ("drums", UiMode::Drums),
        ("seq", UiMode::Seq),
        ("pads", UiMode::Pads),
        ("kaoss", UiMode::Kaoss),
        ("chords", UiMode::Chords),
        ("songs", UiMode::Songs),
        ("presets", UiMode::Presets),
        ("fm", UiMode::Fm),
        ("settings", UiMode::Settings),
        ("log", UiMode::Log),
        ("map", UiMode::Map),
    ];
    for (name, mode) in modes {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(mode);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join(format!("{name}.ppm")));
    }

    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Seq);
        model.seq.cue_beep = true;
        model.seq.status = "BEEP on — clave marks loop start".into();
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("seq_beep_on.ppm"));
    }

    // Smooth font smoke dumps (home + settings are densest labels).
    for (name, mode) in [
        ("home", UiMode::Home),
        ("settings", UiMode::Settings),
        ("synth", UiMode::Synth),
    ] {
        let mut model = NativeModel::new();
        model.font_style = FontStyle::Smooth;
        let mut ob = Outbox::new();
        model.set_mode(mode);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join(format!("{name}_smooth.ppm")));
    }

    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Kaoss);
        let b = model.layout.kaoss_settings_btn;
        model.finger_down(1, b.x + 4, b.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("kaoss_settings.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Synth);
        let b = model.layout.synth_vib;
        model.finger_down(1, b.x + 4, b.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("synth_vib.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Home);
        let p = model.layout.nav_power();
        model.finger_down(1, p.x + 4, p.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("power_menu.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Fm);
        model.tick(1.0 / 60.0, &mut ob);
        let growl = model.layout.fm_recipe_cell(7);
        model.finger_down(1, growl.x + 4, growl.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("fm_growl.ppm"));

        for (name, index) in [("fm_ep", 1), ("fm_brass", 3), ("fm_organ", 5)] {
            let mut rec = NativeModel::new();
            let mut ob2 = Outbox::new();
            rec.set_mode(UiMode::Fm);
            rec.tick(1.0 / 60.0, &mut ob2);
            let cell = rec.layout.fm_recipe_cell(index);
            rec.finger_down(1, cell.x + 4, cell.y + 4, &mut ob2);
            rec.finger_up(1, &mut ob2);
            rec.tick(1.0 / 60.0, &mut ob2);
            dump(&rec, out.join(format!("{name}.ppm")));
        }

        let (ax, ay) = model.layout.fm_op_center(1);
        let (dx, dy) = model.layout.fm_op_center(3);
        model.finger_down(2, ax, ay, &mut ob);
        model.finger_move(2, (ax + dx) / 2, (ay + dy) / 2, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("fm_draw.ppm"));
        model.finger_up(2, &mut ob);
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Drums);
        model.tick(1.0 / 60.0, &mut ob);
        let b = model.layout.kit_wave;
        model.finger_down(1, b.x + 4, b.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("drums_wave.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Drums);
        model.tick(1.0 / 60.0, &mut ob);
        let b = model.layout.kit_note_repeat;
        model.finger_down(1, b.x + 4, b.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("drums_repeat.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Kaoss);
        let b = model.layout.kaoss_scale;
        model.finger_down(1, b.x + 4, b.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("kaoss_scale_picker.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_picker = Some(KaossPicker::Octave);
        model.kaoss_root_midi = 108;
        model.kaoss_octaves = 1;
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("kaoss_octave_picker.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_root_midi = 108;
        model.kaoss_octaves = 1;
        let pad = model.layout.kaoss;
        model.finger_down(1, pad.x + pad.w - 8, pad.y + pad.h / 2, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("kaoss_c8_one_octave.ppm"));
        model.finger_up(1, &mut ob);
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_program = kaoss_ui::KAOSS_PROGRAMS
            .iter()
            .position(|p| p.id == "wah")
            .expect("wah");
        model.tick(1.0 / 60.0, &mut ob);
        let pad = model.layout.kaoss;
        model.finger_down(1, pad.x + pad.w / 2, pad.y + pad.h / 4, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("kaoss_wah.ppm"));
        model.finger_up(1, &mut ob);
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_program = kaoss_ui::KAOSS_PROGRAMS
            .iter()
            .position(|p| p.id == "wah")
            .expect("wah");
        model.kaoss_picker = Some(KaossPicker::Program);
        model.kaoss_show_all = false;
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("kaoss_programs.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Settings);
        let b = model.layout.settings_update;
        model.finger_down(1, b.x + 4, b.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.update_available = true;
        model.update_confirming = true;
        model.update_status = "Running: abc1234 (master)\n\n\
            This deploys new code from GitHub.\n\
            The screen stays on; the kiosk reloads itself when ready.\n\
            Phrases, songs, presets, and settings.json are kept.\n\
            Tap INSTALL NOW to continue, or CANCEL (CHECK)."
            .into();
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("update_confirm.ppm"));
    }
    {
        let mut model = NativeModel::new();
        let mut ob = Outbox::new();
        model.set_mode(UiMode::Settings);
        let b = model.layout.settings_update;
        model.finger_down(1, b.x + 4, b.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.update_status = "Installed master abc1234 (updated engines)\n\
            Reloading kiosk..."
            .into();
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("update_reloading.ppm"));
    }
}
