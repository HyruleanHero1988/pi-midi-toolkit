#!/usr/bin/env python3
"""Talk to the lab Pi over SSH (no one-off tmp_*.py scripts).

Uses ``apps/pidi/.pi-credentials`` (gitignored), same as deploy_master.py.

Examples (from repo root, PowerShell-safe)::

    python deploy/pi_talk.py status
    python deploy/pi_talk.py audio
    python deploy/pi_talk.py logs -n 80
    python deploy/pi_talk.py restart
    python deploy/pi_talk.py run systemctl is-active jambox-engine pidi-native
    python deploy/pi_talk.py sudo systemctl restart jambox-engine
    python deploy/pi_talk.py put dist/armv7/pidi-native /tmp/pidi-native
    python deploy/pi_talk.py get /home/ray/.local/share/pidi/version.json -
    python deploy/pi_talk.py bash path/to/remote.sh
    python deploy/pi_talk.py py path/to/remote.py
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import time

try:
    import paramiko
except ImportError:
    sys.exit("pip install paramiko")

ROOT = pathlib.Path(__file__).resolve().parents[1]
CREDS_PATH = ROOT / "apps" / "pidi" / ".pi-credentials"
REMOTE_REPO = "/home/ray/pi-midi-toolkit"
REMOTE_DATA = "/home/ray/.local/share/pidi"
UNITS = ("jambox-engine", "pidi-native")


def _utf8_stdout() -> None:
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass


def load_creds() -> dict[str, str]:
    if not CREDS_PATH.is_file():
        sys.exit(f"missing credentials: {CREDS_PATH}")
    creds: dict[str, str] = {}
    for line in CREDS_PATH.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        creds[k.strip()] = v.strip()
    for key in ("PI_HOST", "PI_USER", "PI_PASSWORD"):
        if key not in creds or not creds[key]:
            sys.exit(f"{CREDS_PATH} needs {key}")
    return creds


def connect(creds: dict[str, str]) -> paramiko.SSHClient:
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(
        creds["PI_HOST"],
        username=creds["PI_USER"],
        password=creds["PI_PASSWORD"],
        timeout=30,
        allow_agent=False,
        look_for_keys=False,
    )
    return client


def run(
    client: paramiko.SSHClient,
    cmd: str,
    *,
    timeout: int = 120,
    check: bool = False,
    quiet: bool = False,
) -> tuple[int, str]:
    if not quiet:
        print(f"$ {cmd}", flush=True)
    _, stdout, stderr = client.exec_command(cmd, timeout=timeout, get_pty=True)
    text = (stdout.read() + stderr.read()).decode("utf-8", errors="replace")
    code = stdout.channel.recv_exit_status()
    if text:
        sys.stdout.write(text if text.endswith("\n") else text + "\n")
        sys.stdout.flush()
    if not quiet:
        print(f"exit {code}", flush=True)
    if check and code != 0:
        raise SystemExit(code)
    return code, text


def sudo(
    client: paramiko.SSHClient,
    password: str,
    cmd: str,
    *,
    timeout: int = 120,
    check: bool = False,
) -> tuple[int, str]:
    # Prefer -S so we never hang on a TTY password prompt.
    # Print without the password (matches deploy_master.py quoting).
    print(f"$ sudo {cmd}", flush=True)
    wrapped = f"echo '{password}' | sudo -S -p '' {cmd}"
    _, stdout, stderr = client.exec_command(wrapped, timeout=timeout, get_pty=True)
    text = (stdout.read() + stderr.read()).decode("utf-8", errors="replace")
    code = stdout.channel.recv_exit_status()
    if text:
        sys.stdout.write(text if text.endswith("\n") else text + "\n")
        sys.stdout.flush()
    print(f"exit {code}", flush=True)
    if check and code != 0:
        raise SystemExit(code)
    return code, text


def sftp_put(client: paramiko.SSHClient, local: pathlib.Path, remote: str) -> None:
    if not local.is_file():
        sys.exit(f"local file not found: {local}")
    sftp = client.open_sftp()
    try:
        sftp.put(str(local), remote)
    finally:
        sftp.close()
    print(f"put {local} -> {remote} ({local.stat().st_size} bytes)", flush=True)


def sftp_get(client: paramiko.SSHClient, remote: str, local: pathlib.Path | None) -> None:
    sftp = client.open_sftp()
    try:
        with sftp.open(remote, "rb") as rf:
            data = rf.read()
    finally:
        sftp.close()
    if local is None or str(local) == "-":
        sys.stdout.buffer.write(data)
        if not data.endswith(b"\n"):
            sys.stdout.buffer.write(b"\n")
        sys.stdout.buffer.flush()
    else:
        local.parent.mkdir(parents=True, exist_ok=True)
        local.write_bytes(data)
        print(f"get {remote} -> {local} ({len(data)} bytes)", flush=True)


def upload_and_run(
    client: paramiko.SSHClient,
    local: pathlib.Path,
    *,
    interpreter: str,
    timeout: int,
) -> int:
    if not local.is_file():
        sys.exit(f"local file not found: {local}")
    remote = f"/tmp/pi_talk_{int(time.time())}_{local.name}"
    sftp_put(client, local, remote)
    try:
        code, _ = run(
            client,
            f"chmod +x '{remote}'; {interpreter} '{remote}'; ec=$?; rm -f '{remote}'; exit $ec",
            timeout=timeout,
        )
        return code
    finally:
        # Best-effort cleanup if the remote shell died early.
        run(client, f"rm -f '{remote}'", quiet=True, timeout=30)


# --- canned diagnostics -----------------------------------------------------

STATUS_REMOTE = r"""
set -e
echo '=== services'
systemctl is-active jambox-engine pidi-native || true
echo
echo '=== bins'
ls -la /home/ray/pi-midi-toolkit/bin/jambox-engine /home/ray/pi-midi-toolkit/bin/pidi-native 2>/dev/null || true
echo
echo '=== version'
head -20 /home/ray/.local/share/pidi/version.json 2>/dev/null || true
echo
echo '=== recent journal'
journalctl -u jambox-engine -u pidi-native -n 40 --no-pager 2>&1 | tail -40
"""

AUDIO_REMOTE = r"""
python3 - <<'PY'
import glob, json, os, socket, subprocess, time

def sh(cmd: str) -> str:
    try:
        return subprocess.check_output(cmd, shell=True, text=True, stderr=subprocess.STDOUT)
    except subprocess.CalledProcessError as e:
        return e.output or str(e)

print('=== cards / mixer')
print(sh('cat /proc/asound/cards'))
print(sh('amixer -c 0 sget PCM 2>/dev/null | head -20 || true'))
print(sh('amixer -c 0 sget Headphone 2>/dev/null | head -20 || true'))
print('=== hw_params / pcm status')
print(sh('cat /proc/asound/card0/pcm0p/sub0/hw_params 2>/dev/null || true'))
print(sh('cat /proc/asound/card0/pcm0p/sub0/status 2>/dev/null || true'))
print('=== throttle / temp')
print(sh('vcgencmd measure_temp; vcgencmd get_throttled 2>/dev/null || true'))
print('=== processes')
print(sh('ps -o pid,psr,pcpu,pmem,comm -C jambox-engine,pidi-native 2>/dev/null || true'))

sock_path = '/tmp/jambox.sock'
print('=== live /tmp/jambox.sock status (16 samples)')
keys = [
    'callback_frames', 'callback_micros', 'callback_peak_micros', 'xruns',
    'command_drops', 'active_voices', 'active_drums', 'playing_clips',
    'peak', 'load', 'emergency_releases', 'active_repeats',
]
for i in range(16):
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(2.0)
        s.connect(sock_path)
        s.sendall(b'{"cmd":"hello","protocol":1,"client":"pi_talk","realtime_owner":false}\n')
        s.recv(65536)
        s.sendall(b'{"cmd":"status"}\n')
        buf = b''
        while b'\n' not in buf:
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
            if len(buf) > 200_000:
                break
        s.close()
        line = buf.split(b'\n', 1)[0].decode('utf-8', 'replace')
        msg = json.loads(line)
        st = msg.get('status') or msg.get('Status') or msg
        if isinstance(st, dict):
            eng = st.get('engine') or {}
            flat = {k: st.get(k, eng.get(k)) for k in keys}
            print(f'{i:02d} {json.dumps(flat, separators=(",", ":"))}', flush=True)
        else:
            print(i, line[:240], flush=True)
    except Exception as e:
        print(f'{i:02d} err {e}', flush=True)
    time.sleep(0.5)
PY
"""


def cmd_status(client: paramiko.SSHClient, creds: dict[str, str]) -> int:
    code, _ = run(client, STATUS_REMOTE, timeout=60)
    return code


def cmd_audio(client: paramiko.SSHClient, creds: dict[str, str]) -> int:
    code, _ = run(client, AUDIO_REMOTE, timeout=90)
    return code


def cmd_logs(client: paramiko.SSHClient, creds: dict[str, str], n: int, follow: bool) -> int:
    units = " ".join(f"-u {u}" for u in UNITS)
    if follow:
        # Stream briefly; agents usually want a snapshot, not an infinite hang.
        cmd = f"journalctl {units} -n {n} -f --no-pager"
        print(f"$ {cmd}  (30s then stop)", flush=True)
        _, stdout, stderr = client.exec_command(
            f"{cmd} & JPID=$!; sleep 30; kill $JPID 2>/dev/null; wait $JPID 2>/dev/null; true",
            timeout=45,
            get_pty=True,
        )
        text = (stdout.read() + stderr.read()).decode("utf-8", errors="replace")
        if text:
            sys.stdout.write(text if text.endswith("\n") else text + "\n")
        return 0
    code, _ = run(client, f"journalctl {units} -n {n} --no-pager", timeout=60)
    return code


def cmd_restart(client: paramiko.SSHClient, creds: dict[str, str]) -> int:
    pw = creds["PI_PASSWORD"]
    sudo(client, pw, "systemctl restart jambox-engine", timeout=60)
    sudo(client, pw, "systemctl restart pidi-native", timeout=60)
    code, _ = run(client, "systemctl is-active jambox-engine pidi-native", timeout=30)
    return code


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="pi_talk",
        description="SSH helper for the lab Pi (status, logs, run, put/get).",
    )
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("status", help="services, version, recent journal")
    sub.add_parser("audio", help="ALSA mixer + jambox.sock status samples")

    logs = sub.add_parser("logs", help="journalctl for jambox-engine + pidi-native")
    logs.add_argument("-n", type=int, default=80, help="lines (default 80)")
    logs.add_argument("-f", "--follow", action="store_true", help="follow ~30s then stop")

    sub.add_parser("restart", help="restart jambox-engine + pidi-native")

    run_p = sub.add_parser("run", help="run a remote shell command")
    run_p.add_argument("-t", "--timeout", type=int, default=120)
    run_p.add_argument(
        "remote_cmd",
        nargs=argparse.REMAINDER,
        help="remote argv (optional leading --)",
    )

    sudo_p = sub.add_parser("sudo", help="run a remote command with sudo -S")
    sudo_p.add_argument("-t", "--timeout", type=int, default=120)
    sudo_p.add_argument(
        "remote_cmd",
        nargs=argparse.REMAINDER,
        help="remote argv (optional leading --)",
    )

    put = sub.add_parser("put", help="upload a local file")
    put.add_argument("local")
    put.add_argument("remote")

    get = sub.add_parser("get", help="download a remote file (use - for stdout)")
    get.add_argument("remote")
    get.add_argument("local")

    bash = sub.add_parser("bash", help="upload a local .sh and run it on the Pi")
    bash.add_argument("script")
    bash.add_argument("-t", "--timeout", type=int, default=180)

    py = sub.add_parser("py", help="upload a local .py and run it with python3 on the Pi")
    py.add_argument("script")
    py.add_argument("-t", "--timeout", type=int, default=180)

    return p


def _join_remote(args: list[str]) -> str:
    # argparse REMAINDER keeps a leading "--" if the user passed one.
    parts = list(args)
    if parts and parts[0] == "--":
        parts = parts[1:]
    if not parts:
        sys.exit("missing remote command (example: pi_talk.py run uname -a)")
    return " ".join(parts)


def main(argv: list[str] | None = None) -> int:
    _utf8_stdout()
    parser = build_parser()
    args = parser.parse_args(argv)
    creds = load_creds()
    client = connect(creds)
    try:
        if args.cmd == "status":
            return cmd_status(client, creds)
        if args.cmd == "audio":
            return cmd_audio(client, creds)
        if args.cmd == "logs":
            return cmd_logs(client, creds, args.n, args.follow)
        if args.cmd == "restart":
            return cmd_restart(client, creds)
        if args.cmd == "run":
            code, _ = run(client, _join_remote(args.remote_cmd), timeout=args.timeout)
            return code
        if args.cmd == "sudo":
            code, _ = sudo(
                client,
                creds["PI_PASSWORD"],
                _join_remote(args.remote_cmd),
                timeout=args.timeout,
            )
            return code
        if args.cmd == "put":
            sftp_put(client, pathlib.Path(args.local), args.remote)
            return 0
        if args.cmd == "get":
            dest = None if args.local == "-" else pathlib.Path(args.local)
            sftp_get(client, args.remote, dest)
            return 0
        if args.cmd == "bash":
            return upload_and_run(
                client, pathlib.Path(args.script), interpreter="bash", timeout=args.timeout
            )
        if args.cmd == "py":
            return upload_and_run(
                client, pathlib.Path(args.script), interpreter="python3", timeout=args.timeout
            )
        parser.error(f"unknown command {args.cmd}")
        return 2
    finally:
        client.close()


if __name__ == "__main__":
    raise SystemExit(main())
