#!/usr/bin/env python3
"""Updater rules — no display, no network, no GitHub token required."""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parents[1]  # apps/pidi deploy root
sys.path.insert(0, str(ROOT))

from pidi import updater  # noqa: E402


def _git(cwd: pathlib.Path, *args: str) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=str(cwd),
        check=True,
        capture_output=True,
        text=True,
    )
    return (proc.stdout or "").strip()


@unittest.skipUnless(updater.git_available(), "git not installed")
class GitHelpers:
    """Shared temp-repo setup."""


class UrlHelpersTest(unittest.TestCase):
    def test_github_owner_repo_parses_https_and_ssh(self) -> None:
        self.assertEqual(
            updater.github_owner_repo(
                "https://github.com/HyruleanHero1988/pi-midi-toolkit.git"
            ),
            ("HyruleanHero1988", "pi-midi-toolkit"),
        )
        self.assertEqual(
            updater.github_owner_repo(
                "git@github.com:HyruleanHero1988/pi-midi-toolkit.git"
            ),
            ("HyruleanHero1988", "pi-midi-toolkit"),
        )
        self.assertEqual(
            updater.github_owner_repo(
                "https://x-access-token:secret@github.com/Acme/box.git"
            ),
            ("Acme", "box"),
        )

    def test_redact_url_strips_userinfo(self) -> None:
        redacted = updater.redact_url(
            "https://x-access-token:ghp_secret@github.com/Acme/box.git"
        )
        self.assertNotIn("ghp_secret", redacted)
        self.assertIn("github.com/Acme/box.git", redacted)
        self.assertIn("***@", redacted)

    def test_authenticated_https_url_embeds_token(self) -> None:
        url = updater.authenticated_https_url(
            "https://github.com/Acme/box.git", "tok_123"
        )
        self.assertIn("tok_123", url)
        self.assertTrue(url.startswith("https://x-access-token:"))


def _clear_update_env():
    return mock.patch.dict(
        "os.environ",
        {
            "MIDI_TONE_REPO_URL": "",
            "MIDI_TONE_UPDATE_BRANCH": "",
            "MIDI_TONE_UPDATE_TOKEN": "",
            "GITHUB_TOKEN": "",
        },
        clear=False,
    )


class CredentialsTest(unittest.TestCase):
    def test_load_credentials_update_file_wins_over_pi_credentials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            (install / ".pi-credentials").write_text(
                "GITHUB_TOKEN=from-pi\nREPO_URL=https://example.invalid/from-pi.git\n",
                encoding="utf-8",
            )
            (install / ".update-credentials").write_text(
                "GITHUB_TOKEN=from-update\nBRANCH=main\n",
                encoding="utf-8",
            )
            with _clear_update_env():
                creds = updater.load_credentials(install)
            self.assertEqual(creds.token, "from-update")
            self.assertEqual(creds.branch, "main")
            self.assertEqual(creds.repo_url, "https://example.invalid/from-pi.git")

    def test_save_token_writes_gitignored_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            path = updater.save_token("ghp_test_token", install)
            self.assertTrue(path.is_file())
            text = path.read_text(encoding="utf-8")
            self.assertIn("ghp_test_token", text)
            with _clear_update_env():
                creds = updater.load_credentials(install)
            self.assertEqual(creds.token, "ghp_test_token")


class VersionFileTest(unittest.TestCase):
    def test_round_trip_version_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            info = updater.VersionInfo(
                sha="deadbeefcafebabe",
                branch="master",
                source="file",
                repo_url="https://x-access-token:secret@github.com/Acme/box.git",
            )
            updater.write_version_file(install, info)
            loaded = updater.read_version_file(install)
            self.assertEqual(loaded.sha, "deadbeefcafebabe")
            self.assertEqual(loaded.short, "deadbee")
            data = json.loads((install / "version.json").read_text(encoding="utf-8"))
            self.assertNotIn("secret", json.dumps(data))


class OverlayTest(unittest.TestCase):
    def test_overlay_replaces_code_but_keeps_user_data(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            src = root / "src"
            dest = root / "dest"
            (src / "wavetables").mkdir(parents=True)
            (src / "midi_tone.py").write_text("NEW_APP\n", encoding="utf-8")
            (src / "wavetables" / "saw.wav").write_bytes(b"WAV")
            (src / "settings.json").write_text('{"wipe": true}\n', encoding="utf-8")
            (src / "songs").mkdir()
            (src / "songs" / "demo.mid").write_bytes(b"MThd")
            (src / "phrases").mkdir()
            (src / "phrases" / "pad-01.json").write_text('{"wipe":true}\n', encoding="utf-8")

            dest.mkdir()
            (dest / "midi_tone.py").write_text("OLD_APP\n", encoding="utf-8")
            (dest / "settings.json").write_text('{"keep": true}\n', encoding="utf-8")
            (dest / "songs").mkdir()
            (dest / "songs" / "take-001.mid").write_bytes(b"USER")
            (dest / "phrases").mkdir()
            (dest / "phrases" / "pad-01.json").write_text(
                '{"name":"live-phrase"}\n', encoding="utf-8"
            )
            (dest / "user-presets").mkdir()
            (dest / "user-presets" / "slot-01.json").write_text("{}\n", encoding="utf-8")
            (dest / ".venv").mkdir()
            (dest / ".venv" / "marker").write_text("venv\n", encoding="utf-8")

            written = updater.overlay_tree(src, dest)
            self.assertIn("midi_tone.py", written)
            self.assertIn("wavetables/saw.wav", written)
            self.assertNotIn("settings.json", written)
            self.assertNotIn("songs/demo.mid", written)
            self.assertNotIn("phrases/pad-01.json", written)

            self.assertEqual((dest / "midi_tone.py").read_text(encoding="utf-8"), "NEW_APP\n")
            self.assertEqual(
                (dest / "settings.json").read_text(encoding="utf-8"),
                '{"keep": true}\n',
            )
            self.assertEqual((dest / "songs" / "take-001.mid").read_bytes(), b"USER")
            self.assertFalse((dest / "songs" / "demo.mid").exists())
            self.assertEqual(
                (dest / "phrases" / "pad-01.json").read_text(encoding="utf-8"),
                '{"name":"live-phrase"}\n',
            )
            self.assertTrue((dest / "user-presets" / "slot-01.json").is_file())
            self.assertEqual((dest / ".venv" / "marker").read_text(encoding="utf-8"), "venv\n")
            self.assertTrue((dest / "wavetables" / "saw.wav").is_file())

    def test_full_repo_overlay_updates_crates_and_keeps_live_preset(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            src = root / "src"
            dest = root / "dest"
            (src / "crates" / "midi-core" / "src").mkdir(parents=True)
            (src / "apps" / "pidi").mkdir(parents=True)
            (src / "presets").mkdir()
            (src / "deploy").mkdir()
            (src / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
            (src / "crates" / "midi-core" / "src" / "lib.rs").write_text("// new\n", encoding="utf-8")
            (src / "apps" / "pidi" / "midi_tone.py").write_text("NEW\n", encoding="utf-8")
            (src / "apps" / "pidi" / "settings.json").write_text('{"wipe":1}\n', encoding="utf-8")
            (src / "apps" / "pidi" / "phrases").mkdir()
            (src / "apps" / "pidi" / "phrases" / "pad-01.json").write_text(
                '{"wipe":true}\n', encoding="utf-8"
            )
            (src / "apps" / "pidi" / "songs").mkdir()
            (src / "apps" / "pidi" / "songs" / "demo.mid").write_bytes(b"WIPE")
            (src / "presets" / "example.json").write_text("{}\n", encoding="utf-8")
            (src / "presets" / "active.json").write_text('{"wipe":true}\n', encoding="utf-8")
            (src / "deploy" / "midi-engine.service").write_text("[Unit]\n", encoding="utf-8")
            (src / "dist" / "armv7").mkdir(parents=True)
            (src / "dist" / "armv7" / "midi-engine").write_bytes(b"NEWELF")
            (src / "dist" / "armv7" / "jambox-engine").write_bytes(b"NEWJAM")

            (dest / "apps" / "pidi").mkdir(parents=True)
            (dest / "presets").mkdir()
            (dest / "bin").mkdir()
            (dest / "apps" / "pidi" / "midi_tone.py").write_text("OLD\n", encoding="utf-8")
            (dest / "apps" / "pidi" / "settings.json").write_text('{"keep":1}\n', encoding="utf-8")
            (dest / "apps" / "pidi" / "phrases").mkdir()
            (dest / "apps" / "pidi" / "phrases" / "pad-01.json").write_text(
                '{"name":"my-groove"}\n', encoding="utf-8"
            )
            (dest / "apps" / "pidi" / "songs").mkdir()
            (dest / "apps" / "pidi" / "songs" / "take-001.mid").write_bytes(b"USER")
            (dest / "presets" / "active.json").write_text('{"live":true}\n', encoding="utf-8")
            (dest / "bin" / "midi-engine").write_bytes(b"OLDELF")

            written = updater.overlay_tree(src, dest, keep=updater.KEEP_REPO)
            self.assertIn("crates/midi-core/src/lib.rs", written)
            self.assertIn("apps/pidi/midi_tone.py", written)
            self.assertIn("presets/example.json", written)
            self.assertIn("deploy/midi-engine.service", written)
            self.assertIn("dist/armv7/midi-engine", written)
            self.assertIn("dist/armv7/jambox-engine", written)
            self.assertNotIn("presets/active.json", written)
            self.assertNotIn("apps/pidi/settings.json", written)
            self.assertNotIn("apps/pidi/phrases/pad-01.json", written)
            self.assertNotIn("apps/pidi/songs/demo.mid", written)
            self.assertEqual((dest / "apps" / "pidi" / "midi_tone.py").read_text(encoding="utf-8"), "NEW\n")
            self.assertEqual(
                (dest / "apps" / "pidi" / "settings.json").read_text(encoding="utf-8"),
                '{"keep":1}\n',
            )
            self.assertEqual(
                (dest / "apps" / "pidi" / "phrases" / "pad-01.json").read_text(encoding="utf-8"),
                '{"name":"my-groove"}\n',
            )
            self.assertEqual(
                (dest / "apps" / "pidi" / "songs" / "take-001.mid").read_bytes(),
                b"USER",
            )
            self.assertFalse((dest / "apps" / "pidi" / "songs" / "demo.mid").exists())
            self.assertEqual(
                (dest / "presets" / "active.json").read_text(encoding="utf-8"),
                '{"live":true}\n',
            )
            self.assertEqual((dest / "bin" / "midi-engine").read_bytes(), b"OLDELF")
            self.assertTrue((dest / "presets" / "example.json").is_file())
            self.assertEqual((dest / "dist" / "armv7" / "midi-engine").read_bytes(), b"NEWELF")
            self.assertEqual((dest / "dist" / "armv7" / "jambox-engine").read_bytes(), b"NEWJAM")

            notes: list[str] = []
            installed = updater.install_pi_binaries(dest, notes.append)
            self.assertEqual(installed, ["midi-engine", "jambox-engine"])
            self.assertEqual((dest / "bin" / "midi-engine").read_bytes(), b"NEWELF")
            self.assertEqual((dest / "bin" / "jambox-engine").read_bytes(), b"NEWJAM")
            if os.name != "nt":
                self.assertTrue((dest / "bin" / "midi-engine").stat().st_mode & 0o111)
            self.assertTrue(any("midi-engine" in n for n in notes))

    def test_finds_repo_root_in_github_layout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            inner = root / "pi-midi-toolkit-master"
            (inner / "apps" / "pidi").mkdir(parents=True)
            (inner / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
            (inner / "apps" / "pidi" / "midi_tone.py").write_text("# app\n", encoding="utf-8")
            found = updater._find_repo_root(root)
            self.assertEqual(found, inner)

    def test_sync_kiosk_copy_when_install_is_not_inside_repo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = pathlib.Path(tmp) / "pi-midi-toolkit"
            kiosk = pathlib.Path(tmp) / "midi-tone"
            (repo / "apps" / "pidi").mkdir(parents=True)
            kiosk.mkdir()
            (repo / "apps" / "pidi" / "midi_tone.py").write_text("NEW\n", encoding="utf-8")
            (kiosk / "midi_tone.py").write_text("OLD\n", encoding="utf-8")
            (kiosk / "settings.json").write_text('{"keep":1}\n', encoding="utf-8")
            (kiosk / "phrases").mkdir()
            (kiosk / "phrases" / "pad-07.json").write_text('{"keep":"phrase"}\n', encoding="utf-8")
            notes: list[str] = []
            updater._sync_kiosk_from_repo(repo, kiosk, notes.append)
            self.assertEqual((kiosk / "midi_tone.py").read_text(encoding="utf-8"), "NEW\n")
            self.assertEqual((kiosk / "settings.json").read_text(encoding="utf-8"), '{"keep":1}\n')
            self.assertEqual(
                (kiosk / "phrases" / "pad-07.json").read_text(encoding="utf-8"),
                '{"keep":"phrase"}\n',
            )
            self.assertTrue(any("kiosk" in n.lower() for n in notes))


class CheckUpdateTest(unittest.TestCase):
    def test_same_sha_is_not_available(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            updater.write_version_file(
                install,
                updater.VersionInfo(sha="aaa1111", branch="master", source="file"),
            )
            remote = updater.VersionInfo(
                sha="aaa1111", branch="master", source="remote"
            )
            with mock.patch.object(updater, "load_credentials", return_value=updater.Credentials()), mock.patch.object(
                updater, "detect_branch", return_value="master"
            ), mock.patch.object(updater, "remote_head", return_value=remote):
                result = updater.check_for_update(install)
            self.assertFalse(result.available)
            self.assertIn("Already on", result.message)

    def test_different_sha_is_available(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            updater.write_version_file(
                install,
                updater.VersionInfo(sha="aaa1111", branch="master", source="file"),
            )
            remote = updater.VersionInfo(
                sha="bbb2222cccc", branch="master", source="remote"
            )
            with mock.patch.object(updater, "load_credentials", return_value=updater.Credentials()), mock.patch.object(
                updater, "detect_branch", return_value="master"
            ), mock.patch.object(updater, "remote_head", return_value=remote):
                result = updater.check_for_update(install)
            self.assertTrue(result.available)
            self.assertIn("bbb2222", result.message)
            self.assertEqual(result.remote.short, "bbb2222")

    def test_unknown_local_offers_update(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            remote = updater.VersionInfo(
                sha="ccc3333dddd", branch="master", source="remote"
            )
            with mock.patch.object(updater, "load_credentials", return_value=updater.Credentials()), mock.patch.object(
                updater, "detect_branch", return_value="master"
            ), mock.patch.object(updater, "remote_head", return_value=remote):
                result = updater.check_for_update(install)
            self.assertTrue(result.available)
            self.assertEqual(result.local.short, "unknown")

    def test_network_error_is_not_available(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            with mock.patch.object(updater, "load_credentials", return_value=updater.Credentials()), mock.patch.object(
                updater, "detect_branch", return_value="master"
            ), mock.patch.object(
                updater,
                "remote_head",
                side_effect=updater.UpdateError("can't reach the private repo"),
            ):
                result = updater.check_for_update(install)
            self.assertFalse(result.available)
            self.assertIn("private repo", result.error)


class ArchiveSafetyTest(unittest.TestCase):
    def test_refuses_path_traversal_in_tarball(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tar_path = root / "bad.tar.gz"
            with tarfile.open(tar_path, "w:gz") as tf:
                info = tarfile.TarInfo(name="../evil.py")
                data = b"nope"
                info.size = len(data)
                import io

                tf.addfile(info, io.BytesIO(data))
            dest = root / "out"
            with self.assertRaises(updater.UpdateError):
                updater._safe_extract_tar(tar_path, dest)

    def test_finds_midi_tone_dir_in_github_layout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            inner = root / "pi-midi-toolkit-master" / "apps" / "pidi"
            inner.mkdir(parents=True)
            (inner / "midi_tone.py").write_text("# app\n", encoding="utf-8")
            found = updater._find_midi_tone_dir(root)
            self.assertEqual(found, inner)


@unittest.skipUnless(updater.git_available(), "git not installed")
class GitLocalVersionTest(unittest.TestCase):
    def test_local_version_reads_git_head(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = pathlib.Path(tmp)
            _git(repo, "init")
            _git(repo, "config", "user.email", "test@example.com")
            _git(repo, "config", "user.name", "Test")
            (repo / "README").write_text("hi\n", encoding="utf-8")
            _git(repo, "add", "README")
            _git(repo, "commit", "-m", "init")
            sha = _git(repo, "rev-parse", "HEAD")
            info = updater.local_version(repo)
            self.assertEqual(info.sha, sha)
            self.assertEqual(info.source, "git")


class ApplyUpdateTest(unittest.TestCase):
    def test_apply_skips_work_when_already_current(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            sha = "abc1234deadbeef"
            updater.write_version_file(
                install,
                updater.VersionInfo(sha=sha, branch="master", source="file"),
            )
            remote = updater.VersionInfo(
                sha=sha, branch="master", source="remote"
            )
            notes: list[str] = []
            with mock.patch.object(updater, "load_credentials", return_value=updater.Credentials()), mock.patch.object(
                updater, "detect_branch", return_value="master"
            ), mock.patch.object(updater, "remote_head", return_value=remote), mock.patch.object(
                updater, "git_root", return_value=None
            ), mock.patch.object(
                updater, "apply_from_archive", side_effect=AssertionError("should not download")
            ):
                info = updater.apply_update(install, progress=notes.append)
            self.assertEqual(info.sha, sha)
            self.assertTrue(any("Already" in n for n in notes))

    def test_apply_uses_archive_overlay_when_not_a_git_checkout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            install = pathlib.Path(tmp)
            (install / "midi_tone.py").write_text("OLD\n", encoding="utf-8")
            (install / "settings.json").write_text('{"keep": 1}\n', encoding="utf-8")
            remote = updater.VersionInfo(
                sha="fff9999aaaa", branch="master", source="remote"
            )

            def fake_archive(dest, repo_url, branch, token, progress, **_kwargs) -> str:
                src = pathlib.Path(tmp) / "incoming"
                src.mkdir()
                (src / "midi_tone.py").write_text("NEW\n", encoding="utf-8")
                updater.overlay_tree(src, dest)
                progress("overlayed")
                return ""

            with mock.patch.object(updater, "load_credentials", return_value=updater.Credentials()), mock.patch.object(
                updater, "detect_branch", return_value="master"
            ), mock.patch.object(updater, "remote_head", return_value=remote), mock.patch.object(
                updater, "git_root", return_value=None
            ), mock.patch.object(
                updater, "apply_from_archive", side_effect=fake_archive
            ), mock.patch.object(updater, "_pip_install"), mock.patch.object(
                updater, "install_pi_binaries"
            ), mock.patch.object(
                updater, "_restart_engines"
            ):
                info = updater.apply_update(install)
            self.assertEqual(info.sha, "fff9999aaaa")
            self.assertEqual((install / "midi_tone.py").read_text(encoding="utf-8"), "NEW\n")
            self.assertEqual(
                (install / "settings.json").read_text(encoding="utf-8"),
                '{"keep": 1}\n',
            )
            stamped = updater.read_version_file(install)
            self.assertEqual(stamped.sha, "fff9999aaaa")


class InstallPiBinariesTest(unittest.TestCase):
    def test_missing_dist_leaves_existing_bin(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "bin").mkdir()
            (root / "bin" / "midi-engine").write_bytes(b"KEEP")
            notes: list[str] = []
            installed = updater.install_pi_binaries(root, notes.append)
            self.assertEqual(installed, [])
            self.assertEqual((root / "bin" / "midi-engine").read_bytes(), b"KEEP")
            self.assertTrue(any("leaving existing" in n.lower() for n in notes))

    def test_partial_stage_replaces_only_present_engines(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "bin").mkdir()
            (root / "dist" / "armv7").mkdir(parents=True)
            (root / "bin" / "midi-engine").write_bytes(b"OLD_MIDI")
            (root / "bin" / "jambox-engine").write_bytes(b"OLD_JAM")
            (root / "dist" / "armv7" / "midi-engine").write_bytes(b"NEW_MIDI")
            installed = updater.install_pi_binaries(root)
            self.assertEqual(installed, ["midi-engine"])
            self.assertEqual((root / "bin" / "midi-engine").read_bytes(), b"NEW_MIDI")
            self.assertEqual((root / "bin" / "jambox-engine").read_bytes(), b"OLD_JAM")


class StatusTextTest(unittest.TestCase):
    def test_format_status_mentions_user_data(self) -> None:
        text = updater.format_status_lines(None, pathlib.Path("/tmp/does-not-exist-midi-tone"))
        self.assertIn("Running:", text)
        self.assertIn("phrases", text)
        self.assertIn("SSH deploy", text)
        self.assertIn("dist/armv7", text)


if __name__ == "__main__":
    sys.exit(unittest.main())
