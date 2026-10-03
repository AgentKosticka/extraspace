#!/usr/bin/env bash
# Host dependencies. Display-only setup never changes kernel modules.
set -euo pipefail
CHECK_ONLY=0
CAMERA=0
ACCESSORY=0
UNINSTALL=0
for arg in "$@"; do
  case "$arg" in
    --check) CHECK_ONLY=1 ;;
    --camera) CAMERA=1 ;;
    --accessory) ACCESSORY=1 ;;
    --uninstall) UNINSTALL=1 ;;
    --help) echo 'Usage: setup.sh [--check] [--camera] [--accessory] [--uninstall]'; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done
# shellcheck source=scripts/system-setup.sh
source "$(dirname "${BASH_SOURCE[0]}")/system-setup.sh"
if ((UNINSTALL)); then
  ((CAMERA || ACCESSORY)) || { echo 'Choose --camera and/or --accessory with --uninstall.' >&2; exit 2; }
  installed=()
  if ((CAMERA)); then
    installed+=("packaging/extraspace-camera.conf:modprobe.d/extraspace.conf"
                "packaging/extraspace-modules.conf:modules-load.d/extraspace.conf")
  fi
  if ((ACCESSORY)); then installed+=("packaging/70-extraspace-accessory.rules:udev/rules.d/70-extraspace-accessory.rules"); fi
  remove=()
  for entry in "${installed[@]}"; do
    expected=$SYSTEM_REPO/${entry%%:*}
    target=$SYSTEM_ETC/${entry#*:}
    if [[ -e $target ]]; then
      cmp -s "$expected" "$target" || { echo "Refusing to remove modified system configuration: $target" >&2; exit 1; }
      remove+=("$target")
    fi
  done
  if ((${#remove[@]})); then
    if ((CHECK_ONLY)); then printf 'Would remove: %s\n' "${remove[@]}";
    else
      setup_root
      "${ROOT[@]}" rm -f -- "${remove[@]}"
      if ((ACCESSORY)); then "${ROOT[@]}" udevadm control --reload-rules; fi
    fi
  fi
  echo 'Extraspace system setup removed (or previewed). Packages and loaded modules remain; reboot to apply camera autoload changes.'
  exit 0
fi
# Refuse conflicts before package installation or writing any root configuration.
if ((CAMERA)) && [[ -e $CAMERA_DEVICE ]]; then require_camera_owned; fi
OS_RELEASE=${EXTRASPACE_OS_RELEASE:-/etc/os-release}
# shellcheck disable=SC1090
source "$OS_RELEASE"
case "${ID:-} ${ID_LIKE:-}" in
  *ubuntu*|*debian*)
    MANAGER=apt
    PACKAGES=(build-essential pkg-config curl ca-certificates libglib2.0-dev-bin libgtk-4-dev libadwaita-1-dev
      libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libpipewire-0.3-dev
      libclang-dev gstreamer1.0-tools gstreamer1.0-plugins-base
      gstreamer1.0-plugins-good gstreamer1.0-plugins-bad
      gstreamer1.0-plugins-ugly gstreamer1.0-pipewire adb)
    ((ACCESSORY == 0)) || PACKAGES+=(android-sdk-platform-tools-common)
    ((CAMERA == 0)) || PACKAGES+=(v4l2loopback-dkms "linux-headers-$(uname -r)" gstreamer1.0-libav)
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
    ((CAMERA == 0)) || PACKAGES+=(v4l2loopback-dkms gst-libav)
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
ACCESSORY_RULE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/packaging/70-extraspace-accessory.rules
ACCESSORY_RULE_INSTALLED=0
if cmp -s "$ACCESSORY_RULE" "$SYSTEM_ETC/udev/rules.d/70-extraspace-accessory.rules"; then ACCESSORY_RULE_INSTALLED=1; fi
if ((ACCESSORY)); then
  if ((ACCESSORY_RULE_INSTALLED)); then echo 'USB accessory permission rules are installed.';
  else echo 'USB accessory permission rules need installing; reconnect the cable after setup.'; fi
fi
if ((CHECK_ONLY == 0)); then
  ROOT=()
  if ((${#missing[@]})) || { ((CAMERA)) && [[ ! -e $CAMERA_DEVICE ]]; } || { ((ACCESSORY && !ACCESSORY_RULE_INSTALLED)); }; then
    setup_root
  fi
  if ((${#missing[@]})); then
    case "$MANAGER" in
      apt) "${ROOT[@]}" apt-get update; "${ROOT[@]}" apt-get install -y "${missing[@]}" ;;
      dnf) "${ROOT[@]}" dnf install -y "${missing[@]}" ;;
      # Install listed packages only; a full system upgrade belongs to the user.
      pacman) "${ROOT[@]}" pacman -S --needed --noconfirm "${missing[@]}" ;;
    esac
  fi
  if ((ACCESSORY && !ACCESSORY_RULE_INSTALLED)); then
    [[ ! -e $SYSTEM_ETC/udev/rules.d/70-extraspace-accessory.rules ]] || {
      echo 'Refusing to overwrite modified Extraspace accessory rules.' >&2; exit 1;
    }
    "${ROOT[@]}" install -m 0644 "$ACCESSORY_RULE" "$SYSTEM_ETC/udev/rules.d/70-extraspace-accessory.rules"
    "${ROOT[@]}" udevadm control --reload-rules
    echo 'Reconnect the USB cable to activate accessory permissions.'
  fi
  if ((CAMERA)); then
    if [[ -e $CAMERA_DEVICE ]]; then
      require_camera_owned
      echo "$CAMERA_DEVICE is the Extraspace camera; leaving loaded modules alone."
    else
      # Preserve administrator changes rather than replacing same-name files.
      for entry in 'extraspace-camera.conf:modprobe.d' 'extraspace-modules.conf:modules-load.d'; do
        expected=$SYSTEM_REPO/packaging/${entry%%:*}
        target=$SYSTEM_ETC/${entry#*:}/extraspace.conf
        if [[ -e $target ]] && ! cmp -s "$expected" "$target"; then
          echo "Refusing to overwrite modified system configuration: $target" >&2; exit 1
        fi
      done
      "${ROOT[@]}" install -m 0644 "$SYSTEM_REPO/packaging/extraspace-camera.conf" "$SYSTEM_ETC/modprobe.d/extraspace.conf"
      "${ROOT[@]}" install -m 0644 "$SYSTEM_REPO/packaging/extraspace-modules.conf" "$SYSTEM_ETC/modules-load.d/extraspace.conf"
      # Never unload a module that another application might be using.
      "${ROOT[@]}" modprobe v4l2loopback || {
        echo 'Camera module could not load. Check DKMS, kernel headers and Secure Boot/MOK enrollment.' >&2; exit 1;
      }
      [[ -e $CAMERA_DEVICE ]] || { echo "Module is loaded but $CAMERA_DEVICE is unavailable; reboot to apply its configuration." >&2; exit 1; }
      require_camera_owned
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
echo 'Next: ./scripts/install.sh --download-apk (no Android SDK needed), or --build-apk / --apk PATH'
