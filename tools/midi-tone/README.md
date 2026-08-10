# midi-tone (Phase 0 diagnostic → tiny DIY soft-synth)

Hear and see MIDI from the **Akai MPK mini** on the Pi **without** USB-DIN or a hardware synth.

- Opens a MIDI input (prefers a port name containing `MPK`)
- Note-on → wavetable tone through the Pi audio jack / HDMI
- **Modes** (top right): **SYNTH**, **LOOPER**, **PADS**, **SONGS**, **PRESETS**, **LOG** — fully separate UIs
- Synth: voices, A/B morph, knobs, live morph-cycle scope; **MPK pads (ch10) = analog drum voices**; **KIT** drill-down scopes a selected drum
- Looper: record a MIDI note sequence, play it on repeat
- Pads: 16 phrase clips (Bank A+B); record from keys, launch from touch squares **or** MPK pads
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

Hitting pads does **not** steal morph knobs. With DRUM MODE off, Knob 1 stays morph. Opening **MORPH** turns DRUM MODE / FX MODE off. Session-only (not saved across restart).

**FX MODE** (Synth toolbar) — mix-bus effects on the whole soft-synth output (keys + drums + launched phrases):

| Knob | CC | FX control |
|------|----|------------|
| 1 | 70 | Drive / saturation |
| 2 | 71 | Delay time (~50–750 ms) |
| 3 | 72 | Delay feedback |
| 4 | 73 | Delay mix |
| 5 | 74 | Reverb size |
| 6 | 75 | Reverb mix |
| 8 | 77 | Master level (always) |

FX amounts persist in `settings.json` / presets; the FX MODE toggle itself does not.

**Waveforms:** SYNTH shows a live **morph-cycle** scope that redraws as Knob 1 / voice changes. Tap **KIT** for a drum drill-down — pick a pad (touch or MPK), watch its one-shot reshape with pitch / stretch / noise / tone knobs (DRUM MODE turns on while KIT is open). Keeps the main synth screen uncluttered.

Keyboard notes keep the wavetable morph synth. Pad aftertouch still trims the ringing hit. If a pad program uses other note numbers, unknown notes still cycle through the 16 voices.

### Modes

Top-right tabs stay visible:

- **SYNTH** — wavetable soft-synth, voice grid, morph pair
- **LOOPER** — record MIDI notes, then play them back on a loop (free timing; notes only)
- **PADS** — 4×4 phrase clip launcher (MPK Bank A+B); touch or drum pads
- **SONGS** — lists every `.mid` / `.midi` in `songs/`; big ▲ UP / ▼ DOWN to scroll; tempo; LOCAL/USB out
- **PRESETS** — 8 touch slots: SAVE / LOAD / DELETE current sound + full-velocity
- **LOG** — full event history (also has CLEAR / panic)

Last session autosaves to `settings.json` (gitignored) every few seconds and on quit.
Named presets live in `user-presets/slot-01.json` … `slot-08.json`.
Song files live as whatever you put in `songs/` (any `.mid` name).
Phrase pads persist as `phrases/pad-01.json` … `pad-16.json` (gitignored).

### Looper

1. Open **LOOPER**
2. Tap **RECORD**, play notes on the MPK
3. Tap **RECORD** again (or **STOP**) to finish
4. Tap **PLAY** to loop; tap **PLAY**/**STOP** to halt
5. **CLEAR** wipes the take

On stop, the take is **auto-trimmed**: leading silence before the first hit is removed, and trailing silence after the last hit is capped to the largest gap between note-ons (so lag hitting STOP doesn’t leave a dead bar at the loop point). Same trim applies to phrase-pad recordings.

Live playing still works while a loop runs. Voice/morph/knob settings from Synth apply to looped notes too.

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
   - **FOLLOW** / **LOCK** — FOLLOW uses the live morph; LOCK freezes the current morph onto that pad (multi-timbre; up to 4 locked pads at once)
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
2. **Record your own** — LOOPER → record → **SONGS** → **SAVE LOOP** (writes `take-001.mid`, `take-002.mid`, …)
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

Boots a minimal **X11 + Openbox** session that only runs midi-tone fullscreen (restart loop if it crashes). No wallpaper / panel / file manager.

**Display target:** BigTreeTech **Pi TFT70 V2.1** (7″ DSI, 800×480, capacitive GT911) is on order — same pixel budget the UI already uses, larger physical targets, reliable touch. Until it arrives, the existing resistive HDMI/ADS7846 panel still works; `enable-gpio-touch.sh` / `calibrate-touch-y.sh` are **legacy for that panel only**.

```bash
cd ~/midi-tone
sed -i 's/\r$//' *.sh kiosk/openbox/* kiosk/*.desktop
chmod +x install-kiosk.sh kiosk.sh
./install-kiosk.sh
```

Then:
1. `sudo raspi-config` → **Advanced Options → Wayland → X11**
2. `sudo raspi-config` → **System Options → Boot / Auto Login → Desktop Autologin**
3. Choose session **MIDI Tone Kiosk** (install writes `~/.dmrc`)
4. Reboot

Manual test: `./kiosk.sh`  
Logs: `/tmp/midi-tone-kiosk.log`

Files:
- `kiosk.sh` — session entry (Openbox + app restart loop + `--fullscreen`)
- `kiosk/openbox/rc.xml` — undecorated maximized windows
- `kiosk/midi-tone-kiosk.desktop` — X session definition
- `install-kiosk.sh` — packages + session install

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
