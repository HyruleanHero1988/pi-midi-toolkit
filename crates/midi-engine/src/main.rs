//! Host MIDI thru engine: list ports, load JSON preset, transform with midi-core.

mod rt;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use clap::{Parser, Subcommand};
use midir::{MidiInput, MidiOutput, MidiOutputConnection};
use midi_core::{
    process_event, ActiveNotes, ChannelMapMode, EnginePreset, MidiEvent, PortsConfig,
    ProcessorChain, VelocityConfig,
};
use parking_lot::Mutex;
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "midi-engine", about = "Low-latency MIDI thru with remap presets")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List MIDI input and output ports.
    List,
    /// Load a preset and run MIDI thru until Ctrl-C.
    Run {
        /// Path to preset JSON.
        #[arg(short, long)]
        preset: PathBuf,
        /// Override input port name substring (else use preset.ports.input).
        #[arg(long)]
        input: Option<String>,
        /// Override output port name substring (else use preset.ports.output).
        #[arg(long)]
        output: Option<String>,
        /// Reload preset when the file mtime changes (flush stuck notes on swap).
        #[arg(long, default_value_t = true)]
        watch: bool,
        /// Attempt SCHED_FIFO + mlockall (Linux; requires privileges).
        #[arg(long, default_value_t = false)]
        rt: bool,
    },
    /// Print incoming CCs as JSON `cc_map` entries (move a knob to learn).
    Learn {
        /// Input port name substring.
        #[arg(long)]
        input: String,
        /// Stop after this many unique (channel, cc) pairs (0 = until Ctrl-C).
        #[arg(long, default_value_t = 0)]
        count: usize,
        /// Suggested output channel for printed entries (0–15).
        #[arg(long, default_value_t = 0)]
        out_channel: u8,
        /// Suggested output CC for printed entries (same as input if omitted).
        #[arg(long)]
        out_cc: Option<u8>,
    },
    /// Send a short note burst on an output port (commissioning).
    Test {
        /// Output port name substring.
        #[arg(long)]
        output: String,
        /// MIDI channel 0–15.
        #[arg(long, default_value_t = 0)]
        channel: u8,
        /// Note number.
        #[arg(long, default_value_t = 60)]
        note: u8,
        /// How many note-on/off pairs to send.
        #[arg(long, default_value_t = 3)]
        times: u32,
    },
    /// Benchmark transform-chain cost on this CPU (no MIDI I/O).
    Latency {
        /// Number of process_event iterations.
        #[arg(long, default_value_t = 200_000)]
        iterations: u32,
        /// Optional preset to bench (default: all_to + always_full).
        #[arg(long)]
        preset: Option<PathBuf>,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::List => cmd_list(),
        Commands::Run {
            preset,
            input,
            output,
            watch,
            rt,
        } => cmd_run(preset, input, output, watch, rt),
        Commands::Learn {
            input,
            count,
            out_channel,
            out_cc,
        } => cmd_learn(input, count, out_channel, out_cc),
        Commands::Test {
            output,
            channel,
            note,
            times,
        } => cmd_test(output, channel, note, times),
        Commands::Latency { iterations, preset } => cmd_latency(iterations, preset),
    }
}

fn cmd_list() -> Result<(), Box<dyn std::error::Error>> {
    let midi_in = MidiInput::new("midi-engine list")?;
    let midi_out = MidiOutput::new("midi-engine list")?;

    println!("Inputs:");
    let ins = midi_in.ports();
    if ins.is_empty() {
        println!("  (none)");
    } else {
        for (i, p) in ins.iter().enumerate() {
            let name = midi_in.port_name(p).unwrap_or_else(|_| "<unknown>".into());
            println!("  [{i}] {name}");
        }
    }

    println!("Outputs:");
    let outs = midi_out.ports();
    if outs.is_empty() {
        println!("  (none)");
    } else {
        for (i, p) in outs.iter().enumerate() {
            let name = midi_out.port_name(p).unwrap_or_else(|_| "<unknown>".into());
            println!("  [{i}] {name}");
        }
    }
    Ok(())
}

fn find_port_index(names: &[String], needle: &str) -> Result<usize, String> {
    let needle_l = needle.to_ascii_lowercase();
    let matches: Vec<(usize, &String)> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| n.to_ascii_lowercase().contains(&needle_l))
        .collect();
    match matches.as_slice() {
        [] => Err(format!(
            "no port matching `{needle}`. Available: {}",
            names.join(", ")
        )),
        [only] => Ok(only.0),
        many => Err(format!(
            "ambiguous port `{needle}` matched: {}",
            many.iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn input_port_names(midi_in: &MidiInput) -> Vec<String> {
    midi_in
        .ports()
        .iter()
        .map(|p| midi_in.port_name(p).unwrap_or_default())
        .collect()
}

fn output_port_names(midi_out: &MidiOutput) -> Vec<String> {
    midi_out
        .ports()
        .iter()
        .map(|p| midi_out.port_name(p).unwrap_or_default())
        .collect()
}

fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn cmd_run(
    preset_path: PathBuf,
    input_override: Option<String>,
    output_override: Option<String>,
    watch: bool,
    rt: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    rt::apply_rt_hints(rt);

    let preset = EnginePreset::from_path(&preset_path)?;
    let in_needle = input_override
        .clone()
        .unwrap_or_else(|| preset.ports.input.clone());
    let out_needle = output_override
        .clone()
        .unwrap_or_else(|| preset.ports.output.clone());

    let midi_in = MidiInput::new("midi-engine")?;
    let midi_out = MidiOutput::new("midi-engine")?;

    let in_names = input_port_names(&midi_in);
    let out_names = output_port_names(&midi_out);

    let in_idx = find_port_index(&in_names, &in_needle)?;
    let out_idx = find_port_index(&out_names, &out_needle)?;

    let in_port = &midi_in.ports()[in_idx];
    let out_port = &midi_out.ports()[out_idx];

    info!(
        preset = %preset.name,
        input = %in_names[in_idx],
        output = %out_names[out_idx],
        watch,
        "starting thru"
    );

    let chain = Arc::new(ArcSwap::from_pointee(preset.to_chain()));
    let conn_out = midi_out.connect(out_port, "midi-engine-out")?;
    let out = Arc::new(Mutex::new(conn_out));
    let active = Arc::new(Mutex::new(ActiveNotes::new()));
    let running = Arc::new(AtomicBool::new(true));

    let out_cb = Arc::clone(&out);
    let active_cb = Arc::clone(&active);
    let chain_cb = Arc::clone(&chain);

    let _conn_in = midi_in.connect(
        in_port,
        "midi-engine-in",
        move |_stamp, message, _| {
            handle_message(message, &chain_cb, &out_cb, &active_cb);
        },
        (),
    )?;

    let running_ctrl = Arc::clone(&running);
    ctrlc_set(running_ctrl)?;

    let mut last_mtime = file_mtime(&preset_path);
    let opened_ports = (preset.ports.input.clone(), preset.ports.output.clone());

    while running.load(Ordering::SeqCst) {
        if watch {
            if let Some(mtime) = file_mtime(&preset_path) {
                if last_mtime.map(|t| mtime > t).unwrap_or(true) {
                    match EnginePreset::from_path(&preset_path) {
                        Ok(new_preset) => {
                            last_mtime = Some(mtime);
                            if (new_preset.ports.input != opened_ports.0
                                || new_preset.ports.output != opened_ports.1)
                                && input_override.is_none()
                                && output_override.is_none()
                            {
                                warn!(
                                    "preset ports changed; restart the engine to reopen devices"
                                );
                            }
                            info!(preset = %new_preset.name, "reloading preset");
                            flush_stuck(&out, &active);
                            chain.store(Arc::new(new_preset.to_chain()));
                        }
                        Err(e) => warn!("preset reload failed: {e}"),
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(200));
    }

    info!("shutting down — flushing active notes");
    flush_stuck(&out, &active);
    thread::sleep(Duration::from_millis(20));
    Ok(())
}

fn cmd_learn(
    input_needle: String,
    count: usize,
    out_channel: u8,
    out_cc: Option<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    let midi_in = MidiInput::new("midi-engine learn")?;
    let in_names = input_port_names(&midi_in);
    let in_idx = find_port_index(&in_names, &input_needle)?;
    let in_port = &midi_in.ports()[in_idx];

    info!(input = %in_names[in_idx], "listening for CCs — move a control");
    println!("Paste entries into preset cc_map:");

    let seen = Arc::new(Mutex::new(Vec::<(u8, u8)>::new()));
    let done = Arc::new(AtomicBool::new(false));
    let seen_cb = Arc::clone(&seen);
    let done_cb = Arc::clone(&done);
    let target = count;

    let _conn_in = midi_in.connect(
        in_port,
        "midi-engine-learn",
        move |_stamp, message, _| {
            let Some(MidiEvent::ControlChange {
                channel,
                controller,
                ..
            }) = MidiEvent::parse(message)
            else {
                return;
            };
            let mut guard = seen_cb.lock();
            if guard.iter().any(|&(c, n)| c == channel && n == controller) {
                return;
            }
            guard.push((channel, controller));
            let mapped_cc = out_cc.unwrap_or(controller);
            println!(
                "  {{ \"in_channel\": {channel}, \"in_cc\": {controller}, \"out_channel\": {}, \"out_cc\": {mapped_cc} }},",
                out_channel & 0x0f
            );
            let _ = io::stdout().flush();
            if target > 0 && guard.len() >= target {
                done_cb.store(true, Ordering::SeqCst);
            }
        },
        (),
    )?;

    let running = Arc::new(AtomicBool::new(true));
    let running_ctrl = Arc::clone(&running);
    ctrlc_set(running_ctrl)?;

    while running.load(Ordering::SeqCst) && !done.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn cmd_test(
    output_needle: String,
    channel: u8,
    note: u8,
    times: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let midi_out = MidiOutput::new("midi-engine test")?;
    let out_names = output_port_names(&midi_out);
    let out_idx = find_port_index(&out_names, &output_needle)?;
    let out_port = &midi_out.ports()[out_idx];
    let mut conn = midi_out.connect(out_port, "midi-engine-test")?;

    let channel = channel & 0x0f;
    let note = note & 0x7f;
    info!(output = %out_names[out_idx], channel, note, times, "sending test notes");

    for i in 0..times {
        conn.send(&[0x90 | channel, note, 100])?;
        thread::sleep(Duration::from_millis(200));
        conn.send(&[0x80 | channel, note, 0])?;
        if i + 1 < times {
            thread::sleep(Duration::from_millis(150));
        }
    }
    conn.send(&[0xb0 | channel, 123, 0])?;
    Ok(())
}

fn cmd_latency(
    iterations: u32,
    preset_path: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let chain = if let Some(path) = preset_path {
        EnginePreset::from_path(path)?.to_chain()
    } else {
        EnginePreset {
            name: "bench".into(),
            ports: PortsConfig {
                input: "in".into(),
                output: "out".into(),
            },
            channel_map: ChannelMapMode::AllTo { channel: 2 },
            cc_map: vec![],
            velocity: VelocityConfig::AlwaysFull,
        }
        .to_chain()
    };

    let ev = MidiEvent::NoteOn {
        channel: 0,
        note: 60,
        velocity: 64,
    };

    // Warmup
    for _ in 0..10_000 {
        let _ = process_event(&chain, ev);
    }

    let start = Instant::now();
    for _ in 0..iterations {
        let _ = process_event(&chain, ev);
    }
    let elapsed = start.elapsed();
    let ns_per = elapsed.as_nanos() as f64 / iterations as f64;
    println!(
        "process_event: {iterations} iters in {elapsed:?} ({ns_per:.1} ns/event, ~{:.3} µs)",
        ns_per / 1000.0
    );
    println!("(CPU transform only — USB/ALSA hop is separate; target added processing << 1 ms)");
    Ok(())
}

fn handle_message(
    message: &[u8],
    chain: &ArcSwap<ProcessorChain>,
    out: &Mutex<MidiOutputConnection>,
    active: &Mutex<ActiveNotes>,
) {
    let loaded = chain.load();
    let Some(ev) = MidiEvent::parse(message) else {
        if !message.is_empty() {
            if let Err(e) = out.lock().send(message) {
                warn!("send failed: {e}");
            }
        }
        return;
    };

    let produced = process_event(&loaded, ev);
    let mut buf = [0u8; 3];
    let mut out_guard = out.lock();
    let mut active_guard = active.lock();
    for ev in produced.iter() {
        active_guard.observe(ev);
        let n = ev.encode(&mut buf);
        if let Err(e) = out_guard.send(&buf[..n]) {
            warn!("send failed: {e}");
        }
    }
}

fn flush_stuck(out: &Mutex<MidiOutputConnection>, active: &Mutex<ActiveNotes>) {
    let mut buf = [0u8; 3];
    let mut out_guard = out.lock();
    let mut active_guard = active.lock();
    for ev in active_guard.flush_all_note_offs() {
        let n = ev.encode(&mut buf);
        let _ = out_guard.send(&buf[..n]);
    }
    for ch in 0u8..16 {
        let _ = out_guard.send(&[0xb0 | ch, 123, 0]);
        let _ = out_guard.send(&[0xb0 | ch, 121, 0]);
    }
    let _ = io::stdout().flush();
}

fn ctrlc_set(running: Arc<AtomicBool>) -> Result<(), Box<dyn std::error::Error>> {
    ctrlc::set_handler(move || {
        running.store(false, Ordering::SeqCst);
    })?;
    Ok(())
}
