#!/usr/bin/env python3
"""Exercise the version gate against real Git history."""
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]


class CompanionVersion(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Version Test")
        self.git("config", "user.email", "test@example.invalid")
        (self.root / "companion-version").write_text("14\n")
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.2.0"\n')
        self.git("add", ".")
        self.git("commit", "-m", "base")
        self.base = self.git("rev-parse", "HEAD")

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, text=True,
                                       stderr=subprocess.PIPE).strip()

    def check(self, filename, version=None, success=True):
        path = self.root / filename
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('version = "0.3.0"\n' if filename == "Cargo.toml" else "changed\n")
        if version is not None:
            (self.root / "companion-version").write_text(f"{version}\n")
        self.git("add", ".")
        self.git("commit", "-m", "change")
        result = subprocess.run(["python3", str(REPO / "scripts/check-companion-version.py"),
                                 self.base, "HEAD"], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, success, result.stderr)

    def test_sources_require_bump(self):
        self.check("android/app/src/main/Decoder.kt", success=False)

    def test_bumped_sources_pass(self):
        self.check("android/app/src/main/Decoder.kt", version=15)

    def test_build_changes_require_bump(self):
        self.check("android/build.gradle.kts", success=False)

    def test_protocol_vectors_require_bump(self):
        self.check("protocol/golden-vectors.tsv", success=False)

    def test_public_version_requires_bump(self):
        self.check("Cargo.toml", success=False)

    def test_tests_only_do_not_require_bump(self):
        self.check("android/app/src/test/DecoderTest.kt")

    def test_host_only_does_not_require_bump(self):
        self.check("crates/xs-core/src/main.rs")

    def test_decrease_always_fails(self):
        self.check("README.md", version=13, success=False)


if __name__ == "__main__":
    unittest.main()
