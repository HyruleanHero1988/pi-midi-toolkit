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


@dataclass
class WifiNetwork:
    ssid: str
    signal: int = 0
    security: str = ""
    in_use: bool = False

    @property
    def is_open(self) -> bool:
        sec = (self.security or "").strip().upper()
        return sec in ("", "--", "NONE")

    def label(self) -> str:
        mark = "* " if self.in_use else ""
        bits = [f"{mark}{self.ssid}"]
        if self.signal:
            bits.append(f"{self.signal}%")
        bits.append("open" if self.is_open else "secured")
        return " · ".join(bits)


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


def _split_nmcli_line(line: str, expected: int) -> List[str]:
    """Split an nmcli ``-t`` line, honoring backslash escapes."""
    parts: List[str] = []
    buf: List[str] = []
    i = 0
    while i < len(line):
        ch = line[i]
        if ch == "\\" and i + 1 < len(line):
            buf.append(line[i + 1])
            i += 2
            continue
        if ch == ":" and len(parts) < expected - 1:
            parts.append("".join(buf))
            buf = []
            i += 1
            continue
        buf.append(ch)
        i += 1
    parts.append("".join(buf))
    while len(parts) < expected:
        parts.append("")
    return parts[:expected]


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


def save_wifi_credentials(
    install: pathlib.Path,
    ssid: str,
    password: str,
    *,
    iface: str = DEFAULT_IFACE,
) -> None:
    """Persist join credentials for later REJOIN (gitignored file)."""
    path = install / WIFI_CREDS_NAME
    lines = [
        f"WIFI_SSID={ssid}",
        f"WIFI_PASSWORD={password}",
        f"WIFI_IFACE={iface or DEFAULT_IFACE}",
        "",
    ]
    path.write_text("\n".join(lines), encoding="utf-8")
    try:
        path.chmod(0o600)
    except OSError:
        pass


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


def parse_wifi_list(output: str) -> List[WifiNetwork]:
    """Parse ``nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list``."""
    found: Dict[str, WifiNetwork] = {}
    for raw in output.splitlines():
        line = raw.strip()
        if not line:
            continue
        in_use, ssid, signal, security = _split_nmcli_line(line, 4)
        ssid = ssid.strip()
        if not ssid or ssid == "--":
            continue
        try:
            sig = int((signal or "0").strip() or "0")
        except ValueError:
            sig = 0
        active = in_use.strip().lower() in ("yes", "*", "1", "true")
        prev = found.get(ssid)
        if prev is None or sig > prev.signal or (active and not prev.in_use):
            found[ssid] = WifiNetwork(
                ssid=ssid,
                signal=sig,
                security=(security or "").strip(),
                in_use=active or (prev.in_use if prev else False),
            )
    networks = list(found.values())
    networks.sort(key=lambda n: (-int(n.in_use), -n.signal, n.ssid.lower()))
    return networks


def scan_wifi_networks(
    *,
    rescan: bool = True,
    iface: str = DEFAULT_IFACE,
) -> Tuple[List[WifiNetwork], str]:
    """Scan for broadcast SSIDs. Returns ``(networks, error)`` — error empty on success."""
    if not _which_nmcli():
        return [], "nmcli not installed on this box"

    _nmcli(["radio", "wifi", "on"], sudo=True, timeout=8.0)
    _nmcli(["radio", "wifi", "on"], sudo=False, timeout=8.0)

    if rescan:
        code, _out, err = _nmcli(
            ["device", "wifi", "rescan", "ifname", iface],
            sudo=True,
            timeout=20.0,
        )
        if code != 0:
            _nmcli(["device", "wifi", "rescan"], sudo=False, timeout=20.0)

    code, out, err = _nmcli(
        ["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list"],
        timeout=15.0,
    )
    if code != 0:
        return [], err or out or "Wi-Fi scan failed"
    networks = parse_wifi_list(out)
    if not networks:
        return [], "No networks found — move closer or tap SCAN again"
    return networks, ""


def connect_wifi(
    ssid: str,
    password: str = "",
    *,
    iface: str = DEFAULT_IFACE,
    install: Optional[pathlib.Path] = None,
    remember: bool = True,
) -> Tuple[bool, str]:
    """Join a broadcast SSID (touch UI / on-screen keyboard path)."""
    ssid = (ssid or "").strip()
    if not ssid:
        return False, "No network selected"
    if not _which_nmcli():
        return False, "nmcli not installed on this box"

    _nmcli(["radio", "wifi", "on"], sudo=True, timeout=8.0)
    args = [
        "device",
        "wifi",
        "connect",
        ssid,
        "ifname",
        iface or DEFAULT_IFACE,
    ]
    if password:
        args.extend(["password", password])

    code, out, err = _nmcli(args, sudo=True, timeout=60.0)
    if code != 0:
        code, out, err = _nmcli(args, sudo=False, timeout=60.0)
    if code != 0:
        return False, err or out or f"failed to join {ssid}"

    if remember and install is not None:
        try:
            save_wifi_credentials(install, ssid, password, iface=iface)
        except OSError:
            pass

    st = wifi_status(iface)
    if st.connected:
        return True, st.detail or f"joined {ssid}"
    return True, f"joined {ssid}" + (f" — {out}" if out else "")


def ensure_wifi_up(install: pathlib.Path = HERE) -> Tuple[bool, str]:
    """Turn the radio on and bring up Wi-Fi (existing profile or .wifi-credentials)."""
    if not _which_nmcli():
        return False, "nmcli not installed on this box"

    notes: List[str] = []
    code, out, err = _nmcli(["radio", "wifi", "on"], sudo=True)
    if code != 0:
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
        ok, detail = connect_wifi(
            ssid, password, iface=iface, install=install, remember=False
        )
        if ok:
            return True, detail
        return False, detail

    code, out, err = _nmcli(
        ["-t", "-f", "NAME,TYPE", "connection", "show"],
        sudo=False,
    )
    wifi_names: List[str] = []
    if code == 0:
        for line in out.splitlines():
            if ":802-11-wireless" in line or line.endswith(":wifi"):
                wifi_names.append(line.split(":", 1)[0])
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
                "Pick a network on the Wi-Fi screen, or add .wifi-credentials "
                "with WIFI_SSID=… and WIFI_PASSWORD=…"
            )
            return False, (err or out or "no Wi-Fi profile to bring up") + f" — {hint}"
        notes.append(f"connect {iface}")

    st = wifi_status(iface)
    if st.connected:
        return True, st.detail or "; ".join(notes)
    return False, st.detail or (" · ".join(notes) + " — still not connected")
