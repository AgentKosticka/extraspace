#!/usr/bin/env bash
# Host dependencies. Display-only setup never changes kernel modules.
set -euo pipefail
CHECK_ONLY=0
CAMERA=0
for arg in "$@"; do
  case "$arg" in
    --check) CHECK_ONLY=1 ;;
    --camera) CAMERA=1 ;;
    --help) echo 'Usage: setup.sh [--check] [--camera]'; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done
OS_RELEASE=${EXTRASPACE_OS_RELEASE:-/etc/os-release}
# shellcheck disable=SC1090
source "$OS_RELEASE"
case "${ID:-} ${ID_LIKE:-}" in
  *ubuntu*|*debian*)
    MANAGER=apt
    PACKAGES=(build-essential pkg-config libgtk-4-dev libadwaita-1-dev
      libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libpipewire-0.3-dev
      libclang-dev gstreamer1.0-tools gstreamer1.0-plugins-base
      gstreamer1.0-plugins-good gstreamer1.0-plugins-bad
      gstreamer1.0-plugins-ugly gstreamer1.0-pipewire adb)
    ((CAMERA == 0)) || PACKAGES+=(v4l2loopback-dkms "linux-headers-$(uname -r)")
    ;;
  *fedora*|*rhel*)
    MANAGER=dnf
    PACKAGES=(gcc pkgconf-pkg-config gtk4-devel libadwaita-devel
      gstreamer1-devel gstreamer1-plugins-base-devel pipewire-devel clang-devel
      gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-plugins-bad-free
      pipewire-gstreamer android-tools)
    ((CAMERA == 0)) || PACKAGES+=(v4l2loopback)
    ;;
  *arch*)
    MANAGER=pacman
    PACKAGES=(base-devel gtk4 libadwaita gstreamer gst-plugins-base
      gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-plugin-pipewire
      gst-plugin-va pipewire clang android-tools)
    ((CAMERA == 0)) || PACKAGES+=(v4l2loopback-dkms)
    ;;
  *) echo "Unsupported distribution: ${PRETTY_NAME:-unknown}. See README for manual dependencies." >&2; exit 1 ;;
esac
printf 'Extraspace setup — %s\n' "${PRETTY_NAME:-$ID}"
printf 'Session: %s / %s\n' "${XDG_CURRENT_DESKTOP:-unknown}" "${XDG_SESSION_TYPE:-unknown}"
[[ ${XDG_CURRENT_DESKTOP:-} == *GNOME* && ${XDG_SESSION_TYPE:-} == wayland ]] ||
  echo 'Warning: streaming requires a GNOME Wayland session.'
missing=()
for p in "${PACKAGES[@]}"; do
  case "$MANAGER" in
    apt) state=$(dpkg-query -W -f='${Status}' "$p" 2>/dev/null || true); [[ $state == 'install ok installed' ]] || missing+=("$p") ;;
    dnf) rpm -q "$p" &>/dev/null || missing+=("$p") ;;
    pacman) pacman -Q "$p" &>/dev/null || missing+=("$p") ;;
  esac
done
if ((${#missing[@]})); then
  printf 'Missing packages: %s\n' "${missing[*]}"
else
  echo 'Required packages are installed.'
fi
if ((CHECK_ONLY == 0)); then
  if ((EUID == 0)); then ROOT=(); else
    command -v sudo >/dev/null || { echo 'Install sudo or run setup as root.' >&2; exit 1; }
    if ! sudo -n true 2>/dev/null; then
      [[ -t 0 ]] || { echo 'Run setup.sh from a terminal for sudo, or use --check.' >&2; exit 1; }
      sudo -v
    fi
    ROOT=(sudo)
  fi
  if ((${#missing[@]})); then
    case "$MANAGER" in
      apt) "${ROOT[@]}" apt-get update; "${ROOT[@]}" apt-get install -y "${missing[@]}" ;;
      dnf) "${ROOT[@]}" dnf install -y "${missing[@]}" ;;
      # Install listed packages only; a full system upgrade belongs to the user.
      pacman) "${ROOT[@]}" pacman -S --needed --noconfirm "${missing[@]}" ;;
    esac
  fi
  if ((CAMERA)); then
    if [[ -e /dev/video10 ]]; then
      echo '/dev/video10 already exists; leaving it and loaded modules alone.'
    else
      "${ROOT[@]}" tee /etc/modprobe.d/extraspace.conf >/dev/null <<'CONF'
options v4l2loopback video_nr=10 card_label="Extraspace Tablet Camera" exclusive_caps=1 max_buffers=2
CONF
      echo v4l2loopback | "${ROOT[@]}" tee /etc/modules-load.d/extraspace.conf >/dev/null
      # Never unload a module that another application might be using.
      "${ROOT[@]}" modprobe v4l2loopback || {
        echo 'Camera module could not load. Check DKMS, kernel headers and Secure Boot/MOK enrollment.' >&2; exit 1;
      }
      [[ -e /dev/video10 ]] || { echo 'Module is loaded but /dev/video10 is unavailable; reboot to apply its configuration.' >&2; exit 1; }
    fi
  fi
fi
command -v cargo >/dev/null || echo 'Rust is missing: install the Rust toolchain (see README).'
if command -v pkg-config >/dev/null; then
  pkg-config --modversion gtk4 libadwaita-1 gstreamer-1.0 libpipewire-0.3 || true
fi
if command -v gst-inspect-1.0 >/dev/null; then
  encoder=0
  for e in vah264lpenc vah264enc x264enc openh264enc; do
    if gst-inspect-1.0 "$e" &>/dev/null; then echo "Encoder available: $e"; encoder=1; fi
  done
  ((encoder)) || echo 'No H.264 encoder found. Fedora may need RPM Fusion gstreamer1-plugins-ugly.'
fi
if command -v adb >/dev/null; then
  adb devices -l
else
  echo 'ADB is missing.'
fi
if ((CHECK_ONLY)); then echo '--check: no packages or system configuration changed.'; fi
echo 'Next: ./scripts/install.sh --build-apk (Android SDK/JDK 17 required), or --apk /path/to/extraspace.apk'
