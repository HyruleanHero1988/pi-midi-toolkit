//! jambox-engine — realtime audio + sequencer daemon.
//!
//! The kiosk UI connects over a socket and sends JSON. It never renders audio and
//! never schedules a note itself; that is the whole point of this process.

mod audio;
mod bus;
mod ipc;
mod midi;
mod protocol;
mod rt;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use jambox_core::{Clip, ClipEvent, ClipEventKind, Command, JamboxEngine, Quantize, PPQ};
use tracing::{error, info};

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
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
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
        } => run(output, midi_in, midi_out, control, tcp, rt),
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

fn run(output: String, midi_in: String, midi_out: String, control: String, tcp: bool, rt: bool) {
    let running = Arc::new(AtomicBool::new(true));
    {
        let running = running.clone();
        let _ = ctrlc::set_handler(move || {
            info!("shutting down");
            running.store(false, Ordering::Relaxed);
        });
    }

    let (control_side, midi_in_side, midi_out_side, audio_side) = bus::channel();

    let device = match audio::pick_output(&output) {
        Ok(d) => d,
        Err(err) => {
            error!(%err, "audio: no usable output");
            return;
        }
    };

    // RT hints before the stream so the callback thread inherits the policy.
    rt::apply_rt_hints(rt);

    let stream = match audio::start(&device, audio_side) {
        Ok(s) => s,
        Err(err) => {
            error!(%err, "audio: stream failed");
            return;
        }
    };
    info!(
        sample_rate = stream.sample_rate,
        channels = stream.channels,
        "audio: running"
    );

    let _midi_conn = match midi::open_input(&midi_in, midi_in_side) {
        Ok(c) => Some(c),
        Err(err) => {
            info!(%err, "midi: no input (control socket still works)");
            None
        }
    };
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
    let ipc_thread = std::thread::spawn(move || {
        ipc::serve(endpoint, control_side, ipc_running);
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
        (
            jambox_core::FxTarget::Voice(0),
            jambox_core::FxParam::Drive,
        ),
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
