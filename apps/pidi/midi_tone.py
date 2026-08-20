#!/usr/bin/env python3
"""Deploy-root entrypoint (``python midi_tone.py``). Prefer ``python -m pidi``."""
from __future__ import annotations

import sys
from pathlib import Path

_ROOT = Path(__file__).resolve().parent
if str(_ROOT) not in sys.path:
    sys.path.insert(0, str(_ROOT))

from pidi.midi_tone import *  # noqa: F401,F403
from pidi.main import main

if __name__ == "__main__":
    main()
