#!/usr/bin/env python3
"""Verify camera ownership and reversible setup without touching the host system."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]


class SystemSetup(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.etc = self.root / "etc"
        self.dev = self.root / "dev"
        self.sys = self.root / "sys"
        self.mocks = self.root / "mocks"
        for path in [self.etc / "modprobe.d", self.etc / "modules-load.d", self.etc / "udev/rules.d",
                     self.dev, self.sys / "class/video4linux", self.mocks]:
            path.mkdir(parents=True)
        self.release = self.root / "os-release"
        self.release.write_text('ID=ubuntu\nPRETTY_NAME="Test Ubuntu"\n')
        self.env = dict(os.environ, EXTRASPACE_ETC_ROOT=str(self.etc), EXTRASPACE_DEV_ROOT=str(self.dev),
                        EXTRASPACE_SYS_ROOT=str(self.sys), EXTRASPACE_OS_RELEASE=str(self.release),
                        TEST_COMMAND_LOG=str(self.root / "commands"), PATH=str(self.mocks) + ":" + os.environ["PATH"])
        commands = {
            "sudo": 'if [ "$1" = -n ]; then shift; fi\nexec "$@"',
            "dpkg-query": "echo 'install ok installed'",
            "apt-get": 'echo unexpected-package-install >&2; exit 99',
            "adb": 'exit 0', "pkg-config": 'exit 0', "gst-inspect-1.0": 'exit 0',
            "udevadm": 'echo udev-reload >> "$TEST_COMMAND_LOG"',
            "modprobe": 'echo module-load >> "$TEST_COMMAND_LOG"\n'
                        'touch "$EXTRASPACE_DEV_ROOT/video10"\n'
                        'mkdir -p "$EXTRASPACE_SYS_ROOT/devices/virtual/video4linux/video10"\n'
                        'echo "${TEST_MODULE_LABEL:-Extraspace Tablet Camera}" > "$EXTRASPACE_SYS_ROOT/devices/virtual/video4linux/video10/name"\n'
                        'ln -s "$EXTRASPACE_SYS_ROOT/devices/virtual/video4linux/video10" "$EXTRASPACE_SYS_ROOT/class/video4linux/video10"',
        }
        for name, content in commands.items():
            path = self.mocks / name
            path.write_text("#!/bin/sh\nset -e\n" + content + "\n")
            path.chmod(0o755)

    def run_setup(self, *args, success=True):
        result = subprocess.run([str(REPO / "scripts/setup.sh"), *args], env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, success, result.stderr + result.stdout)
        return result

    def device(self, label, virtual=True):
        (self.dev / "video10").touch()
        location = self.sys / ("devices/virtual/video4linux/video10" if virtual else "devices/pci/video4linux/video10")
        location.mkdir(parents=True)
        (location / "name").write_text(label + "\n")
        (self.sys / "class/video4linux/video10").symlink_to(location)

    def test_conflicting_camera_fails_before_system_changes(self):
        self.device("Other Camera")
        self.run_setup("--camera", success=False)
        self.assertFalse((self.etc / "modprobe.d/extraspace.conf").exists())
        self.assertFalse((self.root / "commands").exists())

    def test_camera_setup_includes_the_required_h264_decoder(self):
        query = self.mocks / "dpkg-query"
        query.write_text('#!/bin/sh\nif [ "$3" = gstreamer1.0-libav ]; then exit 1; fi\necho "install ok installed"\n')
        display = self.run_setup("--check")
        self.assertNotIn("Missing packages: gstreamer1.0-libav", display.stdout)
        camera = self.run_setup("--camera", "--check")
        self.assertIn("Missing packages: gstreamer1.0-libav", camera.stdout)

    def test_physical_camera_with_matching_label_is_rejected(self):
        self.device("Extraspace Tablet Camera", virtual=False)
        self.run_setup("--camera", "--check", success=False)

    def test_owned_camera_is_preserved_without_module_reload(self):
        self.device("Extraspace Tablet Camera")
        self.run_setup("--camera")
        self.assertFalse((self.root / "commands").exists())

    def test_camera_setup_checks_identity_after_loading(self):
        self.env["TEST_MODULE_LABEL"] = "Unexpected Camera"
        self.run_setup("--camera", success=False)

    def test_install_preview_and_remove_system_files(self):
        self.run_setup("--camera", "--accessory")
        paths = [self.etc / "modprobe.d/extraspace.conf", self.etc / "modules-load.d/extraspace.conf",
                 self.etc / "udev/rules.d/70-extraspace-accessory.rules"]
        self.assertTrue(all(p.exists() for p in paths))
        before = (self.root / "commands").read_text()
        result = self.run_setup("--uninstall", "--camera", "--accessory", "--check")
        self.assertIn("Would remove:", result.stdout)
        self.assertTrue(all(p.exists() for p in paths))
        self.assertEqual((self.root / "commands").read_text(), before)
        self.run_setup("--uninstall", "--camera", "--accessory")
        self.assertFalse(any(p.exists() for p in paths))
        self.assertTrue((self.dev / "video10").exists())
        self.assertNotIn("module-unload", (self.root / "commands").read_text())
        self.run_setup("--uninstall", "--camera", "--accessory")

    def test_modified_configuration_prevents_all_removal(self):
        self.run_setup("--camera", "--accessory")
        config = self.etc / "modprobe.d/extraspace.conf"
        config.write_text("administrator configuration\n")
        self.run_setup("--uninstall", "--camera", "--accessory", success=False)
        self.assertEqual(config.read_text(), "administrator configuration\n")
        self.assertTrue((self.etc / "udev/rules.d/70-extraspace-accessory.rules").exists())

    def test_modified_configuration_is_not_overwritten(self):
        config = self.etc / "modprobe.d/extraspace.conf"
        config.write_text("administrator configuration\n")
        self.run_setup("--camera", success=False)
        self.assertEqual(config.read_text(), "administrator configuration\n")


if __name__ == "__main__":
    unittest.main()
