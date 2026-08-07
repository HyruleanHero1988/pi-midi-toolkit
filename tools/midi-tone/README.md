# midi-tone (Phase 0 diagnostic → tiny DIY soft-synth)

Hear and see MIDI from the **Akai MPK mini** on the Pi **without** USB-DIN or a hardware synth.

- Opens a MIDI input (prefers a port name containing `MPK`)
- Note-on → wavetable tone through the Pi audio jack / HDMI
- **Modes** (top right): **SYNTH**, **LOOPER**, **LOG** — fully separate UIs
- Synth: voices, A/B morph, knobs, live status
- Looper: record a MIDI note sequence, play it on repeat
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

### Modes

Top-right tabs stay visible:

- **SYNTH** — wavetable soft-synth, voice grid, morph pair
- **LOOPER** — record MIDI notes, then play them back on a loop (free timing; notes only)
- **LOG** — full event history (also has CLEAR / panic)

### Looper

1. Open **LOOPER**
2. Tap **RECORD**, play notes on the MPK
3. Tap **RECORD** again (or **STOP**) to finish
4. Tap **PLAY** to loop; tap **PLAY**/**STOP** to halt
5. **CLEAR** wipes the take

Live playing still works while a loop runs. Voice/morph/knob settings from Synth apply to looped notes too.

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
