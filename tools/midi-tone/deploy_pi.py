#!/usr/bin/env python3
"""Deploy midi-tone to the Pi using .pi-credentials (gitignored)."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time

try:
    import paramiko
except ImportError:
    sys.exit("pip install paramiko")


HERE = pathlib.Path(__file__).resolve().parent
CREDS = HERE / ".pi-credentials"

FILES = [
    "midi_tone.py",
    "sequencer.py",  # imported by midi_tone.py — the app won't start without it
    "screensaver.py",  # TFT idle blank / burn-in guard
    "fetch_akwf.py",
    "fetch_songs.py",
    "requirements.txt",
    "run.sh",
    "launch-desktop.sh",
    "kiosk.sh",
    "install-kiosk.sh",
    "disable-kiosk.sh",
    "setup-venv.sh",
    "install-desktop-shortcut.sh",
    "midi-tone.desktop",
    "README.md",
    "KIOSK.md",
    "fix-audio-headphones.sh",
    "enable-gpio-touch.sh",
    "calibrate-touch-y.sh",
    "prefer-tft70-display.sh",
    "hide-touch-cursor.sh",
    "enable-tft70-dsi.sh",
    "BOOT-RECOVERY-HDMI.txt",
    "fix-touch-x11.sh",
    "set-touch-overlay.sh",
    "splash-x11.py",
    "install-pidi-splash.sh",
    "pi-power.sh",
    "updater.py",
    "kaoss.py",
]

# Extra trees copied recursively
DIRS = [
    "wavetables",
    "kiosk",
    "demo-songs",  # offline Mutopia demos; seeded into songs/ on first launch
    "branding",
]


def load_creds() -> dict[str, str]:
    if not CREDS.exists():
        sys.exit(f"Missing {CREDS}")
    out: dict[str, str] = {}
    for line in CREDS.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        out[k.strip()] = v.strip()
    for key in ("PI_HOST", "PI_USER", "PI_PASSWORD"):
        if key not in out:
            sys.exit(f"Missing {key} in .pi-credentials")
    return out


def connect(creds: dict[str, str]) -> paramiko.SSHClient:
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(
        creds["PI_HOST"],
        username=creds["PI_USER"],
        password=creds["PI_PASSWORD"],
        timeout=60,
        banner_timeout=60,
        auth_timeout=60,
        allow_agent=False,
        look_for_keys=False,
    )
    return client


def run(
    client: paramiko.SSHClient,
    cmd: str,
    timeout: int = 120,
    check: bool = True,
) -> str:
    print(f"$ {cmd}")
    _stdin, stdout, stderr = client.exec_command(cmd, timeout=timeout, get_pty=False)
    out = stdout.read().decode("utf-8", errors="replace")
    err = stderr.read().decode("utf-8", errors="replace")
    code = stdout.channel.recv_exit_status()
    if out.strip():
        print(out.rstrip().encode("ascii", "replace").decode("ascii"))
    if err.strip():
        print(err.rstrip().encode("ascii", "replace").decode("ascii"), file=sys.stderr)
    if check and code != 0:
        raise SystemExit(f"Remote command failed ({code}): {cmd}")
    return out


def deploy(restart: bool) -> None:
    creds = load_creds()
    remote_dir = creds.get("PI_DIR", "~/midi-tone").replace("~", f"/home/{creds['PI_USER']}")
    client = connect(creds)
    try:
        run(client, f"mkdir -p {remote_dir}")
        sftp = client.open_sftp()
        try:
            for name in FILES:
                local = HERE / name
                if not local.exists():
                    print(f"skip missing {name}")
                    continue
                remote = f"{remote_dir}/{name}"
                print(f"put {name} -> {remote}")
                sftp.put(str(local), remote)

            for dirname in DIRS:
                local_dir = HERE / dirname
                if not local_dir.is_dir():
                    print(f"skip missing dir {dirname}")
                    continue
                # Create the whole tree once, then upload files
                run(client, f"mkdir -p {remote_dir}/{dirname}")
                for path in sorted(local_dir.rglob("*")):
                    if path.is_dir():
                        rel_dir = path.relative_to(HERE).as_posix()
                        run(client, f"mkdir -p {remote_dir}/{rel_dir}", check=False)
                        continue
                    rel = path.relative_to(HERE).as_posix()
                    remote_path = f"{remote_dir}/{rel}"
                    print(f"put {rel} -> {remote_path}")
                    sftp.put(str(path), remote_path)
        finally:
            sftp.close()

        run(
            client,
            f"sed -i 's/\\r$//' {remote_dir}/*.sh {remote_dir}/*.desktop "
            f"{remote_dir}/kiosk/*.sh {remote_dir}/kiosk/*.desktop "
            f"{remote_dir}/kiosk/openbox/* 2>/dev/null; "
            f"chmod +x {remote_dir}/*.sh {remote_dir}/kiosk/*.sh "
            f"{remote_dir}/fetch_akwf.py "
            f"{remote_dir}/splash-x11.py 2>/dev/null || true",
        )
        # Stamp the SHA this copy came from so SET → CHECK can compare to GitHub.
        try:
            sha = subprocess.check_output(
                ["git", "rev-parse", "HEAD"],
                cwd=str(HERE.parent.parent),
                text=True,
                timeout=8,
            ).strip()
        except Exception:
            sha = ""
        if sha:
            payload = json.dumps(
                {
                    "sha": sha,
                    "branch": "master",
                    "source": "deploy",
                    "repo_url": "https://github.com/HyruleanHero1988/pi-midi-toolkit.git",
                },
                indent=2,
            )
            run(
                client,
                f"cat > {remote_dir}/version.json <<'EOF'\n{payload}\nEOF",
                check=False,
            )
        # Refresh menu/desktop launchers so they keep using the venv via run.sh
        run(client, f"bash {remote_dir}/install-desktop-shortcut.sh", check=False)

        if restart:
            # Under kiosk, only poke midi_tone — the session loop restarts it.
            # Double-launch (kiosk + launch-desktop) pegs two cores and crunchs audio.
            kiosk_out = run(
                client,
                "pgrep -af 'kiosk\\.sh|midi-tone-kiosk' || true",
                check=False,
            )
            run(client, "pkill -f '[m]idi_tone.py' || true", check=False)
            time.sleep(1.0)
            if kiosk_out.strip():
                print("kiosk session detected — waiting for restart loop")
                time.sleep(5.0)
                run(client, "pgrep -a midi_tone || true", check=False)
                # Confirm new audio line landed in kiosk log
                run(
                    client,
                    "grep 'audio: wavetable' /tmp/midi-tone-kiosk.log | tail -2 || true",
                    check=False,
                )
            else:
                start = (
                    f"bash -lc 'export DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/$(id -u); "
                    f"cd {remote_dir}; "
                    f"./launch-desktop.sh'"
                )
                run(client, start)
                time.sleep(1.5)
            run(client, "tail -n 40 /tmp/midi-tone.log || true", check=False)
            run(client, "tail -n 20 /tmp/midi-tone-kiosk.log || true", check=False)
        print("Deploy OK")
    finally:
        client.close()


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--restart", action="store_true")
    args = p.parse_args()
    deploy(restart=args.restart)


if __name__ == "__main__":
    main()
