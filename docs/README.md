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

Publishing to raygarrison.us is a `repository_dispatch` (`pidi-docs-updated`) from `.github/workflows/deploy-raygarrison-site.yml`. That ping needs the repo secret `RAYGARRISON_SITE_DEPLOY_TOKEN` (PAT with access to `HyruleanHero1988/raygarrison-us-site`). Without it the job skips so master stays green; run **Deploy static site to GitHub Pages** on that site repo by hand to pick up `apps/pidi/docs` from master.
