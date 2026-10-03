#!/usr/bin/env python3
"""Require a monotonic APK version for changes affecting the installed companion."""
import re
import subprocess
import sys


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def check(base, head):
    old = int(git("show", f"{base}:companion-version"))
    new = int(git("show", f"{head}:companion-version"))
    if not 0 < new <= 2100000000 or new < old:
        raise ValueError("companion-version must be positive, valid for Android, and never decrease")
    changed = git("diff", "--name-only", base, head).splitlines()
    affected = [p for p in changed if (p.startswith("android/")
                and not p.startswith("android/app/src/test/")
                and not p.endswith(".md")) or p.startswith("protocol/")]
    # Cargo's public version is embedded in Android's versionName.
    def version(ref):
        text = git("show", f"{ref}:Cargo.toml")
        return re.search(r'^version = "([^"]+)"$', text, re.M).group(1)
    if version(base) != version(head):
        affected.append("Cargo.toml (public version)")
    if affected and new <= old:
        raise ValueError("Android changes require a higher companion-version: " + ", ".join(affected))
    print(f"Companion version verified: {old} -> {new}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("Usage: check-companion-version.py BASE HEAD")
    try:
        check(*sys.argv[1:])
    except (ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
