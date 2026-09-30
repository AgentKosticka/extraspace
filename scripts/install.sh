#!/usr/bin/env bash
# User installation and upgrade. Always rebuild sources unless explicitly skipped.
set -euo pipefail
APP_ID=io.github.tymonoman.Extraspace
REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BIN_DIR=${XDG_BIN_HOME:-$HOME/.local/bin}
DATA_DIR=${XDG_DATA_HOME:-$HOME/.local/share}
CONFIG_DIR=${XDG_CONFIG_HOME:-$HOME/.config}
DESKTOP_DIR=$DATA_DIR/applications
ICON_DIR=$DATA_DIR/icons/hicolor/scalable/apps
NO_BUILD=0
BUILD_APK=0
UNINSTALL=0
APK_SRC=${EXTRASPACE_APK:-}
while (($#)); do
  case "$1" in
    --no-build) NO_BUILD=1 ;;
    --build-apk) BUILD_APK=1 ;;
    --apk) [[ $# -ge 2 && -n $2 ]] || { echo '--apk requires a path' >&2; exit 2; }; APK_SRC=$2; shift ;;
    --uninstall) UNINSTALL=1 ;;
    --help) echo 'Usage: install.sh [--build-apk | --apk PATH] [--no-build] [--uninstall]'; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
if ((UNINSTALL)); then
  rm -f "$BIN_DIR/extraspace" "$DESKTOP_DIR/$APP_ID.desktop" "$ICON_DIR/$APP_ID.svg" \
    "$CONFIG_DIR/autostart/$APP_ID.desktop" "$DATA_DIR/extraspace/extraspace.apk"
  rmdir "$DATA_DIR/extraspace" 2>/dev/null || true
  update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
  echo "Extraspace removed. Settings and logs remain under $CONFIG_DIR/extraspace and the XDG state directory."
  exit 0
fi
[[ -z $APK_SRC || -f $APK_SRC ]] || { echo "APK does not exist: $APK_SRC" >&2; exit 1; }
if ((BUILD_APK)); then
  [[ -z $APK_SRC ]] || { echo 'Choose --build-apk or --apk, not both.' >&2; exit 2; }
  (cd "$REPO_ROOT/android" && ./gradlew assembleRelease --no-daemon)
  APK_SRC=$REPO_ROOT/android/app/build/outputs/apk/release/app-release.apk
fi
if [[ -z $APK_SRC ]]; then
  for candidate in "$REPO_ROOT/android/app/build/outputs/apk/release/app-release.apk" \
    "$REPO_ROOT/android/app/build/outputs/apk/debug/app-debug.apk"; do
    if [[ -f $candidate ]]; then APK_SRC=$candidate; break; fi
  done
fi
if ((NO_BUILD == 0)); then (cd "$REPO_ROOT" && cargo build --release --locked); fi
TARGET_DIR=${CARGO_TARGET_DIR:-$REPO_ROOT/target}
[[ $TARGET_DIR == /* ]] || TARGET_DIR=$REPO_ROOT/$TARGET_DIR
BINARY=$TARGET_DIR/release/extraspace
[[ -x $BINARY ]] || { echo "No release binary: $BINARY" >&2; exit 1; }
mkdir -p "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR" "$DATA_DIR/extraspace"
# Replace by rename so an upgrade never truncates a running executable.
copy_atomic() {
  local src=$1 dest=$2 mode=$3 temp
  temp=$(mktemp "${dest}.XXXXXX")
  if ! install -m"$mode" "$src" "$temp" || ! mv -f "$temp" "$dest"; then
    rm -f "$temp"; return 1
  fi
}
copy_atomic "$BINARY" "$BIN_DIR/extraspace" 755
if [[ -n $APK_SRC ]]; then
  copy_atomic "$APK_SRC" "$DATA_DIR/extraspace/extraspace.apk" 644
elif [[ ! -f $DATA_DIR/extraspace/extraspace.apk ]]; then
  echo 'No companion APK bundled. Install one with --apk PATH; an already installed tablet app can still be used.'
fi
copy_atomic "$REPO_ROOT/packaging/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg" 644
# Desktop Exec has its own quoting rules (not shell syntax), plus entry escapes.
EXEC_PATH=$BIN_DIR/extraspace
EXEC_PATH=${EXEC_PATH//\\/\\\\}
EXEC_PATH=${EXEC_PATH//\"/\\\"}
EXEC_PATH=${EXEC_PATH//\$/\\\$}
EXEC_PATH=${EXEC_PATH//\`/\\\`}
EXEC_PATH=${EXEC_PATH//%/%%}
EXEC_PATH=${EXEC_PATH//\\/\\\\}
DESKTOP_TEMP=$(mktemp "$DESKTOP_DIR/$APP_ID.desktop.XXXXXX")
cat > "$DESKTOP_TEMP" <<ENTRY
[Desktop Entry]
Type=Application
Name=Extraspace
GenericName=Tablet Display
Comment=Use an Android tablet as an extra display and webcam
Exec="$EXEC_PATH"
Icon=$APP_ID
Terminal=false
Categories=Utility;GTK;GNOME;
Keywords=display;monitor;tablet;android;screen;webcam;camera;second screen;
StartupNotify=true
ENTRY
chmod 644 "$DESKTOP_TEMP"
mv -f "$DESKTOP_TEMP" "$DESKTOP_DIR/$APP_ID.desktop"
rm -f "$CONFIG_DIR/autostart/$APP_ID.desktop"
update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
gtk-update-icon-cache -f -t "$DATA_DIR/icons/hicolor" 2>/dev/null || true
echo "Installed Extraspace: $BIN_DIR/extraspace"
echo 'Open Extraspace from the applications grid. Run this installer again to upgrade.'
