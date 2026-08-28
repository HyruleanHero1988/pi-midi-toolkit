//! Dump one PPM per mode for host UI review:
//! `cargo run -p pidi-native --example dump_ui --no-default-features`

use std::path::PathBuf;

use pidi_native::client::Outbox;
use pidi_native::font::FontStyle;
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
        model.set_mode(UiMode::Kaoss);
        let b = model.layout.kaoss_scale;
        model.finger_down(1, b.x + 4, b.y + 4, &mut ob);
        model.finger_up(1, &mut ob);
        model.tick(1.0 / 60.0, &mut ob);
        dump(&model, out.join("kaoss_scale_picker.ppm"));
    }
}
