# PiDI architecture

PiDI is the **kiosk UI** for the Raspberry Pi MIDI box. Audio is owned by
`jambox-engine` (Rust) when available; this tree is the Tk surface plus a
Python PortAudio fallback.

## Layout

```
apps/pidi/                 deploy root (also copied to ~/midi-tone on the Pi)
  midi_tone.py             thin shim — prefer `python -m pidi`
  kiosk.sh / run.sh / …    thin wrappers → bin/ or scripts/install/
  bin/                     real session entrypoints
  scripts/
    install/               install-kiosk, disable-kiosk, setup-venv, splash
    session/               TFT prefer, cursor hide, audio, pi-power
    hw/                    one-off touch / DSI bring-up
    deploy/                deploy_pi.py
  tests/                   unit + UI tests
  pidi/                    Python package
    main.py                CLI
    constants.py
    jambox_client.py
    kaoss.py / sequencer.py / screensaver.py / updater.py
    audio/                 Python soft-synth fallback
    domain/                phrases, songs
    ui/
      app.py               shell + mode chrome
      midi_io.py           MIDI ports / CC / queue drain
      session_io.py        settings / session JSON
      chrome.py / scope.py
      screens/             one mixin per mode
  docs/                    screen reference (bundled to raygarrison.us)
  archive/debug/           obsolete bring-up helpers
```

## Run

```bash
cd apps/pidi   # or ~/midi-tone on the Pi
./run.sh --input MPK
# or:
export PYTHONPATH="$PWD"
python -m pidi --fullscreen
```
