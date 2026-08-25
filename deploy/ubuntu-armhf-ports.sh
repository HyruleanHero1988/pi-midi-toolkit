#!/usr/bin/env bash
# Pin Ubuntu archive/security to amd64 and add ports.ubuntu.com for armhf.
#
# Adding the armhf architecture without this makes apt 404 on
# archive.ubuntu.com and security.ubuntu.com (those mirrors are amd64/i386).
# Used by deploy/build-pi-bins.sh and .github/workflows/build-pi-bins.yml.
set -euo pipefail

if [[ ! -r /etc/os-release ]]; then
  exit 0
fi
# shellcheck disable=SC1091
. /etc/os-release
if [[ "${ID:-}" != "ubuntu" ]]; then
  exit 0
fi

run_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

codename="${VERSION_CODENAME:-jammy}"
keyring="/usr/share/keyrings/ubuntu-archive-keyring.gpg"

run_root python3 - "$codename" "$keyring" <<'PY'
from pathlib import Path
import sys

codename = sys.argv[1]
keyring = sys.argv[2]
list_dir = Path("/etc/apt/sources.list.d")
sources_list = Path("/etc/apt/sources.list")


def pin_list_file(path: Path) -> None:
    if not path.is_file():
        return
    text = path.read_text(encoding="utf-8")
    out = []
    changed = False
    for line in text.splitlines(True):
        stripped = line.lstrip()
        if stripped.startswith("deb ") or stripped.startswith("deb-src "):
            if "[arch=" not in stripped.split("#", 1)[0]:
                if stripped.startswith("deb-src "):
                    line = line.replace("deb-src ", "deb-src [arch=amd64] ", 1)
                else:
                    line = line.replace("deb ", "deb [arch=amd64] ", 1)
                changed = True
        out.append(line)
    if changed:
        path.write_text("".join(out), encoding="utf-8")


def pin_deb822(path: Path) -> None:
    if not path.is_file():
        return
    lines = path.read_text(encoding="utf-8").splitlines(True)
    out = []
    i = 0
    changed = False
    while i < len(lines):
        line = lines[i]
        out.append(line)
        if line.startswith("Types:") and "deb" in line:
            j = i + 1
            has_arch = False
            while (
                j < len(lines)
                and lines[j].strip()
                and not lines[j].startswith("#")
                and not lines[j].startswith("Types:")
            ):
                if lines[j].startswith("Architectures:"):
                    has_arch = True
                    break
                j += 1
            if not has_arch:
                out.append("Architectures: amd64\n")
                changed = True
        i += 1
    if changed:
        path.write_text("".join(out), encoding="utf-8")


if sources_list.is_file():
    pin_list_file(sources_list)

if list_dir.is_dir():
    for path in sorted(list_dir.iterdir()):
        name = path.name
        if name.startswith("ubuntu-ports-armhf"):
            continue
        if name.endswith(".list"):
            pin_list_file(path)
        elif name.endswith(".sources"):
            pin_deb822(path)

deb822 = list_dir / "ubuntu.sources"
ports_sources = list_dir / "ubuntu-ports-armhf.sources"
ports_list = list_dir / "ubuntu-ports-armhf.list"

if deb822.is_file():
    if not ports_sources.is_file():
        ports_sources.write_text(
            "\n".join(
                [
                    "Types: deb",
                    "URIs: http://ports.ubuntu.com/ubuntu-ports",
                    f"Suites: {codename} {codename}-updates {codename}-security",
                    "Components: main universe restricted multiverse",
                    "Architectures: armhf",
                    f"Signed-By: {keyring}",
                    "",
                ]
            ),
            encoding="utf-8",
        )
elif not ports_list.is_file():
    ports_list.write_text(
        "\n".join(
            [
                f"deb [arch=armhf] http://ports.ubuntu.com/ubuntu-ports {codename} main universe restricted multiverse",
                f"deb [arch=armhf] http://ports.ubuntu.com/ubuntu-ports {codename}-updates main universe restricted multiverse",
                f"deb [arch=armhf] http://ports.ubuntu.com/ubuntu-ports {codename}-security main universe restricted multiverse",
                "",
            ]
        ),
        encoding="utf-8",
    )
PY
