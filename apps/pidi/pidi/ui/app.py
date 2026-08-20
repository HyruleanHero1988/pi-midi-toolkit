"""PiDI kiosk application shell (Tk)."""
from __future__ import annotations

import json
import math
import os
import pathlib
import queue
import shutil
import subprocess
import sys
import threading
import time
import tkinter as tk
from tkinter import ttk
from typing import Any, Dict, List, Optional, Tuple

try:
    import numpy as np
except ImportError as e:  # pragma: no cover
    sys.exit("numpy required: pip install numpy (or apt install python3-numpy)\n" + str(e))

try:
    import mido
except ImportError as e:  # pragma: no cover
    sys.exit("mido required: pip install mido python-rtmidi\n" + str(e))

from pidi import updater
from pidi.audio.drums import (
    downsample_waveform,
    drum_model_for_note,
    mpk_note_for_phrase_cell,
    render_drum_preview,
)
from pidi.audio.engine import SineEngine
from pidi.audio.wavetable import load_wavetables
from pidi.constants import *  # noqa: F403
from pidi.domain.phrases import (
    PhraseCell,
    PhrasePadBank,
    clamp_phrase_gain,
    phrase_cell_for_note,
    phrase_pad_label,
    phrase_pad_tile_color,
    scale_velocity,
)
from pidi.domain.songs import (
    SongPlayer,
    list_song_files,
    pick_song_output_name,
    seed_demo_songs,
)
from pidi.jambox_client import (
    JamboxClient,
    connect_or_spawn,
    midi_notice_to_message,
    prefer_python_engine,
)
from pidi.kaoss import (
    GATE_PATTERNS,
    KAOSS_OUT_MODES,
    LED_COLS,
    LED_ROWS,
    PROGRAM_BY_ID,
    ROOT_OCTAVE_MIDI,
    SCALE_BY_ID,
    VIZ_STYLE_LABELS,
    VIZ_STYLES,
    KaossEvent,
    KaossPad,
    KaossProgram,
    clamp01,
    glow_radii,
    glow_step,
    grid_line_widths,
    hsv_to_rgb,
    note_grid_xs,
    note_name as kaoss_note_name,
    pad_led_hex,
    program_hue,
    rgb_hex,
)
from pidi.screensaver import (
    PIXEL_SHIFT_AMPLITUDE,
    IdleWatch,
    PanelBacklight,
    next_timeout_preset,
    orbit_xy,
    pixel_shift_xy,
    timeout_from_env,
    timeout_label,
)
from pidi.sequencer import (
    SEQ_EMPTY,
    SEQ_OVERDUB,
    SEQ_REC_BACKBONE,
    LoopEvent,
    OverdubSequencer,
    trim_loop_take,
)
from pidi.ui.chrome import ChromeMixin
from pidi.ui.midi_io import MidiIoMixin
from pidi.ui.session_io import SessionIoMixin
from pidi.ui.scope import blank_waveform_on_canvas, draw_scope_grid, draw_waveform_on_canvas
from pidi.ui.screens import (
    FxScreenMixin,
    HomeScreenMixin,
    KaossScreenMixin,
    KitScreenMixin,
    LogScreenMixin,
    PadsScreenMixin,
    PresetsScreenMixin,
    SeqScreenMixin,
    SettingsScreenMixin,
    SongsScreenMixin,
    SynthScreenMixin,
)


def format_message(msg: mido.Message) -> str:
    if msg.type == "note_on":
        return f"Note On   ch{msg.channel + 1}  n{msg.note}  vel {msg.velocity}"
    if msg.type == "note_off":
        return f"Note Off  ch{msg.channel + 1}  n{msg.note}"
    if msg.type == "control_change":
        return f"CC        ch{msg.channel + 1}  cc{msg.control}  {msg.value}"
    if msg.type == "pitchwheel":
        return f"PitchBend ch{msg.channel + 1}  {msg.pitch}"
    if msg.type == "program_change":
        return f"Program   ch{msg.channel + 1}  prog {msg.program}"
    if msg.type == "aftertouch":
        return f"AT        ch{msg.channel + 1}  {msg.value}"
    if msg.type == "polytouch":
        return f"PolyAT    ch{msg.channel + 1}  n{msg.note}  {msg.value}"
    return str(msg)


class MidiToneApp(
    MidiIoMixin,
    SessionIoMixin,
    HomeScreenMixin,
    SynthScreenMixin,
    SeqScreenMixin,
    PadsScreenMixin,
    KaossScreenMixin,
    SongsScreenMixin,
    PresetsScreenMixin,
    LogScreenMixin,
    SettingsScreenMixin,
    KitScreenMixin,
    FxScreenMixin,
    ChromeMixin,
):
    def __init__(
        self,
        port_filter: str,
        list_only: bool,
        max_voices: int,
        waves_dir: pathlib.Path,
        fullscreen: bool = False,
    ) -> None:
        self.port_filter = port_filter.strip().lower()
        self.event_q: queue.Queue = queue.Queue(maxsize=EVENT_Q_MAX)
        self._waves_dir = pathlib.Path(waves_dir)
        self._user_waves_dir = USER_WAVETABLES_DIR
        try:
            self._user_waves_dir.mkdir(parents=True, exist_ok=True)
        except Exception:
            pass
        self._tables = load_wavetables(self._waves_dir, self._user_waves_dir)
        self.engine = SineEngine(self._tables, max_voices=max_voices)
        self._jambox: Optional[JamboxClient] = None
        self._jambox_proc: Optional[subprocess.Popen] = None
        self._jambox_owns_midi = False
        # Delay/reverb numbers beside user wavetables (drive/tone already in the wave)
        self._voice_fx_sidecars: Dict[str, Dict[str, float]] = load_user_voice_fx_map(
            self._user_waves_dir
        )
        for vname, snap in self._voice_fx_sidecars.items():
            self.engine.apply_voice_fx_sidecar(vname, snap)
        self._inport: Optional[mido.ports.BaseInput] = None
        self._poll_thread: Optional[threading.Thread] = None
        self._stop = threading.Event()
        self._voice_names = self.engine.voice_names
        self._voice_index = 0
        self._fullscreen = bool(fullscreen)

        if list_only:
            self._print_ports()
            return

        port_name = (
            self._pick_port(retries=20, delay_s=0.5, allow_fallback=False)
            or self._pick_port(retries=1, delay_s=0.0, allow_fallback=True)
        )
        if port_name is None:
            sys.exit("No MIDI input ports found. Is the MPK plugged in?")

        print(f"midi: will use input '{port_name}' (open after UI build)", flush=True)
        print(f"voices: {', '.join(self._voice_names)}", flush=True)

        self._full_vel = True

        # Create the Tk root BEFORE opening PortAudio — on Pi + labwc/Xwayland,
        # starting audio first then Tk can abort during tk.Tk() with no traceback.
        print("ui: creating Tk root", flush=True)
        self.root = tk.Tk()
        print("ui: Tk root ok", flush=True)
        self.root.title("PiDI")
        # Idle watch before the MIDI thread so notes can poke it safely.
        self._idle = IdleWatch(timeout_from_env())
        self._backlight = PanelBacklight()
        # Deploy / kiosk restart while blanked leaves sysfs at 0. Unblank now
        # so the new process does not come up on a dark panel.
        self._backlight.ensure_lit()
        self._saver_canvas: Optional[tk.Canvas] = None
        self._saver_hint: Optional[int] = None
        self._saver_clock: Optional[int] = None
        self._saver_started = 0.0
        self._saver_timeout_btn: Optional[tk.Button] = None
        self._saver_tick_after: Optional[str] = None
        self._shift_started = time.monotonic()
        self._shift_xy = (None, None)
        self.root.bind("<Destroy>", self._on_root_destroy, add="+")
        # TFT70 / Pi panel target is 800×480 (older builds used 800×420 and left a gap)
        self.root.geometry("800x480")
        self.root.configure(bg="#111111")
        self.root.protocol("WM_DELETE_WINDOW", self._on_close)
        if self._fullscreen:
            try:
                self.root.attributes("-fullscreen", True)
            except Exception:
                try:
                    self.root.attributes("-zoomed", True)
                except Exception:
                    self.root.state("zoomed")
            print("ui: fullscreen", flush=True)
        # PiDI branded splash while audio + UI chrome build (covers full window
        # until construction finishes — avoids a blank gap after destroy).
        self._boot_splash_photo = None
        splash_path = pathlib.Path(__file__).resolve().parent / "branding" / "pidi-splash.png"
        splash = tk.Frame(self.root, bg="#000000", highlightthickness=0, borderwidth=0)
        if splash_path.is_file():
            try:
                self._boot_splash_photo = tk.PhotoImage(file=str(splash_path))
            except Exception:
                try:
                    from PIL import Image, ImageTk  # type: ignore

                    im = Image.open(splash_path)
                    self._boot_splash_photo = ImageTk.PhotoImage(im)
                except Exception:
                    self._boot_splash_photo = None
        if self._boot_splash_photo is not None:
            tk.Label(
                splash,
                image=self._boot_splash_photo,
                bg="#000000",
                borderwidth=0,
                highlightthickness=0,
            ).place(relx=0.5, rely=0.5, anchor="center")
        else:
            tk.Label(
                splash,
                text="PiDI",
                font=("DejaVu Sans", 42, "bold"),
                fg="#00d4ff",
                bg="#000000",
            ).place(relx=0.5, rely=0.44, anchor="center")
            tk.Label(
                splash,
                text="Raspberry Pi MIDI Toolkit",
                font=("DejaVu Sans", 14),
                fg="#00d4ff",
                bg="#000000",
            ).place(relx=0.5, rely=0.56, anchor="center")
        splash.place(x=0, y=0, relwidth=1, relheight=1)
        splash.lift()
        self._boot_splash = splash
        try:
            self.root.deiconify()
            self.root.lift()
            self.root.update()
        except Exception:
            self.root.update_idletasks()
        self.root.update_idletasks()
        self._apply_display_geometry()
        self.root.update_idletasks()

        # Defer PortAudio + MIDI until after the heavy Tk build — otherwise the
        # audio callback xruns under GIL while widgets/scopes are constructed.
        self._boot_port_name = port_name

        self._full_vel_btn: Optional[tk.Button] = None
        self._drum_lock_btn: Optional[tk.Button] = None
        self._fx_mode_btn: Optional[tk.Button] = None
        self._bus_fx_mode_btn: Optional[tk.Button] = None
        self._voice_lbl: Optional[tk.Label] = None
        self._wave_canvas: Optional[tk.Canvas] = None
        self._wave_caption: Optional[tk.Label] = None
        self._grid_open = False
        self._grid_frame: Optional[tk.Frame] = None
        self._grid_btns: Dict[str, tk.Button] = {}
        self._morph_ui_open = False
        self._morph_frame: Optional[tk.Frame] = None
        self._morph_pick_side = "a"  # which endpoint the next grid tap sets
        self._morph_side_btns: Dict[str, tk.Button] = {}
        self._morph_grid_btns: Dict[str, tk.Button] = {}
        self._morph_status_lbl: Optional[tk.Label] = None
        self._kit_ui_open = False
        self._kit_frame: Optional[tk.Frame] = None
        self._kit_btns: Dict[int, tk.Button] = {}
        self._kit_all_btn: Optional[tk.Button] = None
        self._kit_wave_canvas: Optional[tk.Canvas] = None
        self._kit_status_var = tk.StringVar(value="")
        self._kit_selected_note = 36  # factory kick
        self._kit_all_drums = False  # FX edit target = shared kit group bus
        self._kit_view = "grid"  # grid | wave (scope is a drill-down)
        self._fx_ui_open = False
        self._fx_frame: Optional[tk.Frame] = None
        self._fx_title_var: Optional[tk.StringVar] = None
        self._fx_target_var: Optional[tk.StringVar] = None
        self._fx_value_vars: Dict[str, tk.StringVar] = {}
        self._fx_prev_mode = "synth"
        self._scope_blanked = False
        self._scope_blanked_synth = False
        self._scope_blanked_drum = False
        self._scope_dirty_synth = False
        self._scope_dirty_drum = False
        self._scope_needs_paint = False
        self._scope_paint_at = 0.0
        self._scope_first_dirty = 0.0
        self._fx_dirty_ui = False
        self._save_voice_open = False
        self._save_voice_frame: Optional[tk.Frame] = None
        self._save_voice_entry: Optional[tk.Entry] = None
        self._save_voice_status: Optional[tk.Label] = None
        self._save_voice_drive_btn: Optional[tk.Button] = None
        self._save_voice_keys: Optional[tk.Frame] = None
        self._save_voice_keys_digits = False
        self._power_ui_open = False
        self._power_frame: Optional[tk.Frame] = None
        self._mode = "synth"  # home | synth | seq | pads | kaoss | songs | log | presets | settings
        self._mode_btns: Dict[str, tk.Button] = {}
        self._seq = OverdubSequencer(self.engine, self._q_put)
        self._phrases = PhrasePadBank(self.engine, self._q_put, PHRASES_DIR)
        self._songs = SongPlayer(self.engine, self._q_put)
        self._kaoss = KaossPad()
        self._kaoss_fx_snap: Optional[Dict[str, Any]] = None
        self._kaoss_tick_armed = False
        self._kaoss_after_id: Optional[str] = None
        self._kaoss_viz_after_id: Optional[str] = None
        self._kaoss_leds: List[int] = []
        self._kaoss_led_geom: Optional[Tuple[Any, ...]] = None
        self._kaoss_glow: Dict[str, Any] = {}
        self._glow_amp = 0.0
        self._glow_t = 0.0
        self._glow_xy: Tuple[float, float] = (0.5, 0.5)
        self._kaoss_ripple_items: List[int] = []
        self._kaoss_ripples: List[Tuple[float, float, float]] = []
        self._kaoss_trail: List[Tuple[float, float, float]] = []
        self._kaoss_canvas: Optional[tk.Canvas] = None
        self._kaoss_status_var = tk.StringVar(value="")
        self._kaoss_axis_label_cache: Optional[Tuple[str, str]] = None
        self._kaoss_prog_btn: Optional[tk.Button] = None
        self._kaoss_scale_btn: Optional[tk.Button] = None
        self._kaoss_key_btn: Optional[tk.Button] = None
        self._kaoss_oct_btn: Optional[tk.Button] = None
        self._kaoss_hold_btn: Optional[tk.Button] = None
        self._kaoss_gate_btn: Optional[tk.Button] = None
        self._kaoss_bpm_lbl: Optional[tk.Label] = None
        self._kaoss_gear_btn: Optional[tk.Button] = None
        self._kaoss_play_btn: Optional[tk.Button] = None
        self._kaoss_picker_open = False
        self._kaoss_picker_kind = ""
        self._picker_ignore_until = 0.0
        self._kaoss_picker_frame: Optional[tk.Frame] = None
        self._kaoss_picker_btns: Dict[str, tk.Button] = {}
        self._kaoss_picker_inner: Optional[tk.Frame] = None
        self._kaoss_picker_drag: Optional[Dict[str, object]] = None
        self._kaoss_picker_count_var = tk.StringVar(value="")
        self._kaoss_scale_open = False
        self._kaoss_scale_btns: Dict[str, tk.Button] = {}
        self._kaoss_settings_open = False
        self._kaoss_settings_frame: Optional[tk.Frame] = None
        self._kaoss_settings_all_btn: Optional[tk.Button] = None
        self._kaoss_settings_axes_btn: Optional[tk.Button] = None
        self._kaoss_settings_grid_btn: Optional[tk.Button] = None
        self._kaoss_settings_grid_lbl: Optional[tk.Label] = None
        self._kaoss_settings_viz_btns: Dict[str, tk.Button] = {}
        self._kaoss_settings_out_btns: Dict[str, tk.Button] = {}
        self._kaoss_settings_ch_btns: Dict[int, tk.Button] = {}
        self._kaoss_play = False
        self._kaoss_play_footer = False
        self._kaoss_header: Optional[tk.Frame] = None
        self._kaoss_footer: Optional[tk.Frame] = None
        self._kaoss_exit_after_id: Optional[str] = None
        self._kaoss_play_exit_from_inside = False
        self._kaoss_play_exit_anchor: Optional[Tuple[float, float]] = None
        self._pads_view = "edit"  # play | edit
        self._phrase_out_mode = "local"  # local | usb | both (shares Songs USB port)
        self._phrases.set_output_hooks(
            get_out_mode=lambda: self._phrase_out_mode,
            ensure_outport=lambda: self._songs.ensure_outport(),
            get_outport=lambda: self._songs.outport(),
        )
        self._phrase_status_var = tk.StringVar(
            value=self._phrases.status_line(view=self._pads_view)
        )
        self._phrase_pad_btns: Dict[int, tk.Button] = {}
        self._phrase_clear_btn: Optional[tk.Button] = None
        self._phrase_mode_btn: Optional[tk.Button] = None
        self._phrase_out_btn: Optional[tk.Button] = None
        self._phrase_view_btns: Dict[str, tk.Button] = {}
        self._phrase_trig_btn: Optional[tk.Button] = None
        self._phrase_voice_btn: Optional[tk.Button] = None
        self._phrase_ch_btn: Optional[tk.Button] = None
        self._phrase_synth_btn: Optional[tk.Button] = None
        self._phrase_vib_btn: Optional[tk.Button] = None
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        self._seq_to_pad_armed = False
        self._seq_to_pad_btn: Optional[tk.Button] = None
        self._phrase_shell: Optional[tk.Frame] = None
        self._seq_status_var = tk.StringVar(value=self._seq.status_line())
        self._seq_layer_var = tk.StringVar(value="no layers yet")
        self._seq_rec_btn: Optional[tk.Button] = None
        self._seq_play_btn: Optional[tk.Button] = None
        self._seq_keep_btn: Optional[tk.Button] = None
        self._seq_drop_btn: Optional[tk.Button] = None
        self._seq_undo_btn: Optional[tk.Button] = None
        self._seq_extend_btn: Optional[tk.Button] = None
        self._seq_len_var = tk.StringVar(value="LEN 1×")
        self._vib_depth_var = tk.StringVar(value="0.50 st")
        self._vib_rate_var = tk.StringVar(value="5.0 Hz")
        self._vib_toggle_btn: Optional[tk.Button] = None
        self._preset_status_var = tk.StringVar(value="Tap a slot, then LOAD or SAVE.")
        self._preset_slot = 0
        self._preset_slot_btns: Dict[int, tk.Button] = {}
        self._active_preset_name: Optional[str] = None
        self._pending_restore_mode: Optional[str] = None
        self._save_preset_open = False
        self._save_preset_frame: Optional[tk.Frame] = None
        self._save_preset_entry: Optional[tk.Entry] = None
        self._save_preset_status: Optional[tk.Label] = None
        self._save_preset_keys: Optional[tk.Frame] = None
        self._save_preset_keys_digits = False
        self._song_status_var = tk.StringVar(
            value="Songs: tap a file to load, set BPM, then PLAY (LOCAL or USB→DIN)."
        )
        self._song_files: List[pathlib.Path] = []
        self._song_selected: Optional[str] = None  # filename in songs/
        self._song_scroll = 0
        self._song_row_btns: List[tk.Button] = []
        self._song_title_cache: Dict[str, str] = {}
        self._song_play_btn: Optional[tk.Button] = None
        self._song_out_btn: Optional[tk.Button] = None
        self._song_loop_btn: Optional[tk.Button] = None
        self._song_bpm_lbl: Optional[tk.Label] = None
        self._song_up_btn: Optional[tk.Button] = None
        self._song_down_btn: Optional[tk.Button] = None
        self._settings_dirty = False
        self._suppress_autosave = False
        self._update_check: Optional[updater.UpdateCheck] = None
        self._update_busy = False
        self._update_confirming = False
        self._settings_status_var = tk.StringVar(value=updater.format_status_lines())
        self._settings_check_btn: Optional[tk.Button] = None
        self._settings_update_btn: Optional[tk.Button] = None
        self._token_ui_open = False
        self._token_frame: Optional[tk.Frame] = None
        self._token_entry: Optional[tk.Entry] = None
        self._token_keys: Optional[tk.Frame] = None
        self._token_keys_digits = False

        # Keep PiDI splash on top while chrome is packed underneath
        splash = getattr(self, "_boot_splash", None)
        if splash is not None:
            try:
                splash.lift()
            except Exception:
                pass

        # Persistent chrome: title, HOME, POWER. Jam modes keep SYNTH/SEQ/PADS
        # on the right; every other screen is reached from HOME tiles.
        self._nav = tk.Frame(self.root, bg="#1d2021")
        self._nav.pack(side=tk.TOP, fill=tk.X, padx=0, pady=0)
        tk.Label(
            self._nav, text="PiDI", font=("DejaVu Sans", 14, "bold"),
            fg="#00d4ff", bg="#1d2021", padx=10, pady=8,
        ).pack(side=tk.LEFT)
        home_btn = self._mk_touch_btn(
            self._nav, "HOME", lambda: self._switch_mode("home"), bg="#3c3836"
        )
        home_btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
        home_btn.pack(side=tk.LEFT, padx=(0, 4))
        self._mode_btns["home"] = home_btn
        self._home_btn = home_btn
        set_btn = self._mk_touch_btn(
            self._nav, "SET", lambda: self._switch_mode("settings"), bg="#3c3836"
        )
        set_btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
        set_btn.pack(side=tk.LEFT, padx=(0, 4))
        self._settings_nav_btn = set_btn
        power_btn = self._mk_touch_btn(
            self._nav, "POWER", self._open_power_menu, bg="#9d0006"
        )
        power_btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
        power_btn.pack(side=tk.LEFT, padx=(0, 4))
        nav_modes = tk.Frame(self._nav, bg="#1d2021")
        nav_modes.pack(side=tk.RIGHT, padx=4, pady=4)
        self._jam_btns: Dict[str, tk.Button] = {}
        for key, label in (
            ("synth", "SYNTH"),
            ("seq", "SEQ"),
            ("pads", "PADS"),
            ("kaoss", "KAOSS"),
        ):
            btn = self._mk_touch_btn(
                nav_modes, label, lambda m=key: self._switch_mode(m), bg="#3c3836"
            )
            btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
            self._jam_btns[key] = btn
            self._mode_btns[key] = btn

        # Mode content host
        self._mode_host = tk.Frame(self.root, bg="#111111")
        self._mode_host.pack(side=tk.TOP, fill=tk.BOTH, expand=True)

        self._synth_shell = tk.Frame(self._mode_host, bg="#111111")
        self._seq_shell = tk.Frame(self._mode_host, bg="#111111")
        self._pads_shell = tk.Frame(self._mode_host, bg="#111111")
        self._phrase_shell = self._pads_shell
        self._kaoss_shell = tk.Frame(self._mode_host, bg="#111111")
        self._songs_shell = tk.Frame(self._mode_host, bg="#111111")
        self._presets_shell = tk.Frame(self._mode_host, bg="#111111")
        self._log_shell = tk.Frame(self._mode_host, bg="#111111")
        self._settings_shell = tk.Frame(self._mode_host, bg="#111111")
        self._home_shell = tk.Frame(self._mode_host, bg="#111111")

        # Bottom touch bar packed first so it never gets crushed / lost
        self._touch = tk.Frame(self._synth_shell, bg="#111111")
        self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)

        row1 = tk.Frame(self._touch, bg="#111111")
        row1.pack(fill=tk.X, pady=(0, 6))
        self._mk_touch_btn(row1, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3
        )
        self._full_vel_btn = self._mk_touch_btn(
            row1, "FULL VEL: ON", self._toggle_full_vel, bg="#689d6a"
        )
        self._full_vel_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
        self._fx_mode_btn = self._mk_touch_btn(
            row1, "FX MODE", self._toggle_fx_mode, bg="#3c3836"
        )
        self._fx_mode_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
        self._bus_fx_mode_btn = self._mk_touch_btn(
            row1, "BUS FX", self._toggle_bus_fx_mode, bg="#3c3836"
        )
        self._bus_fx_mode_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)

        # Voice picker — prev / name / next + full grid
        row2 = tk.Frame(self._touch, bg="#111111")
        row2.pack(fill=tk.X, pady=(0, 6))
        self._mk_touch_btn(row2, "◀ PREV", self._prev_voice, bg="#3c3836").pack(
            side=tk.LEFT, fill=tk.BOTH, padx=3, ipady=10
        )
        self._voice_lbl = tk.Label(
            row2,
            text=self._voice_label_text(),
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#458588",
            padx=8,
            pady=12,
        )
        self._voice_lbl.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
        # Tap the name → open the voice grid (easier than blind PREV/NEXT)
        self._voice_lbl.bind("<ButtonPress-1>", lambda _e: self._open_voice_grid())
        self._mk_touch_btn(row2, "NEXT ▶", self._next_voice, bg="#3c3836").pack(
            side=tk.LEFT, fill=tk.BOTH, padx=3, ipady=10
        )

        row3 = tk.Frame(self._touch, bg="#111111")
        row3.pack(fill=tk.X)
        self._mk_touch_btn(row3, "VOICES", self._open_voice_grid, bg="#458588").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8
        )
        self._mk_touch_btn(row3, "MORPH", self._open_morph_menu, bg="#b16286").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8
        )
        self._mk_touch_btn(row3, "KIT", self._open_kit_explorer, bg="#d79921").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8
        )
        self._drum_lock_btn = self._mk_touch_btn(
            row3, "DRUM MODE", self._toggle_drum_lock, bg="#3c3836"
        )
        self._drum_lock_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)

        self._main = tk.Frame(self._synth_shell, bg="#111111")
        self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)

        header = tk.Frame(self._main, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(6, 2))
        tk.Label(
            header, text="Synth", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text=port_name, font=("DejaVu Sans", 11),
            fg="#8ec07c", bg="#111111",
        ).pack(side=tk.RIGHT)

        # CRT morph-cycle scope — pack early with a reserved height so the
        # chunky touch bar never collapses it on 480p kiosk screens.
        self._wave_caption = tk.Label(
            self._main,
            text="Morph",
            font=("DejaVu Sans", 11),
            fg="#4ade80",
            bg="#111111",
            anchor="w",
        )
        self._wave_caption.pack(fill=tk.X, padx=8, pady=(2, 0))
        self._wave_canvas = tk.Canvas(
            self._main,
            height=150,
            bg=SCOPE_CRT_BG,
            highlightthickness=1,
            highlightbackground="#14532d",
            bd=0,
        )
        self._wave_canvas.pack(fill=tk.X, padx=8, pady=(2, 4))
        self._wave_canvas.pack_propagate(False)
        self._wave_canvas.bind("<Configure>", self._on_synth_scope_configure)

        self.last_var = tk.StringVar(value="Waiting for MIDI…")
        last_lbl = tk.Label(
            self._main, textvariable=self.last_var,
            font=("DejaVu Sans Mono", 13, "bold"), fg="#fabd2f", bg="#111111",
            wraplength=780, justify=tk.LEFT, anchor="w",
        )
        last_lbl.pack(fill=tk.X, padx=8, pady=(2, 0))

        self.active_var = tk.StringVar(value="Active notes: —")
        active_lbl = tk.Label(
            self._main, textvariable=self.active_var,
            font=("DejaVu Sans", 11), fg="#83a598", bg="#111111", anchor="w",
        )
        active_lbl.pack(fill=tk.X, padx=8)

        self.mod_var = tk.StringVar(value=self._format_mod_line())
        mod_lbl = tk.Label(
            self._main, textvariable=self.mod_var,
            font=("DejaVu Sans Mono", 10), fg="#d3869b", bg="#111111", anchor="w",
        )
        mod_lbl.pack(fill=tk.X, padx=8, pady=(0, 2))

        self._active_notes: Dict[Tuple[int, int], int] = {}
        # Select first voice explicitly
        self.engine.set_waveform(self._voice_names[self._voice_index])
        self.root.after(40, self._drain_queue)
        self.root.after(80, lambda: self._paint_synth_waveform(force=True))
        # Default morph pair: first voice ↔ second (or same if only one)
        if len(self._voice_names) > 1:
            self.engine.set_morph_pair(0, 1, morph=0.0)

        PRESETS_DIR.mkdir(parents=True, exist_ok=True)
        SONGS_DIR.mkdir(parents=True, exist_ok=True)
        PHRASES_DIR.mkdir(parents=True, exist_ok=True)
        seeded = seed_demo_songs()

        self._build_seq_mode()
        self._build_pads_mode()
        self._build_kaoss_mode()
        self._build_songs_mode()
        self._build_presets_mode()
        self._build_log_mode()
        self._build_settings_mode()
        self._build_home_mode()
        self._switch_mode("synth")

        # Restore last session (full vel, morph, seq, phrases, UI mode, …)
        restored = self._load_settings_file(SETTINGS_PATH)
        self._refresh_ui_after_session()

        self._append_log(f"Listening on: {port_name}")
        self._append_log(f"Loaded {len(self._voice_names)} voices — VOICES grid / MORPH pair.")
        self._append_log(
            "MPK knobs (keys): morph / tone / attack / release / vib / — / synth lvl"
        )
        self._append_log(
            "Pads = analog drum voices. After a pad (or DRUM LOCK): knobs → "
            "pitch / stretch / noise / drum-tone / — / — / — / drum lvl"
        )
        self._append_log("HOME opens every mode. Jam cluster: SYNTH / SEQ / PADS / KAOSS stay in the top bar.")
        if seeded:
            self._append_log(
                f"Added {seeded} demo song(s) from demo-songs/ (offline classical pack)."
            )
        if restored:
            self._append_log(f"Restored session from {SETTINGS_PATH.name}")
        else:
            self._append_log("No settings.json yet — changes will autosave.")
        self._append_log("If knobs do nothing: Prog Select + Pad 1 (MPC program).")

        self._attach_jambox()

        # Start audio after Tk chrome exists so construction can't starve the callback.
        # Re-resolve MIDI in case the MPK finished enumerating after our earlier pick.
        port_name = (
            self._pick_port(retries=12, delay_s=0.4, allow_fallback=False)
            or self._pick_port(retries=1, delay_s=0.0, allow_fallback=True)
            or getattr(self, "_boot_port_name", None)
            or port_name
        )
        self.engine.start()
        print("midi: audio engine started", flush=True)
        if self._jambox_owns_midi:
            print("midi: input via jambox-engine (mido skipped)", flush=True)
            self._poll_thread = threading.Thread(target=self._midi_loop, daemon=True)
            self._poll_thread.start()
            print("midi: poll thread started", flush=True)
        else:
            self._inport = mido.open_input(port_name)
            print(f"midi: input port open ({port_name})", flush=True)
            self._poll_thread = threading.Thread(target=self._midi_loop, daemon=True)
            self._poll_thread.start()
            print("midi: poll thread started", flush=True)
            if self.port_filter and self.port_filter not in port_name.lower():
                self.root.after(1500, self._maybe_reopen_midi)

        print("ui: construction complete", flush=True)
        # Reveal chrome only after first layout pass
        splash = getattr(self, "_boot_splash", None)
        if splash is not None:
            try:
                self.root.update_idletasks()
                splash.destroy()
            except Exception:
                pass
            self._boot_splash = None
            self._boot_splash_photo = None
            try:
                self.root.update_idletasks()
            except Exception:
                pass
        self.root.after(2000, self._autosave_tick)
        self.root.bind_all("<ButtonPress>", self._on_pointer_activity, add="+")
        self.root.bind_all("<ButtonRelease>", self._on_pointer_activity, add="+")
        self.root.bind_all("<Motion>", self._on_pointer_activity, add="+")
        self._saver_tick_after = self.root.after(1000, self._screensaver_tick)
        self._apply_pixel_shift()
        self._append_log(
            f"TFT burn-in guard: {timeout_label(self._idle.timeout_sec)} "
            "— tap the panel to wake; MIDI does not."
        )


    def _voice_label_text(self) -> str:
        left, right, blend = self.engine.morph_neighbors()
        if left == right:
            return f"{self._voice_index + 1}/{len(self._voice_names)}  {left.upper()}"
        pct = int(round(blend * 100))
        return f"{left.upper()} → {right.upper()}  {pct}%"


    def _format_mod_line(self) -> str:
        st = self.engine.modulation_state()
        if self.engine.fx_knob_focus():
            delay_ms = int((0.05 + st.get("fx_delay_time", 0.0) * 0.70) * 1000)
            target = self.engine.fx_edit_label()
            prefix = "BUS FX" if self.engine.bus_fx_mode() else f"FX {target}"
            return (
                f"{prefix}  "
                f"Drive:{int(st.get('fx_drive', 0.0) * 127):3d}  "
                f"Dly:{delay_ms:3d}ms  "
                f"Fb:{int(st.get('fx_delay_fb', 0.0) * 127):3d}  "
                f"Dmix:{int(st.get('fx_delay_mix', 0.0) * 127):3d}  "
                f"Rvb:{int(st.get('fx_reverb_mix', 0.0) * 127):3d}  "
                f"Syn:{int(st.get('synth_level', st['level']) * 127):3d}  "
                f"Drm:{int(st.get('drum_level', 1.0) * 127):3d}"
            )
        if self.engine.drum_knob_focus():
            return (
                "DRUM MODE  "
                f"Pitch:{int(st['drum_pitch'] * 127):3d}  "
                f"Stretch:{int(st['drum_decay'] * 127):3d}  "
                f"Noise:{int(st['drum_noise'] * 127):3d}  "
                f"Tone:{int(st['drum_tone'] * 127):3d}  "
                f"DrmLvl:{int(st.get('drum_level', 1.0) * 127):3d}"
            )
        left, right, blend = self.engine.morph_neighbors()
        if left == right:
            morph_txt = left
        else:
            morph_txt = f"{left}→{right}"
        depth, rate, always = self.engine.vib_state()
        amount = max(float(st["mod"]), always)
        vib_txt = f"{depth:.1f}st@{rate:.1f}Hz" if amount > 0.01 else "off"
        return (
            f"Morph:{int(blend * 100):3d}% ({morph_txt})  "
            f"Tone:{int(st['tone'] * 127):3d}  "
            f"Syn:{int(st.get('synth_level', st['level']) * 127):3d}  "
            f"Drm:{int(st.get('drum_level', 1.0) * 127):3d}  "
            f"Bend:{st['bend']:+.2f}  "
            f"Vib:{vib_txt}"
        )


    def _overlay_busy(self) -> bool:
        return (
            self._power_ui_open
            or self._grid_open
            or self._morph_ui_open
            or self._kit_ui_open
            or self._save_voice_open
            or self._save_preset_open
            or self._fx_ui_open
            or self._token_ui_open
            or self._kaoss_picker_open
            or self._kaoss_settings_open
            or bool(getattr(self, "_idle", None) and self._idle.active)
        )


    def _select_preset_slot(self, slot: int) -> None:
        self._preset_slot = max(0, min(PRESET_SLOTS - 1, slot))
        path = self._preset_path(self._preset_slot)
        if path.is_file():
            self._preset_status_var.set(
                f"Slot {self._preset_slot + 1} selected — LOAD restores full session; SAVE overwrites."
            )
        else:
            self._preset_status_var.set(
                f"Slot {self._preset_slot + 1} empty — SAVE stores the full session (name it)."
            )
        self._paint_preset_slots()


    def _refresh_kit_status(self) -> None:
        pitch, decay, noise, tone = self.engine.drum_macros()
        label = phrase_pad_label(
            max(0, min(15, self._kit_selected_note - PHRASE_PAD_BASE))
        )
        model = self._kit_model_selected().replace("_", " ")
        macros = (
            f"pitch {int(pitch * 127)} · stretch {int(decay * 127)} · "
            f"noise {int(noise * 127)} · tone {int(tone * 127)}"
        )
        if self._kit_all_drums:
            self._kit_status_var.set(
                f"ALL DRUMS · FX shared kit bus · knobs reshape body · {macros}"
            )
        elif getattr(self, "_kit_view", "grid") == "wave":
            self._kit_status_var.set(
                f"{label} · {model} · {macros} · scope {int(DRUM_SCOPE_SEC * 1000)} ms"
            )
        else:
            self._kit_status_var.set(
                f"{label} · {model} · {macros} · tap pad to play · WAVE for scope"
            )


    def _fill_kaoss_picker(self) -> None:
        inner = self._kaoss_picker_inner
        drag = self._kaoss_picker_drag
        if inner is None or drag is None:
            return
        for child in inner.winfo_children():
            child.destroy()
        _title, count, cols, items = self._kaoss_picker_spec(self._kaoss_picker_kind)
        self._kaoss_picker_count_var.set(count)
        self._kaoss_picker_btns = {}
        current = self._kaoss_picker_current()
        for i, (item_id, label) in enumerate(items):
            row, col = divmod(i, cols)
            btn = self._mk_scroll_select_btn(
                inner,
                label,
                lambda value=item_id: self._kaoss_picker_choose(value),
                drag,
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=16)
            btn.grid(row=row, column=col, sticky="nsew", padx=3, pady=3, ipadx=4, ipady=8)
            self._kaoss_picker_btns[item_id] = btn
        for col in range(cols):
            inner.grid_columnconfigure(col, weight=1)
        if self._kaoss_picker_kind == "scale":
            self._kaoss_scale_btns = self._kaoss_picker_btns
        binder = drag.get("_bind_tree")
        if callable(binder):
            binder(inner)
        self._paint_kaoss_picker()


    def _switch_mode(self, mode: str) -> None:
        mode = mode if mode in UI_MODES else "synth"
        # Close synth-only overlays before swapping shells
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._save_preset_open:
            self._close_save_preset(restore_main=False)
        if self._token_ui_open:
            self._close_update_token(restore_main=False)
        if self._kaoss_picker_open:
            self._close_kaoss_picker(restore_main=False)
        if self._kaoss_settings_open:
            self._close_kaoss_settings(restore_main=False)

        # Leaving pads while recording: keep the take
        if self._mode == "pads" and mode != "pads":
            if self._phrases.is_recording():
                self._phrases.stop_record()
            self._phrase_clear_armed = False
            self._phrase_mode_armed = False
            if mode != "seq":
                self._seq_to_pad_armed = False
        if mode not in ("pads", "seq"):
            self._seq_to_pad_armed = False
        # Leaving KAOSS: lift the pad unless HOLD is latched; stop the LED tick
        if self._mode == "kaoss" and mode != "kaoss":
            self._kaoss_leave_play()
            self._kaoss_cancel_viz()
            if not self._kaoss.hold:
                self._kaoss_apply(self._kaoss.release(), ended=True)
            else:
                # HOLD keeps the last note, but VIB / FILTER overlays must not
                # follow you onto SYNTH / SEQ.
                self._kaoss_restore_fx()

        self._mode = mode
        self._home_shell.pack_forget()
        self._synth_shell.pack_forget()
        self._seq_shell.pack_forget()
        self._pads_shell.pack_forget()
        self._kaoss_shell.pack_forget()
        self._songs_shell.pack_forget()
        self._presets_shell.pack_forget()
        self._log_shell.pack_forget()
        self._settings_shell.pack_forget()
        if self._grid_frame is not None:
            self._grid_frame.pack_forget()
        if self._morph_frame is not None:
            self._morph_frame.pack_forget()
        if self._kit_frame is not None:
            self._kit_frame.pack_forget()
        if self._fx_frame is not None:
            try:
                self._fx_frame.pack_forget()
            except Exception:
                pass
        if self._save_preset_frame is not None:
            try:
                self._save_preset_frame.pack_forget()
            except Exception:
                pass
        if self._save_voice_frame is not None:
            try:
                self._save_voice_frame.pack_forget()
            except Exception:
                pass

        if mode == "home":
            self._home_shell.pack(fill=tk.BOTH, expand=True)
        elif mode == "seq":
            self._seq_shell.pack(fill=tk.BOTH, expand=True)
            self._refresh_seq_status()
        elif mode == "pads":
            self._pads_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_phrase_pads()
        elif mode == "kaoss":
            self._kaoss_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_kaoss()
            self._kaoss_draw_grid()
            self._kaoss_arm_tick()
            self._kaoss_arm_viz()
        elif mode == "songs":
            self._songs_shell.pack(fill=tk.BOTH, expand=True)
            # Rescan directory each visit so dropped-in .mid files appear
            self._paint_song_slots()
            self._refresh_song_status()
        elif mode == "presets":
            self._presets_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_preset_slots()
        elif mode == "log":
            self._log_shell.pack(fill=tk.BOTH, expand=True)
            try:
                self.log.see(tk.END)
            except Exception:
                pass
        elif mode == "settings":
            self._settings_shell.pack(fill=tk.BOTH, expand=True)
            self._refresh_settings_status()
        else:
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            # Ensure synth children are packed (overlays may have forgotten them)
            try:
                self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            except Exception:
                pass
            try:
                self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            except Exception:
                pass
            self._paint_synth_waveform(force=True)
        self._paint_mode_btns()


    def _paint_mode_btns(self) -> None:
        jam = self._mode in JAM_NAV_MODES
        for key, btn in self._jam_btns.items():
            if jam:
                if not btn.winfo_ismapped():
                    btn.pack(side=tk.LEFT, padx=1)
            else:
                btn.pack_forget()
            on = jam and key == self._mode
            color = "#458588" if on else "#3c3836"
            btn.configure(bg=color, activebackground=color)
        home = self._mode_btns.get("home")
        if home is not None:
            color = "#458588" if self._mode == "home" else "#3c3836"
            home.configure(bg=color, activebackground=color)
        settings_nav = getattr(self, "_settings_nav_btn", None)
        if settings_nav is not None:
            color = "#458588" if self._mode == "settings" else "#3c3836"
            try:
                settings_nav.configure(bg=color, activebackground=color)
            except tk.TclError:
                pass


    def _mk_touch_btn(self, parent: tk.Misc, text: str, command, bg: str = "#3c3836") -> tk.Button:
        """Touch-friendly button: fire on press (resistive panels often miss click)."""
        btn = tk.Button(
            parent, text=text,
            font=("DejaVu Sans", 14, "bold"), fg="#fbf1c7", bg=bg,
            activeforeground="#fbf1c7", activebackground=bg,
            relief=tk.FLAT, bd=0, padx=8, pady=12, cursor="hand2",
            takefocus=0,
        )

        def _fire(_event: object = None) -> str:
            # Debounce bounce from ADS7846. Fire on press only — pairing
            # ButtonPress + Button.command double-triggers on release.
            now = time.monotonic()
            last = getattr(btn, "_last_fire", 0.0)
            if now - last < 0.18:
                return "break"
            btn._last_fire = now  # type: ignore[attr-defined]
            command()
            return "break"

        # No command= callback: resistive panels often never complete a click.
        btn.bind("<ButtonPress-1>", _fire)
        return btn


    def _select_voice_index(self, idx: int, *, close_grid: bool = False) -> None:
        if not self._voice_names:
            return
        self._voice_index = idx % len(self._voice_names)
        name = self._voice_names[self._voice_index]
        # VOICES / PREV / NEXT set morph-A and park at pure A (B stays as morph target)
        self.engine.set_morph_index(self._voice_index)
        snap = self._voice_fx_sidecars.get(name)
        if snap is not None:
            self.engine.apply_voice_fx_sidecar(name, snap)
        self._mark_settings_dirty()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())
        self.last_var.set(f"Voice → {name.upper()}")
        self._q_put(("log", f"Voice → {name}", False))
        if self._grid_open:
            self._paint_voice_grid()
            if close_grid:
                self._close_voice_grid()
        if self._morph_ui_open:
            self._paint_morph_menu()
        if not self._overlay_busy():
            self._paint_synth_waveform(force=True)


    def _sync_voice_index_from_morph(self) -> None:
        """Keep UI index on the nearer morph endpoint while Knob1 moves."""
        a_idx, b_idx = self.engine.morph_pair_indices()
        blend = self.engine.morph()
        self._voice_index = a_idx if blend < 0.5 else b_idx
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        if self._grid_open:
            self._paint_voice_grid()
        if self._morph_ui_open:
            self._paint_morph_menu()


    def _open_save_voice(self) -> None:
        """Bake morph + drive + tone into a new dry wavetable (shape only)."""
        if self._save_voice_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        # Leave other overlays so the name pad is full-screen
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)

        self._save_voice_open = True
        self._save_voice_keys_digits = False
        self._synth_shell.pack_forget()

        self._save_voice_frame = tk.Frame(self._mode_host, bg="#111111")
        self._save_voice_frame.pack(fill=tk.BOTH, expand=True)

        header = tk.Frame(self._save_voice_frame, bg="#111111")
        header.pack(fill=tk.X, padx=6, pady=(6, 2))
        tk.Label(
            header,
            text="SAVE VOICE",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        a, b, blend = self.engine.morph_neighbors()
        hint = a if a == b else f"{a}→{b} {int(blend * 100)}%"
        tk.Label(
            header,
            text=f"bake shape · {hint}",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        tk.Label(
            self._save_voice_frame,
            text=(
                "Wave shape: morph + drive + tone → .wav. "
                "Alongside: delay/reverb amounts in a tiny .fx.json "
                "(drive stays in the wave, not double-applied)."
            ),
            font=("DejaVu Sans", 10),
            fg="#a89984",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, padx=8, pady=(0, 4))

        name_row = tk.Frame(self._save_voice_frame, bg="#111111")
        name_row.pack(fill=tk.X, padx=8, pady=4)
        suggested = self.engine.suggested_save_voice_name()
        self._save_voice_entry = tk.Entry(
            name_row,
            font=("DejaVu Sans Mono", 18),
            bg="#1d2021",
            fg="#fbf1c7",
            insertbackground="#fbf1c7",
            relief=tk.FLAT,
        )
        self._save_voice_entry.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, ipady=10)
        self._save_voice_entry.insert(0, suggested)
        self._save_voice_entry.focus_set()

        self._save_voice_status = tk.Label(
            self._save_voice_frame,
            text=(
                f"Will write {self._user_waves_dir.name}/{suggested}.wav "
                f"+ {suggested}.fx.json"
            ),
            font=("DejaVu Sans Mono", 11),
            fg="#83a598",
            bg="#111111",
            anchor="w",
        )
        self._save_voice_status.pack(fill=tk.X, padx=8, pady=(0, 4))

        opt = tk.Frame(self._save_voice_frame, bg="#111111")
        opt.pack(fill=tk.X, padx=6, pady=2)
        self._mk_touch_btn(
            opt, "SUGGEST", self._reset_save_voice_name, bg="#458588"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)
        self._mk_touch_btn(
            opt, "⌫", lambda: self._save_voice_type("\b"), bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)
        self._mk_touch_btn(
            opt, "CLR", lambda: self._save_voice_type("\x15"), bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)

        # SAVE/CANCEL claim their strip first — the keyboard shrinks, never the
        # buttons that end the job.
        footer = tk.Frame(self._save_voice_frame, bg="#111111")
        footer.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
        self._mk_touch_btn(
            footer, "SAVE", self._confirm_save_voice, bg="#689d6a"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10)
        self._mk_touch_btn(
            footer, "CANCEL", self._close_save_voice, bg="#9d0006"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10)

        keys = tk.Frame(self._save_voice_frame, bg="#111111")
        keys.pack(fill=tk.BOTH, expand=True, padx=4, pady=2)
        self._save_voice_keys = keys
        self._paint_save_voice_keyboard()
        self._append_log("SAVE VOICE — bake wave shape + keep delay/reverb alongside")


    def _paint_save_voice_keyboard(self) -> None:
        keys = getattr(self, "_save_voice_keys", None)
        if keys is None:
            return
        for w in keys.winfo_children():
            w.destroy()
        if self._save_voice_keys_digits:
            rows = ("1234567890", "-.")
            toggle_label = "ABC"
        else:
            rows = ("qwertyuiop", "asdfghjkl", "zxcvbnm_")
            toggle_label = "123"
        for r, row in enumerate(rows):
            fr = tk.Frame(keys, bg="#111111")
            fr.pack(fill=tk.BOTH, expand=True, pady=1)
            for ch in row:
                self._mk_touch_btn(
                    fr,
                    ch.upper() if ch.isalpha() else ch,
                    lambda c=ch: self._save_voice_type(c),
                    bg="#3c3836",
                ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=6)
        fr = tk.Frame(keys, bg="#111111")
        fr.pack(fill=tk.BOTH, expand=True, pady=1)
        self._mk_touch_btn(
            fr, toggle_label, self._toggle_save_voice_keys, bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=6)


    def _toggle_save_voice_keys(self) -> None:
        self._save_voice_keys_digits = not self._save_voice_keys_digits
        self._paint_save_voice_keyboard()


    def _reset_save_voice_name(self) -> None:
        if self._save_voice_entry is None:
            return
        suggested = self.engine.suggested_save_voice_name()
        self._save_voice_entry.delete(0, tk.END)
        self._save_voice_entry.insert(0, suggested)
        self._update_save_voice_status()


    def _save_voice_type(self, ch: str) -> None:
        entry = self._save_voice_entry
        if entry is None:
            return
        if ch == "\b":
            cur = entry.get()
            entry.delete(0, tk.END)
            entry.insert(0, cur[:-1])
        elif ch == "\x15":
            entry.delete(0, tk.END)
        else:
            entry.insert(tk.END, ch)
        self._update_save_voice_status()


    def _update_save_voice_status(self) -> None:
        if self._save_voice_status is None or self._save_voice_entry is None:
            return
        name = sanitize_voice_name(self._save_voice_entry.get())
        if name in BUILTIN_VOICE_NAMES:
            self._save_voice_status.configure(
                text=f"'{name}' is a built-in — pick another name", fg="#fb4934"
            )
            return
        path = self._user_waves_dir / f"{name}.wav"
        exists = name in self.engine.voice_names or path.is_file()
        tag = "overwrite" if exists else "new"
        self._save_voice_status.configure(
            text=(
                f"{tag}: {self._user_waves_dir.name}/{name}.wav "
                f"+ {name}.fx.json"
            ),
            fg="#fabd2f" if exists else "#83a598",
        )


    def _confirm_save_voice(self) -> None:
        if self._save_voice_entry is None:
            return
        raw = self._save_voice_entry.get()
        name = sanitize_voice_name(raw)
        if not name:
            if self._save_voice_status is not None:
                self._save_voice_status.configure(text="Need a name", fg="#fb4934")
            return
        if name in BUILTIN_VOICE_NAMES:
            if self._save_voice_status is not None:
                self._save_voice_status.configure(
                    text=f"Cannot replace built-in '{name}'", fg="#fb4934"
                )
            return
        try:
            key, cycle, sidecar = self.engine.save_current_voice(name)
            wav_path = self._user_waves_dir / f"{key}.wav"
            fx_path = voice_fx_sidecar_path(self._user_waves_dir, key)
            write_wavetable_wav(wav_path, cycle, sample_rate=44100)
            write_voice_fx_sidecar(fx_path, sidecar)
            self._voice_fx_sidecars[key] = dict(sidecar)
        except Exception as exc:
            if self._save_voice_status is not None:
                self._save_voice_status.configure(text=f"Save failed: {exc}", fg="#fb4934")
            self._append_log(f"SAVE VOICE failed: {exc}")
            return

        self._voice_names = self.engine.voice_names
        try:
            self._voice_index = self._voice_names.index(key)
        except ValueError:
            self._voice_index = 0
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())
        self._mark_settings_dirty()
        self._append_log(
            f"Saved voice '{key}' → {wav_path.name} + {fx_path.name} "
            f"(dly={int(sidecar.get('fx_delay_mix', 0) * 127)} "
            f"rvb={int(sidecar.get('fx_reverb_mix', 0) * 127)})"
        )
        self._close_save_voice()
        self._paint_synth_waveform(force=True)


    def _close_save_voice(self, restore_main: bool = True) -> None:
        if not self._save_voice_open:
            return
        if self._save_voice_frame is not None:
            self._save_voice_frame.destroy()
            self._save_voice_frame = None
        self._save_voice_entry = None
        self._save_voice_status = None
        self._save_voice_drive_btn = None
        self._save_voice_keys = None
        self._save_voice_open = False
        if restore_main and self._mode == "synth":
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            if self._voice_lbl is not None:
                self._voice_lbl.configure(text=self._voice_label_text())
            self.mod_var.set(self._format_mod_line())
            self._paint_synth_waveform(force=True)


    def _open_voice_grid(self) -> None:
        if self._grid_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        self._grid_open = True
        self._synth_shell.pack_forget()

        self._grid_frame = tk.Frame(self._mode_host, bg="#111111")
        self._grid_frame.pack(fill=tk.BOTH, expand=True)

        header, body, footer = self._pack_screen_regions(
            self._grid_frame, header_pady=(6, 2), body_padx=4, footer_padx=6, footer_pady=6
        )
        tk.Label(
            header,
            text="VOICES — tap · drag to scroll",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            text=f"{len(self._voice_names)} loaded",
            font=("DejaVu Sans", 12),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        _wrap, _canvas, inner, drag = self._build_touch_scroll_area(body)

        cols = 4 if len(self._voice_names) > 8 else 3
        self._grid_btns = {}
        for i, name in enumerate(self._voice_names):
            r, c = divmod(i, cols)
            btn = self._mk_scroll_select_btn(
                inner,
                name.upper(),
                lambda idx=i: self._select_voice_index(idx, close_grid=True),
                drag,
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=14)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3, ipadx=4, ipady=6)
            self._grid_btns[name] = btn
        for c in range(cols):
            inner.grid_columnconfigure(c, weight=1)

        vib = tk.Frame(footer, bg="#111111")
        vib.pack(fill=tk.X, pady=(0, 4))
        tk.Label(
            vib, text="VIB", font=("DejaVu Sans", 12, "bold"),
            fg="#a89984", bg="#111111", padx=4,
        ).pack(side=tk.LEFT)
        self._vib_toggle_btn = self._mk_touch_btn(
            vib, "WHEEL", self._toggle_vib_always, bg="#3c3836"
        )
        self._vib_toggle_btn.configure(font=("DejaVu Sans", 12, "bold"), padx=6)
        self._vib_toggle_btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=(0, 8), ipady=6)

        def _vib_btn(text: str, command) -> None:
            btn = self._mk_touch_btn(vib, text, command, bg="#504945")
            btn.configure(font=("DejaVu Sans", 12, "bold"), padx=6)
            btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=2, ipady=6)

        def _vib_value(var: tk.StringVar) -> None:
            tk.Label(
                vib, textvariable=var, font=("DejaVu Sans Mono", 12, "bold"),
                fg="#fabd2f", bg="#111111", width=8,
            ).pack(side=tk.LEFT)

        _vib_btn("DEPTH −", lambda: self._nudge_vib_depth(-VIB_DEPTH_STEP))
        _vib_value(self._vib_depth_var)
        _vib_btn("DEPTH +", lambda: self._nudge_vib_depth(VIB_DEPTH_STEP))
        _vib_btn("RATE −", lambda: self._nudge_vib_rate(-VIB_RATE_STEP))
        _vib_value(self._vib_rate_var)
        _vib_btn("RATE +", lambda: self._nudge_vib_rate(VIB_RATE_STEP))

        actions = tk.Frame(footer, bg="#111111")
        actions.pack(fill=tk.X)
        self._mk_touch_btn(
            actions, "SAVE AS…", self._open_save_voice, bg="#689d6a"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
        self._mk_touch_btn(actions, "CLOSE", self._close_voice_grid, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12
        )
        self._paint_vib_controls()
        self._paint_voice_grid()
        self._arm_overlay_guard()


    def _paint_vib_controls(self) -> None:
        depth, rate, always = self.engine.vib_state()
        self._vib_depth_var.set(f"{depth:.2f} st")
        self._vib_rate_var.set(f"{rate:.1f} Hz")
        if self._vib_toggle_btn is not None:
            on = always > 0.01
            color = "#b16286" if on else "#3c3836"
            try:
                self._vib_toggle_btn.configure(
                    text="ON" if on else "WHEEL", bg=color, activebackground=color
                )
            except Exception:
                pass


    def _toggle_vib_always(self) -> None:
        _depth, _rate, always = self.engine.vib_state()
        value = self.engine.set_vib_always(0.0 if always > 0.01 else 1.0)
        self._mark_settings_dirty()
        self._paint_vib_controls()
        self.mod_var.set(self._format_mod_line())
        self._append_log(f"Vibrato {'always on' if value > 0.01 else 'follows mod wheel'}")


    def _nudge_vib_depth(self, delta: float) -> None:
        depth = self.engine.nudge_vib_depth(delta)
        st = self.engine.modulation_state()
        # Turning depth up with the wheel down would be silent — engage it so
        # the control you just touched is the one you hear.
        if depth > 0.001 and float(st.get("mod", 0.0)) < 0.01:
            _d, _r, always = self.engine.vib_state()
            if always < 0.01:
                self.engine.set_vib_always(1.0)
                self._append_log("Vibrato ON (screen control)")
        self._mark_settings_dirty()
        self._paint_vib_controls()
        self.mod_var.set(self._format_mod_line())


    def _nudge_vib_rate(self, delta: float) -> None:
        self.engine.nudge_vib_rate(delta)
        self._mark_settings_dirty()
        self._paint_vib_controls()


    def _paint_voice_grid(self) -> None:
        if not self._grid_btns:
            return
        current = self._voice_names[self._voice_index] if self._voice_names else ""
        for name, btn in self._grid_btns.items():
            on = name == current
            color = "#458588" if on else "#3c3836"
            btn.configure(bg=color, activebackground=color)


    def _close_voice_grid(self, restore_main: bool = True) -> None:
        if not self._grid_open:
            return
        if self._grid_frame is not None:
            self._grid_frame.destroy()
            self._grid_frame = None
        self._grid_btns = {}
        self._vib_toggle_btn = None
        self._grid_open = False
        if restore_main and self._mode == "synth":
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            if self._voice_lbl is not None:
                self._voice_lbl.configure(text=self._voice_label_text())
            self.mod_var.set(self._format_mod_line())
            self._paint_synth_waveform(force=True)


    def _open_morph_menu(self) -> None:
        """Pick morph endpoints A and B; Knob 1 blends A→B."""
        if self._morph_ui_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        # Hand knobs back to morph while editing the pair
        if self.engine.drum_mode() or self.engine.fx_mode() or self.engine.bus_fx_mode():
            self.engine.set_drum_mode(False)
            self.engine.set_fx_mode(False)
            self.engine.set_bus_fx_mode(False)
            self._paint_drum_lock_btn()
            self._paint_fx_mode_btn()
            self._paint_bus_fx_mode_btn()
            self.mod_var.set(self._format_mod_line())

        self._morph_ui_open = True
        self._morph_pick_side = "a"
        # Remember the pair we came in with so CANCEL can put it back
        a_idx, b_idx = self.engine.morph_pair_indices()
        self._morph_undo = (a_idx, b_idx, self.engine.morph(), self._voice_index)
        self._synth_shell.pack_forget()

        self._morph_frame = tk.Frame(self._mode_host, bg="#111111")
        self._morph_frame.pack(fill=tk.BOTH, expand=True)

        header, body, footer = self._pack_screen_regions(
            self._morph_frame, header_pady=(6, 2), body_padx=4, footer_padx=6, footer_pady=6
        )
        title = tk.Frame(header, bg="#111111")
        title.pack(fill=tk.X)
        tk.Label(
            title,
            text="MORPH PAIR",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._morph_status_lbl = tk.Label(
            title,
            text="tap A/B · drag to scroll",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        )
        self._morph_status_lbl.pack(side=tk.RIGHT)

        # A / B selector row
        pair_row = tk.Frame(header, bg="#111111")
        pair_row.pack(fill=tk.X, pady=(4, 2))
        self._morph_side_btns = {}
        for side, label in (("a", "A"), ("b", "B")):
            btn = self._mk_touch_btn(
                pair_row,
                f"{label}: …",
                lambda s=side: self._set_morph_pick_side(s),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 14, "bold"), pady=10)
            btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
            self._morph_side_btns[side] = btn

        swap_btn = self._mk_touch_btn(pair_row, "SWAP", self._swap_morph_pair, bg="#504945")
        swap_btn.configure(font=("DejaVu Sans", 13, "bold"), pady=10)
        swap_btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=3)

        _wrap, _canvas, inner, drag = self._build_touch_scroll_area(body)

        cols = 4 if len(self._voice_names) > 8 else 3
        self._morph_grid_btns = {}
        for i, name in enumerate(self._voice_names):
            r, c = divmod(i, cols)
            btn = self._mk_scroll_select_btn(
                inner,
                name.upper(),
                lambda idx=i: self._assign_morph_endpoint(idx),
                drag,
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 12, "bold"), pady=10)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3, ipadx=2, ipady=4)
            self._morph_grid_btns[name] = btn
        for c in range(cols):
            inner.grid_columnconfigure(c, weight=1)

        self._mk_touch_btn(
            footer, "SAVE AS…", self._open_save_voice, bg="#689d6a"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=12)
        self._mk_touch_btn(footer, "DONE", self._close_morph_menu, bg="#458588").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=12
        )
        self._mk_touch_btn(footer, "CANCEL", self._cancel_morph_menu, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=12
        )
        self._paint_morph_menu()
        self._arm_overlay_guard()


    def _set_morph_pick_side(self, side: str) -> None:
        self._morph_pick_side = "b" if side == "b" else "a"
        self._paint_morph_menu()


    def _assign_morph_endpoint(self, idx: int) -> None:
        side = self._morph_pick_side
        self.engine.set_morph_endpoint(side, idx)
        self._mark_settings_dirty()
        name = self._voice_names[idx]
        # After setting A, auto-arm B so picking a pair is two taps
        if side == "a":
            self._morph_pick_side = "b"
            self._voice_index = idx
        else:
            self._morph_pick_side = "a"
        self.last_var.set(f"Morph {side.upper()} → {name.upper()}")
        self._q_put(("log", f"Morph {side.upper()} → {name}", False))
        self._paint_morph_menu()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())


    def _swap_morph_pair(self) -> None:
        a, b = self.engine.morph_pair_indices()
        blend = self.engine.morph()
        # Swap endpoints and invert blend so the sound stays put
        self.engine.set_morph_pair(b, a, morph=1.0 - blend)
        self._mark_settings_dirty()
        self._q_put(("log", "Morph pair swapped", False))
        self._paint_morph_menu()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())


    def _paint_morph_menu(self) -> None:
        if not self._morph_ui_open:
            return
        a_name, b_name, blend = self.engine.morph_neighbors()
        for side, btn in self._morph_side_btns.items():
            name = a_name if side == "a" else b_name
            armed = side == self._morph_pick_side
            label = f"{'●' if armed else '○'} {side.upper()}: {name.upper()}"
            color = "#b16286" if armed else "#3c3836"
            btn.configure(text=label, bg=color, activebackground=color)
        if self._morph_status_lbl is not None:
            self._morph_status_lbl.configure(
                text=f"Knob1 blends  {a_name} → {b_name}  ({int(blend * 100)}%)"
            )
        for name, btn in self._morph_grid_btns.items():
            if name == a_name and name == b_name:
                color = "#689d6a"
            elif name == a_name:
                color = "#458588"
            elif name == b_name:
                color = "#d3869b"
            else:
                color = "#3c3836"
            btn.configure(bg=color, activebackground=color)


    def _cancel_morph_menu(self) -> None:
        """CANCEL means the pair you walked in with, not the one you auditioned."""
        undo = getattr(self, "_morph_undo", None)
        if undo is not None:
            a_idx, b_idx, blend, voice_idx = undo
            self.engine.set_morph_pair(a_idx, b_idx, morph=blend)
            self._voice_index = voice_idx
            self._mark_settings_dirty()
            self._q_put(("log", "Morph pair restored (CANCEL)", False))
        self._close_morph_menu()


    def _close_morph_menu(self, restore_main: bool = True) -> None:
        if not self._morph_ui_open:
            return
        if self._morph_frame is not None:
            self._morph_frame.destroy()
            self._morph_frame = None
        self._morph_side_btns = {}
        self._morph_grid_btns = {}
        self._morph_status_lbl = None
        self._morph_ui_open = False
        if restore_main and self._mode == "synth":
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            if self._voice_lbl is not None:
                self._voice_lbl.configure(text=self._voice_label_text())
            self.mod_var.set(self._format_mod_line())
            self._paint_synth_waveform(force=True)


    def _prev_voice(self) -> None:
        self._select_voice_index(self._voice_index - 1)


    def _next_voice(self) -> None:
        self._select_voice_index(self._voice_index + 1)


    def _toggle_full_vel(self) -> None:
        self._full_vel = not self._full_vel
        self._paint_full_vel_btn()
        self._mark_settings_dirty()
        self._append_log(f"Full velocity → {'ON' if self._full_vel else 'OFF'}")


    def _refresh_active(self) -> None:
        if not self._active_notes:
            self.active_var.set("Active notes: —")
            return
        parts = [
            f"{midi_note_name(n)}(ch{ch + 1})"
            for (ch, n), _ in sorted(self._active_notes.items(), key=lambda x: x[0][1])
        ]
        self.active_var.set("Active notes: " + ", ".join(parts))


    def _append_log(self, line: str) -> None:
        ts = time.strftime("%H:%M:%S")
        self.log.configure(state=tk.NORMAL)
        self.log.insert(tk.END, f"{ts}  {line}\n")
        # Trim less often — Text ops are expensive on the Pi and starve touch
        if not hasattr(self, "_log_lines"):
            self._log_lines = 0
        self._log_lines += 1
        if self._log_lines > LOG_MAX + 20:
            end_line = int(float(self.log.index("end-1c").split(".")[0]))
            if end_line > LOG_MAX:
                self.log.delete("1.0", f"{end_line - LOG_MAX}.0")
            self._log_lines = LOG_MAX
        self.log.see(tk.END)
        self.log.configure(state=tk.DISABLED)


    def _clear_log(self) -> None:
        self.log.configure(state=tk.NORMAL)
        self.log.delete("1.0", tk.END)
        self.log.configure(state=tk.DISABLED)
        self._log_lines = 0


    def _panic(self) -> None:
        try:
            self._seq.stop()
        except Exception:
            pass
        try:
            self._phrases.stop_all()
        except Exception:
            pass
        try:
            self._kaoss_cancel_tick()
            self._kaoss_cancel_viz()
            self._kaoss_apply(self._kaoss.panic(), ended=True)
        except Exception:
            pass
        if self._songs.is_playing():
            self._songs.stop()
        self.engine.all_notes_off()
        self._active_notes.clear()
        self._refresh_active()
        self._refresh_seq_status()
        self._refresh_phrase_status()
        self._refresh_song_status()
        self._paint_kaoss()
        if self._mode == "kaoss":
            self._kaoss_trail.clear()
            self._kaoss_ripples.clear()
            self._kaoss_paint_leds()
            self._kaoss_arm_viz()
        self._append_log("All Notes Off")


    def _apply_display_geometry(self) -> None:
        """Fill the active X screen (TFT70 is 800×480; older default was 800×420)."""
        try:
            sw = int(self.root.winfo_screenwidth())
            sh = int(self.root.winfo_screenheight())
        except Exception:
            sw, sh = 800, 480
        if sw < 320 or sh < 240:
            sw, sh = 800, 480

        if self._fullscreen:
            # Kiosk: true fullscreen when the WM supports it; always size to screen too
            try:
                self.root.attributes("-fullscreen", True)
            except Exception:
                try:
                    self.root.attributes("-zoomed", True)
                except Exception:
                    try:
                        self.root.state("zoomed")
                    except Exception:
                        pass
            self.root.geometry(f"{sw}x{sh}+0+0")
            print(f"ui: fullscreen {sw}x{sh}", flush=True)
            return

        self.root.geometry(f"{sw}x{sh}+0+0")
        print(f"ui: geometry {sw}x{sh}", flush=True)


    def _on_pointer_activity(self, event: object = None) -> None:
        idle = getattr(self, "_idle", None)
        if idle is None:
            return
        # While blanked, ANY pointer event must wake — capacitive panels often
        # emit release-only or motion-only bursts instead of ButtonPress.
        if idle.active or self._saver_canvas is not None:
            self._hide_screensaver()
            return
        # Awake: hover/motion must not keep the panel from ever blanking.
        etype = str(getattr(event, "type", "") or "")
        if etype in ("6", "Motion"):
            return
        idle.poke()


    def _on_root_destroy(self, event: tk.Event) -> None:  # type: ignore[name-defined]
        if event.widget is not self.root:
            return
        self._stop.set()
        self._cancel_screensaver_tick()
        try:
            self._backlight.restore()
        except Exception:
            pass


    def _cancel_screensaver_tick(self) -> None:
        aid = self._saver_tick_after
        self._saver_tick_after = None
        if aid is None:
            return
        try:
            self.root.after_cancel(aid)
        except Exception:
            pass


    def _arm_screensaver_tick(self) -> None:
        self._cancel_screensaver_tick()
        if self._stop.is_set():
            return
        try:
            if not self.root.winfo_exists():
                return
            self._saver_tick_after = self.root.after(1000, self._screensaver_tick)
        except tk.TclError:
            self._saver_tick_after = None


    def _screensaver_tick(self) -> None:
        if self._stop.is_set():
            return
        try:
            if not self.root.winfo_exists():
                return
        except tk.TclError:
            return
        if self._idle.due():
            self._show_screensaver()
        elif self._idle.active:
            self._nudge_screensaver_orbit()
        else:
            self._apply_pixel_shift()
        self._arm_screensaver_tick()


    def _show_screensaver(self, *, force: bool = False) -> None:
        if self._saver_canvas is not None:
            self._nudge_screensaver_orbit()
            return
        if not force and not self._idle.due():
            return
        self._idle.activate()
        self._saver_started = time.monotonic()
        canvas = tk.Canvas(
            self.root,
            bg="#000000",
            highlightthickness=0,
            bd=0,
            cursor="none",
        )
        canvas.place(x=0, y=0, relwidth=1, relheight=1)
        try:
            canvas.lift()
            canvas.focus_set()
        except Exception:
            pass
        # Local grab only. grab_set_global() can swallow capacitive events so
        # tap-to-wake never fires while the backlight is already at 0.
        try:
            canvas.grab_set()
        except Exception:
            pass
        # Capacitive panels sometimes emit release-only or motion-only bursts.
        canvas.bind("<ButtonPress>", self._on_screensaver_tap)
        canvas.bind("<ButtonRelease>", self._on_screensaver_tap)
        canvas.bind("<Motion>", self._on_screensaver_tap)
        canvas.bind("<B1-Motion>", self._on_screensaver_tap)
        self._saver_canvas = canvas
        self._saver_hint = canvas.create_text(
            40,
            40,
            text="tap to wake",
            fill="#4a4a4a",
            font=("DejaVu Sans", 18, "bold"),
            anchor="nw",
        )
        self._saver_clock = canvas.create_text(
            40,
            72,
            text=time.strftime("%H:%M"),
            fill="#3a3a3a",
            font=("DejaVu Sans", 14),
            anchor="nw",
        )
        self._nudge_screensaver_orbit()
        self._backlight.dim()
        print("ui: screensaver on", flush=True)


    def _nudge_screensaver_orbit(self) -> None:
        canvas = self._saver_canvas
        if canvas is None or self._saver_hint is None:
            return
        try:
            w = int(canvas.winfo_width())
            h = int(canvas.winfo_height())
        except Exception:
            w, h = 800, 480
        if w <= 1 or h <= 1:
            w, h = 800, 480
        elapsed = time.monotonic() - self._saver_started
        x, y = orbit_xy(elapsed, w, h, 220, 56)
        canvas.coords(self._saver_hint, x, y)
        if self._saver_clock is not None:
            canvas.itemconfigure(self._saver_clock, text=time.strftime("%H:%M"))
            canvas.coords(self._saver_clock, x, y + 28)


    def _apply_pixel_shift(self) -> None:
        """Nudge chrome a couple of pixels so bold boxes don't sit still."""
        if getattr(self, "_idle", None) is not None and self._idle.active:
            return
        elapsed = time.monotonic() - getattr(self, "_shift_started", time.monotonic())
        dx, dy = pixel_shift_xy(elapsed)
        if (dx, dy) == getattr(self, "_shift_xy", (None, None)):
            return
        self._shift_xy = (dx, dy)
        gutter = PIXEL_SHIFT_AMPLITUDE
        try:
            self._nav.pack_configure(
                padx=(gutter + dx, gutter - dx),
                pady=(gutter + dy, 0),
            )
            self._mode_host.pack_configure(
                padx=(gutter + dx, gutter - dx),
                pady=(0, gutter - dy),
            )
        except Exception:
            pass


    def _on_screensaver_tap(self, _event: object = None) -> str:
        self._hide_screensaver()
        return "break"


    def _hide_screensaver(self) -> None:
        idle = getattr(self, "_idle", None)
        if idle is not None:
            idle.poke()
        canvas = self._saver_canvas
        self._saver_canvas = None
        self._saver_hint = None
        self._saver_clock = None
        if canvas is not None:
            try:
                canvas.grab_release()
            except Exception:
                pass
            try:
                canvas.destroy()
            except Exception:
                pass
            print("ui: screensaver off", flush=True)
        restored = self._backlight.restore()
        if not restored:
            # Last-resort unblank if PanelBacklight had no saved state
            try:
                path = pathlib.Path("/sys/class/backlight/10-0045/brightness")
                max_path = pathlib.Path("/sys/class/backlight/10-0045/max_brightness")
                value = "255"
                if max_path.is_file():
                    value = max_path.read_text(encoding="ascii").strip() or "255"
                if path.is_file():
                    path.write_text(f"{value}\n", encoding="ascii")
                power = pathlib.Path("/sys/class/backlight/10-0045/bl_power")
                if power.is_file():
                    power.write_text("0\n", encoding="ascii")
            except OSError:
                pass
        self._apply_pixel_shift()


    def _blank_screen_now(self) -> None:
        self._close_power_menu(restore_main=True)
        self._show_screensaver(force=True)


    def _cycle_screensaver_timeout(self) -> None:
        self._idle.timeout_sec = next_timeout_preset(self._idle.timeout_sec)
        self._idle.poke()
        self._mark_settings_dirty()
        if self._saver_timeout_btn is not None:
            self._saver_timeout_btn.configure(text=timeout_label(self._idle.timeout_sec))
        self._append_log(f"TFT burn-in guard → {timeout_label(self._idle.timeout_sec)}")


    def _open_power_menu(self) -> None:
        """Confirm screen for safe Pi shutdown / reboot (kiosk has no desktop power UI)."""
        if self._power_ui_open:
            return
        # Close other overlays so POWER is always reachable
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._save_preset_open:
            self._close_save_preset(restore_main=False)
        if self._token_ui_open:
            self._close_update_token(restore_main=False)
        if self._kaoss_picker_open:
            self._close_kaoss_picker(restore_main=False)
        if self._kaoss_settings_open:
            self._close_kaoss_settings(restore_main=False)
        if self._kaoss_play:
            self._kaoss_leave_play()

        self._power_ui_open = True
        prev = self._mode
        for shell in (
            self._synth_shell,
            self._seq_shell,
            self._pads_shell,
            self._kaoss_shell,
            self._songs_shell,
            self._presets_shell,
            self._log_shell,
            self._settings_shell,
            self._home_shell,
        ):
            try:
                shell.pack_forget()
            except Exception:
                pass

        self._power_frame = tk.Frame(self._mode_host, bg="#111111")
        self._power_frame.pack(fill=tk.BOTH, expand=True)
        self._power_frame._prev_mode = prev  # type: ignore[attr-defined]

        header, body, footer = self._pack_screen_regions(
            self._power_frame,
            header_padx=10,
            header_pady=(16, 8),
            body_padx=10,
            body_pady=6,
            footer_padx=10,
            footer_pady=12,
        )
        tk.Label(
            header,
            text="POWER",
            font=("DejaVu Sans", 22, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._saver_timeout_btn = self._mk_touch_btn(
            header,
            timeout_label(self._idle.timeout_sec),
            self._cycle_screensaver_timeout,
            bg="#3c3836",
        )
        self._saver_timeout_btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
        self._saver_timeout_btn.pack(side=tk.RIGHT)

        footer.columnconfigure(0, weight=1)
        footer.columnconfigure(1, weight=1)
        self._mk_touch_btn(
            footer, "CANCEL", self._close_power_menu, bg="#504945"
        ).grid(row=0, column=0, sticky="ew", padx=(0, 4), ipady=18)
        self._mk_touch_btn(
            footer, "SCREEN OFF", self._blank_screen_now, bg="#1d2021"
        ).grid(row=0, column=1, sticky="ew", padx=(4, 0), ipady=18)

        tk.Label(
            body,
            text="Shut down cleanly before unplugging. Reboot restarts into kiosk. "
            "SCREEN OFF blanks the TFT (tap to wake; playing MIDI will not). "
            "While the UI is up it also pixel-shifts so bold chrome cannot ghost.",
            font=("DejaVu Sans", 13),
            fg="#ebdbb2",
            bg="#111111",
            wraplength=740,
            justify=tk.LEFT,
            anchor="w",
        ).pack(side=tk.TOP, fill=tk.X, padx=2, pady=(4, 16))

        # Equal-height actions — pack(expand) alone gives the first button the leftover
        actions = tk.Frame(body, bg="#111111")
        actions.pack(fill=tk.BOTH, expand=True)
        actions.rowconfigure(0, weight=1, uniform="power")
        actions.rowconfigure(1, weight=1, uniform="power")
        actions.columnconfigure(0, weight=1)
        shut = self._mk_touch_btn(
            actions, "SHUT DOWN", lambda: self._pi_power("poweroff"), bg="#9d0006"
        )
        shut.configure(font=("DejaVu Sans", 18, "bold"))
        shut.grid(row=0, column=0, sticky="nsew", pady=(0, 6), ipady=12)
        reboot = self._mk_touch_btn(
            actions, "REBOOT", lambda: self._pi_power("reboot"), bg="#d79921"
        )
        reboot.configure(font=("DejaVu Sans", 18, "bold"))
        reboot.grid(row=1, column=0, sticky="nsew", pady=(6, 0), ipady=12)


    def _close_power_menu(self, restore_main: bool = True) -> None:
        if not self._power_ui_open:
            return
        prev = "synth"
        if self._power_frame is not None:
            prev = getattr(self._power_frame, "_prev_mode", "synth")
            self._power_frame.destroy()
            self._power_frame = None
        self._power_ui_open = False
        if restore_main:
            self._switch_mode(prev if prev in UI_MODES else "synth")


    def _pi_power(self, action: str) -> None:
        """Reboot/poweroff via pi-power.sh / systemctl — never just quit the app."""
        action = "reboot" if action == "reboot" else "poweroff"
        self._append_log(f"Power → {action}…")
        self.last_var.set(f"Powering {action}…")
        try:
            self._panic()
        except Exception:
            pass
        try:
            self._save_settings_file(SETTINGS_PATH, quiet=True)
        except Exception:
            pass
        # Give the UI a moment to flush logs / audio stop
        self.root.update_idletasks()

        power_sh = str(
            pathlib.Path(__file__).resolve().parents[2] / "scripts" / "session" / "pi-power.sh"
        )

        def _run() -> None:
            # Only commands covered by /etc/sudoers.d/midi-tone-power (plain
            # poweroff/reboot — flag variants need a password and must not be
            # used here). Never treat app exit as shutdown.
            cmds = [
                ["sudo", "-n", power_sh, action],
                ["sudo", "-n", "systemctl", action],
                (
                    ["sudo", "-n", "poweroff"]
                    if action == "poweroff"
                    else ["sudo", "-n", "reboot"]
                ),
            ]
            last_err = ""
            for cmd in cmds:
                try:
                    r = subprocess.run(
                        cmd,
                        capture_output=True,
                        text=True,
                        timeout=25,
                        check=False,
                    )
                    if r.returncode == 0:
                        return
                    last_err = (r.stderr or r.stdout or f"exit {r.returncode}").strip()
                except Exception as exc:
                    last_err = str(exc)
            self._q_put(
                (
                    "log",
                    f"Power {action} failed: {last_err or 'no permission'} — "
                    f"run ./install-kiosk.sh (adds sudoers) or: sudo systemctl {action}",
                    False,
                )
            )

        threading.Thread(target=_run, daemon=True).start()
        # Also show immediate feedback on the confirm screen
        if self._power_frame is not None:
            tk.Label(
                self._power_frame,
                text=f"Sending {action}… screen will go dark.",
                font=("DejaVu Sans", 14, "bold"),
                fg="#fabd2f",
                bg="#111111",
            ).pack(fill=tk.X, padx=12, pady=8)


    def _on_close(self) -> None:
        self._stop.set()
        self._cancel_screensaver_tick()
        try:
            self._hide_screensaver()
        except Exception:
            pass
        try:
            self._seq.stop()
        except Exception:
            pass
        try:
            self._phrases.stop_all()
        except Exception:
            pass
        try:
            self._kaoss_cancel_tick()
            self._kaoss_cancel_viz()
            self._kaoss_apply(self._kaoss.panic(), ended=True)
        except Exception:
            pass
        try:
            self._songs.stop()
            self._songs.close_outport()
        except Exception:
            pass
        try:
            self._save_settings_file(SETTINGS_PATH, quiet=False)
        except Exception:
            pass
        if self._inport is not None:
            try:
                self._inport.close()
            except Exception:
                pass
        if self._jambox is not None:
            try:
                self._jambox.close()
            except Exception:
                pass
            self._jambox = None
        proc = self._jambox_proc
        if proc is not None:
            try:
                proc.terminate()
            except Exception:
                pass
            self._jambox_proc = None
        self.engine.stop()
        self.root.destroy()


    def run(self) -> None:
        self.root.mainloop()
