#!/usr/bin/env bash
# One entry point for host setup, Rust build and the published companion APK.
set -euo pipefail
REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SETUP_ARGS=()
INSTALL_ARGS=(--download-apk)
APK_CHOICE=0
while (($#)); do
  case "$1" in
    --camera) SETUP_ARGS+=(--camera) ;;
    --apk) [[ $# -ge 2 && -n $2 ]] || { echo '--apk requires a path' >&2; exit 2; }; INSTALL_ARGS=(--apk "$2"); APK_CHOICE=$((APK_CHOICE + 1)); shift ;;
    --build-apk) INSTALL_ARGS=(--build-apk); APK_CHOICE=$((APK_CHOICE + 1)) ;;
    --help) echo 'Usage: install-ubuntu.sh [--camera] [--apk PATH | --build-apk]'; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
((APK_CHOICE < 2)) || { echo 'Choose --apk or --build-apk, not both.' >&2; exit 2; }
((EUID != 0)) || { echo 'Run this installer as your normal user; setup asks for sudo when needed.' >&2; exit 1; }
# shellcheck disable=SC1091
source /etc/os-release
[[ ${ID:-} == ubuntu ]] || { echo 'This installer is for Ubuntu. Use setup.sh and install.sh on other distributions.' >&2; exit 1; }
"$REPO_ROOT/scripts/setup.sh" "${SETUP_ARGS[@]}"
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null; then
  RUSTUP_SCRIPT=$(mktemp)
  trap 'rm -f "$RUSTUP_SCRIPT"' EXIT
  echo 'Installing the Rust toolchain for your user account…'
  curl --fail --location --show-error --proto '=https' --proto-redir '=https' \
    --tlsv1.2 https://sh.rustup.rs -o "$RUSTUP_SCRIPT"
  sh "$RUSTUP_SCRIPT" -y --profile minimal --default-toolchain stable
fi
"$REPO_ROOT/scripts/install.sh" "${INSTALL_ARGS[@]}"
