#!/usr/bin/env python3
"""Convenience launcher: deploy the current git branch to the lab Pi."""

from __future__ import annotations

import pathlib
import runpy
import sys

SCRIPT = (
    pathlib.Path(__file__).resolve().parent.parent
    / "apps"
    / "pidi"
    / "scripts"
    / "deploy"
    / "deploy_native.py"
)

if __name__ == "__main__":
    sys.argv[0] = str(SCRIPT)
    runpy.run_path(str(SCRIPT), run_name="__main__")
