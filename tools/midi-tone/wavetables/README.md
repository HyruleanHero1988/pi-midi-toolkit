# Wavetables for midi-tone

Bundled single-cycle WAVs (2048 samples, mono 16-bit, 44.1 kHz), loaded at startup.

## Built-in procedural voices

`sine`, `square`, `saw`, `triangle` are generated in code (no files).

## Adventure Kid Waveforms (AKWF)

Most `.wav` files here are resampled from [AKWF-FREE](https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE)
by Kristoffer Karl Axel Ekstrand — **CC0 1.0** (public domain). See `AKWF-LICENSE.md`.

Drop any extra mono WAV single-cycle into this folder (any length); midi-tone resamples it
to 2048 points on load. Name the file `myvoice.wav` → it appears as voice `myvoice`.

To pull more from AKWF:

```bash
python3 fetch_akwf.py --list
python3 fetch_akwf.py epiano organ flute
```
