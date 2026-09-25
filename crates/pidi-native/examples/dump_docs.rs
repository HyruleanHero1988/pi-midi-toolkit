//! Capture 800×480 PPM frames of every native kiosk mode / overlay for docs.
//!
//! ```bash
//! cargo run -p pidi-native --example dump_docs --no-default-features -- docs/screens
//! ```

use std::env;
use std::path::{Path, PathBuf};

use pidi_native::chords::{self, Overlay as ChordsOverlay};
use pidi_native::client::Outbox;
use pidi_native::kaoss_ui::KaossPicker;
use pidi_native::kaoss_viz::KaossVizStyle;
use pidi_native::layout::Rect;
use pidi_native::mode::UiMode;
use pidi_native::model::NativeModel;
use pidi_native::phrases::PhrasePad;
use pidi_native::render::{self, Frame};

fn isolate_session() {
    // Deterministic dumps: no host settings.json / phrases / screensaver.
    env::set_var("PIDI_SETTINGS_PATH", "/tmp/pidi-docs-nosession.json");
    env::set_var("PIDI_PHRASES_DIR", "/tmp/pidi-docs-phrases");
    env::set_var("PIDI_PRESETS_DIR", "/tmp/pidi-docs-presets");
    env::set_var("PIDI_SONGS_DIR", "/tmp/pidi-docs-songs");
    env::set_var("MIDI_TONE_SCREENSAVER_SEC", "3600");
    let _ = std::fs::create_dir_all("/tmp/pidi-docs-phrases");
    let _ = std::fs::create_dir_all("/tmp/pidi-docs-presets");
    let _ = std::fs::create_dir_all("/tmp/pidi-docs-songs");
}

fn dump(model: &NativeModel, dir: &Path, name: &str) {
    let mut frame = Frame::new();
    render::draw(&mut frame, model);
    let path = dir.join(format!("{name}.ppm"));
    frame.write_ppm(&path).expect("write ppm");
    println!("wrote {}", path.display());
}

fn fresh() -> (NativeModel, Outbox) {
    let mut model = NativeModel::new();
    let mut ob = Outbox::new();
    model.ensure_library_loaded_with(Some(&mut ob));
    model.tick(1.0 / 60.0, &mut ob);
    (model, ob)
}

fn tap(model: &mut NativeModel, id: i32, rect: Rect, ob: &mut Outbox) {
    let x = rect.x + rect.w.min(8) / 2;
    let y = rect.y + rect.h.min(8) / 2;
    model.finger_down(id, x, y, ob);
    model.finger_up(id, ob);
    model.tick(1.0 / 60.0, ob);
}

fn tick_n(model: &mut NativeModel, ob: &mut Outbox, n: u32) {
    for _ in 0..n {
        model.tick(1.0 / 60.0, ob);
    }
}

fn fill_demo_pads(model: &mut NativeModel) {
    for (i, loop_mode) in [(0usize, true), (1, true), (4, false), (8, true)] {
        model.phrases[i] = PhrasePad {
            empty: false,
            loop_mode,
            length_ticks: 3840,
            length_secs: 2.0,
            ..PhrasePad::default()
        };
        model.phrases[i].empty = false;
        model.phrases[i].loop_mode = loop_mode;
    }
}

fn pose_kaoss_finger(model: &mut NativeModel, ob: &mut Outbox) {
    let pad = model.layout.kaoss;
    let px = pad.x + (pad.w * 2) / 5;
    let py = pad.y + (pad.h * 2) / 5;
    model.finger_down(1, px, py, ob);
    tick_n(model, ob, 10);
}

fn main() {
    isolate_session();
    let out = PathBuf::from(env::args().nth(1).unwrap_or_else(|| "docs/screens".into()));
    std::fs::create_dir_all(&out).expect("create docs/screens");

    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Home);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "00-home");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Synth);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "01-synth");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Seq);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "02-seq");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Pads);
        fill_demo_pads(&mut model);
        let edit = model.layout.pads_edit;
        tap(&mut model, 1, edit, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "03-pads-edit");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Pads);
        fill_demo_pads(&mut model);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "04-pads-play");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Songs);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "05-songs");
    }
    {
        let demo = PathBuf::from("/tmp/pidi-docs-songs/viz-demo.mid");
        let events = vec![
            jambox_protocol::WireClipEvent::midi(0, true, 0, 48, 90),
            jambox_protocol::WireClipEvent::midi(0, true, 0, 60, 110),
            jambox_protocol::WireClipEvent::midi(0, true, 0, 67, 100),
            jambox_protocol::WireClipEvent::midi(480, false, 0, 60, 0),
            jambox_protocol::WireClipEvent::midi(720, true, 0, 64, 105),
            jambox_protocol::WireClipEvent::midi(960, false, 0, 48, 0),
            jambox_protocol::WireClipEvent::midi(960, false, 0, 67, 0),
            jambox_protocol::WireClipEvent::midi(1440, false, 0, 64, 0),
            jambox_protocol::WireClipEvent::midi(960, true, 9, 36, 120),
            jambox_protocol::WireClipEvent::midi(1080, false, 9, 36, 0),
        ];
        let _ = pidi_native::songs::write_smf_type0(&demo, &events, 1920, 120.0);
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Songs);
        let viz = model.layout.song_viz;
        tap(&mut model, 1, viz, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "05-songs-viz");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Presets);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "06-presets");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Log);
        model.push_log("note_on ch1 C4 vel 110");
        model.push_log("SEQ rec start");
        model.push_log("CHORDS C");
        model.push_log("kaoss LEAD hold");
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "07-log");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Synth);
        let wave_a = model.layout.synth_wave_a;
        tap(&mut model, 1, wave_a, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "08-morph");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Drums);
        let kick = model.layout.kit_pad_cell(4);
        tap(&mut model, 1, kick, &mut ob);
        let repeat = model.layout.kit_note_repeat;
        tap(&mut model, 1, repeat, &mut ob);
        let quarter = model.layout.kit_repeat_choice_cell(1);
        tap(&mut model, 1, quarter, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "09-drums");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Drums);
        let kick = model.layout.kit_pad_cell(4);
        tap(&mut model, 1, kick, &mut ob);
        let repeat = model.layout.kit_note_repeat;
        tap(&mut model, 1, repeat, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "09-drums-repeat");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Drums);
        let wave = model.layout.kit_wave;
        tap(&mut model, 1, wave, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "09-drums-wave");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Home);
        let power = model.layout.nav_power();
        tap(&mut model, 1, power, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "10-power");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Settings);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "11-settings");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_viz_style = KaossVizStyle::Glow;
        pose_kaoss_finger(&mut model, &mut ob);
        dump(&model, &out, "12-kaoss");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_picker = Some(KaossPicker::Scale);
        model.kaoss_show_all = true;
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "13-kaoss-scales");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_viz_style = KaossVizStyle::Glow;
        let full = model.layout.kaoss_full;
        tap(&mut model, 1, full, &mut ob);
        pose_kaoss_finger(&mut model, &mut ob);
        dump(&model, &out, "14-kaoss-full");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Kaoss);
        let settings = model.layout.kaoss_settings_btn;
        tap(&mut model, 1, settings, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "15-kaoss-settings");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Chords);
        model.chords_palette = chords::progression_in_key(&chords::PROGRESSIONS[0], 0);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "16-chords");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Arp);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "27-arp");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Chords);
        model.chords_overlay = Some(ChordsOverlay::Changes);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "17-chords-changes");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Map);
        model.channel_map_bits[0] = 1 << 5;
        model.channel_map_bits[2] = (1 << 6) | (1 << 7);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "18-map");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Map);
        model.map_out_edit = Some(0);
        model.channel_map_bits[0] = 1 << 5;
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "18-map-out");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Ports);
        model.midi_inputs = vec![
            "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0".into(),
            "Keystation Mini 32".into(),
            "Midi Through:Midi Through Port-0 14:0".into(),
        ];
        model.midi_outputs = vec![
            "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0".into(),
            "jambox-out:jambox-out 129:0".into(),
        ];
        model.midi_in_filter.clear();
        model.midi_out_filter = "U2MIDI".into();
        model.midi_in_connected =
            "Keystation Mini 32 | U2MIDI PRO:U2MIDI PRO MIDI 1 20:0".into();
        model.midi_out_connected = "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0".into();
        model.last_midi_activity = "IN  ch1 n60 v100".into();
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "25-ports");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Synth);
        let vib = model.layout.synth_vib;
        tap(&mut model, 1, vib, &mut ob);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "19-synth-vib");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_picker = Some(KaossPicker::Program);
        model.kaoss_show_all = true;
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "20-kaoss-programs");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Kaoss);
        model.kaoss_settings_open = true;
        model.kaoss_color_picker_open = true;
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "21-kaoss-color");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Chords);
        model.chords_overlay = Some(ChordsOverlay::Key);
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "22-chords-key");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Settings);
        model.wifi_kb_open = true;
        model.wifi_kb_ssid = "Cafe".into();
        model.wifi_kb_text = "ab".into();
        model.wifi_kb_show = true;
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "23-wifi-keyboard");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Settings);
        model.wifi_kb_open = true;
        model.wifi_kb_ssid = "Cafe".into();
        model.wifi_kb_text = "a+".into();
        model.wifi_kb_show = true;
        model.wifi_kb_sym = true;
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "24-wifi-keyboard-symbols");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Fx);
        model.fx_bus = [0.22, 0.38, 0.28, 0.18];
        model.synth_params[2] = 0.72;
        model.drum_level = 0.48;
        model.punch_amount[0] = 0.72;
        model.punch_armed[0] = true;
        model.punch_amount[7] = 0.45;
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "25-fx");
    }
    {
        let (mut model, mut ob) = fresh();
        model.set_mode(UiMode::Mix);
        fill_demo_pads(&mut model);
        model.phrases[0].gain = 1.4;
        model.phrases[1].gain = 0.6;
        model.phrases[4].gain = 1.8;
        model.phrase_playing[0] = true;
        model.phrase_playing[4] = true;
        model.synth_params[2] = 0.72;
        model.drum_level = 0.48;
        model.seq_level = 1.2;
        tick_n(&mut model, &mut ob, 2);
        dump(&model, &out, "26-mix");
    }
}
