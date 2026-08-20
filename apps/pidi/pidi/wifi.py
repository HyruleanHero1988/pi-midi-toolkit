"""Wi-Fi helpers for the PiDI SET screen (NetworkManager / nmcli)."""
from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
from dataclasses import dataclass
from typing import Dict, List, Optional, Tuple

from pidi.constants import HERE

WIFI_CREDS_NAME = ".wifi-credentials"
DEFAULT_IFACE = "wlan0"


@dataclass
class WifiStatus:
    radio: str = "unknown"  # enabled | disabled | unknown
    device: str = ""
    state: str = "unknown"  # connected | disconnected | unavailable | …
    connection: str = ""
    ssid: str = ""
    ipv4: str = ""
    online: bool = False
    detail: str = ""

    @property
    def connected(self) -> bool:
        return self.state == "connected"


def _which_nmcli() -> Optional[str]:
    return shutil.which("nmcli")


def _run(
    args: List[str],
    *,
    timeout: float = 25.0,
    sudo: bool = False,
) -> Tuple[int, str, str]:
    cmd = list(args)
    if sudo:
        cmd = ["sudo", "-n", *cmd]
    try:
        proc = subprocess.run(
            cmd,
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except FileNotFoundError:
        return 127, "", "command not found"
    except subprocess.TimeoutExpired:
        return 124, "", "timed out"
    return (
        int(proc.returncode),
        (proc.stdout or "").strip(),
        (proc.stderr or "").strip(),
    )


def _nmcli(args: List[str], *, sudo: bool = False, timeout: float = 25.0) -> Tuple[int, str, str]:
    nmcli = _which_nmcli()
    if not nmcli:
        return 127, "", "nmcli not installed"
    return _run([nmcli, *args], timeout=timeout, sudo=sudo)


def _parse_kv_blob(text: str) -> Dict[str, str]:
    out: Dict[str, str] = {}
    for line in text.splitlines():
        if ":" not in line:
            continue
        k, v = line.split(":", 1)
        out[k.strip()] = v.strip()
    return out


def load_wifi_credentials(install: pathlib.Path = HERE) -> Dict[str, str]:
    """Read WIFI_SSID / WIFI_PASSWORD from env or gitignored `.wifi-credentials`."""
    out: Dict[str, str] = {}
    path = install / WIFI_CREDS_NAME
    if path.is_file():
        try:
            for line in path.read_text(encoding="utf-8").splitlines():
                line = line.strip()
                if not line or line.startswith("#") or "=" not in line:
                    continue
                k, v = line.split("=", 1)
                out[k.strip()] = v.strip()
        except OSError:
            pass
    for key in ("WIFI_SSID", "WIFI_PASSWORD", "WIFI_IFACE"):
        env = os.environ.get(key, "").strip()
        if env:
            out[key] = env
    return out


def wifi_status(iface: str = DEFAULT_IFACE, *, quick: bool = False) -> WifiStatus:
    st = WifiStatus(device=iface)
    if not _which_nmcli():
        st.detail = "nmcli missing — install NetworkManager"
        return st

    code, out, err = _nmcli(["-t", "-f", "WIFI", "radio"], timeout=3.0)
    if code == 0 and out:
        st.radio = out.splitlines()[0].strip().lower() or "unknown"
    else:
        st.detail = err or out or "radio query failed"

    code, out, _err = _nmcli(
        ["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"],
        timeout=3.0,
    )
    if code == 0:
        for line in out.splitlines():
            parts = line.split(":")
            if len(parts) < 4:
                continue
            dev, typ, state, conn = parts[0], parts[1], parts[2], parts[3]
            if dev == iface or (typ == "wifi" and not st.state.startswith("connected")):
                if typ != "wifi" and dev != iface:
                    continue
                st.device = dev
                st.state = state.split(" ")[0].lower()
                st.connection = conn
                if typ == "wifi" and dev == iface:
                    break

    code, out, _err = _nmcli(
        [
            "-t",
            "-f",
            "GENERAL.STATE,GENERAL.CONNECTION,IP4.ADDRESS,GENERAL.CONNECTION",
            "device",
            "show",
            st.device or iface,
        ],
        timeout=3.0,
    )
    if code == 0:
        kv = _parse_kv_blob(out)
        state = kv.get("GENERAL.STATE", "")
        if state:
            # e.g. "100 (connected)"
            st.state = "connected" if "connected" in state.lower() else st.state
        st.connection = kv.get("GENERAL.CONNECTION", st.connection) or st.connection
        for key, val in kv.items():
            if key.startswith("IP4.ADDRESS") and val:
                st.ipv4 = val.split("/")[0].strip()
                break

    if st.connection and st.connection not in ("", "--"):
        code, out, _err = _nmcli(
            ["-t", "-f", "802-11-wireless.ssid", "connection", "show", st.connection],
            timeout=3.0,
        )
        if code == 0 and ":" in out:
            st.ssid = out.split(":", 1)[1].strip()

    if not quick:
        # Prefer active scan row when connected (can be slow on some Pi Wi-Fi stacks).
        code, out, _err = _nmcli(
            ["-t", "-f", "ACTIVE,SSID", "device", "wifi", "list"],
            timeout=8.0,
        )
        if code == 0:
            for line in out.splitlines():
                if line.startswith("yes:"):
                    ssid = line.split(":", 1)[1].strip()
                    if ssid:
                        st.ssid = ssid
                    break

        code, _out, _err = _run(
            ["ping", "-c1", "-W2", "1.1.1.1"],
            timeout=5.0,
            sudo=False,
        )
        st.online = code == 0
    if st.connected and st.ssid:
        detail = f"{st.ssid} · {st.ipv4}" if st.ipv4 else st.ssid
        if not quick:
            detail += " · online" if st.online else " · no internet"
        st.detail = detail
    elif st.connected:
        st.detail = st.ipv4 or "connected"
    elif st.radio == "disabled":
        st.detail = "radio off"
    elif st.state and st.state != "unknown":
        st.detail = st.state
    return st


def format_wifi_line(status: Optional[WifiStatus] = None, *, quick: bool = True) -> str:
    st = status or wifi_status(quick=quick)
    return f"Wi-Fi: {st.detail}"


def ensure_wifi_up(install: pathlib.Path = HERE) -> Tuple[bool, str]:
    """Turn the radio on and bring up Wi-Fi (existing profile or .wifi-credentials)."""
    if not _which_nmcli():
        return False, "nmcli not installed on this box"

    notes: List[str] = []
    code, out, err = _nmcli(["radio", "wifi", "on"], sudo=True)
    if code != 0:
        # Retry without sudo — some images allow netdev group
        code2, out2, err2 = _nmcli(["radio", "wifi", "on"], sudo=False)
        if code2 != 0:
            return False, err or err2 or out or out2 or "could not enable Wi-Fi radio"
        notes.append("radio on")
    else:
        notes.append("radio on")

    creds = load_wifi_credentials(install)
    ssid = (creds.get("WIFI_SSID") or "").strip()
    password = creds.get("WIFI_PASSWORD") or ""
    iface = (creds.get("WIFI_IFACE") or DEFAULT_IFACE).strip() or DEFAULT_IFACE

    if ssid and password:
        args = [
            "device",
            "wifi",
            "connect",
            ssid,
            "password",
            password,
            "ifname",
            iface,
        ]
        code, out, err = _nmcli(args, sudo=True, timeout=60.0)
        if code != 0:
            code, out, err = _nmcli(args, sudo=False, timeout=60.0)
        if code != 0:
            return False, err or out or f"failed to join {ssid}"
        notes.append(f"joined {ssid}")
    else:
        # Prefer an existing Wi-Fi connection profile (Imager "preconfigured", etc.)
        code, out, err = _nmcli(
            ["-t", "-f", "NAME,TYPE", "connection", "show"],
            sudo=False,
        )
        wifi_names: List[str] = []
        if code == 0:
            for line in out.splitlines():
                if ":802-11-wireless" in line or line.endswith(":wifi"):
                    wifi_names.append(line.split(":", 1)[0])
        # Prefer "preconfigured", then first wifi profile
        prefer = [n for n in wifi_names if n == "preconfigured"] + [
            n for n in wifi_names if n != "preconfigured"
        ]
        brought = False
        for name in prefer:
            code, out, err = _nmcli(["connection", "up", name], sudo=True, timeout=45.0)
            if code != 0:
                code, out, err = _nmcli(
                    ["connection", "up", name], sudo=False, timeout=45.0
                )
            if code == 0:
                notes.append(f"up {name}")
                brought = True
                break
        if not brought:
            code, out, err = _nmcli(
                ["device", "connect", iface], sudo=True, timeout=45.0
            )
            if code != 0:
                code, out, err = _nmcli(
                    ["device", "connect", iface], sudo=False, timeout=45.0
                )
            if code != 0:
                hint = (
                    "Add .wifi-credentials with WIFI_SSID=… and WIFI_PASSWORD=…"
                )
                return False, (err or out or "no Wi-Fi profile to bring up") + f" — {hint}"
            notes.append(f"connect {iface}")

    st = wifi_status(iface)
    if st.connected:
        return True, st.detail or "; ".join(notes)
    return False, st.detail or (" · ".join(notes) + " — still not connected")
