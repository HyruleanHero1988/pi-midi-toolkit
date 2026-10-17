# SMK-37 Elite — arrival setup

PiDI talks to **any class-compliant USB MIDI controller**. The Akai MPK mini
mk3 and the M-VAVE SMK-37 Elite (this AliExpress 37-key / 16-pad unit, also
sold as SMK-37 Pro, Starrykey 37 Play, MKE-P37) each have a named profile.
Plug either one in, or swap mid-session: PORTS → IN AUTO still grabs every
USB MIDI input, and the live map follows the port name.

This file is the desk checklist for when the Elite lands. Software already
selects the `smk-37-elite` profile from ALSA names containing `smk`,
`m-vave`, `starrykey`, or `mke-p37`. You should not need a rebuild unless
the factory CC map disagrees with the overlay.

## 0. What PiDI already does

| Surface | MPK mini mk3 | SMK-37 Elite |
| --- | --- | --- |
| On-screen piano | Hidden when plugged in | Hidden when plugged in |
| Keys | Notes + pitch/mod wheels | Notes + pitch/mod wheels |
| Knobs 1–8 | CC 70–77 absolute pots (FX page: punch depth) | CC 48–55 **relative** encoders (FX page: punch depth nudges) |
| Knobs 9–16 | — | CC 56–63 → bus FX |
| Faders | — | CC 20–25 → MIX LIVE / KIT / SEQ DRM / KEY / KSS / MIC |
| Pads 1–8 | Bank A row-swapped (40–43 / 36–39) punch + kit | Sequential 36–43 punch + kit |
| Pads 9–16 | Bank B row-swapped | Sequential 44–51 kit / phrases |
| FULL VEL | Default **on** (MPK pads are anemic) | Default **off** (curve is usable) |
| Sustain | CC 64 | CC 64 (pedal jack) |
| Pad aftertouch | — | Poly pressure → punch amount on FX |
| MIDI OUT auto | Skips the MPK | Skips the SMK (U2MIDI stays DIN) |
| Headphone / mic auto | Skips MPK USB audio | Skips the Elite USB sound card |

Unknown class-compliant keyboards keep the generic (MPK-compatible) map so
they still play notes.

## 1. Power and cables

1. Use the Elite’s **USB-C** cable into a powered hub or a Pi USB port that
   can supply the board. If ALSA drops the device under load, put it on a
   powered hub — do not share a flaky wall-wart with the Pi.
2. Leave the 3.5 mm MIDI jack unconnected. PiDI’s DIN path is the U2MIDI PRO.
3. Optional: TRS sustain pedal in the Elite’s pedal jack (CC 64).
4. Do **not** pick the Elite as SET → AUDIO output or as the USB mic. The
   engine already ranks the Pi headphone jack and skips SMK/M-VAVE names
   unless the filter is `smk`.

## 2. Firmware and MidiSuite (Windows/macOS, once)

Do this on a laptop **before** the unit lives on the Pi.

1. Install [M-VAVE MidiSuite](https://www.m-vave.com/) (or the vendor’s
   current editor). Update firmware if the editor offers it.
2. Set **Local Control = OFF** so USB MIDI does not echo internally.
3. Keys on channel 1. Pads on channel 10 (zero-based 9) — same as MPK.
4. Encoders: **relative signed** (1 = +1, 127 = −1). If the editor only has
   “Inc/Dec” or “Relative 2”, that is the one we want.
5. If the editor can set encoder CCs, use **48–55** (bank 1) and **56–63**
   (bank 2). Faders **20–25**. If you cannot set CCs here, we remap in the
   overlay JSON after a learn pass.
6. Save the scene on the hardware. Unplug from the laptop.

Bluetooth MIDI is not used. Leave the radio off.

## 3. First plug into the Pi

```bash
# Device showed up?
aseqdump -l
# Expect something like:
#   20:0  SMK-37 Elite-Master
#   21:0  SMK-37 Elite-Private   (often empty / clock)
#   22:0  SMK-37 Elite-External  (LED / feedback)

cargo run -p jambox-engine -- devices
# MIDI inputs should list the Elite. MIDI outputs should still pick U2MIDI
# when the OUT filter is empty.

# Watch live traffic (keys, pads, knobs):
aseqdump -p 'SMK-37 Elite-Master'
```

On the kiosk: SET → PORTS. IN can stay **AUTO**. OUT should be **U2MIDI**,
not any `SMK-…` port. SYNTH should hide the on-screen keyboard.

## 4. Learn pass (15 minutes with `aseqdump`)

Write down what the unit actually sends. Compare to
[`presets/controllers/smk-37-elite.json`](presets/controllers/smk-37-elite.json).

| Control | Twist / press | Expected | If different |
| --- | --- | --- | --- |
| Key C3 | note on ch 1 | note 60, velocity 1–127 | leave FULL VEL off unless it is stuck at 127 |
| Pitch wheel | bend | pitch_bend | already handled |
| Mod wheel | CC 1 | vibrato amount | already handled |
| Encoder 1 clockwise one tick | CC 48 = 1 | Morph +1/64 (FX page: punch depth +1/64) | if it jumps 0–127, set `knob_encoding` to `absolute` |
| Encoder 1 counterclockwise | CC 48 = 127 | Morph −1/64 (FX page: punch depth −1/64) | same |
| Encoder bank 2, knob 1 | CC 56 | bus Drive | edit `knob_bank2_ccs` |
| Fader 1 | CC 20 | MIX LIVE | edit `fader_ccs` |
| Pad 1 (top-left) | ch 10 note 36 | kit kick / punch RPT | edit `phrase_notes` / `punch_pad_notes` |
| Pad 5 | ch 10 note 40 | kit cell 4 | sequential 4×4, no MPK row-swap |
| Pad aftertouch | poly pressure | punch amount on FX | skip if the unit has no pad pressure |
| Sustain pedal | CC 64 ≥ 64 | hold live keys | if a fader is also CC 64, remap the fader |
| Pad bank 2 transport | notes 91–95 / 76 | ignored (not drums) | add extras to `transport_notes` |

Copy the overlay onto the Pi and edit it. User files win over factory
presets:

```bash
mkdir -p ~/.local/share/pidi/controllers
cp presets/controllers/smk-37-elite.json ~/.local/share/pidi/controllers/
# edit CCs, then:
sudo systemctl restart jambox-engine pidi-native
```

`PIDI_CONTROLLERS_DIR` overrides that path. Factory maps load from
`PIDI_REPO_ROOT/presets/controllers` when that env is set, otherwise from
`presets/controllers` under the process working directory (the engine unit
already `WorkingDirectory=`s the git tree). The user copy still wins.

## 5. Swap with the MPK mini

1. Unplug one, plug the other. Wait for PORTS to show the new IN name
   (hotplug is ~400 ms).
2. MPK: knobs 70–77, Bank A pads row-swapped, FULL VEL on (unless you had
   toggled it).
3. Elite: knobs 48–55 relative, 4×4 pads 36–51, FULL VEL off (unless you
   had toggled it).
4. Both can stay plugged in. Empty IN filter opens every USB MIDI input;
   each port keeps its own profile, so MPK knobs and Elite encoders do not
   steal each other’s CCs.
5. Do not set OUT to either keyboard unless you are debugging pad LEDs.

## 6. RGB pads

Clip launch/stop sends note-on/off to a port whose name matches **both**
the Elite and `external` / `third`. That is a stub: many M-VAVE boards light
pads from those notes, some need SysEx we have not dumped yet.

If pads stay dark:

```bash
aseqdump -p 'SMK-37 Elite-External'   # watch what the editor sends
# or send a probe from another machine while MidiSuite lights a pad
```

Paste the bytes into an issue. Do **not** point OUT at External for the DIN
synth — the LED thread is a side channel.

## 7. Audio trap

The Elite is a USB composite device (MIDI + a tiny sound card). PiDI will
not auto-select it for headphones or the mic. If `aplay -l` still shows it
as default on a raw ALSA session, that is outside the engine — keep
`jambox-engine` `--output headphone` (or empty; analog ranks highest).

To force the Elite card on purpose: SET → AUDIO filter `smk`.

## 8. When to rebuild vs edit JSON

| Change | Where |
| --- | --- |
| Factory CC numbers, relative vs absolute, extra transport notes | `~/.local/share/pidi/controllers/smk-37-elite.json` |
| New controller brand | add a profile in `crates/midi-core/src/controller.rs` + a JSON file |
| MPK behaviour | leave `mpk-mini` alone |

After JSON-only edits, restart the two systemd units. After a Rust change,
deploy as usual (`deploy/deploy-native-slice.sh` / CI armv7 bins).
