#!/usr/bin/env python3
"""Deploy-root shim — LightDM display-setup and older docs expect this path."""
from __future__ import annotations

import pathlib
import runpy

TARGET = pathlib.Path(__file__).resolve().parent / "pidi" / "splash-x11.py"
runpy.run_path(str(TARGET), run_name="__main__")
