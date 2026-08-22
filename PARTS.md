# Parts list

Living inventory for the Pi MIDI box. Update **Paid** and **Status** as things land. Currency is USD. Parked items are chosen but not buying yet.

**Status:** `have` · `ordered` · `selected` · `need` · `parked`

| Part | Qty | Status | Paid | Link | Notes |
|------|-----|--------|------|------|-------|
| Raspberry Pi 2 Model B v1.1 | 1 | have | — | | Compute today. Upgrade path Pi 4/5 if DSI/audio hits a wall. |
| microSD (Raspberry Pi OS) | 1 | have | — | | Stock image + `deploy/setup-pi.sh`. |
| 5 V wall wart (Pi micro-USB) | 1 | have | — | | Desk power until the UPS. |
| Ethernet cable | 1 | have | — | | Pi 2 has no Wi-Fi. SSH / SET updates. |
| BigTreeTech Pi TFT70 V2.1 | 1 | ordered | — | [kb-3d](https://kb-3d.com/store/controllers-displays-drivers/2677-bigtreetech-pi-tft43-tft50-tft70-v21-touchscreen-panel-for-raspberry-pi-pi-2-1734017888380.html) | 7″ 800×480 DSI, GT911. 165×100 mm. Short DSI cable in the box — fold 180°, do not twist. Replace link/paid with the order you actually used. |
| Case | 1 | selected | — | — | Add store link + paid price. Must leave USB, Ethernet, HDMI, 3.5 mm, and (later) X728 USB-C reachable. |
| Akai MPK mini mk3 | 1 | have | — | | USB MIDI in. Pads = drums / phrase launch. |
| USB-MIDI-to-DIN adapter | 1 | need | — | — | Out to hardware synth when that path exists. Class-compliant; name substring e.g. `U2MIDI`. |
| Headphones or powered speakers | 1 | have | — | — | Pi 3.5 mm jack. |
| Powered USB hub | 1 | need | — | — | Only if MPK + DIN brown out the Pi 2 USB. |

**Spent so far:** add screen (+ case) when priced. Running total of filled **Paid** cells: —

## Parked — battery UPS

See [PLAN.md](PLAN.md#portable-battery-ups-parked). Hang the X728 off the Pi GPIO with a G341 so it is not stacked on the TFT70. Charge the X728, not the Pi micro-USB. Button → `pi-power.sh`.

| Part | Qty | Status | Paid | Link | Notes |
|------|-----|--------|------|------|-------|
| Geekworm X728 v2.5 + 5 V / 4 A USB-C brick | 1 | parked | | [Amazon combo](https://www.amazon.com/Geekworm-Raspberry-Adapter-Management-Compatible/dp/B09KMX7Z2P) | Easiest cart. Board-only: [Amazon](https://www.amazon.com/Geekworm-Raspberry-Management-Detection-Shutdown/dp/B087FXLZZH) · [Geekworm](https://geekworm.com/products/x728). PSU alone: [Amazon](https://www.amazon.com/Geekworm-Raspberry-Adapter-Charger-Support/dp/B09J856PND). |
| Geekworm G341 90° GPIO adapter | 1 | parked | | [Geekworm](https://geekworm.com/products/gpio-1-to-2-extender) · [Amazon UK](https://www.amazon.co.uk/dp/B0BD79QW8K) | Puts the X728 beside the Pi in-plane. |
| Samsung 35E 18650 (unprotected flat-top) | 2 | parked | | [18650BatteryStore](https://www.18650batterystore.com/products/samsung-35e-18650-3500mah-8a-battery) · [Orbtronic](https://www.orbtronic.com/samsung-35e-18650-battery-inr1865035e-flat-top) | Same brand/age. No protection PCB. ~65 mm. |

## How to update

1. Change **Status** when it moves (`selected` → `ordered` → `have`).
2. Put the amount actually paid in **Paid** (include tax/shipping if you care about true cost).
3. Point **Link** at the order page you used, not a generic search.
4. Recount **Spent so far**.
