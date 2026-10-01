#!/usr/bin/env bash
# Build the last published, tested source with its immutable companion release.
set -euo pipefail
REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
(($# == 0)) || { echo 'Usage: install-published.sh' >&2; exit 2; }
for command in curl git tar; do
  command -v "$command" >/dev/null || { echo "Install $command to install a published build." >&2; exit 1; }
done
RELEASE_BASE=${EXTRASPACE_RELEASE_BASE_URL:-https://github.com/AgentKosticka/extraspace/releases/download}
[[ $RELEASE_BASE == https://* ]] || { echo 'Release URL must use HTTPS.' >&2; exit 1; }
PUBLISHED_DIR=$(mktemp -d)
trap 'rm -rf "$PUBLISHED_DIR"' EXIT
curl --fail --location --silent --show-error --retry 3 --connect-timeout 15 --max-time 60 \
  --proto '=https' --proto-redir '=https' "$RELEASE_BASE/continuous/tested-commit.txt" -o "$PUBLISHED_DIR/commit.txt"
COMMIT=$(cat "$PUBLISHED_DIR/commit.txt")
[[ $COMMIT =~ ^[0-9a-f]{40}$ ]] || { echo 'Invalid published source commit.' >&2; exit 1; }
echo "Installing tested source $COMMIT with its matching APK…"
# Fetch objects without switching branches or changing the user's working files.
git -C "$REPO_ROOT" fetch --no-tags origin "$COMMIT"
mkdir "$PUBLISHED_DIR/source"
git -C "$REPO_ROOT" archive "$COMMIT" | tar -x -C "$PUBLISHED_DIR/source"
# Reuse compiled dependencies across published installs instead of discarding
# a full GTK build when the temporary source tree is removed.
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/extraspace/target}
[[ $CARGO_TARGET_DIR == /* ]] || export CARGO_TARGET_DIR="$REPO_ROOT/$CARGO_TARGET_DIR"
export EXTRASPACE_RELEASE_URL="$RELEASE_BASE/build-$COMMIT"
"$PUBLISHED_DIR/source/scripts/install.sh" --download-apk
