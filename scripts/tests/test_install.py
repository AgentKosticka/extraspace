#!/usr/bin/env python3
"""Exercise installation/upgrade/removal in isolated XDG directories."""
import os
import hashlib
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
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

    def test_upgrade_preserves_private_gpu_launcher(self):
        data = self.root / "data/extraspace"
        data.mkdir(parents=True)
        launcher = data / "launch-gpu"
        launcher.write_text("#!/bin/sh\nexit 0\n")
        launcher.chmod(0o755)
        driver = data / "intel-va/usr/lib/x86_64-linux-gnu/dri/iHD_drv_video.so"
        driver.parent.mkdir(parents=True)
        driver.write_bytes(b"test-driver")
        self.run_install("--no-build", "--apk", str(self.apk))
        self.assertIn(f'Exec="{launcher}"', self.desktop.read_text())
        # A launcher without its driver must not mask the normal installation.
        driver.unlink()
        self.run_install("--no-build", "--apk", str(self.apk))
        self.assertNotIn(f'Exec="{launcher}"', self.desktop.read_text())

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
        self.assertIn('Exec=/usr/bin/env "', entry)
        self.assertIn("%%", entry)
        self.assertIn("Terminal=false", entry)
        self.assertFalse((self.root / "config/autostart/io.github.tymonoman.Extraspace.desktop").exists())
        self.run_install("--uninstall")
        self.assertFalse(installed.exists())
        self.assertFalse(self.desktop.exists())
        self.assertEqual(settings.read_text(), '{"scale":1.75}')

    def test_desktop_launch_round_trips_backslashes_and_reserved_characters(self):
        # Validate the two desktop-entry escaping layers through the real launcher,
        # not just desktop-file-validate or a parser that mirrors the installer.
        marker = self.root / "launched"
        self.binary.write_text('#!/bin/sh\nprintf launched > "$EXTRASPACE_LAUNCH_MARKER"\n')
        launcher = (
            "import sys\nfrom gi.repository import Gio\n"
            "app = Gio.DesktopAppInfo.new_from_filename(sys.argv[1])\n"
            "assert app is not None\napp.launch([], None)\n"
        )
        for suffix in ['one\\', 'two\\\\', 'before\\"$`% and spaces']:
            with self.subTest(suffix=suffix):
                self.env["XDG_BIN_HOME"] = str(self.root / suffix)
                marker.unlink(missing_ok=True)
                self.run_install("--no-build", "--apk", str(self.apk))
                env = dict(self.env, EXTRASPACE_LAUNCH_MARKER=str(marker))
                result = subprocess.run(["/usr/bin/python3", "-c", launcher, str(self.desktop)],
                                        env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                deadline = time.monotonic() + 3
                while not marker.exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertTrue(marker.exists(), result.stderr)
                self.assertEqual(marker.read_text(), "launched")

    def test_invalid_apk_or_option_does_not_install(self):
        self.run_install("--no-build", "--apk", str(self.root / "missing.apk"), success=False)
        self.run_install("--apk", success=False)
        self.run_install("--unknown", success=False)
        self.run_install("--download-apk", "--build-apk", success=False)
        self.run_install("--download-apk", "--apk", str(self.apk), success=False)
        self.assertFalse(self.bin_dir.exists())

    def mock_release(self):
        assets = self.root / "release"
        assets.mkdir()
        (assets / "extraspace.apk").write_bytes(self.apk.read_bytes())
        digest = hashlib.sha256(self.apk.read_bytes()).hexdigest()
        (assets / "extraspace.apk.sha256").write_text(digest + "  extraspace.apk\n")
        (assets / "companion-version").write_text((REPO / "companion-version").read_text())
        mocks = self.root / "download-mocks"
        mocks.mkdir()
        curl = mocks / "curl"
        curl.write_text("#!/usr/bin/env python3\n" +
                        "import os, sys, shutil\nfrom pathlib import Path\n" +
                        "shutil.copyfile(Path(os.environ['TEST_RELEASE']) / sys.argv[-3].rsplit('/', 1)[-1], sys.argv[-1])\n")
        curl.chmod(0o755)
        self.env.update(TEST_RELEASE=str(assets),
                        PATH=str(mocks) + ":" + os.environ["PATH"],
                        EXTRASPACE_RELEASE_URL="https://example.invalid/continuous")
        return assets

    def test_download_verifies_release_before_installing(self):
        assets = self.mock_release()
        self.run_install("--download-apk", "--no-build")
        self.assertEqual((self.root / "data/extraspace/extraspace.apk").read_bytes(), self.apk.read_bytes())
        self.run_install("--uninstall")
        (assets / "extraspace.apk").write_bytes(b"tampered")
        result = self.run_install("--download-apk", "--no-build", success=False)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertFalse((self.bin_dir / "extraspace").exists())

    def test_download_rejects_other_companion_version(self):
        assets = self.mock_release()
        (assets / "companion-version").write_text("999999\n")
        result = self.run_install("--download-apk", "--no-build", success=False)
        self.assertIn("version differs", result.stderr)
        self.assertFalse(self.bin_dir.exists())

    def test_published_install_uses_matching_source_without_changing_checkout(self):
        assets = self.mock_release()
        source = self.root / "published-source"
        source.mkdir()
        (source / "scripts").mkdir()
        for name in ["install.sh", "install-published.sh"]:
            shutil.copy2(REPO / "scripts" / name, source / "scripts" / name)
        (source / "packaging").mkdir()
        shutil.copy2(REPO / "packaging/io.github.tymonoman.Extraspace.svg", source / "packaging")
        (source / "companion-version").write_text("9\n")
        (source / "README.md").write_text("tested source\n")
        def git(*args):
            return subprocess.check_output(["git", "-C", str(source), *args], text=True,
                                           stderr=subprocess.PIPE).strip()
        git("init", "-b", "main")
        git("config", "user.name", "Installer Test")
        git("config", "user.email", "test@example.invalid")
        git("add", ".")
        git("commit", "-m", "Published version")
        commit = git("rev-parse", "HEAD")
        git("remote", "add", "origin", source.as_uri())
        # Simulate a newer main companion still waiting for its CI build.
        (source / "companion-version").write_text("11\n")
        git("commit", "-am", "New unpublished companion")
        new_head = git("rev-parse", "HEAD")
        (source / "README.md").write_text("local edits must survive\n")
        (assets / "companion-version").write_text("9\n")
        (assets / "tested-commit.txt").write_text(commit + "\n")
        mocks = Path(self.env["PATH"].split(":")[0])
        cargo = mocks / "cargo"
        cargo.write_text('#!/bin/sh\ncat companion-version > "$TEST_BUILT_VERSION"\n')
        cargo.chmod(0o755)
        built_version = self.root / "built-version"
        self.env.update(EXTRASPACE_RELEASE_BASE_URL="https://example.invalid/releases/download",
                        TEST_BUILT_VERSION=str(built_version))
        curl = mocks / "curl"
        curl.write_text(curl.read_text() +
                        "with open(os.environ['TEST_URL_LOG'], 'a') as log: log.write(sys.argv[-3] + '\\n')\n")
        url_log = self.root / "urls"
        self.env["TEST_URL_LOG"] = str(url_log)
        result = subprocess.run([str(source / "scripts/install-published.sh")],
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual(built_version.read_text(), "9\n")
        self.assertEqual(git("rev-parse", "HEAD"), new_head)
        self.assertEqual((source / "README.md").read_text(), "local edits must survive\n")
        self.assertEqual((source / "companion-version").read_text(), "11\n")
        self.assertIn(f"build-{commit}/extraspace.apk", url_log.read_text())
        self.assertEqual((self.root / "data/extraspace/extraspace.apk").read_bytes(), self.apk.read_bytes())
        # Untrusted or incomplete pointers fail before fetching/building sources.
        built_version.unlink()
        (assets / "tested-commit.txt").write_text("main; invalid\n")
        result = subprocess.run([str(source / "scripts/install-published.sh")],
                                env=self.env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Invalid published source commit", result.stderr)
        self.assertFalse(built_version.exists())

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

        # An ordinary upgrade with satisfied dependencies needs no sudo at all.
        (mocks / "dpkg-query").write_text("#!/bin/sh\necho 'install ok installed'\n")
        result = subprocess.run([str(REPO / "scripts/setup.sh")], env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Required packages are installed", result.stdout)
        self.assertNotIn("must-not-run", result.stderr)


if __name__ == "__main__":
    unittest.main()
