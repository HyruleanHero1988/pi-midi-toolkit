# PiDI documentation

Native SDL/KMSDRM kiosk (`pidi-native`), 800×480.

- **[Screen reference](index.html)** — labeled captures of every mode and overlay (HOME, SYNTH, SEQ, PADS, KAOSS, CHORDS, …).
- **[NATIVE_KIOSK.md](../NATIVE_KIOSK.md)** — architecture, run, and Pi appliance.
- **[README.md](../README.md)** — repo overview, engines, deploy.

Recapture after UI changes:

```bash
./scripts/capture-pidi-docs.sh
```

That writes `docs/screens/*.png` from the dummy renderer and copies the same tree to `apps/pidi/docs/` for [raygarrison.us](https://raygarrison.us).
