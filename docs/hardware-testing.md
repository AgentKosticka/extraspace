# Validation report — 30 September 2026

Changes were based on `sal0-h/extraspace` at `12815fc`, with the implementation
and shared history of `Tymonoman/extraspace` at `b5bfd52` used as a reference.
The fork's low-latency x264 settings, drop-only capture pacing, immediate
MediaCodec presentation and cursor overlay were retained.

## Environment

* Ubuntu 26.04.1 LTS, GNOME Shell 50.1, Wayland
* libmutter-18-0 50.1-0ubuntu2.4 (stock, unpatched)
* Samsung SM-X620, Android 16, USB debugging authorized
* USB 2.0 High Speed, 2880×1800 tablet panel
* GStreamer 1.28.2, GTK 4.22.4, libadwaita 1.9.1, PipeWire 1.6.2
* x264 software encoding; no registered VA H.264 encoder on this host
* Companion version 8 for baseline, built version 9 for final reconnect tests

## Completed checks

* Baseline Rust workspace tests and Android release build passed.
* Two baseline real-device cycles streamed a 1920×1200 virtual monitor at 1.5×.
* Three subsequent real-device cycles verified decoding and correct idle FPS.
  A tablet screenshot confirmed desktop output and cursor rendering.
* The tablet display was moved to the left with all modes/transforms unchanged.
  Subsequent creation restored the virtual output to x=0 and the physical output
  to x=1920. The physical monitor returned to x=0 after teardown.
* Two final cycles forcibly stopped the companion after decoded frames arrived.
  The engine detected closure, removed the output, reconnected, and restored the
  saved placement. An additional `adb reconnect` during streaming exercised the
  USB transport loss path and recovered automatically.
* Decoder queue depth remained at zero in the observed steady-state samples.
  Idle decoded FPS fell to zero instead of falsely reporting the 2 Hz health rate.
  Small numbers of startup/capture drops occurred in some cycles; zero drops is
  not claimed for every run.
* Mirror mode streamed the physical monitor's actual 1920×1080 mode rather than
  negotiating the tablet's 1920×1200 dimensions. The 10-second run decoded over
  300 frames with queue depth zero and no reported drops.
* Teardown left no virtual monitor or Extraspace ADB forwards after the cycles.
* Rust formatting, workspace tests, Clippy with warnings denied, and optimized
  release builds passed. New regression coverage includes counter-based FPS,
  transparent ADB discovery errors, monitor identity/geometry matching, and XML
  compatibility/malformed-input handling.
* Android `assembleRelease` and `lintRelease` passed; version 9 was installed
  with `adb install -r -g`, retaining the existing companion's data.
* Installer tests used isolated XDG directories, including paths with spaces and
  desktop-entry metacharacters, atomic upgrades with an open old binary handle,
  explicit/missing APKs, uninstall/settings retention and Ubuntu `--check`.
* Installed the optimized host, companion APK, icon and desktop entry in the user
  account. Launched through the desktop entry from `/tmp` with isolated test
  preferences; the app found the installed APK, streamed, restored placement,
  exposed its normal Quit action, and wrote its desktop log. Reopening the launcher
  kept the same registered application. Quit removed the monitor and forwards.
  The original user settings were retained.

## Desktop packaging and tray follow-up

* The desktop SVG is generated from the Android adaptive-icon paths and background
  color, and CI checks for drift.
* Added a default-off, persisted Keep Running in Tray option, with Open, Connect,
  Disconnect and Quit in the real StatusNotifier menu. The host does not depend
  on GTK 3/AppIndicator libraries.
* On this GNOME session, the tray registered successfully. Hide Window and
  closing with Ctrl+W kept the same process and virtual monitor running. Tray
  activation and launching the app again restored the same window. Disconnect
  removed the virtual output; Connect recreated it. Quit while hidden removed
  the output and Extraspace ADB forwards.
* A private D-Bus GUI test registered a test StatusNotifier watcher, closed the
  window to the tray, then removed the watcher. The hidden window was restored,
  close without a tray kept it accessible, and explicit Quit exited cleanly.
* Five isolated installer checks passed, including valid release download,
  checksum corruption, companion-version mismatch and conflicting APK options.
* Android release build and lint passed with the persistent signing configuration.
  The existing certificate is stored in an encrypted Actions secret, with explicit
  user approval, to preserve APK upgrade compatibility.

## Limits

These are software/device observations, not a measured motion-to-photon latency
benchmark. Control RTT is shown separately and is not a visual latency claim.
A physical cable unplug/replug and GNOME logout/login were not performed; the
transport-loss test used `adb reconnect`. Persistent profiles are saved to disk
and normalize virtual identity, but logout/login recovery still needs a separate
session test.

GPU encode, other distributions/tablets, camera passthrough through a real
v4l2loopback device, complex clone layouts and multiple virtual outputs were not
validated here. Camera modules were not installed or unloaded for these tests.
Android lint retains pre-existing warnings about orientation/resizability,
backup configuration and obsolete SDK checks; it reports no build-blocking errors.
Stock Mutter's underlying virtual-monitor crashes remain upstream concerns.
The app does not enable native scaled modes on an unverified compositor, and
position restoration deliberately refuses incompatible geometry.
