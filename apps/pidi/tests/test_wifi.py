#!/usr/bin/env python3
"""Headless smoke for wifi helpers (nmcli optional)."""
from __future__ import annotations

import pathlib
import sys
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from pidi import wifi  # noqa: E402


class WifiHelpersTest(unittest.TestCase):
    def test_load_credentials_from_file_and_env(self) -> None:
        with mock.patch.dict("os.environ", {"WIFI_SSID": "FromEnv"}, clear=False):
            with mock.patch.object(wifi, "HERE", pathlib.Path("/tmp/does-not-exist-pidi")):
                creds = wifi.load_wifi_credentials(pathlib.Path("/tmp/does-not-exist-pidi"))
            self.assertEqual(creds.get("WIFI_SSID"), "FromEnv")

    def test_format_line_when_nmcli_missing(self) -> None:
        with mock.patch.object(wifi, "_which_nmcli", return_value=None):
            line = wifi.format_wifi_line()
        self.assertIn("Wi-Fi:", line)

    def test_ensure_up_reports_missing_nmcli(self) -> None:
        with mock.patch.object(wifi, "_which_nmcli", return_value=None):
            ok, detail = wifi.ensure_wifi_up()
        self.assertFalse(ok)
        self.assertIn("nmcli", detail.lower())


if __name__ == "__main__":
    raise SystemExit(unittest.main())
