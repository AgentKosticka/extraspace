#!/usr/bin/env bash
# Resolve the moving latest endpoint once; every subsequent fetch uses a fixed tag.
set -euo pipefail
API_URL=${EXTRASPACE_RELEASE_API_URL:-https://api.github.com/repos/AgentKosticka/extraspace/releases/latest}
[[ $API_URL == https://* ]] || { echo 'Release API URL must use HTTPS.' >&2; exit 1; }
curl --fail --location --silent --show-error --retry 3 --connect-timeout 15 --max-time 60 \
  --proto '=https' --proto-redir '=https' "$API_URL" |
  python3 -c 'import json, re, sys
release = json.load(sys.stdin)
tag = release.get("tag_name", "")
if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag) or release.get("draft") or release.get("prerelease") or release.get("immutable") is not True:
    sys.exit("Latest release must be a published immutable versioned release.")
print(tag)'
