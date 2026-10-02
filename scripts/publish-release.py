#!/usr/bin/env python3
"""Publish a verified, fixed version tag from a draft. Never mutate published assets."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib

ASSETS = ("extraspace.apk", "extraspace.apk.sha256", "companion-version", "commit.txt")


def run_gh(*args, missing_ok=False):
    result = subprocess.run(["gh", *args], text=True, capture_output=True)
    if result.returncode:
        if missing_ok and "HTTP 404" in result.stderr:
            return None
        raise RuntimeError(result.stderr.strip())
    return result.stdout


def verify_bundle(folder, sha, companion):
    if (folder / "commit.txt").read_text().strip() != sha:
        raise ValueError("Release source commit differs from the tested commit")
    if (folder / "companion-version").read_text().strip() != companion:
        raise ValueError("Release companion version differs from source")
    apk = (folder / "extraspace.apk").read_bytes()
    checksum = (folder / "extraspace.apk.sha256").read_text().strip()
    if not apk or checksum != hashlib.sha256(apk).hexdigest() + "  extraspace.apk":
        raise ValueError("Release APK checksum is invalid")


def publish(folder, tag, sha, version, companion, repo, gh=run_gh):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag) or tag != "v" + version:
        raise ValueError("Release tag must match the workspace version")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid source commit")
    verify_bundle(folder, sha, companion)
    if not json.loads(gh("api", f"repos/{repo}/immutable-releases"))["enabled"]:
        raise ValueError("Enable GitHub release immutability before publishing")
    metadata = gh("api", f"repos/{repo}/releases/tags/{tag}", missing_ok=True)
    release = json.loads(metadata) if metadata else None
    if release and not release["draft"] and not release["immutable"]:
        raise ValueError("Existing version is mutable; refusing to overwrite it")
    with tempfile.TemporaryDirectory(prefix="extraspace-release-") as temporary:
        root = Path(temporary)
        notes = root / "notes.md"
        notes.write_text(f"""Extraspace {version}

Source commit: `{sha}`. Companion version: {companion}.
Rust tests (MSRV and stable), Clippy, GTK lifecycle tests, Android unit tests,
lint, APK signature verification, installer tests and ShellCheck passed in CI.

Install the matched source and APK on Ubuntu:

    ./scripts/install-ubuntu.sh --published

This fixed version's source tag and assets are protected by GitHub release
immutability. Builds from main are available as CI artifacts, not releases.

CI run: https://github.com/{repo}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}
""")
        if release is None:
            gh("release", "create", tag, "--draft", "--target", sha,
               "--title", f"Extraspace {version}", "--notes-file", str(notes))
        if release is None or release["draft"]:
            # A failed upload remains a draft. Retry can repair it without touching
            # any published release or a public installation pointer.
            gh("release", "upload", tag, *(str(folder / name) for name in ASSETS), "--clobber")
        downloaded = root / "downloaded"
        downloaded.mkdir()
        gh("release", "download", tag, "--dir", str(downloaded))
        verify_bundle(downloaded, sha, companion)
        for name in ASSETS:
            if (downloaded / name).read_bytes() != (folder / name).read_bytes():
                raise ValueError(f"Uploaded asset differs from CI: {name}")
        if release is None or release["draft"]:
            gh("release", "edit", tag, "--draft=false", "--latest", "--notes-file", str(notes))
        published = json.loads(gh("api", f"repos/{repo}/releases/tags/{tag}"))
        if published["draft"] or not published["immutable"]:
            raise ValueError("GitHub did not confirm an immutable published release")


if __name__ == "__main__":
    project = Path(__file__).resolve().parents[1]
    version = tomllib.loads((project / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    publish(Path(sys.argv[1]), os.environ["GITHUB_REF"].removeprefix("refs/tags/"),
            os.environ["GITHUB_SHA"], version, (project / "companion-version").read_text().strip(),
            os.environ["GH_REPO"])
