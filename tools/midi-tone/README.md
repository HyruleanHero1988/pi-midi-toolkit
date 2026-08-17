# midi-tone (Phase 0 diagnostic → tiny DIY soft-synth)

Hear and see MIDI from the **Akai MPK mini** on the Pi **without** USB-DIN or a hardware synth.

- Opens a MIDI input (prefers a port name containing `MPK`)
- Note-on → wavetable tone through the Pi audio jack / HDMI
- **Modes** (top right): **SYNTH**, **SEQ**, **PADS**, **KAOSS**, **SONGS**, **PRESETS**, **LOG** — fully separate UIs
- Synth: voices, A/B morph, knobs, live morph-cycle scope; **MPK pads (ch10) = analog drum voices**; **KIT** drill-down scopes a selected drum
- Sequencer: record a backbone loop, then overdub layers of drums and keys over it
- Pads: 16 phrase clips (Bank A+B); record from keys, launch from touch squares **or** MPK pads
- Kaoss: full-screen XY pad (Kaossilator-style notes + original Kaoss Pad MIDI CCs) for the soft-synth and/or USB→DIN
- Songs: scrolling list of every `.mid` in `songs/`, tempo, play local and/or USB→DIN
- Presets: 8 save slots + autosave last session (`settings.json`)
- Log: full scrolling MIDI/event history
- Bundled [Adventure Kid (AKWF)](https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE) single-cycles (**CC0**) plus built-in sine/square/saw/triangle

## On the Pi (via SSH is fine)

```bash
cd ~/midi-tone
# after scp from PC:
sed -i 's/\r$//' *.sh
chmod +x setup-venv.sh run.sh install-desktop-shortcut.sh
./setup-venv.sh
```

Run (window appears on the Pi screen):

```bash
export DISPLAY=:0
export XDG_RUNTIME_DIR=/run/user/$(id -u)
./run.sh --input MPK
```

Useful flags:

```bash
./run.sh --input MPK --voices 12          # polyphony (default 12)
./run.sh --waves-dir ./wavetables         # extra single-cycle WAVs
./run.sh --list                           # MIDI ports + loaded voices
```

Plug headphones or speakers into the Pi. Play the MPK — you should hear tones and see Note On/Off / CC lines.

### MPK mini knobs (morph + tone)

Factory **Prog Select → Pad 1** (MPC program) maps knobs to CC70–77:

| Knob | CC | Control |
|------|----|---------|
| 1 | 70 | **Morph** — blend between the chosen A/B pair |
| 2 | 71 | Tone / brightness (low-pass) |
| 3 | 72 | Attack |
| 4 | 73 | Release |
| 5 | 74 | Vibrato depth |
| 6 | 75 | Vibrato rate |
| 8 | 77 | Level |

Joystick Y still sends **CC1** = vibrato amount. PREV/NEXT jumps morph to a voice; Knob 1 sweeps continuously between them.

**Vibrato without a wheel:** the **VOICES** screen has a vibrato row — `DEPTH −/+` (0–2 semitones) and `RATE −/+` (1–9 Hz), plus a toggle:

- **WHEEL** — vibrato follows CC1 (joystick), as it always has
- **ON** — vibrato runs at the set depth with no wheel input; the joystick can still push it further (whichever asks for more wins)

Raising depth from the screen while the joystick is centred flips the toggle to **ON**, so the control you just touched is the one you hear. Depth, rate and the toggle are saved in `settings.json` and presets, and knobs 5/6 keep editing the same values.

If a knob does nothing, check the event log for its CC number — your MPK program may differ.

### Drum pads (channel 10) — all 16 MPK pads

MPK drum pads no longer play pitched wavetable keys. They trigger **procedural analog-style one-shots** (Synsonics / TR-ish).

Factory MPC program (`Prog Select` → Pad 1): **Bank A = notes 36–43**, **Bank B = 44–51** (8 pads × 2 banks). Pads 1–4 are the bottom row L→R; 5–8 the top row.

| Bank A note | Sound | Bank B note | Sound |
|-------------|-------|-------------|-------|
| 36 | kick | 44 | kick_tight |
| 37 | snare | 45 | rimshot |
| 38 | clap | 46 | shaker |
| 39 | hat_closed | 47 | hat_pedal |
| 40 | hat_open | 48 | tom_hi |
| 41 | tom_lo | 49 | cowbell |
| 42 | tom_mid | 50 | clave |
| 43 | rim | 51 | ride |

**Drum knobs** — only when **DRUM MODE** is on (Synth toolbar toggle):

| Knob | CC | Drum control |
|------|----|----------------|
| 1 | 70 | Pitch / tune |
| 2 | 71 | Noise brightness (tone) |
| 3 | 72 | Stretch / decay length |
| 4 | 73 | Noise amount |
| 8 | 77 | Master level (always) |

Hitting pads does **not** steal morph knobs. With DRUM MODE off, Knob 1 stays morph. Opening **MORPH** turns DRUM MODE / FX MODE / BUS FX off. Session-only (not saved across restart).

**FX MODE** (Synth toolbar) — **per-instrument inserts** + a shared kit bus. Knobs edit the current target:

- Default target: the **nearer morph endpoint** wavetable (`voice:saw`, …).
- Open **KIT** → **ALL DRUMS** → shared **kit-group** FX (`drums`) so one echo/drive hits the whole kit without wetting the melody.
- Open **KIT** and tap a single pad → that **drum model** only (`drum:kick`, …). Closing KIT keeps `drums` if that was selected; otherwise returns to the morph voice.
- Locked phrase pads keep their `morph_a` wavetable’s FX chain.

**BUS FX** (separate toolbar button) — **master mix-bus** wet after keys + drums are summed. Same knob map as FX MODE, but the whole soft-synth output gets drive/delay/reverb. Insert FX still runs underneath; the two modes are mutually exclusive for knob focus (and with DRUM MODE).

| Knob | CC | FX control (FX MODE or BUS FX) |
|------|----|------------|
| 1 | 70 | Drive / saturation |
| 2 | 71 | Delay time (~50–750 ms) |
| 3 | 72 | Delay feedback |
| 4 | 73 | Delay mix |
| 5 | 74 | Reverb size |
| 6 | 75 | Reverb mix |
| 8 | 77 | Master level (always) |

Amounts persist in `settings.json` / presets (`voice_fx` / `drum_fx` / `drum_group_fx` / `bus_fx`); the mode toggles themselves do not.

**Waveforms:** SYNTH shows a live **morph-cycle** scope that redraws as Knob 1 / voice changes. Tap **KIT** for a drum drill-down — pick a pad (touch or MPK), watch its one-shot reshape with pitch / stretch / noise / tone knobs (DRUM MODE turns on while KIT is open). Keeps the main synth screen uncluttered.

**SAVE AS…** (from **VOICES** or **MORPH**) writes a user voice under `user-wavetables/`:

- `<name>.wav` — morph + **drive** + **tone** baked into the single-cycle shape
- `<name>.fx.json` — delay/reverb numbers alongside (mix + time/feedback/size). Drive is not stored here (already in the wave)

Selecting that voice restores the delay/reverb sidecar. Built-ins can’t be overwritten.

Keyboard notes keep the wavetable morph synth. Pad aftertouch still trims the ringing hit. If a pad program uses other note numbers, unknown notes still cycle through the 16 voices.

### Modes

Top-right tabs stay visible:

- **SYNTH** — wavetable soft-synth, voice grid, morph pair
- **SEQ** — record a backbone loop, overdub layers over it, KEEP / DROP / UNDO (free timing; notes only)
- **PADS** — 4×4 phrase clip launcher (MPK Bank A+B); touch or drum pads
- **KAOSS** — XY touch pad with a Kaoss-style LED field: play scale notes (local and/or USB MIDI) or sweep FX; HOLD / GATE ARP
- **SONGS** — lists every `.mid` / `.midi` in `songs/`; big ▲ UP / ▼ DOWN to scroll; tempo; LOCAL/USB out
- **PRESETS** — 8 touch slots: SAVE / LOAD / DELETE current sound + full-velocity
- **LOG** — full event history (also has CLEAR / panic)

Last session autosaves to `settings.json` (gitignored) every few seconds and on quit.
Named presets live in `user-presets/slot-01.json` … `slot-08.json`.
User-saved morph wavetables live in `user-wavetables/*.wav` (+ `*.fx.json` for delay/reverb, gitignored).
Song files live as whatever you put in `songs/` (any `.mid` name).
Phrase pads persist as `phrases/pad-01.json` … `pad-16.json` (gitignored).

### Sequencer (SEQ)

808-style overdubbing: build a beat by adding to it while it plays. Drums and keys both record.

1. Open **SEQ**, tap **REC BACKBONE**, play the groove
2. Tap **REC** again — the take is trimmed, its length becomes the loop, and it starts playing
3. Tap **REC** again to **overdub**: play more drums or a melody over the running loop. What you played shows up on the next pass, so you hear the layer in place before deciding
4. **KEEP** flattens the layer onto the sequence · **DROP** throws it away · **UNDO** peels the last kept layer back off
5. **STOP** halts playback (material stays) · **CLEAR** starts over

Each take (backbone and every overdub) **bakes the vibrato it was played with** — depth, rate, and amount — onto that layer. Key notes from that layer keep their own LFO on playback; changing the live vibrato afterwards (or recording a dry layer over a wobbly one) doesn't rewrite older layers. Drum hits never take vibrato. The layer strip shows `vib 0.9st` / `vib none` next to each take.

The backbone is the only take that sets length; everything after it is measured in backbone cycles.

- **LEN ×2 / ÷2** grows or shrinks the sequence in whole cycles. The groove tiles underneath, so a doubled sequence lets you overdub a fill that only happens the second time around. ÷2 refuses to cut a layer that is longer than the target.
- **OVERDUB: WRAP** (default) folds a long take back onto the same cycle, the way a drum machine does. **OVERDUB: EXTEND** instead stretches the sequence to as many whole backbone cycles as the take needs (up to 8).
- Layers are a stack, not one flat list, so UNDO works layer by layer. **SONGS → SAVE SEQ** exports the flattened result as `.mid`.

On stop, every free-timing take is **auto-trimmed**: leading silence before the first hit is removed, and trailing silence after the last hit is capped to the largest gap between note-ons (so lag hitting REC doesn’t leave a dead bar at the loop point). The same trim runs on phrase-pad recordings.

Live playing still works while the sequence runs. Voice/morph/knob settings from Synth apply to sequenced notes too. Recording keeps going if you switch modes — handy for changing voice mid-overdub.

### Kaoss pad

The 7″ capacitive panel is the instrument. **KAOSS** is a Kaossilator-style
play pad plus the original Kaoss Pad’s factory MIDI, so the same finger drives
the onboard wavetable engine **and** a hardware synth on USB→DIN.

| Gesture / control | What it does |
|-------------------|--------------|
| Finger on the pad | Note-on. L-shaped axes: **X** along the bottom (scale pitch), **Y** up the left (tone / morph / vibrato — see PROG). LED field: hue follows X, glow + trail under the finger, tap ripples, GATE / BPM pulse the rim |
| Slide | Legato to the next scale degree; Y keeps sculpting |
| Lift | Note-off — unless **HOLD** is on (last XY stays sounding) |
| **PROG** | `LEAD` / `MORPH` / `VIB` play notes; `FILTER` / `ECHO` / `DRIVE` / `SPACE` are Kaoss-Pad FX (momentary unless HOLD) |
| **SCALE** / **KEY** / **OCT** | **SCALE** opens a VOICES-style grid of full names (Major, Mixolydian, Miyakobushi, …) — not the 3-letter Korg codes. KEY / OCT still cycle. Default list is a short starter set |
| **SHOW ALL** | PRESETS → **KAOSS: ALL**, or the **SHOW ALL** button on the pad — unlocks every factory Kaossilator scale (31 + PRO+ extras) and every XY program the engine can drive |
| **GATE** | Off, 1/8, 1/16, or triplet retrigger while the pad is down (BPM − / +) |
| **OUT** | `LOCAL` / `USB` / `BOTH` — same USB port as Songs / Pads |
| **CH** | MIDI channel 1–16 for notes + CCs |

External synths see the original Kaoss Pad factory map: **CC#12 = X**, **CC#13 = Y**,
**CC#92 = pad touch** (127 down / 0 up), plus note-on/off on the chosen channel.
FX programs write the mix-bus insert while the finger is down and restore the
previous bus on lift (HOLD freezes the wet). Playing into **SEQ** while it is
recording captures the pad notes onto the take.

### Phrase Pads

Two views (top-right of PADS): **EDIT** (record / fine-tune) and **PLAY** (perform).

**EDIT**
1. Open **PADS** (defaults to EDIT)
2. Tap an **empty** square (or matching MPK pad) to arm record
3. Play **keyboard and/or drum pads** — both are captured into that cell
4. **STOP REC** or tap that square again to finish (saved under `phrases/`)
5. Tap a **filled** square to launch + select it
6. Bottom rows:
   - **MODE** then tap a pad → ONE-SHOT ↔ LOOP (or use **TRIG** on the selected pad)
   - **CLEAR** then tap a pad → erase
   - **FOLLOW** / **LOCK** — FOLLOW uses the live morph and the live master level; LOCK freezes the current morph **and the master level** onto that pad (multi-timbre; up to 4 locked pads at once)
   - **VOL− / VOL+** — per-pad trim, 10% a tap (10–200%). Audible on the next note, so you can balance a pad against the mix while it loops. Switching a pad back to FOLLOW resets its trim to 100%
   - **VIB** — vibrato is captured when you stop recording, so a phrase keeps the wobble it was played with no matter what the rig does later. Tap to switch that pad between its baked value (`VIB 0.9st`, or `VIB none` if you recorded dry) and the live rig (`VIB live`); LOCK re-captures it along with the voice
   - **CH:rec** / **CH:n** — emit on recorded channels or force MIDI ch 1–16
   - **SYNTH** / **MIDI** — local soft-synth on/off (off = sequence only over USB/DIN)
   - **OUT: LOCAL / USB / BOTH** — session routing (shares the Songs USB port)
7. **STOP ALL** stops playing phrases

**PLAY**
- Launch / stop filled pads only (empty pads do not arm record)
- **STOP ALL** + **OUT** — minimal chrome for performance

While a phrase is **recording**, MPK pads play/record **drum voices** (not launch). When not recording, those pads launch/arm phrases (EDIT) or launch only (PLAY). Synth mode still always plays the 16-pad drum kit. Locked pads show a `·` mark next to the trigger icon.

### Songs

**How to load:** open **SONGS**, use **▲ UP / ▼ DOWN** if needed, **tap a file row** (turns purple), then **PLAY**.

Ways to get files into `songs/`:

1. **Bundled demos (offline)** — `demo-songs/` ships **12 classical Mutopia MIDIs** with the deploy (Bach, Beethoven, Debussy, Joplin, Mozart, Satie, …). On launch, any missing ones are copied into `songs/` (no internet). See `demo-songs/LICENSE.txt`.
2. **Record your own** — SEQ → record → **SONGS** → **SAVE SEQ** (writes `take-001.mid`, `take-002.mid`, …)
3. **Optional online refresh** — if the Pi has network:
   ```bash
   cd ~/midi-tone
   ./venv/bin/python fetch_songs.py --list
   ./venv/bin/python fetch_songs.py --all
   ```
4. **Copy / scp any `.mid`** into `songs/` — it shows up in the list next time you open SONGS (or restart).

`songs/` is Pi-local (gitignored). Redeploying code does **not** wipe it; **DELETE** removes the selected file.

Transport:

- Set **BPM** (− / + / ±5); optional **SONG LOOP**
- **OUT:** **LOCAL** (soft-synth) → **USB** (MIDI out / DIN) → **BOTH**
- **PLAY** / **STOP** / **DELETE**

Song USB out is a file-player path (not live thru remap).

### Voices / wavetables

Tap **VOICES** (or the current voice name) for a full-screen grid of large buttons — one per loaded wavetable. **PREV / NEXT** still step one at a time.

Tap **MORPH** to pick a pair: arm **A** or **B**, tap two voices, **DONE**. Knob 1 then blends only **A → B** (not the whole library). **SWAP** flips the pair.

Drop any mono single-cycle `.wav` into `wavetables/` and restart — the stem becomes the voice name.

Fetch more AKWF cycles (needs network):

```bash
./venv/bin/python fetch_akwf.py --list
./venv/bin/python fetch_akwf.py --all
```

### Desktop icon

```bash
cd ~/midi-tone
sed -i 's/\r$//' install-desktop-shortcut.sh   # if copied from Windows
chmod +x install-desktop-shortcut.sh
./install-desktop-shortcut.sh
```

Then double-click **MIDI Tone** on the desktop. If needed: right-click → **Allow Launching**.

**SSH note:** Tk needs a display. Prefer a terminal *on the Pi desktop*. Over SSH use `ssh -X ray@192.168.1.225` (X11 forwarding) if your PC has an X server.

### Kiosk mode (no Pi desktop shell)

Boots a minimal **X11** session that only runs midi-tone fullscreen (restart
loop if it crashes). No wallpaper / panel / file manager / labwc.

**Display target:** BigTreeTech **Pi TFT70 V2.1** (7″ DSI, 800×480, capacitive GT911).
Legacy resistive HDMI/ADS7846 helpers (`enable-gpio-touch.sh` / `calibrate-touch-y.sh`) are for the old panel only.

**New TFT70 bring-up (DSI):**
```bash
cd ~/midi-tone
sed -i 's/\r$//' enable-tft70-dsi.sh
chmod +x enable-tft70-dsi.sh
./enable-tft70-dsi.sh    # removes ads7846 overlay, enables vc4-kms-dsi-7inch
sudo reboot
```
After reboot expect `800x480` on DSI and a Goodix/GT911 (or bridge) touch device in `libinput list-devices`.
If SSH is up but the screen is still blank, confirm the DSI ribbon orientation and that HDMI is no longer required.

**Full bring-up guide (new Pi → same state as the lab unit):** see
[`KIOSK.md`](KIOSK.md).

```bash
cd ~/midi-tone
sed -i 's/\r$//' *.sh kiosk/openbox/* kiosk/*.desktop kiosk/lightdm/* 2>/dev/null
chmod +x install-kiosk.sh disable-kiosk.sh kiosk.sh
./install-kiosk.sh          # packages + session + enable boot (needs sudo)
sudo reboot
```

Restore the normal desktop later:

```bash
./disable-kiosk.sh
sudo reboot
```

Manual test: `./kiosk.sh`  
Logs: `/tmp/midi-tone-kiosk.log` and `/tmp/midi-tone.log`.

Files:
- `kiosk.sh` — session entry (display prefer + cursor hide + app restart loop + `--fullscreen`)
- `prefer-tft70-display.sh` / `hide-touch-cursor.sh` / `enable-tft70-dsi.sh` — TFT70 helpers
- `kiosk/openbox/rc.xml` — undecorated maximized windows
- `kiosk/midi-tone-kiosk.desktop` — X session definition
- `kiosk/lightdm/99-midi-tone-kiosk.conf` — LightDM seat defaults
- `install-kiosk.sh` / `disable-kiosk.sh` — enable / restore desktop


## Screen reference

Open [`docs/index.html`](docs/index.html) in a browser for labeled 800×480 captures of every mode (SYNTH, SEQ, PADS, KAOSS, SONGS, PRESETS, LOG) and the VOICES / MORPH / KIT / POWER / SAVE AS / KAOSS scales overlays.

Re-capture after UI changes (needs Tk + an 800×480 X display, or the script starts Xvfb itself):

```bash
python capture_ui_docs.py
```

## On Windows (optional host test)

```powershell
cd tools\midi-tone
python -m pip install -r requirements.txt
python midi_tone.py --list
python midi_tone.py --input MPK
```

## Panic

UI button **All Notes Off**, or MIDI CC 123.

## Touch tips (Pi panel)

- Buttons fire on **press** (resistive panels often miss a full click).
- Event history lives in **LOG** mode (not buried under the synth controls).
- If taps feel vertically offset: `./calibrate-touch-y.sh`
