# PiDI architecture

PiDI is the **archived Tk kiosk UI**. Audio is owned by `jambox-engine`; the
live surface is `crates/pidi-native`. This tree is the Tk surface plus a
Python PortAudio fallback.

## Layout

```
apps/pidi/                 deploy root (also copied to ~/midi-tone on the Pi)
  VERSION                  product semver (Settings display; bump for releases)
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
    constants.py           APP_VERSION from VERSION; modes; paths
    jambox_client.py
    kaoss.py / sequencer.py / screensaver.py / updater.py
    audio/                 Python soft-synth fallback
    domain/                phrases, songs
    ui/
      app.py               shell + mode chrome
      midi_io.py           MIDI ports / CC / queue drain
      session_io.py        settings / session JSON
      chrome.py / scope.py
      screens/             home 4×2; settings hub (Update/Wi‑Fi nested); …
  docs/                    copy of the native screen reference (bundled to raygarrison.us)
                           canonical source: repo-root docs/
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
