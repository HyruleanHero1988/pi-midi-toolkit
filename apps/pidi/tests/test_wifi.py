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

    def test_parse_wifi_list_dedupes_and_sorts(self) -> None:
        raw = "\n".join(
            [
                ":Cafe:40:WPA2",
                "yes:HomeNet:80:WPA2",
                ":Cafe:55:WPA2",
                ":OpenGuest:20:",
                ":Weird\\:Name:10:WPA3",
            ]
        )
        nets = wifi.parse_wifi_list(raw)
        ssids = [n.ssid for n in nets]
        self.assertEqual(ssids[0], "HomeNet")
        self.assertTrue(nets[0].in_use)
        cafe = next(n for n in nets if n.ssid == "Cafe")
        self.assertEqual(cafe.signal, 55)
        guest = next(n for n in nets if n.ssid == "OpenGuest")
        self.assertTrue(guest.is_open)
        weird = next(n for n in nets if n.ssid == "Weird:Name")
        self.assertEqual(weird.signal, 10)

    def test_connect_wifi_requires_ssid(self) -> None:
        ok, detail = wifi.connect_wifi("")
        self.assertFalse(ok)
        self.assertIn("No network", detail)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
