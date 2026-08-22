"""Touch keyboard layout coverage."""
from __future__ import annotations

from pidi.ui.touch_keyboard import missing_printable_ascii, reachable_characters


def test_switch_layout_covers_letters_digits_space() -> None:
    have = reachable_characters()
    for ch in "abcdefghijklmnopqrstuvwxyz":
        assert ch in have
        assert ch.upper() in have
    for ch in "0123456789":
        assert ch in have
    assert " " in have


def test_common_wifi_punctuation_reachable() -> None:
    have = reachable_characters()
    for ch in "-_!@#$%^&*()+=[]{}|;:'\",.<>?/\\`~":
        assert ch in have, f"missing {ch!r}"


def test_missing_printable_ascii_is_empty_or_documented() -> None:
    # Full printable ASCII should be reachable for WPA2 passwords.
    missing = missing_printable_ascii()
    assert missing == [], f"unreachable: {missing!r}"
