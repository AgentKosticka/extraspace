#!/usr/bin/env python3
"""Exercise interruption safety and malformed artifacts without GitHub writes."""
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("publish_release", Path(__file__).resolve().parents[1] / "publish-release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
SHA = "a" * 40


class Publication(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.folder = Path(self.tmp.name)
        (self.folder / "extraspace.apk").write_bytes(b"signed-ci-apk")
        (self.folder / "extraspace.apk.sha256").write_text(hashlib.sha256(b"signed-ci-apk").hexdigest() + "  extraspace.apk\n")
        (self.folder / "companion-version").write_text("14\n")
        (self.folder / "commit.txt").write_text(SHA + "\n")
        self.remote = None
        self.calls = []
        self.corrupt = False
        self.enabled = True

    def gh(self, *args, **kwargs):
        self.calls.append(args)
        if args[0] == "api":
            if args[1].endswith("immutable-releases"):
                return json.dumps({"enabled": self.enabled})
            return json.dumps(self.remote) if self.remote else None
        if args[1] == "create":
            self.remote = {"draft": True, "immutable": False}
        if args[1] == "download":
            target = Path(args[args.index("--dir") + 1])
            for name in release.ASSETS:
                shutil.copy2(self.folder / name, target / name)
            if self.corrupt:
                (target / "extraspace.apk").write_bytes(b"incomplete-upload")
        if args[1] == "edit":
            self.remote = {"draft": False, "immutable": True}
        return ""

    def publish(self, tag="v0.2.0"):
        release.publish(self.folder, tag, SHA, "0.2.0", "14", "owner/repo", self.gh)

    def test_draft_is_verified_before_publication(self):
        self.publish()
        operations = [call[1] for call in self.calls if call[0] == "release"]
        self.assertEqual(operations, ["create", "upload", "download", "edit"])
        self.assertIn("--draft", self.calls[2])

    def test_interrupted_upload_stays_draft_and_can_resume(self):
        self.corrupt = True
        with self.assertRaises(ValueError): self.publish()
        self.assertTrue(self.remote["draft"])
        self.assertFalse(any(call[:2] == ("release", "edit") for call in self.calls))
        self.corrupt = False
        self.calls.clear()
        self.publish()
        self.assertFalse(self.remote["draft"])
        self.assertFalse(any(call[:2] == ("release", "create") for call in self.calls))

    def test_retries_never_mutate_a_published_version(self):
        self.remote = {"draft": False, "immutable": True}
        self.publish()
        self.assertEqual([call[1] for call in self.calls if call[0] == "release"], ["download"])

    def test_mutable_release_and_disabled_immutability_are_rejected(self):
        self.remote = {"draft": False, "immutable": False}
        with self.assertRaises(ValueError): self.publish()
        self.remote = None
        self.enabled = False
        with self.assertRaises(ValueError): self.publish()
        self.assertFalse(any(call[0] == "release" for call in self.calls))

    def test_invalid_version_or_inputs_fail_before_remote_writes(self):
        for tag in ["continuous", "v0.3.0", "v0.2.0;bad"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError): self.publish(tag)
        (self.folder / "commit.txt").write_text("b" * 40)
        with self.assertRaises(ValueError): self.publish()
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
