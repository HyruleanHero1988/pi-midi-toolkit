"""Settings / session JSON load-save for MidiToneApp."""
from __future__ import annotations

import json
import os
import pathlib
from typing import Any, Dict

from pidi.constants import SETTINGS_PATH, SETTINGS_VERSION, SONG_OUT_MODES, UI_MODES


class SessionIoMixin:
    def _refresh_ui_after_session(self) -> None:
        """Repaint chrome after settings.json / preset LOAD."""
        self._paint_full_vel_btn()
        self._paint_drum_lock_btn()
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._sync_voice_index_from_morph()
        self._build_pads_mode()
        self._paint_kaoss()
        self._paint_song_slots()
        self._refresh_song_status()
        self._refresh_seq_status()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())
        pending = self._pending_restore_mode
        self._pending_restore_mode = None
        if isinstance(pending, str) and pending in UI_MODES:
            self._switch_mode(pending)
        try:
            self._paint_synth_waveform(force=True)
        except Exception:
            pass


    def _session_dict(self) -> Dict[str, Any]:
        return {
            "version": SETTINGS_VERSION,
            "full_velocity": bool(self._full_vel),
            "active_preset": self._active_preset_name,
            "ui_mode": self._mode,
            "pads_view": self._pads_view,
            "synth": self.engine.snapshot_settings(),
            # Mode toggles — restore the editing context, not just the sound.
            "drum_mode": bool(self.engine.drum_mode()),
            "fx_mode": bool(self.engine.fx_mode()),
            "bus_fx_mode": bool(self.engine.bus_fx_mode()),
            "fx_edit_kind": str(self.engine.fx_edit_kind()),
            "seq": self._seq.export_state(),
            "phrases": self._phrases.export_bank(),
            "pads": {
                "view": self._pads_view,
                "out_mode": self._phrase_out_mode,
            },
            "songs": {
                "selected": self._song_selected,
                "bpm": float(self._songs.bpm()),
                "loop": bool(self._songs.loop_enabled()),
                "out_mode": self._songs.out_mode(),
            },
            "screensaver_sec": float(self._idle.timeout_sec),
            "kaoss": self._kaoss.snapshot(),
        }


    def _apply_session_dict(self, data: Dict[str, Any]) -> None:
        self._suppress_autosave = True
        try:
            if "full_velocity" in data:
                self._full_vel = bool(data["full_velocity"])
            if "screensaver_sec" in data and "MIDI_TONE_SCREENSAVER_SEC" not in os.environ:
                try:
                    self._idle.timeout_sec = max(0.0, float(data["screensaver_sec"]))
                except (TypeError, ValueError):
                    pass
            if "active_preset" in data:
                name = data["active_preset"]
                self._active_preset_name = str(name) if name else None
            synth = data.get("synth")
            if isinstance(synth, dict):
                self.engine.apply_settings(synth)
            # Restore mode toggles after apply_settings (which clears them)
            if "drum_mode" in data:
                self.engine.set_drum_mode(bool(data["drum_mode"]))
            if bool(data.get("bus_fx_mode")):
                self.engine.set_bus_fx_mode(True)
            elif bool(data.get("fx_mode")):
                self.engine.set_fx_mode(True)
                kind = str(data.get("fx_edit_kind", "voice") or "voice")
                if kind == "drums":
                    self.engine.set_fx_edit_drums()
                elif kind == "drum":
                    # Keep last kit selection if present; else nearer voice is fine
                    pass
                elif kind == "bus":
                    self.engine.set_fx_edit_bus()
                else:
                    self.engine.set_fx_edit_voice(None)
            seq = data.get("seq")
            if isinstance(seq, dict):
                try:
                    self._seq.import_state(seq)
                except Exception as exc:
                    print(f"seq restore failed: {exc}", flush=True)
            phrases = data.get("phrases")
            if isinstance(phrases, dict):
                try:
                    self._phrases.import_bank(phrases, persist=True)
                except Exception as exc:
                    print(f"phrases restore failed: {exc}", flush=True)
            pads = data.get("pads")
            if isinstance(pads, dict):
                view = str(pads.get("view", self._pads_view) or "edit")
                self._pads_view = "play" if view == "play" else "edit"
                out = str(pads.get("out_mode", self._phrase_out_mode) or "local")
                self._phrase_out_mode = out if out in SONG_OUT_MODES else "local"
            elif "pads_view" in data:
                view = str(data.get("pads_view") or "edit")
                self._pads_view = "play" if view == "play" else "edit"
            kaoss = data.get("kaoss")
            if isinstance(kaoss, dict):
                self._kaoss.apply(kaoss)
            songs = data.get("songs")
            if isinstance(songs, dict):
                if "bpm" in songs:
                    self._songs.set_bpm(float(songs["bpm"]))
                if "loop" in songs:
                    self._songs.set_loop(bool(songs["loop"]))
                if "out_mode" in songs:
                    self._songs.set_out_mode(str(songs["out_mode"]))
                selected = songs.get("selected")
                if not selected and "slot" in songs:
                    # Back-compat with older slot-based settings
                    try:
                        selected = f"song-{int(songs['slot']) + 1:02d}.mid"
                    except Exception:
                        selected = None
                self._refresh_song_file_list(prefer=str(selected) if selected else None)
                path = self._selected_song_path()
                if path is not None and path.is_file():
                    self._songs.load(path)
                    if "bpm" in songs:
                        self._songs.set_bpm(float(songs["bpm"]))
            ui_mode = data.get("ui_mode")
            if isinstance(ui_mode, str) and ui_mode in UI_MODES:
                # Defer switch until chrome exists; store for caller
                self._pending_restore_mode = ui_mode
            self._settings_dirty = False
        finally:
            self._suppress_autosave = False


    def _load_settings_file(self, path: pathlib.Path) -> bool:
        if not path.is_file():
            return False
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except Exception as exc:
            print(f"settings load failed ({path}): {exc}", flush=True)
            return False
        if not isinstance(data, dict):
            return False
        self._apply_session_dict(data)
        return True


    def _save_settings_file(self, path: pathlib.Path, *, quiet: bool = False) -> bool:
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(path.suffix + ".tmp")
            tmp.write_text(
                json.dumps(self._session_dict(), indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            tmp.replace(path)
            self._settings_dirty = False
            if not quiet:
                print(f"settings saved → {path}", flush=True)
            return True
        except Exception as exc:
            print(f"settings save failed ({path}): {exc}", flush=True)
            return False


    def _mark_settings_dirty(self) -> None:
        if self._suppress_autosave:
            return
        self._settings_dirty = True


    def _autosave_tick(self) -> None:
        if self._stop.is_set():
            return
        if self._settings_dirty:
            self._save_settings_file(SETTINGS_PATH, quiet=True)
        if not self._stop.is_set():
            self.root.after(2000, self._autosave_tick)

