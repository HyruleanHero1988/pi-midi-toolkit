# Jambox architecture: findings and next direction

Status: **architecture recommendation / prototype brief**  
Research snapshot: **2026-08-22**  
Target: Raspberry Pi 2 Model B v1.1 + BigTreeTech Pi TFT70 V2.1 + Akai MPK mini mk3

This document records the clean-slate architecture review prompted by the gap
between the current PiDI jambox experience and a dedicated instrument:
screen flashes, delayed text, a slow KAOSS CELLS visualizer, and notes that can
continue for seconds after a finger leaves the pad.

It does not commit the project to an immediate rewrite. The first commitment is
to build and measure a narrow native vertical slice on the real hardware. Keep
the existing PiDI application usable while that prototype proves or disproves
the proposed direction.

## Executive decision

The jambox outgrew an interface architecture that began as a MIDI diagnostic.
That does **not** mean the product goal, Pi 2, or Rust engine direction is
fundamentally wrong.

Keep:

- the dual-purpose product: jambox plus MIDI remapper;
- a strict process/thread boundary between presentation and realtime music;
- the Rust DSP, MIDI transforms, transport concepts, tests, and data formats;
- cross-compilation and an appliance-style deployment;
- headless behavioral models and host-side tests.

Change:

- replace Tk as the eventual production renderer for high-rate instrument
  surfaces;
- make one native Rust engine the sole owner of audio, MIDI devices, transport,
  sequencing, KAOSS gates, and repeat lanes;
- remove Python wall-clock scheduling and the Python audio fallback from the
  production appliance after migration;
- use different semantics for reliable edges and continuous controls;
- reduce and measure audio latency instead of using ~100 ms as a stability
  default;
- treat five-contact multitouch as a product input, not mouse emulation.

Do **not** adopt JUCE merely because Akai uses it. For this project the leading
prototype is a small Rust UI using SDL touch/input plus DRM/KMS and OpenGL ES 2,
with direct libinput/libevdev available if SDL's KMS touch path is unreliable on
the TFT70. Qt Quick/EGLFS is the polished comparison candidate. LVGL is a viable
embedded alternative.

## Product and hardware constraints

### Product

PiDI remains one appliance with two equal jobs:

1. **Jambox:** wavetable synth, drums, KAOSS, loops, phrases, songs, and presets.
2. **MIDI remapper:** USB MIDI in, transforms, and USB/DIN out.

The winning property is not feature count. It is an instrument that is obvious
under the hands, starts reliably, stays in time, and never lets display work
delay a note release.

### Hardware

| Component | Constraint |
|---|---|
| Pi 2 Model B v1.1 | Quad Cortex-A7 at 900 MHz, 1 GB RAM, four cores, shared 512 KB L2 |
| TFT70 V2.1 | 7-inch DSI, 800×480 at 60 Hz, GT911, advertised five contacts |
| Pi graphics | VideoCore IV / VC4, OpenGL ES 2, DRM/KMS |
| Audio today | Pi onboard headphone output through ALSA |
| Controller | MPK mini mk3 over USB MIDI |
| Later output | USB-to-DIN MIDI |

The Pi 2 has enough compute for the intended sound engine if the implementation
is bounded and measured. The current system underuses its GPU while asking a
single Tk/X11 event thread to do excessive fine-grained work.

The exact Pi 2 + BTT TFT70 DSI/GT911 combination is still a physical validation
gate. Do not assume all five contacts reach userspace until `evtest` proves it.

## Why the current system feels unlike an appliance

The package split under [`apps/pidi`](apps/pidi) improved maintainability but
did not change the runtime algorithms. The current application remains a
transitional system:

- Tk owns presentation and high-rate touch handling.
- Python still contains a complete fallback synth.
- Python threads still schedule SEQ, phrase pads, songs, and KAOSS gate ticks.
- `jambox-engine` owns Rust audio and MPK ingest when connected.
- UI-injected notes and continuous parameters travel as newline-delimited JSON.
- engine MIDI is echoed back to the UI for display and state mirroring.

That creates two timing models and two partial sources of truth.

## Code-backed findings

### 1. Tk rendering is the main visual ceiling

KAOSS CELLS is a fixed 12×7 field. Every visual tick computes and applies a
color to all **84** canvas rectangles. The tick runs every 80 ms. KAOSS motion
also updates the status, axes, cursor, trails, and IPC state on the Tk thread.

Relevant code:

- `apps/pidi/pidi/ui/screens/kaoss.py`
  - `_kaoss_on_move`
  - `_kaoss_viz_tick`
  - `_kaoss_paint_leds`
  - `_kaoss_draw_cursor`
  - `_kaoss_refresh_axis_labels`
- `apps/pidi/pidi/kaoss.py`
  - `LED_COLS = 12`
  - `LED_ROWS = 7`
  - the existing comment notes that a larger matching field made the Pi crawl.

Tk Canvas is also used with delete/recreate patterns:

- the moving KAOSS cursor is deleted and rebuilt;
- scope traces are deleted and recreated;
- grid and axis items are rebuilt on configuration changes;
- some screens destroy and rebuild their widget trees;
- mode changes unpack every shell before packing the next one.

Tk does not submit one atomic, double-buffered instrument frame. Incremental
widget and canvas mutations can therefore be both expensive and visibly
flashy.

### 2. Some screen flashes are deliberate

The CRT scope pipeline immediately deletes the waveform when it becomes dirty,
then repaints after a debounce of up to roughly 100 ms. This is a direct,
code-backed explanation for a visible blank flash during morph and knob moves.

Mode switching and PLAY/EDIT changes can briefly expose an empty host while
shells or children are unpacked, destroyed, rebuilt, and repacked.

The eventual native renderer should preserve the previous complete frame until
the next complete frame is ready.

### 3. KAOSS remote echo floods hidden UI work

On a remote Rust engine, a KAOSS move can generate:

- CC12 and CC13 when their seven-bit values change;
- one or two local parameter updates on every motion sample;
- note-off plus note-on when the quantized pitch changes;
- CC92 on finger down/up.

`jambox-engine` broadcasts every ingested MIDI event back to the UI. CC12,
CC13, and CC92 are not classified as continuous controls by
`apps/pidi/pidi/ui/midi_io.py`; each echo becomes an ordinary log item.

The queue drain then:

- updates `last_var`;
- inserts the event into the Tk `Text` log;
- scrolls that log to the end;
- toggles the widget state.

This still occurs while the Log screen is hidden. Engine-echoed note events
also update active-note display state and recording-related Python models.

This is not an infinite feedback loop, but it is a high-rate round trip that
amplifies touch work and can starve the same Tk thread responsible for handling
finger-up.

Relevant code:

- `apps/pidi/pidi/ui/screens/kaoss.py::_kaoss_apply`
- `crates/jambox-engine/src/midi.rs::ingest`
- `apps/pidi/pidi/jambox_client.py::drain_midi`
- `apps/pidi/pidi/ui/midi_io.py::_handle_midi_body`
- `apps/pidi/pidi/ui/midi_io.py::_drain_queue`
- `apps/pidi/pidi/ui/app.py::_append_log`

### 4. The queues use the wrong semantics for continuous input

`JamboxClient` has one 512-entry FIFO for notes, CCs, parameters, clips, and
panic commands. A finger-up note-off is appended after all earlier motion and
parameter messages. If the queue is full, `send()` drops the new message; that
message can be the note-off.

The Tk event queue is a separate bounded FIFO that drops the oldest item. It is
drained every 40 ms with a cap of 12–24 items per tick.

Continuous XY positions should not be reliable FIFO messages. If 100 positions
arrive before the consumer runs, only the newest position matters. A note-off,
panic, clip stop, or touch cancellation must never wait behind those 100 stale
positions.

### 5. Audio latency is deliberately large

The Python fallback defaults to a 1,536-frame block and requests 100 ms
latency.

The Rust engine requests `cpal::BufferSize::Default`. Its own code records that
the bcm2835 headphone device commonly yields about 4,410 frames at 44.1 kHz:
roughly **100 ms**.

All live MIDI and UI commands are converted to `ScheduledCommand::now` only
when an audio callback begins. The core can schedule events at an exact frame
inside a block, but live ingest does not preserve or derive such a frame.

Therefore even a clean, unloaded touch can wait nearly one current callback
period before making sound. The display pipeline is not the only source of
poor feel.

Relevant code:

- `apps/pidi/pidi/constants.py`: `BLOCKSIZE`, `LATENCY_SEC`
- `crates/jambox-engine/src/audio.rs::pick_stream_config`
- `crates/jambox-engine/src/audio.rs::drain`
- `crates/jambox-core/src/command.rs::ScheduledCommand`

### 6. The Rust cutover is incomplete

Good:

- DSP and transport live in a no-I/O core.
- audio/control/MIDI communicate through bounded rings;
- clip events can be frame-accurate;
- parsing and socket I/O are outside the audio callback;
- the daemon has headless and benchmark modes.

Incomplete or incorrect:

- SEQ, phrase pads, songs, and KAOSS gate timing still use Python wall clocks;
- the kiosk does not yet use the Rust clip upload/launch path as its sole
  sequencer;
- live events are block-quantized;
- USB/DIN clip output currently discards frame offsets before the MIDI sender;
- `StatusReply.xruns` reports rejected control commands, not callback misses;
- actual CPAL deadline misses are not counted;
- the documented no-allocation audio contract is violated during clip swaps:
  moving a value out of a `Box` frees it, and `Box::new(previous)` allocates;
- tracing occurs in the callback on first use and oversized blocks;
- realtime scheduling is requested on the creating thread before workers start
  rather than explicitly set and verified on only the intended threads.

The split remains the correct direction. It must be completed and made true in
implementation.

### 7. Aggregate CPU can hide one saturated core

Tk is fundamentally single-event-threaded. On a four-core Pi, one saturated UI
core can appear as only about 25% aggregate CPU. The Settings diagnostic is
aggregate, so a low-looking total does not prove UI headroom.

No production setup currently guarantees:

- the `performance` CPU governor during a session;
- explicit per-thread CPU affinity;
- per-core load visibility;
- realtime policy verification in the actual callback;
- undervoltage/throttle history in diagnostics.

The Pi is not being intentionally CPU-capped by the project's 10–15% DSP
budget. That budget is prudent. The current system instead buys stability with
large buffers and spends CPU inefficiently on UI mutation.

## Symptom-to-cause summary

| Symptom | Primary mechanisms |
|---|---|
| Screen flashes | delete-before-repaint scope; widget/shell rebuilds; no atomic frame |
| Text lags | 40 ms capped queue drain; remote MIDI echo; hidden `Text` mutation |
| CELLS crawls | 84 Tcl item updates per tick plus motion/status/axis work |
| Notes arrive late | audio callback boundary, commonly ~100 ms today |
| Notes continue after lift | Tk-delayed release; note-off behind stale FIFO traffic; possible dropped note-off; then audio block boundary |
| Loops can drift or feel loose | Python wall-clock scheduling remains in production paths |

## What to retain and what to retire

### Retain

- `jambox-core` DSP algorithms, transport concepts, and tests;
- `midi-core` transform behavior and stuck-note tracking;
- musical behavior in the headless KAOSS and sequencer models;
- songs, phrases, presets, and user-data formats;
- Rust as the realtime implementation language;
- UI/realtime process isolation;
- offline rendering and host-side tests;
- armv7 cross-compilation and committed device binaries;
- the existing Tk app as a baseline during prototype evaluation.

### Retire from the final production appliance

- Tk as the high-rate instrument renderer;
- Python PortAudio fallback in normal operation;
- Python wall-clock ownership of musical timing;
- two engine daemons that can compete to open the same MIDI hardware;
- unversioned, high-rate newline JSON as the semantics for continuous input;
- self-echoed MIDI logging on every motion;
- reliable FIFO treatment of obsolete XY states;
- delete/recreate rendering for animation;
- full widget-tree rebuilds during ordinary navigation.

Python remains useful for deployment, data conversion, development tools,
fixture generation, and possibly a desktop simulator.

## Recommended target architecture

```text
┌────────────────────────────────────────────────────────────┐
│ pidi-ui (native, non-realtime)                              │
│                                                            │
│ DRM/KMS + GLES frame renderer                              │
│ libinput/evdev or SDL touch events                         │
│ screen state, hit testing, presentation, settings          │
│                                                             │
│ may drop animation frames; never owns musical time          │
└──────────────────────────────┬─────────────────────────────┘
                               │
          versioned typed control protocol
          reliable edges + latest-state mailboxes
                               │
┌──────────────────────────────▼─────────────────────────────┐
│ pidi-engine (native, sole hardware/music owner)             │
│                                                            │
│ MIDI input/router/remapper           control worker         │
│ audio callback + DSP                 persistence bridge     │
│ sample-clock transport               MIDI output scheduler  │
│ clips, songs, phrases                touch/repeat ownership  │
│                                                            │
│ one MPK open; route to local synth, recorder, and DIN       │
└────────────────────────────────────────────────────────────┘
```

### Why one engine daemon

The dual-purpose product does not require two processes competing for the MPK.
One engine can keep modular subsystems while owning each hardware port once.
Every incoming event gets one timestamp and can be routed to:

- local synth;
- recorder/clip capture;
- remap/transformation;
- USB/DIN output;
- UI telemetry at a sampled rate.

The UI remains a separate process. That boundary is valuable: a rendering
crash or slow frame must not interrupt audio or move the beat.

### State ownership

| State | Owner |
|---|---|
| Audio voices, drums, FX memory | Engine |
| Transport and all repeat/clip/song timing | Engine |
| MIDI ports and routing | Engine |
| Active touch-owned musical actions | Engine |
| Current screen, animation, layout | UI |
| Persisted musical settings | Engine/domain model; UI edits through commands |
| Logs/telemetry | Engine emits sampled/bounded notices; UI renders on demand |

Avoid mirrored mutable DSP state in the UI. The UI should display snapshots,
not implement a second synth to discover current values.

## Input and protocol design

### Two transport classes

#### Reliable edge queue

Must be ordered, acknowledged where appropriate, and never dropped behind
continuous traffic:

- `TouchDown`
- `TouchUp`
- `TouchCancel`
- `NoteOn`
- `NoteOff`
- `AllNotesOff`
- `Panic`
- `StartRepeat`
- `StopRepeat`
- `LaunchClip`
- `StopClip`
- mode/routing changes

#### Latest-value mailboxes

One current value per control/gesture. New writes replace obsolete pending
values:

- XY position by gesture ID;
- knob positions;
- parameter drags;
- meters and waveform snapshots;
- animation state;
- diagnostic counters.

If 100 XY samples arrive before the engine consumes them, apply the newest
sample. Do not replay a finger's obsolete route through the pad.

### Gesture ownership

Use a stable session and gesture/contact ID:

```text
TouchDown { session, gesture, x, y }
TouchMove { session, gesture, latest_x, latest_y }
TouchUp   { session, gesture }
```

Rules:

- a gesture captures a target on finger-down;
- release/cancel affects only that gesture's action;
- `TouchUp` has priority over all motion mailboxes;
- UI disconnect cancels every gesture owned by that UI session;
- a watchdog prevents a lost client from leaving held notes;
- panic clears touch, repeat, clip, and voice ownership.

Use a typed protocol with an explicit version handshake. JSON remains fine for
low-rate commissioning and diagnostics, but it should not define high-rate
realtime semantics.

## Five-contact multitouch

The TFT70 listing advertises five-point touch. The GT911 controller is designed
for five concurrent contacts and reports tracking IDs. Linux type-B multitouch
represents contacts with `ABS_MT_SLOT`, `ABS_MT_TRACKING_ID`, and independent
positions.

### Hardware proof

After the TFT70 is connected:

```bash
sudo evtest
```

Select the Goodix/GT911 device and test one through five fingers. Confirm:

- slots 0–4 are available;
- each active contact has a distinct non-negative tracking ID;
- each slot receives independent X/Y updates;
- lifting a finger emits tracking ID `-1` for only that contact;
- contact coordinates align with the 800×480 display.

Also inspect `libinput debug-events` if available. The reseller specification
is not enough; the kernel driver must expose all contacts.

### Drum repeat example

A held kick can repeat on quarter notes in 4/4 while other fingers play:

```text
finger 41 down on KICK
  → StartRepeat(owner=41, note=kick, division=1/4, quantize=beat)

finger 52 down/up on SNARE
  → immediate snare hit

finger 73 down/up on CLOSED HAT
  → immediate hat hit

finger 41 up
  → StopRepeat(owner=41)
```

The engine schedules the repeat on its sample clock. The UI does not run a
250 ms timer. Rendering may stall without changing the repeat.

Possible divisions:

- 1/4
- 1/8
- 1/8 triplet
- 1/16
- 1/32, only if musically and computationally useful

Multiple contacts may own independent lanes, for example kick at 1/4 and closed
hat at 1/16 while remaining contacts tap other drums.

The GT911 reports contact area, not dependable musical pressure. Do not assume
velocity-sensitive touch. Use a fixed velocity, an accent state, a configurable
pad velocity, or position within a pad.

### Contact handling

Maintain:

```text
FingerId → CapturedTarget
```

Hit-test on down and capture that pad. Decide explicitly whether moving outside
the pad:

- keeps ownership until lift;
- cancels at the boundary; or
- slides into another pad.

For repeat lanes, capture-until-lift is the safest default.

## UI framework assessment

Research snapshot: 2026-08-22. Every candidate still requires a prototype on
the exact armv7 + VC4 + DSI + GT911 stack.

| Stack | Direct appliance path | Independent multitouch | License / concern | Fit |
|---|---|---|---|---|
| Rust + SDL2/3 + GLES | KMSDRM; custom renderer | Yes, touch/finger IDs | SDL zlib, Rust bindings MIT; KMS touch quirks need proof | **Leading performance prototype** |
| Qt Quick/QML | EGLFS/KMS + libinput | Yes, `MultiPointTouchArea` | LGPL/GPL/commercial; heavier | **Polished comparison** |
| LVGL 9 | DRM + evdev | Yes, by mapping contacts to multiple pointer inputs | MIT; C/FFI and driver work | Strong embedded option |
| GTK 4 | X11/Wayland | Yes, touch sequences/gestures | LGPL; desktop stack | Capable, not preferred |
| egui | backend-dependent | Core has touch IDs | MIT/Apache; no first-choice direct Pi KMS integration | Prototype option |
| Slint LinuxKMS | DRM/KMS + libinput | Backend sees IDs, but standard `TouchArea` still lacks clean independent multi-control behavior | GPLv3 or paid embedded terms; backend maturity | Reject for drum grid today |
| JUCE 9 | X11 on Linux; custom work for direct KMS | Linux XInput2 multitouch | AGPL/commercial; C++ rewrite; armv7 not routine | Not recommended |
| Tkinter current path | X11 mouse-style bindings | No | Existing single-pointer behavior | Baseline only |

### SDL recommendation

Use SDL for:

- KMSDRM fullscreen creation;
- event pumping and explicit `FingerId` events;
- host-window development;
- optional dummy/headless rendering tests.

Use OpenGL ES 2 for:

- one complete frame per refresh;
- a single mesh/instance buffer for the 84-cell field;
- cached text/glyph atlases;
- batched rectangles and lines;
- a glow shader or a small number of layered primitives.

If SDL's KMSDRM touch path mishandles the TFT70's window ID or coordinate
normalization, keep SDL for display/rendering and read contacts through
libinput/libevdev directly. Do not fall back to mouse emulation.

### Qt comparison

Qt Quick provides the most mature high-level multi-contact API:
`MultiPointTouchArea`. EGLFS can run without a desktop compositor and uses
libinput by default.

Its costs are a larger runtime, QML/C++ alongside Rust, and uncertain Pi 2
headroom. A one-screen comparison is more useful than debating it abstractly.

### Why not JUCE

JUCE is not what makes an MPC fast. Akai combines a bounded product, custom
hardware integration, tuned DSP, and years of engineering.

JUCE 9 now supports XInput2 multitouch on Linux, but:

- standard Linux JUCE remains centered on X11 rather than an upstream direct
  DRM/KMS UI backend;
- a custom embedded backend recreates much of the work we are choosing a
  framework to avoid;
- it moves the application to C++ or requires a Rust bridge;
- armv7 is expected to work but is not a routinely exercised target;
- JUCE is AGPL or commercial, while the Rust workspace is MIT.

Our strongest code is already the Rust audio/MIDI core. Rewriting it into JUCE
does not address the actual Tk event/render and queue defects.

## Renderer rules

- Keep the previous complete frame visible until the next frame is complete.
- KAOSS targets 60 Hz; mostly static screens render only when dirty.
- Input dispatch is independent of rendering.
- Never log or lay out text per motion event.
- Cache glyphs, labels, and static screen geometry.
- Update the 84 cells as one batched draw, not 84 cross-language widget calls.
- A missed frame is discarded, not replayed.
- UI telemetry is sampled and bounded.
- Do not rebuild screen object trees during routine navigation.
- Multi-touch contact edges are consumed before animation work.

## Audio and transport plan

### Make the callback contract true

The audio callback may:

- pop preallocated commands;
- perform bounded arithmetic;
- update preallocated voice/transport state;
- write the provided output buffer;
- publish lock-free counters/snapshots.

It may not:

- allocate or deallocate;
- lock;
- log;
- parse;
- perform filesystem or socket I/O;
- send MIDI syscalls.

Fix clip ownership so the callback swaps an owning pointer without freeing or
boxing values. Reclamation must happen on a non-realtime thread.

### Reduce and measure buffering

Test direct ALSA configurations on the real onboard output rather than assuming
the default:

| Candidate | One-period duration at 44.1 kHz | Purpose |
|---|---:|---|
| 1,024 frames | 23.2 ms | Conservative first explicit configuration |
| 512 frames | 11.6 ms | Primary target |
| 256 frames | 5.8 ms | Attempt only if the driver/configuration accepts it reliably |

Test two and three periods where configurable. Record the actual negotiated:

- device;
- sample format;
- sample rate;
- period size;
- buffer size;
- callback frame count;
- xruns/deadlines.

The bcm2835 output has constraints and may reject a superficially valid fixed
size. Prefer a supported 512/1,024-frame configuration to an accidental ~4,410
frame default. If onboard audio cannot meet the feel target, document that as a
hardware limit rather than hiding it with a larger buffer.

### Timing ownership

Move into the engine:

- SEQ playback and overdub clock;
- phrase playback;
- songs;
- KAOSS gate/retrigger;
- drum note repeat;
- quantized launch and stop;
- USB/DIN timestamp scheduling.

The audio transport is the one musical clock. The UI edits and launches; it
does not sleep until note deadlines.

### Live-event timing

Preserve timestamps from MIDI/touch ingest and derive a frame offset into the
next renderable period where practical. Do not claim sample-accurate live input
while every live command is forced to frame zero of whichever callback sees it.

### Diagnostics

Expose real counters:

- actual callback count and frames;
- callback runtime p50/p95/p99/max;
- callback deadline misses / xruns;
- command-ring full count by class;
- UI reliable-edge queue depth;
- continuous-state overwrite count;
- audio and MIDI output late-event count;
- per-core CPU;
- engine/UI RSS;
- current CPU governor and frequency;
- `vcgencmd get_throttled` state/history;
- temperature;
- client disconnect and emergency-release count.

Do not label dropped control messages as xruns.

## CPU scheduling and affinity

Multiprocessing and CPU affinity are different:

- separate processes escape Python's GIL and isolate faults;
- affinity can reduce migration and contention jitter;
- neither makes inefficient rendering require fewer operations.

The current Rust daemon already supplies the major multiprocessing benefit.
Core 3 is not inherently faster than any other Pi 2 core.

Affinity is a late optimization after queue, renderer, and buffer corrections.
A possible measured layout is:

| Core | Role |
|---|---|
| 0 | Kernel housekeeping and most unrelated IRQs |
| 1 | MIDI I/O, IPC, storage/control |
| 2 | Native UI and rendering |
| 3 | Audio callback only |

Do not pin the entire engine service to core 3; that also places parsing,
hotplug, IPC, and other workers there. Set affinity in the actual callback
thread with `pthread_setaffinity_np`/`sched_setaffinity`, then verify it.

If measurement justifies true isolation:

- reserve core 3 with an appropriate cpuset or boot-time isolation;
- use `nohz_full`/RCU offload where supported and measured;
- route unrelated IRQs to housekeeping cores;
- keep an RT-throttling safety mechanism;
- set SCHED_FIFO only on intended realtime threads;
- lock memory before performance;
- set and restore the `performance` governor for instrument sessions.

Isolation can make performance worse if the chosen core receives interrupts,
the wrong threads inherit RT policy, or one core cannot meet the callback
deadline. Compare p99/max callback timing and xruns with and without it.

## Native vertical slice prototype

Do not port every screen first. Build the smallest slice that exercises every
risky boundary on the real Pi.

### Scope

The prototype boots to one native KAOSS/drum performance surface and provides:

1. direct TFT70 output through DRM/KMS at 800×480;
2. OpenGL ES 2 rendering;
3. all reported GT911 contacts with stable IDs;
4. the 12×7 CELLS visual at 60 Hz;
5. touch down/move/up/cancel;
6. one wavetable voice;
7. the 16-drum grid;
8. one touch-owned quarter-note kick repeat;
9. simultaneous free drum contacts while the repeat is held;
10. MPK note input;
11. Rust-engine communication using reliable edges plus latest XY;
12. reduced, explicit ALSA buffering;
13. visible diagnostic counters;
14. UI-process kill/restart with automatic engine emergency release.

Do not include Wi-Fi setup, updater UI, every preset screen, full songs, or
visual polish until this slice passes.

### Baseline comparison

Run the same test matrix on the current Tk app:

| Variable | Values |
|---|---|
| Engine | Python fallback / Rust remote |
| KAOSS visual | GLOW / CELLS |
| Gesture | tap / diagonal drag / rapid pitch scrub / five contacts |
| Audio | negotiated current default / explicit candidates |
| Load | idle / held voices + drums + FX |

Capture:

- touch event rate;
- time from physical/reportable up to note-off dispatch;
- client queue depth and drops;
- UI queue depth and drops;
- frame time;
- callback frames/runtime;
- audio stop time from loopback where possible.

### Acceptance criteria

These are prototype gates, not promises that measurements already satisfy:

| Signal | Gate |
|---|---|
| Touch-to-sound | p95 ≤ 20–25 ms on the chosen onboard-audio configuration |
| Release dispatch | p99 within one audio period; never behind continuous state |
| Stuck notes | zero after lift, cancel, UI crash, and reconnect tests |
| Intentional release tail | distinguished from dispatch delay in telemetry |
| KAOSS frame time | p95 ≤ 16.7 ms at 800×480; no growing backlog |
| Static screens | render only when dirty |
| Audio deadlines | zero misses in a long worst-case jam |
| Repeat timing | sample-clocked, no UI-timer drift |
| Multitouch | five contacts tracked independently if hardware exposes five |
| Queue behavior | no unbounded growth; continuous state overwrites old state |
| Memory | stable over repeated navigation and long performance |
| Fault isolation | killing UI releases its owned notes and audio continues safely |

If the native slice cannot pass, identify whether the limit is:

- TFT/driver multitouch;
- VC4/KMS rendering;
- ALSA/onboard audio;
- DSP compute;
- protocol/scheduling.

Only then consider changing hardware. Do not infer a Pi 2 limit from Tk behavior.

### Decision gate

Proceed with a native UI migration only if the vertical slice materially beats
the measured Tk baseline and the framework works reliably on the exact TFT70.
If SDL touch is the only failure, test direct libinput. If custom UI cost is the
failure, compare Qt Quick or LVGL on the same slice before changing the engine
direction.

## Immediate stabilization before the prototype

These are worthwhile even if Tk is later retired:

1. Suppress self-echoed KAOSS CC12/13/92 and touch-note log work.
2. Do not mutate the hidden Tk log for every high-rate event.
3. Give note-off, cancel, all-notes-off, and panic a reliable priority lane.
4. Coalesce XY and parameter motion to latest values.
5. Add a UI-session disconnect panic/watchdog.
6. Stop rebuilding KAOSS labels/axes on every motion sample.
7. Stop blanking scope traces before the replacement is ready.
8. Add real queue/drop/latency counters.
9. Measure the actual ALSA callback and negotiate smaller supported buffers.

These changes establish a fair baseline and make the existing instrument safer
while the native slice is developed.

## Migration sequence after a successful slice

### 1. Harden the engine

- make callback ownership allocation-free;
- add real deadline telemetry;
- preserve timestamped MIDI out;
- make RT policy and affinity explicit per thread;
- version the control protocol;
- add emergency release by client session.

### 2. Consolidate hardware ownership

- create one engine process that opens MPK/audio/DIN once;
- route events internally to synth, recorder, remapper, and output;
- define mode/routing state independently from the UI screen.

### 3. Move all musical clocks

- KAOSS gate and repeats;
- SEQ;
- phrase pads;
- songs and quantized starts/stops.

### 4. Port UI by risk/value

Suggested order:

1. KAOSS/full-pad and multitouch drums;
2. SYNTH controls and scopes;
3. SEQ transport/edit;
4. PADS;
5. SONGS/PRESETS;
6. HOME/SET/LOG/MAP.

Keep user-data formats stable and build adapters rather than forcing data loss.

### 5. Retire duplicate production paths

- remove Python PortAudio startup and fallback from the appliance;
- remove Python musical scheduling;
- keep Python tools/simulator only where useful;
- make systemd the sole engine lifecycle owner;
- retain a safe recovery/commissioning path.

## Test and release requirements

Add:

- Rust toolchain pinning;
- ordinary host CI for Rust and Python/domain tests;
- ARMv7 cross-build verification;
- native UI headless/screenshot tests;
- protocol-version tests;
- queue saturation and priority tests;
- five-contact synthetic touch tests;
- touch-cancel/UI-disconnect stuck-note tests;
- audio callback allocation checks where practical;
- offline DSP benchmarks;
- physical hardware smoke scripts for DSI, GT911, ALSA, MPK, and DIN.

For appliance updates:

- stage atomic versioned releases rather than overlaying arbitrary source;
- keep engine/UI protocol compatibility explicit;
- preserve user data;
- provide rollback if a new UI cannot talk to the installed engine;
- verify health before promoting an update.

## Open questions requiring hardware evidence

1. Does this exact TFT70 expose five Linux MT slots on the Pi 2?
2. Does SDL2 or SDL3 KMSDRM deliver correct contact IDs and coordinates?
3. Does the DSI connector appear reliably to direct DRM/KMS clients?
4. Which explicit ALSA period/buffer configurations work on bcm2835 headphones?
5. What touch-to-audio p95/p99 can onboard audio actually achieve?
6. How much Pi 2 headroom remains under worst-case voices, drums, and active FX?
7. Does native 60 Hz CELLS rendering leave enough shared memory/GPU bandwidth?
8. Does core isolation improve callback max latency after the major fixes?
9. Is SDL's custom UI workload preferable to Qt Quick or LVGL after one screen?
10. What repeat capture behavior feels best when a finger slides off a pad?

## Reference links

### Hardware and Linux

- [Raspberry Pi 2 Model B](https://www.raspberrypi.com/products/raspberry-pi-2-model-b/)
- [BTT TFT70 V2.1 listing](https://kb-3d.com/store/controllers-displays-drivers/2677-bigtreetech-pi-tft43-tft50-tft70-v21-touchscreen-panel-for-raspberry-pi-pi-2-1734017888380.html)
- [Linux multitouch protocol](https://docs.kernel.org/input/multi-touch-protocol.html)
- [Linux thread affinity](https://man7.org/linux/man-pages/man3/pthread_setaffinity_np.3.html)
- [Raspberry Pi throttling flags](https://github.com/raspberrypi/documentation/blob/master/documentation/asciidoc/computers/os/graphics-utilities.adoc)

### UI frameworks

- [SDL2 touch events](https://wiki.libsdl.org/SDL2/README-touch)
- [SDL3 touch API](https://github.com/libsdl-org/SDL/blob/main/include/SDL3/SDL_touch.h)
- [Qt Quick MultiPointTouchArea](https://doc.qt.io/qt-6/qml-qtquick-multipointtoucharea.html)
- [Qt embedded Linux input](https://doc.qt.io/qt-6/inputs-linux-device.html)
- [LVGL multitouch pointers](https://docs.lvgl.io/master/details/main-modules/indev/pointer.html)
- [LVGL Linux DRM](https://docs.lvgl.io/latest/en/html/API/drivers/display/drm/index.html)
- [Slint LinuxKMS](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backend_linuxkms/)
- [JUCE licensing](https://github.com/juce-framework/JUCE/blob/master/LICENSE.md)

## Bottom line

The current problems are evidence of a transitional architecture, not evidence
that the Pi 2 cannot be an instrument.

Preserve the Rust realtime direction. Finish the engine's ownership of musical
time and hardware. Replace fine-grained Tk mutation with a native, complete-frame
renderer if and only if the focused vertical slice proves it on the real Pi 2
and TFT70. Treat multitouch, note release, bounded queues, and measured latency
as foundational requirements rather than later polish.
