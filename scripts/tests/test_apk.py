#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile
from apk_fixture import apk

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("apk_check", REPO / "scripts/check-apk.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ApkChecks(unittest.TestCase):
    def test_utf8_and_utf16_binary_manifest(self):
        with tempfile.TemporaryDirectory() as root:
            for encoding in (True, False):
                path = Path(root) / "sample.apk"
                apk(path, 18, utf8=encoding)
                self.assertEqual(module.check(path, 18), (module.PACKAGE, 18))
    def test_stale_newer_and_wrong_package(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "sample.apk"
            for version, package in [(17, module.PACKAGE), (19, module.PACKAGE), (18, "unrelated.app")]:
                apk(path, version, package)
                with self.assertRaises(ValueError): module.check(path, 18)
    def test_duplicate_manifest_and_invalid_xml(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "sample.apk"
            apk(path, 18)
            with zipfile.ZipFile(path, "a") as archive:
                archive.writestr("AndroidManifest.xml", b"bad")
            with self.assertRaises(ValueError): module.check(path, 18)
            for data in [b"", b"<manifest/>", bytes.fromhex("0300080009000000")]:
                with self.assertRaises(ValueError): module.manifest_info(data)

if __name__ == "__main__": unittest.main()
