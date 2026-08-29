//! jambox-engine — realtime audio + sequencer daemon.
//!
//! The kiosk UI connects over a socket and sends JSON. It never renders audio and
//! never schedules a note itself; that is the whole point of this process.

mod audio;
mod bus;
mod headless;
mod ipc;
mod mailbox;
mod midi;
mod protocol;
mod rt;
mod waves;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use jambox_core::{Clip, ClipEvent, ClipEventKind, Command, JamboxEngine, Quantize, WaveBank, PPQ};
use tracing::info;

#[derive(Parser)]
#[command(name = "jambox-engine", about = "Realtime jambox audio + sequencer")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List audio output devices and MIDI ports.
    Devices,
    /// Run the engine (audio + MIDI + control socket).
    Run {
        /// Audio output device name substring (e.g. "headphone").
        #[arg(long, default_value = "")]
        output: String,
        /// MIDI input name substring (e.g. "MPK").
        #[arg(long, default_value = "")]
        midi_in: String,
        /// MIDI output name substring for clip/DIN emit.
        #[arg(long, default_value = "")]
        midi_out: String,
        /// Control socket path (Unix) or host:port.
        #[arg(long, default_value = "/tmp/jambox.sock")]
        control: String,
        /// Listen on TCP instead of a Unix socket.
        #[arg(long)]
        tcp: bool,
        /// Ask the kernel for realtime scheduling and locked memory.
        #[arg(long)]
        rt: bool,
        /// Run without a sound card (host/CI testing of control + sequencing).
        #[arg(long)]
        null_audio: bool,
        /// Directory of single-cycle `.wav` files (kiosk `wavetables/`).
        #[arg(long, default_value = "")]
        waves: String,
        /// Extra wavetable dir (kiosk `user-wavetables/`). Later dirs add/replace names.
        #[arg(long, default_value = "")]
        user_waves: String,
        /// Preferred ALSA/cpal callback frames. 0 keeps ALSA default periods
        /// (reliable on bcm2835 Headphones). N>0 tries Fixed(N), then 512/1024/256.
        #[arg(long, default_value_t = 0)]
        buffer_frames: u32,
    },
    /// Offline render benchmark — the PLAN's CPU headroom check, no device needed.
    Bench {
        /// Sample rate to simulate.
        #[arg(long, default_value_t = 48_000)]
        sample_rate: u32,
        /// Frames per block.
        #[arg(long, default_value_t = 256)]
        block: usize,
        /// Seconds of audio to render.
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
        /// Held notes during the bench.
        #[arg(long, default_value_t = 8)]
        voices: u8,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Cmd::Devices => devices(),
        Cmd::Run {
            output,
            midi_in,
            midi_out,
            control,
            tcp,
            rt,
            null_audio,
            waves,
            user_waves,
            buffer_frames,
        } => run(
            output,
            midi_in,
            midi_out,
            control,
            tcp,
            rt,
            null_audio,
            waves,
            user_waves,
            buffer_frames,
        ),
        Cmd::Bench {
            sample_rate,
            block,
            seconds,
            voices,
        } => bench(sample_rate, block, seconds, voices),
    }
}

fn devices() {
    println!("Audio outputs:");
    for name in audio::list_outputs() {
        println!("  {name}");
    }
    let (ins, outs) = midi::list_ports();
    println!("MIDI inputs:");
    for name in ins {
        println!("  {name}");
    }
    println!("MIDI outputs:");
    for name in outs {
        println!("  {name}");
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    output: String,
    midi_in: String,
    midi_out: String,
    control: String,
    tcp: bool,
    rt: bool,
    null_audio: bool,
    waves: String,
    user_waves: String,
    buffer_frames: u32,
) {
    let running = Arc::new(AtomicBool::new(true));
    {
        let running = running.clone();
        let _ = ctrlc::set_handler(move || {
            info!("shutting down");
            running.store(false, Ordering::Relaxed);
        });
    }

    let mut bank = WaveBank::with_builtins();
    if !waves.trim().is_empty() {
        waves::load_dir(std::path::Path::new(&waves), &mut bank);
    }
    if !user_waves.trim().is_empty() {
        waves::load_dir(std::path::Path::new(&user_waves), &mut bank);
    }

    let (control_side, midi_in_side, midi_out_side, audio_side) = bus::channel();
    let hub = Arc::new(ipc::ClientHub::default());
    let midi_map = Arc::new(midi::MidiMap::default());
    let midi_in_bus = Arc::new(std::sync::Mutex::new(midi_in_side));

    // RT hints before the stream so the callback thread inherits the policy.
    rt::apply_rt_hints(rt);

    let audio_health = Arc::new(audio::AudioHealth::new());

    if null_audio {
        let running = running.clone();
        std::thread::spawn(move || {
            headless::run(
                48_000,
                buffer_frames.max(64) as usize,
                audio_side,
                bank,
                running,
            );
        });
    } else {
        // Reopens after cable unplug / ALSA death / IPC audio_reopen; does not
        // kill the control socket.
        audio::spawn_output(
            output,
            audio_side,
            bank,
            buffer_frames,
            Arc::clone(&audio_health),
            running.clone(),
        );
    }

    midi::spawn_input(
        midi_in,
        Arc::clone(&midi_in_bus),
        Arc::clone(&hub),
        Arc::clone(&midi_map),
        running.clone(),
    );
    midi::spawn_output(midi_out, midi_out_side, running.clone());

    let endpoint = if tcp {
        ipc::Endpoint::Tcp(control)
    } else {
        #[cfg(unix)]
        {
            ipc::Endpoint::Unix(std::path::PathBuf::from(control))
        }
        #[cfg(not(unix))]
        {
            ipc::Endpoint::Tcp(control)
        }
    };

    let ipc_running = running.clone();
    let ipc_health = Arc::clone(&audio_health);
    let ipc_thread = std::thread::spawn(move || {
        ipc::serve(
            endpoint,
            control_side,
            hub,
            midi_map,
            midi_in_bus,
            ipc_health,
            ipc_running,
        );
    });

    while running.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = ipc_thread.join();
}

/// Render a worst-case jam offline and report the realtime ratio.
fn bench(sample_rate: u32, block: usize, seconds: f64, voices: u8) {
    let mut engine = JamboxEngine::new(sample_rate as f64);
    engine.sync_fx_slots();

    // A loop running under held notes — the shape of a real performance.
    let clip = Clip::new(
        (0..4)
            .map(|beat| ClipEvent {
                tick: beat * PPQ,
                kind: ClipEventKind::NoteOn {
                    channel: jambox_core::DRUM_CHANNEL,
                    note: 36 + (beat as u8 % 4),
                    velocity: 110,
                },
            })
            .collect(),
        PPQ * 4,
    );
    engine
        .sequencer_mut()
        .slot_mut(0)
        .expect("slot")
        .set_clip(Some(clip));

    let mut warmup: Vec<jambox_core::ScheduledCommand> = Vec::new();
    warmup.push(jambox_core::ScheduledCommand::now(Command::LaunchClip {
        slot: 0,
        quantize: Quantize::Off,
    }));
    for i in 0..voices {
        warmup.push(jambox_core::ScheduledCommand::now(Command::NoteOn {
            channel: 0,
            note: 48 + i,
            velocity: 110,
        }));
    }
    // Everything wet: the worst case the stress test asks about.
    for (target, param) in [
        (jambox_core::FxTarget::Bus, jambox_core::FxParam::ReverbMix),
        (jambox_core::FxTarget::Bus, jambox_core::FxParam::DelayMix),
        (
            jambox_core::FxTarget::DrumGroup,
            jambox_core::FxParam::DelayMix,
        ),
        (jambox_core::FxTarget::Voice(0), jambox_core::FxParam::Drive),
    ] {
        warmup.push(jambox_core::ScheduledCommand::now(Command::SetFx {
            target,
            param,
            value: 0.5,
        }));
    }

    let mut buf = vec![0.0f32; block];
    let mut midi = jambox_core::MidiOutSink::new();
    engine.render(&mut buf, &warmup, &mut midi);

    let total_blocks = ((seconds * sample_rate as f64) / block as f64).ceil() as usize;
    let start = std::time::Instant::now();
    let mut peak = 0.0f32;
    for _ in 0..total_blocks {
        engine.render(&mut buf, &[], &mut midi);
        peak = peak.max(buf.iter().fold(0.0f32, |m, v| m.max(v.abs())));
    }
    let elapsed = start.elapsed().as_secs_f64();
    let audio_seconds = (total_blocks * block) as f64 / sample_rate as f64;
    let ratio = elapsed / audio_seconds;

    println!("blocks       : {total_blocks} × {block} frames @ {sample_rate} Hz");
    println!("audio        : {audio_seconds:.2} s");
    println!("cpu          : {elapsed:.3} s");
    println!("realtime     : {:.1}% of one core", ratio * 100.0);
    println!("peak         : {peak:.3}");
    println!(
        "headroom     : {}",
        if ratio < 0.15 {
            "in budget (<15% per PLAN)"
        } else if ratio < 0.5 {
            "usable, watch polyphony"
        } else {
            "over budget — simplify DSP or cut voices"
        }
    );
}
