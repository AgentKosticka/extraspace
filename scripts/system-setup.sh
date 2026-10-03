#!/usr/bin/env bash
# Helpers shared by setup/removal. Roots can be redirected for isolated tests.
# These variables are read by the sourcing setup script.
# shellcheck disable=SC2034
SYSTEM_REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SYSTEM_ETC=${EXTRASPACE_ETC_ROOT:-/etc}
CAMERA_DEV_ROOT=${EXTRASPACE_DEV_ROOT:-/dev}
CAMERA_SYS_ROOT=${EXTRASPACE_SYS_ROOT:-/sys}
CAMERA_DEVICE=$CAMERA_DEV_ROOT/video10
camera_owned() {
  local actual label
  [[ -e $CAMERA_DEVICE ]] || return 1
  actual=$(readlink -f "$CAMERA_SYS_ROOT/class/video4linux/video10") || return 1
  [[ $actual == "$CAMERA_SYS_ROOT/devices/virtual/video4linux/video10" ]] || return 1
  label=$(cat "$CAMERA_SYS_ROOT/class/video4linux/video10/name" 2>/dev/null) || return 1
  [[ $label == 'Extraspace Tablet Camera' ]]
}
require_camera_owned() {
  camera_owned || {
    echo "$CAMERA_DEVICE is not the Extraspace v4l2loopback camera; refusing to use it. Resolve the video10 conflict before running setup.sh --camera." >&2
    return 1
  }
}
setup_root() {
  ROOT=()
  if ((EUID != 0)); then
    command -v sudo >/dev/null || { echo 'Install sudo or run system setup as root.' >&2; return 1; }
    if ! sudo -n true 2>/dev/null; then
      [[ -t 0 ]] || { echo 'Run setup.sh from a terminal for sudo, or use --check.' >&2; return 1; }
      sudo -v
    fi
    ROOT=(sudo)
  fi
}
