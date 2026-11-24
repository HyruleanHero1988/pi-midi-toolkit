# YouTube reply: feature requests from @ianhill5074

Source: a YouTube reply to [@HyruleanHero1988](https://www.youtube.com/) from **@ianhill5074** (edited). A later reply from **@brawndothethirstmutilator9848** is only product-direction color, not a feature list.

This note translates that comment into plain language and scores each ask against **pi-midi-toolkit / PiDI** as it exists today: a Raspberry Pi jambox (wavetable + FM + analog drums, overdub SEQ, pads, Kaoss, punch-in FX) plus a MIDI remap appliance. It is **not** an Elektron Digitakt, even though a lot of the language below comes from that world.

Feasibility here means “could we put this on the Pi 2 kiosk without fighting the design law (obvious under the hands, UI off the audio/MIDI hot path, no sample-library host / tracker clone).” It is not a schedule.

| Score | Meaning |
|-------|---------|
| **Already close** | We have a cousin; this is mostly UI or a small extension. |
| **Good fit** | Matches a pillar we already have (synth, SEQ, remap). Bounded DSP/UI. |
| **Possible, expensive** | Doable in software, but needs a new editor, a new engine path, or a lot of kiosk chrome. |
| **Poor fit** | Assumes a sampler / step-sequencer identity we are not building, or hardware we do not have. |

---

## Product-direction comments

These are not feature requests so much as an argument about what the box *is*.

### 1. Skip extra synth engines (Pocket Operator knockoff, “hat on a hat”)

**What they mean.** The commenter hears people asking for more oscillators / toy synths and pushes back: if the instrument’s job is mangling *sampled* audio, a mini Pocket Operator or another rudimentary synth stacked on top is redundant. Tools for shaping a recorded sound already exist; a second synthesis language is a different product.

**What PiDI is.** PiDI is already synth-first, not sampler-first. The live tone comes from wavetable morph voices, a four-operator FM playground, and analog-style drum models. There is live **mic/line capture** into punch-in FX, and **SAVE AS…** can bake a morph (+ drive/tone) into a user wavetable, but there is no Digitakt-style sample pool, start/end editor, or slice grid.

**Feasibility: poor fit as stated, and we should not take the implied identity shift.** Agreeing with “don’t add a third toy synth” is cheap and on-brand (PLAN already refuses picotracker/EP feature-parity). Disagreeing with “this is a sampler, so stop synthesizing” is also correct: dropping wavetable/FM/drums in favor of a sample mangler would be a different appliance. A Pocket Operator clone on top of what we already ship really *would* be a hat on a hat.

### 2. Extra FX are cool, but bake them into the samples

**What they mean.** Live send/insert FX are nice for performance, but a sampler workflow often **resamples**: print the wet sound into a new sample, then play that. You spend polyphony and CPU on playback, not on a running delay/reverb. That is how a lot of Digitakt / SP-404 people work.

**What PiDI has.** Voice bake already prints morph + drive + tone into a `.wav` and parks delay/reverb in a sidecar `.fx.json`. Punch-in FX (RPT, TAPE, SLICE, CRUSH, …) run live on a short ring, including the mic. There is no “resample this mix onto a pad” path.

**Feasibility: already close for wavetables; possible-but-expensive for a real resample workflow.** Extending SAVE AS… (or a SEQ/PADS “print”) to flatten bus FX into a user wavetable is a small, on-brand win. A Digitakt-style “record the mix into a chromatically playable sample” needs a sample player we have explicitly not been chasing.

---

## Quality-of-life requests

### 3. Sample start and end auto-snap to the nearest zero crossing

**What they mean.** A *zero crossing* is a sample where the waveform is at (or very near) silence, passing from + to − or the reverse. If a loop or one-shot starts/ends on a loud non-zero sample, you get a click. Auto-snap walks start/end points to the closest zero so chops are clean without a magnifying-glass editor.

**What PiDI has.** Wavetables are single-cycle tables loaded at boot; drums are synthesized one-shots; punch FX use a live ring. Nobody sets sample start/end today, so there is nothing to snap.

**Feasibility: trivial math, poor fit until we have a sample editor.** Finding the nearest zero in a PCM buffer is a few dozen lines. It only matters if we add user samples with adjustable start/end. Until then, skip it.

### 4. Manual slice points in “the slice machine” (they forgot the name)

**What they mean.** Almost certainly Digitakt’s **Slice** machine (sometimes talked about as grid slices / edit slices): you chop one long recording into regions and play each region as its own hit. Factory slicing is even divisions (8, 16, 32…). *Manual* slice points means tapping the waveform to say “the snare starts *here*,” not wherever the equal grid landed. Other boxes call this Chop (SP-404) or slices (Octatrack).

**What PiDI has.** FX pad **SLICE** is an EP-style amplitude gate: it chops the *live bus* on a 1/4–1/32 grid (or a smooth capture span). It is a tremolo/slicer, not a sample-chop editor. There are no stored slice markers.

**Feasibility: poor fit as a sample editor; a small cousin is possible on punch SLICE.** A real slice machine needs a sample engine, a waveform UI, and pad→slice mapping — a product we said we are not. A cheaper steal: let SLICE use a user gate pattern (hits/rests) instead of a fixed 45% open window. That is still a live FX, not Digitakt Slice.

### 5. Retrig over MIDI

**What they mean.** **Retrig** is Elektron’s automatic re-strike: while a note is held (or for a set number of hits), the sound fires again at a rate, often with a velocity ramp so rolls decay. “Over MIDI” means either (a) an incoming MIDI note starts that roll on the box, or (b) the box *emits* the extra note-ons so a DIN synth rolls too — not only the local engine.

**What PiDI has.** `RepeatRack` already sample-clocks drum repeats from a held contact (the kick-repeat gesture). PLAN Phase 2 is a general per-note retrigger (interval, hold vs latch, velocity of repeats). SEQ/pads can already emit USB MIDI, but repeats are a local audio/MIDI-in gesture, not a first-class MIDI processor.

**Feasibility: good fit.** This is on the existing roadmap. The useful extra is making repeats a MIDI-domain processor (local *and* USB→DIN) with musical divisions, so a roll on the MPK is heard on a hardware synth. Keep it one note, one rate — that is retrig, not an arp.

### 6. Set audio channels up to poly-chain

**What they mean.** Digitakt **Poly Chain** daisy-chains two (or more) units so unused voices spill onto the next box. One eight-voice machine becomes sixteen. “Audio channels” is their wording for tracks/voices. They even wonder if some community firmware already does it. This is a *multi-device voice pool*, not “make our synth polyphonic” (it already is: 16 wavetable voices, 8 FM).

**What PiDI has.** One Pi, fixed polyphony, voice stealing inside `VoicePool`. MIDI MAP can fan one input channel onto several output channels, which is routing, not extra local voices.

**Feasibility: poor fit as Digitakt poly-chain; a MIDI overflow cousin is possible.** Chaining two PiDIs (or PiDI + Digitakt) into one voice pool needs a net/MIDI protocol, sample-accurate voice steal across boxes, and a second device on the desk. That is a lot of identity for almost no solo-player value. If the real pain is “I ran out of voices,” cheaper answers are: steal policy, a “full → send overflow notes to USB” option, or more voices on a Pi 4/5 later.

### 7. Digitone-style arpeggiator

**What they mean.** Elektron’s Digitone arp is a **per-track** arpeggiator on a polyphonic synth: hold a chord, and the box walks those notes (rate, length, offset, modes) while you play. The commenter flags extra work on Digitakt because **DT tracks are monophonic** — a chord-arp has to fake polyphony or steal. They want that Digitone *feel*, not our current “one root transposes a written interval pattern.”

**What PiDI has.** A key-relative arp already lives in `jambox-core` / ARP UI: authored intervals, MPK-style walk orders, octaves, gate, latch, divisions. Incoming notes retarget the **root**. They are explicitly *not* a held chord pool. PLAN still lists a distinct arp mode as later work, and the kiosk arp UI is unfinished relative to that plan.

**Feasibility: good fit, and we are most of the way there.** Adding a “held notes” pool (chord arp) beside the existing “pattern from root” mode is the Digitone-shaped step. Do **not** wait on Digitakt-style mono-track firmware tricks; our engine is already polyphonic. Keep it a dedicated ARP mode so it does not collapse into SEQ or drum retrig.

### 8. Assignable DADSR envelope

**What they mean.** A **DADSR** envelope is Delay → Attack → Decay → Sustain → Release. Delay is a pause after note-on before the rise starts. “Assignable” means that shape is not glued to volume: you can point it at filter, pitch, FM index, etc. Digitakt’s amp envelope is a simpler A/H/D; people ask for a full assignable envelope so one-shots and pads can plume, duck, or swell independently of the amp.

**What PiDI has.** Wavetable voices are attack + release only (amp slew). FM operators have a compressed env/sustain, not a five-stage editor. Drums have pitch env + decay/stretch. There is no modulation matrix.

**Feasibility: good fit for amp DADSR on the morph synth; assignable destinations are possible-but-expensive.** Five-stage amp on `VoicePool` is cheap DSP (we already slew amp every sample). The cost is kiosk chrome: five parameters, per-voice vs global, and “what does delay mean on a drum.” A general “this envelope drives filter *and* pitch” matrix is a synth-workstation feature; if we do it, one extra destination (tone/filter) is enough.

### 9. One LFO that targets multiple things

**What they mean.** Classic groovebox LFOs often have **one destination**. Players want one LFO to wiggle several parameters at once (filter + pitch + delay send, with per-destination depth) without burning two LFO slots. Digitakt recently grew extra LFO design; this is that wish.

**What PiDI has.** Separate modulators: a global **tone LFO** (Kaoss WAH), a **flanger LFO** inside FX, and **per-layer vibrato** baked into SEQ takes. PLAN also parks a SEQ-Lite-style “Berlin-school” MIDI LFO (note / vel / length / CC) as later loop FX, not a synth matrix.

**Feasibility: good fit as “one LFO, two or three depths”; poor fit as a modular matrix.** A single shared LFO with amount knobs for pitch, tone, and maybe delay mix matches how people actually play and stays obvious. A many-to-many mod matrix is the opposite of the design law.

### 10. Choose whether to send PC / CC / transport MIDI per channel or track

**What they mean.** **PC** = Program Change (patch recalls). **CC** = Control Change (knobs, pedals, sustain). **Transport** = clock, start, stop, continue. On a multi-track box you often want drums to emit notes but *not* dump a program change into the bass synth, or you want clock to one device and not another. Per-channel (or per-track) mute of those message types is a MIDI filter.

**What PiDI has.** MAP remaps channels, CCs, and velocity. `midi-core` already parses program change, but the thru path does not filter by message class, and **realtime transport/clock is ignored** (`parse` drops status `≥ 0xF0`). PLAN already names “MIDI filter (block channel / message types)” as the natural next remap feature. There is no MIDI-clock master/slave yet.

**Feasibility: good fit, and high leverage for the remap pillar.** Per-output-channel flags for “notes / CC / PC” are a small preset field plus a MAP checkbox grid. Transport is a bigger, still-aligned piece: we would have to *start* handling clock bytes, then allow send/block per port. Do filters first; clock later with Songs/Pads OUT.

### 11. Euclidean sequencer

**What they mean.** A **Euclidean** rhythm spreads *K* hits as evenly as possible across *N* steps (Bjorklund). 5 hits in 16 steps is the classic “almost clave” pattern; rotate the ring to move the downbeat. It is a tiny control surface: hits, steps, rotate — and it sounds like a drummer instead of a straight 4-on-the-floor grid.

**What PiDI has.** SEQ is a **free-timing overdub looper** (backbone take + KEEP/DROP/UNDO), not a step sequencer. PLAN explicitly parks a 16-step “tap cell → play note” overlay as optional later, and says the 808 mental model does not need a piano roll.

**Feasibility: good fit as a small SEQ/drums overlay; poor fit as a new sequencer identity.** The algorithm is nothing. The product question is whether we grow a step grid. A DRUMS or SEQ page with HITS / LEN / ROTATE that *emits* into the existing looper (or the repeat rack) would steal the musical idea without becoming a tracker. A full Euclidean track sequencer would fight “don’t clone picotracker.”

### 12. Parameter-lock master FX, compressor, input, even tempo (Risset rhythms)

**What they mean.** Elektron **parameter locks** (p-locks) store a knob value on **one sequencer step**. The commenter wants that not only on the sound, but on *master* things: send FX, compressor, audio-input settings, **or tempo**. Locking tempo per step (or automating it smoothly) is how you fake **Risset rhythms**: a looping acceleration that feels like it never resets, because pitch/tempo and the pattern are tuned to hide the loop point (the auditory analog of a Shepard tone).

**What PiDI has.** SEQ records notes and timing, plus baked per-layer vibrato. No per-step parameter locks. No compressor. Punch/bus FX and mic trim are global, not sequenced. Tempo is a transport/Songs BPM, not a per-event value.

**Feasibility: possible-but-expensive for sound p-locks; poor fit for locking the master bus and tempo.** Per-step locks want a step sequencer. Tempo locks fight the “one sample-accurate clock” rule and make USB→DIN partners fall over unless we become a MIDI-clock master. Risset-style *feel* is cheaper as a dedicated FX (tape/repeat already pitch-bends time locally) than as sequenced master tempo. A compressor is new DSP we do not have; skip it unless mix metering says we need one.

### 13. Hook an external USB device and read sample or project data

**What they mean.** Treat a USB stick / SSD as a library: projects, samples, maybe presets, without copying everything onto internal storage first. Digitakt II and a lot of DJ desks work this way.

**What PiDI has.** User data lives on the Pi (`~/.local/share/pidi/`: settings, songs, phrases, user presets, user wavetables). USB is used for **MIDI** (and optionally a **USB mic**). Wi-Fi power gating exists; mass-storage host does not. PLAN’s “not chasing” list includes becoming a sample-library host. Pi 2 USB power + MIDI + audio on one hub is already a brown-out risk.

**Feasibility: possible-but-expensive, and easy to regret on Pi 2.** Linux can mount a USB drive. Reading `.mid` / wavetable `.wav` / JSON off it is straightforward. The hard parts are: a kiosk browser that cannot stall the audio process, hotplug while a jam is running, and not tripping the hub. A conservative version — “import this folder into user data when idle” — is the only one I would consider. Live streaming samples off a stick is a sampler product.

### 14. SSD as DJ decks with someone else’s Digitakt

**What they mean.** Two performers, two boxes. DJs pass a stick between CDJs and keep playing. If the other player has a **Digitakt**, they want to dock the same SSD, grab the same sample/project pool, and “go off” without a laptop session swap.

**What PiDI has.** None of that. Digitakt project/sample format is proprietary. We speak Standard MIDI Files and our own JSON/WAV.

**Feasibility: poor fit.** We will not parse Digitakt projects. Interop that *is* honest: export/import WAV + MIDI + PiDI JSON on a FAT32 stick (see §13), maybe a “set folder” convention both humans can see. Instant deck-style hot-swap of a foreign SSD while both boxes play is not a Pi 2 kiosk goal.

---

## Side comment: “over 52 firmwares”

@brawndothethirstmutilator9848 told the commenter to look closer at a large firmware scene (Elektron / community OS, “not just more sounds”). That is advice *to them*, not a request *of PiDI*. We should not chase firmware-count feature parity. Steal individual ideas (MIDI filters, retrig, Euclidean overlay) when they match the jambox or remap pillars.

---

## Summary

| # | Request | Score | One-line verdict |
|---|---------|-------|------------------|
| 1 | No more synth engines / PO clone | Poor fit (identity) | We are a synth jambox; skip a toy-PO layer, keep wavetable/FM/drums. |
| 2 | Bake FX into samples | Already close | Extend voice bake / print; don’t build a resample sampler. |
| 3 | Zero-crossing snap | Poor fit (no sample editor) | Trivial later if we ever edit PCM start/end. |
| 4 | Manual slice points | Poor fit / small FX cousin | Punch SLICE is a gate, not Digitakt Slice. |
| 5 | Retrig over MIDI | **Good fit** | Phase 2 retrig + emit to USB. |
| 6 | Poly-chain channels | Poor fit | Overflow-to-DIN maybe; multi-box voice pool no. |
| 7 | Digitone-style arp | **Good fit** | Chord-pool mode on the arp we already have. |
| 8 | Assignable DADSR | **Good fit** (amp) | Five-stage amp yes; full matrix later/never. |
| 9 | Multi-target LFO | **Good fit** (2–3 dests) | One LFO, a few depths; no modular matrix. |
| 10 | Filter PC/CC/transport per channel | **Good fit** | Natural MAP next step; clock after that. |
| 11 | Euclidean sequencer | **Good fit** as overlay | Hits/len/rotate into SEQ or drums; not a new DAW. |
| 12 | P-lock master / tempo (Risset) | Poor fit | No step p-locks; don’t sequence the master clock. |
| 13 | USB stick for samples/projects | Possible, expensive | Idle import only; live USB library is out of scope. |
| 14 | SSD decks with a Digitakt | Poor fit | WAV/MIDI/JSON handoff maybe; Digitakt format no. |

**Highest-value steals if we answer this comment with work rather than a shrug:** MIDI message filters on MAP, MIDI-domain retrig, a held-chord arp mode, and (if we want one “musical toy”) a Euclidean overlay on drums/SEQ. Those sit on engines we already own and do not require pretending PiDI is a sampler.
