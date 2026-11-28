# PiDI hardware build plan

This document is the hardware source of truth for the PiDI appliance: board layout, power, audio, jacks, enclosure revisions, and build rules. Last updated: 2026-11-12 8:45 AM ET. Check this before answering hardware questions; update it when build decisions change.

> **Note:** Paths under `/workspace/...` refer to files on an external working machine (enclosure CAD, handoff tarballs, wiring diagrams). Those paths are not in this repository.

## Project

- Raspberry Pi synth/MIDI appliance. Repo: https://github.com/HyruleanHero1988/pi-midi-toolkit
- Trello: https://trello.com/b/hrrSktTg/pi-midi-toolkit (enclosure card https://trello.com/c/T80UUNN4)
- Enclosure files: /workspace/pidi-enclosure (OpenSCAD; latest versioned exports_v2xx/, handoff tarballs in /workspace/handoff/)
- Long-term goal (11/11): replicate all 23 Clipper FM-1 synth engines + 407 voices (github.com/isod89/clipper-fm1, GPL-3.0-only). Backlog card P3: https://trello.com/c/9zcXwy0o. License route NOT final, but Ray is LEANING GPL-3 (11/11): he always meant PiDI to be fully open source, does not want anyone taking it closed and selling it, and picked MIT arbitrarily. Do not port Clipper code until he confirms. Repo relicense to GPL-3.0-or-later opened as PR https://github.com/HyruleanHero1988/pi-midi-toolkit/pull/91 (11/11, not merged yet).
- Ray hopes to build a second unit for his brother. May sell units later (no hand-soldered boards in sold units).

## v1 scope

- Pi 4B 8GB, 64-bit OS = official target (Pi 2 is legacy).
- USB MIDI only (no DIN / TRS MIDI panel jacks). No HDMI cutout. Main volume is software. Hardware headphone-level knob under consideration (11/28, see Audio).
- Ray does NOT own a 3D printer; case printed outside / Elegoo Saturn 2 resin split (3 front pieces at speaker dividers + 2 back pieces with offset seam, extra screws). Resin: no hot heat-set inserts (cold/mechanical or nut+epoxy).
- Fasteners: M3 for printed parts, M2.5 for board mounts. Ray has an M2.5 screw/standoff/washer assortment.

## Soldering rules

- No soldering on Pi, HiFiBerry, X728 (expensive boards) — plug-in only.
- OK to solder: amp breakouts, jacks, wire splices, a few passives. Ray is proficient.
- No big DIY boards (e.g., MIDI interface).

## Board stack / physical layout (CONFIRMED by Ray 11/11 with photos)

1. Pi 4B screwed directly to the back of the screen (BTT Pi TFT70-type 7"), oriented like his Pi 2 photo: power/HDMI/audio edge faces the case TOP wall; USB/Ethernet to the left; GPIO header along the edge toward the case bottom; screen FPC ribbon exits at the right and must not be bent/twisted.
2. HiFiBerry DAC2 ADC Pro sits directly on top of the Pi (its passthrough header meter-tested good on all 40 pins, no shorts).
3. Expansion header on the HiFiBerry passthrough with pins 2, 4, and 38 PULLED.
4. 40-pin female-to-female IDC ribbon (10–20 cm) from that expansion header to the Geekworm X728 UPS.
5. X728 lies flat beside the Pi, just past the GPIO edge, header edge facing the Pi so the ribbon runs short and straight (Ray's project-box photo).
- No G341 extender (superseded).

## Power

- 11/13: Ray CANCELLED the order with the 40-pin ribbon and XH leads (misrouted, about a week late). Plan now: wire the X728 directly with individual Dupont signal wires from the HiFiBerry passthrough header (no ribbon, no soldering on boards). Signals: pin 3 SDA, pin 5 SCL (fuel gauge), pin 29 GPIO5 (button shutdown), pin 32 GPIO12 (boot OK), pin 37 GPIO26 (software power-off), pin 31 GPIO6 (power-loss detect), GND (pin 6), optional pin 36 GPIO16 (charge control). NOT pins 2/4 (5V) or pin 38 (X728 buzzer GPIO20 = HiFiBerry ADC data). Ray is reusing thin wires from old Halloween "try me" buttons: OK for signals, NOT for the Pi 5V power lead (needs 22 AWG or thicker). Still need a replacement 22 AWG XH lead for the Pi power.
- Ray's straight USB-C panel-mount extension (~30 cm, already owned): rear-panel USB-C -> X728 USB-C charge input.
- Pi power: X728 XH 5V OUT -> 22 AWG JST-XH 2-pin lead -> soldered splice -> Jienk 90-degree USB-C male pigtail (22 AWG, 25 cm, Amazon B0D4122Q98, bought Nov 11 2026) -> Pi USB-C, cable exiting sideways along the roof. NOT through GPIO (ribbon wires too thin). Straight screw-terminal adapter and 12V PD trigger cables rejected.
- Amps get 5V from the X728 5V line too; splices SOLDERED + heat shrink (no WAGOs, no wire nuts).
- X728 cells: 2x Samsung 30Q 18650 flat-top unprotected (ordered from 18650 Battery Store).
- Power button: blue 12 mm momentary LED-ring (ownership unconfirmed), on right side.

## Audio

- HiFiBerry DAC2 ADC Pro: DAC out + ADC in (line-in jack). Output is LINE LEVEL.
- TWO amps, both foam-taped to the case wall:
  - MAX98306 (Adafruit) speaker amp -> 2x Tang Band T1-1942SB speakers. Never ground a speaker terminal (BTL).
  - Headphone path: 11/28 Ray's RCA test showed HiFiBerry line out (~2Vrms) drives headphones loud enough, so NO headphone amp. Headphone jack fed directly from the J5 split. The amp boards Ray owns are TPA6132 (not MAX97220); keep them as spares.
  - Hardware safety volume (Ray wants this so a software glitch can't blast his ears): passive dual-gang 10k log pot between the split and the headphone jack. It physically caps the headphone level. Not yet bought or placed in the case.
  - Series ~33-47 ohm resistors on tip/ring of each output jack, to protect the DAC from a mono plug shorting the ring to ground.
  - Open (11/28): one combined phones/line-out jack vs separate jacks. Ray leans toward keeping two long term. The case still has both.
- HiFiBerry line out splits to both amps; also feeds rear line-out jack.
- Speaker mute (decided 11/28): SOFTWARE-controlled. Jack isolated N-O switches (pins 5-6) are jack-detect inputs to a Pi GPIO (e.g. GPIO17, internal pull-up). The Pi drives MAX98306 SD via GPIO27 + 2N7000 (pull low = shutdown). Default: mute speakers when a plug is inserted. Menu setting to keep speakers on (e.g. recording via line out). The switch no longer cuts SD directly.
- Audio leads from HiFiBerry headers via Dupont female jumpers (cut one end, solder to jack).
- Wire: Ray's 22 AWG solid hookup is fine (leave slack, heat shrink).

## Jacks

- Headphone (right side) + line-out (rear): Same Sky SJ3-35063AG (bought 4 from Digi-Key). Unthreaded 6 mm bushing -> printed clamp (jack_clamp.stl, 2x M3x8 into inserts each). Pins: 1 sleeve, 2 ring, 3 tip, 5-6 isolated switch.
- Line-in (rear): Same Sky SJ3-35053D (M8x0.75 thread, nut).
- Rear jacks kept clear of tilt-mount towers for bulky plugs. Headphone on right so the cable doesn't drape over the Akai MPK mini.
- Old Qingpu PJ3410 (3-pin TRS, no switch): Ray is KEEPING them for hobby use (MIDI adapters, test leads, amp/speaker tests). Not used in the case.

## Rear panel

- USB-C (charge), 2x panel USB-A (Adafruit cables), line-in, line-out.

## Case status (v2.26, 11/12)

- Pi orientation corrected to match the photo (v2.1-v2.21 had it flipped 180 deg). Pi on M2.5x8 standoffs.
- X728 beside the GPIO edge does NOT fit in the printed case (~25 mm room vs 63 mm board): X728 stays upright in Bay B; 20 cm ribbon routed along floor. (Project-box build can still use Ray's flat-beside layout.)
- RULE (Ray, Nov 11 2026): NO bumps/blisters on the case. Walls stay flat; if anything needs more room, increase the overall case dimensions instead.
- v2.26 (11/12): Jienk 90-deg Pi USB-C plug MEASURED by Ray. The back of the overmold is 19.0 mm above the Pi PCB edge (18.75 read from the photo + parallax), i.e. piu_plug_len 17.5 from the mouth (= old estimate). case_h stays 112, 6.5 mm under the flat roof. The overmold is L-shaped: head 12 x 7.5, side arm/strain relief to 18.5 mm from the axis, cable OD 3.2 (modelled 3.5). Plug it in with the cable toward the HDMI side (+x), away from the micro-SD (reversed from Ray's photo; the photo orientation would hit the bracket top bar). The bracket top-bar notch was widened x138-167 -> x138-174 (flat, 2 mm bridge kept). Clearances: bridge 1.0, notch bottom 2.0, notch left 1.6, HiFiBerry 2.5, roof 6.5; pigtail keep-out to the bracket 2.0. Reprint ONLY screen_bracket. Files: exports_v226/, WIRING_v2.26.md, /workspace/handoff/pidi-enclosure-v2.26.tar.gz, exports_v226/pidi_v226_full_assembly.stl.
- v2.25 (11/12): rear panel tidied. ONE port row z14: line-in x51, line-out x75, 5V IN x99, USB-A pair x198/x237.5 (pitch 39.5; 908 flange 38.3x11.9). All labels/icons on one line z25 above the ports, >=3.5 mm from cuts, 2.6 mm from flanges. Made room by raising the X728 4 mm (PCB z20.5-107.5), starting the UPS-bay vents at z31, moving split screw x243 -> x259.5 and the clip/floor anchor -> x218. USB-A cables L (USB3) -> USB-A 1, U (USB2) -> USB-A 2, R16, use 125/99 of 259 mm. Screen stack fixed to Ray's 15 mm (6 + 9 corner mounts, was 6 + 17): bracket face y23 -> y15, bosses shortened. Glass flush with the front face; opening 165.8 x 100.8 (outline + 0.4/side), NO lip, the bracket holds the screen. Bracket top-bar notch x138-167 z93-104 (2 mm bridge) for the 90-deg Pi USB-C plug. Case still 333.8 x 71 x 112; depth NOT shrunk (speakers + rear jack bodies set it; at most ~2-3 mm possible, ask Ray first). Reprint front_center, back_left, back_right, screen_bracket. Files: /workspace/pidi-enclosure/exports_v225/, WIRING_v2.25.md, handoff /workspace/handoff/pidi-enclosure-v2.25.tar.gz, exports_v225/pidi_v225_full_assembly.stl.
- v2.24 (11/12): top USB-C blister REMOVED, so the roof is flat (max Z 112.0 on every case part). Case stays 333.8 x 71 x 112 mm (height unchanged). The Pi USB-C port face (z85.5) is 24.0 mm below the roof underside. The earlier "15.5 mm" was the straight adapter's limit (it needed 8 mm of axial wire bend); a 90-degree plug gets the full 24.0. Jienk plug ESTIMATE `piu_plug_len = 17.5` (params.scad) + 1 mm clearance leaves 6.5 mm spare. case_h auto-grows (flat walls, 2 mm per extra mm) only if the measured plug is over 23.0 mm. Pigtail cable modelled exiting +x, back through the bracket opening at x~160, behind the bracket at z101, down to the X728 XH 5V OUT splice. Reprint only front_center. Files: /workspace/pidi-enclosure/exports_v224/, WIRING_v2.24.md, handoff /workspace/handoff/pidi-enclosure-v2.24.tar.gz, assembled view-only STL exports_v224/pidi_v224_full_assembly.stl.
- Pi USB-A (v2.23): Ray's 2 Adafruit 908 panel-mount USB-A cables with STRAIGHT plugs fit the Pi's two lower USB-A ports. Required widening the case 16 mm (317.8 -> 333.8 mm; centre bay +8 mm each side), right bracket frame cut, X728 moved 8 mm, rear USB-A 2 moved to x207 z10, back split at x151, dock towers moved out 8 mm. Spare cable length (~144/55 mm) to be zip-tied at assembly (not modelled).
- WAGO bay removed; amps on back wall.

## Bench tests before soldering (REMIND RAY when soldering starts)

- MAX98306 SD voltage test (added 11/13): power the amp at 5V with NOTHING connected to SD and meter SD to GND.
  - ~3.3 V or lower: wire Pi GPIO27 straight to SD (high = play, low = mute; GPIO27 defaults low at boot so speakers stay muted until PiDI starts) and skip the 2N7000.
  - Near 5 V: keep the 2N7000 (GPIO27 high = mute), because Pi GPIOs are not 5V-tolerant.
  - Ray has no way to power the amp yet; do this once the 5V supply is wired.
- Meter SJ3-35063AG pins 5-6: open with no plug, closed with a plug in.
- Meter polarity of X728 XH 5V OUT and the Jienk pigtail before the first splice.
- If the PJ3410 is used for line-in, beep tip/ring/sleeve to its lugs.

## Audio wiring decisions (11/13)

- J5 (HiFiBerry analog out): pin 2 R, pin 6 L, pins 3/4 GND, pins 1/5 +5V (NEVER use). Twisted L/R/GND to a solder junction, then split three ways to the MAX98306, the headphone jack (via the safety pot, no amp), and the line-out jack. No buffer is needed.
- MAX98306: signal on L+/R+, L-/R- tied to audio GND at the amp. Outputs are BTL: 4 separate speaker wires, never ground or join the speaker minus terminals. Gain jumper 6 dB; cap the digital volume in software.
- Line-in goes to J4 (balanced input header), not the onboard 3.5 mm jack: tip (L) to J4 pin 5, ring (R) to J4 pin 2, sleeve to J4 pin 3/4, and J4 pins 6 and 1 jumpered to GND. Software: ADC inputs {VIN1P, VIN1M}[DIFF] / {VIN2P, VIN2M}[DIFF]. SJ3-35053D pins: 1 sleeve, 2 tip, 3 ring, 4/5 switches unused. A PJ3410 is equally fine for line-in (same M8 hole); the SJ3-35053D was picked only because Ray was considering returning the PJ3410s.
- Mic-bias jumpers J1 and J3 stay OPEN.
- Wiring diagram: /workspace/pidi/wiring/pidi_wiring_diagram.png (+ .svg, make_diagram.py).

## Open items

- (Closed 11/12: Jienk 90-degree plug measured, 19.0 mm from the PCB edge; modelled in v2.26.)
- Ray to confirm the screen glass/panel thickness split (model: 6 + 9 = 15 mm) and the Adafruit 908 flange size (38.3 x 11.9 assumed).
- Confirm Ray owns the screen and the blue power button.
- Ray to verify the SJ3-35063AG real dimensions/switch when they arrive.

## Update 11/13 evening

- Pi power cable BUILT: Ray soldered the XH lead directly onto the Jienk 90° USB-C pigtail, with heat-shrink over the joint. No replacement XH lead needed; don't push extra purchases.
- Pre-power check (no USB-C meter): meter on Rx1. XH black to the USB-C plug's metal shell should read about 0 Ω; XH red to the shell should not move the needle. Then, with the X728 on, 10V DCV red-to-red should read about 5V. Do NOT test from a USB-C charger (it outputs nothing without the CC resistor, so the reading is meaningless).
- Speakers: use stranded clear zip-cord speaker wire (about 16-18 AWG), a separate pair per speaker, striped side = +.
- 11/14: Ray ordered a perfboard kit. Plan: mount the MAX98306 breakout on headers (no headphone amp as of 11/28) on the perfboard, and run every in/out wire to perfboard pads (strain relief, no repeated soldering on the amps). The J5 three-way split junction, mute parts (2N7000 or 100 Ω), and a star ground also go on the perfboard. Layout TBD when the kit arrives (Ray will send a photo).

## Architecture rule: engine owns audio and MIDI (Ray, Nov 24, 2026)

- jambox-engine outputs all audio directly to the sound card. Audio never goes to or through the UI, whatever UI tech is used.
- Hardware MIDI (MPK: keys ch1, drums/pads ch10) goes straight into the engine, never through the UI.
- The UI is a non-realtime remote control and display only (user actions out, display state back). If the UI freezes, MIDI playing keeps sounding.
- JSON over the local socket (or a WebSocket for a browser UI) is fine for UI messages.