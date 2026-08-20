"""MIDI ports, CC handling, and event-queue drain for MidiToneApp."""
from __future__ import annotations

import queue
import time
from typing import Optional

import mido

from pidi.constants import (
    CC_ATTACK,
    CC_LEVEL,
    CC_MORPH,
    CC_RELEASE,
    CC_TONE,
    CC_VIB_DEPTH,
    CC_VIB_RATE,
)
from pidi.jambox_client import connect_or_spawn, midi_notice_to_message, prefer_python_engine


class MidiIoMixin:
    def _print_ports(self) -> None:
        names = mido.get_input_names()
        if not names:
            print("No MIDI inputs.")
            return
        print("MIDI inputs:")
        for i, n in enumerate(names):
            print(f"  [{i}] {n}")
        print(f"Wavetables ({len(self._voice_names)}): {', '.join(self._voice_names)}")


    def _attach_jambox(self) -> None:
        """Connect to jambox-engine (spawn if MIDI_TONE_SPAWN=1)."""
        if prefer_python_engine():
            return
        client, proc = connect_or_spawn(
            waves=self._waves_dir,
            user_waves=self._user_waves_dir,
            midi_in=self.port_filter or "MPK",
        )
        self._jambox = client
        self._jambox_proc = proc
        self._jambox_owns_midi = client is not None and client.connected
        if not self._jambox_owns_midi:
            return
        self.engine.attach_remote(client)
        self._append_log("Sound + MIDI: jambox-engine")
        print("midi: using jambox-engine (Python audio/MIDI fallback idle)", flush=True)


    def _maybe_reopen_midi(self) -> None:
        """If we started without the filtered device, adopt it when it appears."""
        if self._stop.is_set() or not self.port_filter or self._jambox_owns_midi:
            return
        try:
            current = ""
            if self._inport is not None:
                current = str(getattr(self._inport, "name", "") or "")
            if self.port_filter in current.lower():
                return
            wanted = self._pick_port(retries=1, delay_s=0.0, allow_fallback=False)
            if not wanted:
                self.root.after(2000, self._maybe_reopen_midi)
                return
            old = self._inport
            self._inport = mido.open_input(wanted)
            if old is not None:
                try:
                    old.close()
                except Exception:
                    pass
            self._append_log(f"MIDI reconnected: {wanted}")
            print(f"midi: reopened input ({wanted})", flush=True)
            self.last_var.set(f"MIDI: {wanted}")
        except Exception as exc:
            print(f"midi: reopen failed: {exc}", flush=True)
            self.root.after(2500, self._maybe_reopen_midi)


    def _pick_port(
        self,
        *,
        retries: int = 1,
        delay_s: float = 0.4,
        allow_fallback: bool = True,
    ) -> Optional[str]:
        """Resolve MIDI in. Retries help after kiosk restarts (MPK port briefly busy)."""
        retries = max(1, int(retries))
        for attempt in range(retries):
            names = mido.get_input_names()
            if not names:
                if attempt + 1 < retries:
                    time.sleep(delay_s)
                    continue
                return None
            if self.port_filter:
                for n in names:
                    if self.port_filter in n.lower():
                        return n
                if attempt + 1 < retries:
                    time.sleep(delay_s)
                    continue
                if not allow_fallback:
                    return None
                for n in names:
                    if "through" not in n.lower():
                        print(f"No input matching '{self.port_filter}'; using {n}", flush=True)
                        return n
                print(f"No input matching '{self.port_filter}'. Available:", flush=True)
                for n in names:
                    print(f"  {n}", flush=True)
                print(f"Falling back to: {names[0]}", flush=True)
                return names[0]
            for n in names:
                if "mpk" in n.lower():
                    return n
            for n in names:
                if "through" not in n.lower():
                    return n
            return names[0]
        return None


    def _midi_loop(self) -> None:
        while not self._stop.is_set():
            try:
                if self._inport is not None:
                    for msg in self._inport.iter_pending():
                        self._handle_midi(msg)
                elif self._jambox is not None:
                    for notice in self._jambox.drain_midi():
                        msg = midi_notice_to_message(notice)
                        if msg is not None:
                            self._handle_midi(msg, from_engine=True)
            except Exception as exc:
                tb = __import__("traceback").format_exc()
                print(tb, flush=True)
                self._q_put(("log", f"MIDI ERROR: {exc}", False))
            time.sleep(0.001)


    def _q_put(self, item: tuple) -> None:
        """Never block the MIDI thread on a full UI queue — drop oldest junk."""
        try:
            self.event_q.put_nowait(item)
            return
        except queue.Full:
            pass
        try:
            self.event_q.get_nowait()
        except queue.Empty:
            pass
        try:
            self.event_q.put_nowait(item)
        except queue.Full:
            pass


    def _put_continuous_log(self, line: str) -> None:
        """Throttle high-rate messages so the Tk queue can't freeze the UI."""
        now = time.monotonic()
        if now - getattr(self, "_last_cont_put", 0.0) < 0.08:
            self._pending_cont_log = line
            return
        self._last_cont_put = now
        self._pending_cont_log = None
        self._q_put(("log", line, True))


    def _knob_ui_feedback(self, label: Optional[str], *, morph: bool = False) -> None:
        """Coalesce high-rate knob UI updates — don't flood the event queue."""
        self._mod_dirty = True
        if morph:
            self._morph_dirty_ui = True
            self._schedule_scope_paint("synth", blank=True)
        if self.engine.drum_knob_focus() and self._kit_ui_open:
            self._schedule_scope_paint("drum", blank=True)
        if self._fx_ui_open and self.engine.fx_knob_focus():
            self._fx_dirty_ui = True
        if label:
            # Status line only; skip log spam (was making knobs feel laggy on Pi)
            self._pending_cont_log = label


    def _handle_knob_cc(self, control: int, value: int) -> Optional[str]:
        """Map MPK factory knobs. Returns a short UI label or None if unmapped."""
        if self.engine.fx_knob_focus():
            if control == CC_MORPH:
                self.engine.set_fx_drive(value)
                self._mark_settings_dirty()
                return f"FxDrive {value}"
            if control == CC_TONE:
                self.engine.set_fx_delay_time(value)
                self._mark_settings_dirty()
                ms = int((0.05 + (value / 127.0) * 0.70) * 1000)
                return f"FxDelay {ms}ms"
            if control == CC_ATTACK:
                self.engine.set_fx_delay_fb(value)
                self._mark_settings_dirty()
                return f"FxDlyFb {value}"
            if control == CC_RELEASE:
                self.engine.set_fx_delay_mix(value)
                self._mark_settings_dirty()
                return f"FxDlyMix {value}"
            if control == CC_VIB_DEPTH:
                self.engine.set_fx_reverb_size(value)
                self._mark_settings_dirty()
                return f"FxRvbSz {value}"
            if control == CC_VIB_RATE:
                self.engine.set_fx_reverb_mix(value)
                self._mark_settings_dirty()
                return f"FxRvbMix {value}"
            if control == CC_LEVEL:
                self.engine.set_synth_level(value)
                self._mark_settings_dirty()
                return f"SynLvl {value}"
            return None

        # Only in explicit DRUM MODE do knobs edit drum macros
        if self.engine.drum_knob_focus():
            if control == CC_MORPH:
                self.engine.set_drum_pitch(value)
                self._mark_settings_dirty()
                return f"DrumPitch {value}"
            if control == CC_TONE:
                self.engine.set_drum_tone(value)
                self._mark_settings_dirty()
                return f"DrumTone {value}"
            if control == CC_ATTACK:
                self.engine.set_drum_decay(value)
                self._mark_settings_dirty()
                return f"DrumStretch {value}"
            if control == CC_RELEASE:
                self.engine.set_drum_noise(value)
                self._mark_settings_dirty()
                return f"DrumNoise {value}"
            if control == CC_LEVEL:
                self.engine.set_drum_level(value)
                self._mark_settings_dirty()
                return f"DrmLvl {value}"
            # Other knobs ignored while drum-focused (keep level usable)
            if control in (CC_VIB_DEPTH, CC_VIB_RATE):
                return None

        if control == CC_MORPH:
            self.engine.set_morph(value)
            self._mark_settings_dirty()
            left, right, blend = self.engine.morph_neighbors()
            if left == right:
                return f"Morph  {value}  ({left})"
            return f"Morph  {value}  ({left}→{right} {int(blend * 100)}%)"
        if control == CC_TONE:
            self.engine.set_tone(value)
            self._mark_settings_dirty()
            return f"Tone   {value}"
        if control == CC_ATTACK:
            self.engine.set_attack(value)
            self._mark_settings_dirty()
            st = self.engine.modulation_state()
            return f"Attack {value}  ({st['attack'] * 1000:.0f} ms)"
        if control == CC_RELEASE:
            self.engine.set_release(value)
            self._mark_settings_dirty()
            st = self.engine.modulation_state()
            return f"Release {value}  ({st['release'] * 1000:.0f} ms)"
        if control == CC_VIB_DEPTH:
            self.engine.set_vib_depth(value)
            self._mark_settings_dirty()
            st = self.engine.modulation_state()
            return f"VibDepth {value}  ({st['vib_depth']:.2f} st)"
        if control == CC_VIB_RATE:
            self.engine.set_vib_rate(value)
            self._mark_settings_dirty()
            st = self.engine.modulation_state()
            return f"VibRate {value}  ({st['vib_hz']:.1f} Hz)"
        if control == CC_LEVEL:
            self.engine.set_synth_level(value)
            self._mark_settings_dirty()
            return f"SynLvl {value}"
        return None


    def _handle_midi(self, msg: mido.Message, *, from_engine: bool = False) -> None:
        if from_engine:
            self.engine._echoing = True
        try:
            self._handle_midi_body(msg, from_engine=from_engine)
        finally:
            if from_engine:
                self.engine._echoing = False


    def _handle_midi_body(self, msg: mido.Message, *, from_engine: bool = False) -> None:
        continuous = msg.type == "pitchwheel" or (
            msg.type == "control_change"
            and (msg.control == 1 or msg.control in KNOB_CCS)
        )
        pads_mode = self._mode == "pads"

        if msg.type == "note_on" and msg.velocity > 0:
            is_drum = msg.channel == DRUM_CHANNEL
            phrase_recording = self._phrases.is_recording()
            # SEQ → PAD: MPK pad picks the destination clip (works from SEQ or PADS).
            if is_drum and self._seq_to_pad_armed:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None:
                    self._seq_to_pad_armed = False
                    self._q_put(("seq_to_pad", cell))
                    self._q_put(
                        (
                            "log",
                            f"Pad→SEQ {phrase_pad_label(cell)}  note {msg.note}",
                            False,
                        )
                    )
                    return
            # PADS mode: MPK pads launch/arm phrases. While a take is recording,
            # the armed cell (and other empty pads) stay drums; filled pads still
            # launch — same as tapping the grid. Run on the UI thread.
            if pads_mode and is_drum:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None and self._drum_pad_is_phrase_control(cell):
                    self._q_put(("pad_midi", cell, msg.note, msg.velocity))
                    return
            # KIT explorer: hitting an MPK pad selects that voice for the scope
            if self._kit_ui_open and is_drum and phrase_cell_for_note(msg.note) is not None:
                self._q_put(("kit_sel", msg.note))
            vel = msg.velocity if is_drum or not self._full_vel else 127
            if not from_engine:
                self.engine.note_on(msg.channel, msg.note, vel)
            self._seq.record_note(True, msg.channel, msg.note, vel)
            if pads_mode or phrase_recording:
                self._phrases.record_note(True, msg.channel, msg.note, vel)
            self._q_put(("on", msg.channel, msg.note, vel))
            if self._seq.is_recording():
                self._q_put(("seq",))
            if is_drum:
                model = drum_model_for_note(msg.note)
                rec_tag = " +rec" if phrase_recording else ""
                line = (
                    f"Pad/{model:<10} ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  vel {msg.velocity}{rec_tag}"
                )
                if self.engine.drum_mode():
                    self._q_put(("mod",))
                if phrase_recording:
                    self._q_put(("phrase",))
            elif self._full_vel and msg.velocity != 127:
                line = (
                    f"Note On   ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  vel {msg.velocity}→127"
                )
            else:
                line = format_message(msg)
            self._q_put(("log", line, False))
        elif msg.type == "note_off" or (msg.type == "note_on" and msg.velocity == 0):
            # Phrase-launch pads have no held note — but drum takes while recording do
            if pads_mode and msg.channel == DRUM_CHANNEL:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None and self._drum_pad_is_phrase_control(cell):
                    return
            if not from_engine:
                self.engine.note_off(msg.channel, msg.note)
            self._seq.record_note(False, msg.channel, msg.note, 0)
            if pads_mode or self._phrases.is_recording():
                self._phrases.record_note(False, msg.channel, msg.note, 0)
            self._q_put(("off", msg.channel, msg.note))
            if self._seq.is_recording():
                self._q_put(("seq",))
            self._put_continuous_log(format_message(msg))
        elif msg.type == "polytouch":
            if pads_mode and msg.channel == DRUM_CHANNEL:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None and self._drum_pad_is_phrase_control(cell):
                    return
            self.engine.set_pad_pressure(msg.channel, msg.note, msg.value)
            if msg.channel == DRUM_CHANNEL:
                self._put_continuous_log(
                    f"PadPress ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  press {msg.value}"
                )
            else:
                self._put_continuous_log(format_message(msg))
        elif msg.type == "aftertouch":
            if (
                pads_mode
                and msg.channel == DRUM_CHANNEL
                and not self._phrases.is_recording()
            ):
                return
            self.engine.set_pad_pressure(msg.channel, None, msg.value)
            if msg.channel == DRUM_CHANNEL:
                self._put_continuous_log(
                    f"PadPress ch{msg.channel + 1}  (all)  press {msg.value}"
                )
            else:
                self._put_continuous_log(format_message(msg))
        elif msg.type == "pitchwheel":
            self.engine.set_pitch_bend(msg.pitch)
            self._q_put(("mod",))
            self._put_continuous_log(format_message(msg))
        elif msg.type == "control_change":
            if msg.control == 1:
                self.engine.set_mod_wheel(msg.value)
                self._q_put(("mod",))
                self._put_continuous_log(format_message(msg))
            elif msg.control in KNOB_CCS:
                drum_focus = self.engine.drum_knob_focus()
                fx_focus = self.engine.fx_knob_focus()
                label = self._handle_knob_cc(msg.control, msg.value)
                self._knob_ui_feedback(
                    label,
                    morph=(msg.control == CC_MORPH and not drum_focus and not fx_focus),
                )
            elif msg.control == 123:
                self.engine.all_notes_off()
                self._q_put(("panic",))
                self._q_put(("log", format_message(msg), False))
            else:
                self._q_put(("log", format_message(msg), continuous))
        else:
            self._q_put(("log", format_message(msg), False))


    def _drain_queue(self) -> None:
        # Cap work per tick so a flood can't freeze touch for seconds
        processed = 0
        backlog = self.event_q.qsize()
        limit = 12 if backlog > 80 else 24
        try:
            while processed < limit:
                item = self.event_q.get_nowait()
                processed += 1
                kind = item[0]
                if kind == "log":
                    _, line, continuous = item
                    self.last_var.set(line)
                    if continuous:
                        now = time.monotonic()
                        if now - getattr(self, "_last_cont_log", 0.0) >= 0.12:
                            self._last_cont_log = now
                            self._append_log(line)
                    else:
                        self._append_log(line)
                elif kind == "on":
                    _, ch, note, vel = item
                    self._active_notes[(ch, note)] = vel
                    self._refresh_active()
                elif kind == "off":
                    _, ch, note = item
                    self._active_notes.pop((ch, note), None)
                    self._refresh_active()
                elif kind == "mod":
                    self._mod_dirty = True
                elif kind == "morph":
                    self._morph_dirty_ui = True
                    self._mod_dirty = True
                elif kind == "panic":
                    self._active_notes.clear()
                    self._refresh_active()
                elif kind == "seq":
                    self._refresh_seq_status()
                elif kind == "phrase":
                    self._refresh_phrase_status()
                elif kind == "seq_to_pad":
                    self._finish_seq_to_pad(int(item[1]))
                elif kind == "pad_midi":
                    self._on_pad_midi(int(item[1]), int(item[2]), int(item[3]))
                elif kind == "song":
                    self._refresh_song_status()
                elif kind == "update":
                    self._update_busy = False
                    result = item[1]
                    extra = item[2]
                    if result is not None:
                        self._update_check = result
                    if extra == "confirm" and result is not None and result.available:
                        self._update_confirming = True
                        self._settings_status_var.set(
                            "This deploys new code from GitHub, then restarts.\n"
                            "Phrases, songs, presets, and settings.json are not touched "
                            "(same as SSH deploy).\n"
                            "Rust engines come from committed dist/armv7 — not built on this Pi.\n"
                            "Tap INSTALL NOW to continue, or CANCEL."
                        )
                        self._paint_settings_buttons()
                    elif extra and extra != "confirm":
                        self._settings_status_var.set(str(extra))
                        self._paint_settings_buttons()
                    else:
                        self._refresh_settings_status()
                    if result is not None and result.message:
                        self._append_log(result.message)
                elif kind == "update_progress":
                    self._settings_status_var.set(str(item[1]))
                    self.last_var.set(str(item[1]))
                elif kind == "update_done":
                    self._update_busy = False
                    info, err = item[1], item[2]
                    if err:
                        self._update_confirming = False
                        self._settings_status_var.set(f"Update failed: {err}")
                        self._paint_settings_buttons()
                        self._append_log(f"Update failed: {err}")
                    else:
                        short = getattr(info, "short", "latest")
                        self._settings_status_var.set(f"Installed {short} — restarting…")
                        self._append_log(f"Update installed {short}")
                        self.root.after(250, self._restart_after_update)
                elif kind == "kit_sel":
                    self._select_kit_note(int(item[1]), audition=False)
        except queue.Empty:
            pass
        pending = getattr(self, "_pending_cont_log", None)
        if pending is not None:
            self.last_var.set(pending)
            self._pending_cont_log = None
        # Apply coalesced knob/mod UI once per tick (keeps drum knobs snappy)
        if getattr(self, "_morph_dirty_ui", False):
            self._morph_dirty_ui = False
            self._sync_voice_index_from_morph()
            self._mod_dirty = True
        if getattr(self, "_mod_dirty", False):
            self._mod_dirty = False
            self.mod_var.set(self._format_mod_line())
            if self._fx_ui_open:
                self._refresh_fx_panel()
            if self._grid_open:
                self._paint_vib_controls()
            if self._kit_ui_open and getattr(self, "_kit_view", "grid") != "wave":
                self._refresh_kit_status()
        if getattr(self, "_fx_dirty_ui", False):
            self._fx_dirty_ui = False
            if self._fx_ui_open:
                self._refresh_fx_panel()
        # Coalesced CRT redraw — blanked immediately, painted after debounce
        if getattr(self, "_scope_needs_paint", False):
            if time.monotonic() >= float(getattr(self, "_scope_paint_at", 0.0)):
                self._flush_scope_paint()
        # Keep touch bar stacked above log chrome if packing ever races
        if self._mode == "synth" and not self._overlay_busy():
            try:
                self._touch.lift()
            except Exception:
                pass
        if not self._stop.is_set():
            self.root.after(40, self._drain_queue)

