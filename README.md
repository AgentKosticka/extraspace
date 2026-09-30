<div align="center">

# Extraspace

**Turn an Android tablet into a real second monitor for GNOME — over USB, with touch.**

[![CI](https://github.com/AgentKosticka/extraspace/actions/workflows/ci.yml/badge.svg)](https://github.com/AgentKosticka/extraspace/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![GNOME](https://img.shields.io/badge/GNOME-46%2B-4A86CF.svg)](https://www.gnome.org)
[![Wayland](https://img.shields.io/badge/Wayland-native-green.svg)](https://wayland.freedesktop.org)

![Extraspace running on an Android tablet](assets/demo.gif)

<sub>Recorded on the tablet itself. That is a GNOME monitor, not a screenshot —
running over a USB cable. Control round trip is separate from visual latency.</sub>

</div>

---

Extraspace makes your Android tablet appear in **Settings → Displays** as a genuine
monitor. Not a screen-share window, not a VNC session — a real output you can drag
windows onto, arrange, and give its own workspaces. Touch the tablet and it drives
the cursor there.

It also sends the tablet's camera back the other way, exposing it to Linux as an
ordinary webcam that Firefox, Zoom, OBS and Cheese pick up automatically.

Everything runs over the **USB cable**. No network, no cloud, no account.

```
┌──────────────────────────┐         ┌────────────────────────┐
│  GNOME / Wayland         │   USB   │  Android tablet        │
│                          │◄───────►│                        │
│  ┌────────┐  ┌────────┐  │         │   ┌────────────────┐   │
│  │  DP-3  │  │ HDMI-1 │  │         │   │   Meta-0       │   │
│  └────────┘  └────────┘  │         │   │  "Virtual      │   │
│  ┌────────────────────┐  │         │   │   remote       │   │
│  │      Meta-0        │──┼─────────┼──►│   monitor"     │   │
│  │  (the tablet)      │◄─┼─ touch ─┼───│                │   │
│  └────────────────────┘  │         │   └────────────────┘   │
│  /dev/video10 ◄──────────┼── cam ──┼───  camera             │
└──────────────────────────┘         └────────────────────────┘
```

## Why this exists

Most "tablet as second screen" tools on Linux mirror an existing display, or need
X11, or route video over Wi-Fi with the latency that implies. On GNOME Wayland
specifically, creating a *new* output has historically meant DisplayLink drivers
or the EVDI kernel module.

It turns out mutter can already do it. `org.gnome.Mutter.ScreenCast` has a
`RecordVirtual` method that creates a monitor with no backing hardware, and
`org.gnome.Mutter.RemoteDesktop` can inject touch events whose coordinates are
*relative to that stream* — so input lands on the right monitor with no geometry
maths at all. Extraspace is a well-behaved GNOME app wrapped around those two APIs.

## Status

This fork builds on [sal0-h/extraspace](https://github.com/sal0-h/extraspace),
including its low-latency frame pacing, independent decoder drain, cursor overlay,
and GPU capture/encoding path. The original implementation is
[Tymonoman/extraspace](https://github.com/Tymonoman/extraspace).

The current validation environment is Ubuntu 26.04.1, GNOME Shell 50.1 / Mutter
50.1-0ubuntu2.4, Wayland, and a Samsung SM-X620 running Android 16 over USB 2.0
High Speed, with a 2880×1800 panel. The default 1.5× setting sends 1920×1200
pixels through x264 on this machine. Native GNOME scaling requires a verified
patched Mutter; it is deliberately not enabled on stock Ubuntu.

Display streaming, touch, repeated connection cycles, and conservative monitor
placement restoration are supported. Camera passthrough is optional and requires
v4l2loopback; see the hardware test report for the exact coverage and limitations.
Other hardware and distributions still need validation.

The statistics panel reports **Control RTT**, **Decoded Frame Rate**, bitrate,
and encoder. Control RTT includes the USB/control path, but excludes capture,
encoding, decoding and display presentation. It is **not visual latency**.
Decoded FPS counts frames released by MediaCodec; a still desktop can correctly
show 0 fps. An external camera / timer experiment is needed for actual
motion-to-photon latency; the app does not claim to measure it.

### Verifying it without a tablet

Everything above the tablet's decoder can be exercised on one machine, which is
also how you develop this if you do not have an Android device to hand.

**The whole host pipeline, against a simulated tablet:**

```console
$ cargo run -p xs-core --example fake_tablet
simulated tablet listening on 27183/27184/27185
  [tablet] sent Hello (2000x1200)
  [host] Creating the display…
  [host] streaming 1332x800 via OpenH264 (software, fallback)
  adapting bitrate new_kbps=7393
  adapting bitrate new_kbps=8393

--- simulated tablet results ---
  first video frame   152 ms after start
  video frames        57
  keyframes           3
  pings answered      20
  touch events sent   21

  PASS
```

That covers session orchestration, the handshake, virtual monitor creation,
capture, encoding, framing over real sockets, touch injection into the
compositor, and the adaptive controller probing upward on healthy samples. It
does not cover MediaCodec or anything USB-specific.

**Just the capture half, writing a file you can inspect:**

```console
$ cargo run -p xs-video --example capture_test
```

It writes `/tmp/extraspace-capture.h264` — `ffprobe` confirms Constrained
Baseline 1332×800, and `ffmpeg -i … -f null -` decodes every frame without error.

If you try it and something breaks, an issue with `RUST_LOG=debug` output is very
welcome.

## Requirements

- **GNOME 46 or newer on Wayland.** This is not portable to other compositors:
  it depends on mutter-specific D-Bus APIs that KDE, Sway and friends do not have.
  GNOME 50+ additionally lets the monitor be pinned to an exact mode.
- An **Android 11+** tablet (API 30, for `MediaCodec` low-latency decoding).
- A **USB cable** and USB debugging enabled on the tablet.
- Any GPU. Encoding is done on the CPU by default and needs roughly one core.

## Getting started

Four steps, once. After that it is plug in the cable and open the app.

### 1. Turn on USB debugging, on the tablet

This is the step everyone forgets, and it is why most first runs fail.

1. **Settings → About tablet** → tap **Build number** seven times.
2. **Settings → System → Developer options** → turn on **USB debugging**.
3. Plug in the USB cable. Accept the *Allow USB debugging?* prompt,
   ticking **Always allow from this computer**.

Skip step 3 and Extraspace will tell you exactly that, by name, rather than
failing with something cryptic.

> A surprising number of USB cables are charge-only and carry no data. If your
> tablet charges but never appears, try a different cable before anything else.

### 2. Install on Ubuntu (recommended)

On **Ubuntu 24.04 or newer with GNOME on Wayland**, run:

```bash
git clone https://github.com/AgentKosticka/extraspace
cd extraspace
./scripts/install-ubuntu.sh
```

If `git` is missing, install it first with `sudo apt install git`.
The installer asks for sudo to install Ubuntu dependencies, installs a user Rust
compiler if needed, builds the desktop app, and downloads the published companion
APK with a SHA-256 check. **No Android SDK or JDK is needed.** The application,
APK, matching Android/desktop icon, and launcher are installed in your user
account. Do not run the installer with sudo.

After installation, press **Super**, type **Extraspace**, and click its icon.
You can also find it in GNOME's applications grid. No terminal or repository
working directory is needed after installation. Opening it again presents the
existing window. It does not autostart at login.

For an upgrade, close Extraspace, then run:

```bash
git pull --ff-only
./scripts/install-ubuntu.sh
```

To include the optional tablet webcam, add `--camera`. This installs v4l2loopback
DKMS and headers for your running kernel. Secure Boot may require MOK enrollment.
Display-only installation leaves camera modules alone.

The [latest tested APK](https://github.com/AgentKosticka/extraspace/releases/download/continuous/extraspace.apk)
is also available separately. Pushes to `main` run Rust checks and an Android
release build, lint and signature verification. Only after both jobs pass does
CI publish the APK, checksum, companion version and source commit to the
[continuous release](https://github.com/AgentKosticka/extraspace/releases/tag/continuous).
This is a development prerelease; `v*` tag pushes produce versioned releases.
If the published companion version differs from your checkout, installation
stops and asks you to update sources or build the APK locally.

### 3. Other distributions and source APK builds

For Fedora/Arch, or to manage setup separately:

```bash
./scripts/setup.sh --check
./scripts/setup.sh
./scripts/install.sh --download-apk
```

Setup detects Ubuntu/Debian, Fedora and Arch package names and installs GTK,
libadwaita, PipeWire/GStreamer headers, encoder plugins and ADB. Outside the Ubuntu
installer, install Rust/Cargo yourself. The GUI requires libadwaita 1.5+ and
GNOME 46+ on Wayland. Fedora may need RPM Fusion for x264. For camera support,
add `--camera` to setup; Arch DKMS users need headers for their actual kernel.

To build the companion yourself, install JDK 17 and an Android SDK containing
platform/build tools 35, then run:

```bash
export ANDROID_HOME="$HOME/Android/Sdk" # adjust to your SDK location
./scripts/install.sh --build-apk
```

Or use a specific APK with `./scripts/install.sh --apk /path/to/extraspace.apk`.
The Ubuntu installer accepts `--apk PATH` and `--build-apk` as alternatives to
its default download. `--download-apk`, `--build-apk` and `--apk` are mutually
exclusive. The normal installer always rebuilds with `Cargo.lock`; `--no-build`
explicitly uses an existing release binary. Files are replaced by rename, so an
upgrade does not truncate a running executable. A previously installed APK is
retained when no new APK is supplied. `EXTRASPACE_APK=/path/to/app.apk` also works
for a source launch; an invalid override is reported instead of silently ignored.

The host installs a missing companion or replaces an older version while
preserving its data. It never silently uninstalls an app to bypass a signing-key
mismatch and does not downgrade a newer app. Host and Gradle read the same
`companion-version` file. Developer changes to the companion should increment
that number, or be installed explicitly with `adb install -r -g PATH.apk`.

Remove the user installation with `./scripts/install.sh --uninstall`. Your
settings, placement profiles, and logs remain. No system packages are removed.

### 4. Use it

Connect the USB cable, accept Android's USB debugging prompt, and open Extraspace.
It waits for an authorized tablet, creates the virtual display, and starts
streaming. Use **Settings → Displays** to arrange the tablet. If the connection
is lost, the app tears down the display and tries to reconnect. Turning Extra
Display off stops reconnecting; use **Connect** to start again.

Placement is stored under `$XDG_CONFIG_HOME/extraspace/monitor-layouts.json`
(default `~/.config/extraspace`). Profiles are separated by tablet identity,
physical outputs and active monitor geometry. Once capture produces a real
frame, an exact match can restore positions and primary display. On stock Mutter
this does not change resolution, scale, orientation or enabled outputs. A changed
monitor set or mode falls back to GNOME's layout rather than forcing an old one.
Arrange that configuration once to create its own profile. Clone groups and
multiple simultaneous virtual monitors are currently excluded.

GNOME's `monitors.xml` is still checked for incompatible virtual-monitor modes,
scales, rotations or disabled virtual outputs. These can leave a virtual CRTC
unconfigured and crash stock Mutter. Compatible configurations and physical-only
layouts are retained; before incompatible entries are removed, the original file
is backed up as `monitors.xml.extraspace-bak`. Malformed or unknown XML is never
rewritten. GNOME also keeps configurations in memory, so editing the file is not
a complete compositor fix. The existing scaled-mode/patched-Mutter gate and
fixed PipeWire negotiation remain in place. See
[the monitor investigation](docs/monitor-persistence.md).

### Running in the tray

Open the main menu (**☰**) and enable **Keep Running in Tray**. Closing the
window (or pressing **Ctrl+W**) then hides it while the display and reconnect handling continue running.
The tray menu has **Open Extraspace**, **Connect**, **Disconnect** and **Quit**.
**Hide Window** hides it immediately; opening Extraspace from the applications
grid also restores the existing window. **Quit** or **Ctrl+Q** always stops the
session and removes the virtual monitor, even with tray mode enabled.
The preference is saved and is off by default.

GNOME needs the **AppIndicator and KStatusNotifierItem Support** extension.
Ubuntu normally includes it; enable **Ubuntu AppIndicators** in the Extensions
app if necessary. On other GNOME installations, install the extension supported
by your distribution. If there is no tray, the window stays accessible. If tray
support disappears while the window is hidden, Extraspace presents it again.

## Usage

| Setting | What it does |
|---|---|
| **Mode** | *Extend* adds a new monitor. *Mirror* copies an existing one. |
| **Scale** | How large the desktop is drawn on the tablet. Stock Mutter uses a smaller framebuffer; patched Mutter can use native GNOME scaling. |
| **Tablet Camera** | Feeds the tablet camera into `/dev/video10`. |

The camera appears as **“Extraspace Tablet Camera”** in Firefox, Zoom, OBS,
Cheese and anything else that reads a webcam. It only shows up in those lists
while Extraspace is actually running, so it does not clutter your camera picker
the rest of the time.

### About scale

A 10.4" tablet at its native 2000×1200 renders GNOME at a size that is technically
correct and practically unreadable. Extraspace handles this by creating the
virtual monitor *smaller* than the panel and letting the tablet upscale:

| Scale | Monitor created | Result |
|---|---|---|
| 1× | 2000 × 1200 | Pin-sharp, very small text |
| **1.5×** (default) | 1332 × 800 | Comfortable — the sweet spot |
| 2× | 1000 × 600 | Large text, noticeably soft |

Higher scale also costs less bandwidth, because there are fewer pixels to encode.

## How it works

The interesting part is the sequence, which is not obvious from mutter's interface
XML and took some experimentation to get right:

1. `RemoteDesktop.CreateSession()` → read its `SessionId`.
2. `ScreenCast.CreateSession({"remote-desktop-session-id": id})`. **Linking the two
   sessions is what makes injected input land on the virtual monitor.**
3. `ScreenCast.Session.RecordVirtual({ modes, is-platform, cursor-mode })`.
   - `is-platform: true` makes it a real monitor rather than a shared surface.
   - `modes` pins it to an exact resolution so PipeWire cannot renegotiate it.
4. Subscribe to `PipeWireStreamAdded` **before** starting, or the signal is missed.
5. `RemoteDesktop.Session.Start()` — *not* `ScreenCast.Session.Start()`, which
   mutter rejects for a linked session with *"Must be started from remote desktop
   session"*. Teardown is symmetric.

From there it is a normal GStreamer pipeline:

```
PipeWire capture + cursor overlay → appsrc → videorate (drop-only) → vapostproc → vah264lpenc → h264parse → appsink → USB
```

Frames are requested from mutter as dma-bufs, so they stay on the GPU from
compositing through to encode. Where the VA-API plugin is missing, the same
pipeline runs with `videoconvert → x264enc` over copied frames instead.

and on the tablet, `MediaCodec` → `TextureView`. Touches travel back on a separate
socket and become `NotifyTouchDown/Motion/Up` calls, whose coordinates are already
in the virtual monitor's space.

### Notes from building it

Things that cost time, recorded so they cost you less:

- **`vulkanh264enc` silently ignores its bitrate setting.** It advertises CBR and
  accepts the property, but produces byte-identical output at 5, 15 and 40 Mbit/s.
  Unusable for adaptive streaming. Extraspace does not offer it.
- **Fedora strips NVENC out of GStreamer's `nvcodec` plugin.** Only the CUDA
  utility elements register; `nvh264enc` does not exist, and `plugins-freeworld`
  does not add it. `x264enc` at `veryfast` manages ~168 fps at 2000×1200 on a
  mid-range CPU, which is 2.8× more than needed, so this matters less than it sounds.
- **`x264enc` takes kbit/s but `openh264enc` takes bit/s.** A 1000× error waiting
  to happen; the conversion lives in exactly one function.
- **At panel resolution the cost is moving pixels, not encoding them.** A
  2296×1428 frame is 13 MB, and the software chain moved it through DRAM four
  times: mutter's readback, our copy, `videoconvert`, then x264. Measured on an
  i7-1355U, `videoconvert` plus x264 cost 22.4 ms per frame — a 45 fps ceiling
  before mutter did any work — and going from 2 to 6 x264 threads, or down to
  `ultrafast`, recovered barely a tenth of it. VA-API encode of the same frames
  costs about 1 ms, but swapping *only* the encoder gains nothing, because
  uploading the frame costs what the conversion did. Taking dma-bufs from mutter
  removes all four passes at once.
- **A dma-buf's row pitch is not `width * 4`.** The GPU pads each row out to a
  tile boundary: 2296 pixels are allocated as 9216 bytes rather than 9184, and
  1316 as 5376 rather than 5264. Letting GStreamer infer the pitch from the caps
  shifts every row and shears the image diagonally, so the pitch mutter reports
  is attached to each buffer as a `GstVideoMeta`. Widths that happen to be
  aligned — 1920 is exactly 15 tiles — look perfect either way, which makes this
  a bug that only appears at the resolutions that matter. Verify at those.
- **A slow consumer throttles the compositor.** Mutter will not produce a new
  frame until the previous buffer comes back, so encoding speed sets the capture
  rate rather than merely following it. Under one animated load at 1920×1080,
  mutter delivered 14.5 fps to the software chain and 39.4 fps to the GPU one.
- **USB 2.0 is not the bottleneck.** Raw 2000×1200@60 would need ~550 MB/s, far
  beyond the ~30 MB/s a High Speed link gives you. Encoded H.264 at 15 Mbit/s is
  under 2 MB/s — roughly 15× headroom.
- **`adb forward` accepts connections to nothing.** adb accepts the *local* TCP
  connection whether or not anything is listening on the device, then closes it
  once the remote open fails. So `connect()` succeeds on the first attempt even
  when the companion app has not started, and the failure appears milliseconds
  later as an unexplained EOF mid-handshake. Retrying on connection-refused never
  helps, because connection-refused never happens. The only reliable readiness
  signal is bytes.
- **Half-closing one direction kills the whole channel.** Tokio's
  `OwnedWriteHalf` calls `shutdown(Write)` when dropped, and adb's forwarder tears
  down the entire unix socket to the device when either direction closes. Splitting
  a read-only stream and dropping the unused write half is therefore fatal — the
  peer's next write gets EPIPE. Streams used one way are not split at all here.
- **Mutter's embedded cursor only updates when the desktop is damaged.** Pointer
  motion is a hardware plane, not damage, so a still window freezes the cursor.
  Metadata mode plus a single PipeWire consumer that blits `SPA_META_Cursor` is
  the fix. A second consumer (`pipewiresrc` plus a listener) makes
  gst-plugin-pipewire abort on unfixed caps. A fullscreen GTK "damage pump"
  becomes an opaque black window on Meta-0.
- **Default `videorate` invents frames.** On a damage-driven capture (idle ~11
  fps) it duplicates the last buffer to fill 60 fps holes and builds seconds of
  fake catch-up latency. The pipeline is drop-only.
- **Draining MediaCodec only when input arrives loses the last frame.** Because
  mutter sends only on damage, an idle desktop delivers nothing for seconds at a
  time; if output is drained inside the input path, the final frame stays decoded
  but unrendered until something else changes. A dedicated drain thread fixed both
  that and the queue-depth telemetry, which had been reporting a stuck 6–8 and
  driving the bitrate to the floor for entire sessions.
- **"60 fps" is a ceiling, not a rate.** Mutter only emits a frame when something
  on the monitor actually changes, so a still desktop measures around **11 fps**
  and well under 1 Mbit/s. That is exactly what you want — an idle screen should
  be nearly free — but it quietly breaks anything that reasons in frame counts.
  Both encoders express their keyframe interval in frames, so the obvious
  `framerate × 2` puts keyframes *ten seconds* apart while idle, and a tablet
  that reconnects sits on a black screen until one arrives. The interval is sized
  against the idle rate instead, and a keyframe is requested explicitly whenever a
  tablet attaches.

## Troubleshooting

**"No tablet found"** — check `adb devices`. If it is empty, the cable may be
charge-only; many are. If it says `unauthorized`, accept the prompt on the tablet.

**"Something went wrong: no usable H.264 encoder"** — run
`./scripts/setup.sh`. Ubuntu uses `gstreamer1.0-plugins-ugly` for x264; Fedora uses
`gstreamer1-plugins-ugly` from RPM Fusion.

**The virtual camera does not appear** — run `./scripts/setup.sh --camera` to create
`/dev/video10`. Note that `exclusive_caps=1` deliberately hides it from
applications while nothing is feeding it, so it only shows up once streaming starts.

**The tablet shows a black screen** — check `adb logcat -s extraspace`. A protocol
mismatch is reported explicitly on both sides.

Desktop launches retain logs in `$XDG_STATE_HOME/extraspace/extraspace.log`
(default `~/.local/state/extraspace/extraspace.log`), with one previous log and a
5 MiB limit for each. `RUST_LOG=debug` enables detailed pacing/health logs. Logs
can contain device identities and touch coordinates; review them before sharing.

For a diagnostic report without a window:

```bash
extraspace --diagnostics
# or from a source build:
./target/release/extraspace --diagnostics
```

This reports session type, GNOME and ADB versions, encoder availability, companion
path, settings/log locations and optional camera status. The device list omits
serial numbers. Detailed Android errors are available with `adb logcat -s extraspace`.
A signing-key mismatch requires choosing a matching APK or intentionally removing
the old companion yourself (which deletes its app data).

## Roadmap

- [ ] Keyboard passthrough (`NotifyKeyboardKeycode` — the plumbing is already there)
- [ ] Stylus with pressure, for tablets with an active pen
- [ ] Wi-Fi transport, reusing the same protocol
- [ ] Audio to the tablet speakers
- [ ] Tablet microphone as a PipeWire source
- [ ] Camera controls: tap-to-focus, torch, zoom
- [ ] Follow tablet rotation live

## Development

```bash
cargo test                                          # unit tests, no hardware needed
cargo run -p xs-mutter --example virtual_monitor    # create a monitor for 5 seconds
RUST_LOG=debug cargo run                            # verbose
python3 scripts/tests/test_install.py               # isolated installer/setup checks
dbus-run-session -- /usr/bin/python3 scripts/tests/test_tray.py ./target/release/extraspace # GUI tray lifecycle
cargo run --release -p xs-core --example device_cycle -- 3 10  # real-device cycles
XS_TEST_LOSS=1 cargo run --release -p xs-core --example device_cycle -- 2 20
cd android && ./gradlew assembleRelease lintRelease # companion build + lint
```

| Crate | Responsibility |
|---|---|
| `xs-proto` | Wire protocol, shared with the Kotlin side |
| `xs-mutter` | Virtual monitor + input injection over D-Bus |
| `xs-video` | PipeWire capture → H.264 |
| `xs-transport` | adb orchestration, framed sockets |
| `xs-camera` | H.264 → `/dev/video10` |
| `xs-core` | Session orchestration, adaptive bitrate |
| `xs-ui` | GTK4 / libadwaita front end |

`xs-mutter` is the only crate that touches mutter's private D-Bus API, so if a
future GNOME release changes it, the damage is contained to one file.

### APK releases and signing

`.github/workflows/ci.yml` builds and verifies `extraspace.apk`, then calls the
reusable release workflow with that same artifact. Failed Rust/Android checks or
pull requests cannot publish releases. Each release includes `extraspace.apk`,
`extraspace.apk.sha256`, `companion-version` and `commit.txt`.

Maintainers must configure the encrypted repository secret
`EXTRASPACE_KEYSTORE_BASE64` with a persistent base64-encoded Android keystore
(alias `androiddebugkey`, key/store password `android`). This fork is configured
with the key matching the tested tablet installation. The workflow fails rather
than publishing an APK with a fresh random debug key. The key is restored only
for trusted main/tag builds, kept outside the checkout, and removed afterward.
Never commit a keystore. Android updates require the same signing certificate.

Local builds use your Android debug key by default. For another signing key,
Gradle accepts `EXTRASPACE_KEYSTORE`, `EXTRASPACE_KEYSTORE_PASSWORD`,
`EXTRASPACE_KEY_ALIAS` and `EXTRASPACE_KEY_PASSWORD`. A local APK with a different
certificate cannot replace an installed published APK without intentionally
uninstalling the old app and losing its data. Increment `companion-version` when
changing the Android implementation so the desktop app upgrades it automatically.

The desktop icon is generated from Android's adaptive-icon vector and background
color. After changing those resources, run `python3 scripts/sync-icon.py` and
commit the SVG. CI checks that the two stay in sync.

## Contributing

Issues and pull requests welcome. The most useful thing right now is testing on
other tablets and other GNOME versions — please include your GNOME version,
distribution, and tablet model.

## License

[GPL-3.0-or-later](LICENSE).

Note that the default encoder, x264, is itself GPL-licensed, so a distributed
binary would carry GPL obligations regardless. Licensing the project this way
keeps the situation unambiguous.
