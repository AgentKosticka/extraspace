#!/usr/bin/env python3
"""Exercise installation/upgrade/removal in isolated XDG directories."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]


class Installation(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="extraspace-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.binary = self.root / "target/release/extraspace"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_text("#!/bin/sh\necho old\n")
        self.binary.chmod(0o755)
        self.bin_dir = self.root / 'bin with spaces "$`%\\'
        self.env = dict(os.environ, XDG_BIN_HOME=str(self.bin_dir),
                        XDG_DATA_HOME=str(self.root / "data"),
                        XDG_CONFIG_HOME=str(self.root / "config"),
                        CARGO_TARGET_DIR=str(self.root / "target"))
        self.env.pop("EXTRASPACE_APK", None)
        self.apk = self.root / "custom.apk"
        self.apk.write_bytes(b"explicit-test-apk")
        self.desktop = self.root / "data/applications/io.github.tymonoman.Extraspace.desktop"

    def run_install(self, *args, success=True):
        result = subprocess.run([str(REPO / "scripts/install.sh"), *args],
                                env=self.env, capture_output=True, text=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def test_upgrade_replaces_running_binary_and_preserves_settings(self):
        settings = self.root / "config/extraspace/config.json"
        settings.parent.mkdir(parents=True)
        settings.write_text('{"scale":1.75}')
        self.run_install("--no-build", "--apk", str(self.apk))
        installed = self.bin_dir / "extraspace"
        # An open handle continues to see the old inode while the new path changes.
        with installed.open() as old:
            self.binary.write_text("#!/bin/sh\necho new\n")
            self.run_install("--no-build", "--apk", str(self.apk))
            self.assertIn("old", old.read())
            self.assertIn("new", installed.read_text())
        self.assertEqual((self.root / "data/extraspace/extraspace.apk").read_bytes(), self.apk.read_bytes())
        if shutil.which("desktop-file-validate"):
            subprocess.run(["desktop-file-validate", str(self.desktop)], check=True, capture_output=True)
        entry = self.desktop.read_text()
        self.assertIn('Exec="', entry)
        self.assertIn("%%", entry)
        self.assertIn("Terminal=false", entry)
        self.assertFalse((self.root / "config/autostart/io.github.tymonoman.Extraspace.desktop").exists())
        self.run_install("--uninstall")
        self.assertFalse(installed.exists())
        self.assertFalse(self.desktop.exists())
        self.assertEqual(settings.read_text(), '{"scale":1.75}')

    def test_invalid_apk_or_option_does_not_install(self):
        self.run_install("--no-build", "--apk", str(self.root / "missing.apk"), success=False)
        self.run_install("--apk", success=False)
        self.run_install("--unknown", success=False)
        self.assertFalse(self.bin_dir.exists())

    def test_check_detects_ubuntu_without_privileged_commands(self):
        release = self.root / "os-release"
        release.write_text('ID=ubuntu\nID_LIKE=debian\nPRETTY_NAME="Test Ubuntu"\n')
        mocks = self.root / "mocks"
        mocks.mkdir()
        for name, content in {
            "dpkg-query": "exit 1",
            "adb": "echo 'List of devices attached'",
            "gst-inspect-1.0": "exit 0",
            "pkg-config": "echo 'test headers'",
            "sudo": "echo sudo-must-not-run >&2; exit 99",
            "apt-get": "echo apt-must-not-run >&2; exit 99",
        }.items():
            path = mocks / name
            path.write_text("#!/bin/sh\n" + content + "\n")
            path.chmod(0o755)
        env = dict(self.env, EXTRASPACE_OS_RELEASE=str(release), PATH=str(mocks) + ":" + os.environ["PATH"])
        result = subprocess.run([str(REPO / "scripts/setup.sh"), "--check"], env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("libpipewire-0.3-dev", result.stdout)
        self.assertIn("gstreamer1.0-plugins-ugly", result.stdout)
        self.assertNotIn("v4l2loopback", result.stdout)
        self.assertNotIn("must-not-run", result.stderr)


if __name__ == "__main__":
    unittest.main()
