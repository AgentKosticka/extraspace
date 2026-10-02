# Android connections and remembered displays — 1 October 2026

Implemented in the Extraspace host and Android companion:

- Persistent installation UUID in Android private preferences, shared by ADB and AOA. Backup/device transfer excludes that identity. Old apps retain the ADB serial fallback; existing layout profiles migrate to UUIDs.
- Per-device Extraspace preferences in `~/.config/extraspace/device-settings.json`: render scale, display mode, source, frame rate, encoder, bitrate limits, and camera choice/state. Failed encoder connections can be repaired without an old device profile overriding the repair.
- GNOME logical scale and placement restoration in `monitor-layouts.json`, after the first captured frame. Physical modes/scales, transforms and topology must still match; the active virtual mode must advertise the saved scale. Existing compositor crash guards stay in place.
- Android setup screen with panel information, identity, connection choice, retry, instructions, offline color/grid/touch check, and access to settings through Android Back. Camera requests Android runtime permission. The screen stays awake during streaming and can sleep while waiting. ADB disconnect leaves this screen usable.
- ADB automatic APK upgrades and fresh activity launches remain. AOA negotiates Google's accessory protocol through libusb, uses Android's system USB permission, and multiplexes the same framed video, input, cursor, camera and telemetry over native bulk endpoints. Reconnect requests Hello again; SessionEnd returns Android to setup. USB output stays off the UI thread; pending USB permission can be cancelled promptly from the PC.
- Experimental device frame selection, off by default: choose the newest already-decoded output when several are ready. H.264 reference inputs are retained. Capture and encoding remain on the PC; this is not an encoding-offload mode or a measured latency improvement.

Validation on the connected Samsung SM-X620, Android 16 / GNOME Wayland:

- ADB: two real video sessions; change GNOME scale to 2× and verify it survives reconnect at unchanged 1920×1200 stream resolution, with positions restored. Original 1× GNOME scale restored afterward.
- AOA: real USB mode negotiation, Android app association prompt, Deny/retry/Allow, decoded video and telemetry in two sessions, the same UUID as ADB, and the same scale/placement reconnect check. Camera frame multiplexing is unit tested; live camera streaming over AOA was not exercised.
- Cancellation while waiting for accessory consent passed; no monitor was left behind.
- 93 Rust unit tests passed, one optional GTK test ignored by the workspace default. Clippy passed with warnings denied.
- Android release APK assembly, lint (0 errors), and two USB framing unit tests passed.
- Nine installer tests and the private-bus tray lifecycle test passed. USB dialog/Quit regression test passed, including Quit while the dialog is open.
- Offline setup and display-check screens were visually inspected on the tablet.

AOA testing temporarily changed the tablet's current USB function. It was returned to its saved charging/ADB setup afterward. Existing USB permissions worked on this PC; no system packages or udev rules were installed. `scripts/setup.sh --accessory` and narrow uaccess rules are supplied for machines that need them.

Your saved GPU encoder and bitrate preferences are retained. Local validation logs and screenshots were retained in an ignored scratch directory. This hardware test does not establish compatibility with every Android vendor or a performance benefit from the experiment.

## Quality, latency and power follow-up

The installed final host and companion 13 include two power fixes:

- Damage-driven input now advertises a variable frame rate to GStreamer, and videorate explicitly caps it at the selected rate before conversion/encoding. Previously, fixed-rate input caps could put videorate into passthrough and feed capture bursts above 60 fps into the encoder/decoder. A regression test submits 120 timestamped frames in one second and verifies about 60 outputs; two frames ten seconds apart still produce only two outputs, with no duplication.
- Android's blocking decoder output wait changed from 20 ms to 250 ms. The codec wakes the wait immediately when output is ready; this cuts idle timeout wakeups from 50 to four per second while retaining the existing 500 ms shutdown budget. Hardware playback was checked after this change.

Tests on Intel i5-1235U/iHD low-power H.264 encoding and Samsung SM-X620 hardware decoding, direct SurfaceView, camera off, 60 fps target, experimental frame selection off:

| Stream | Motion encode throughput | Capture to encoded median / p95 | Decoder submission to release median / p95 |
|---|---:|---:|---:|
| 1920×1200, 30 Mbps ceiling | 59.3 fps | 11.5 / 22.2 ms | 12.7 / 18.7 ms |
| 2880×1800 native, 50 Mbps ceiling | 37.1 fps | 24.6 / 39.8 ms | 18.3 / 24.8 ms |

Throughput uses approximately 33-second moving-bar windows. Latency distributions are from instrumented stream sessions, including startup and subsequent desktop updates. Runs were sequential on the current desktop session, not controlled laboratory benchmarks. GStreamer tracing itself adds overhead. The two stage times are measured on separate clocks and are not a motion-to-photon measurement; neither is the MediaCodec render callback a physical scanout measurement. Decode queue remained at 0–2 frames in the motion windows, without growing backlog. No transport/decoder drops were reported in these two runs; intentional raw-frame rate limiting is excluded from congestion accounting.

Native 2880×1800 with GNOME logical scale 1.5 was confirmed without shrinking the stream; a tablet screenshot was inspected. Full native resolution does not meet the 60 fps target on this setup. The original 1920×1200 / render scale 1.5 / GPU / 6–30 Mbps preferences are retained. Native pixels remain available through Render Scale 1× followed by Ubuntu's display scale. Resolution and frame rate are not silently reduced in response to congestion. H.264's current 4:2:0 video path is compressed and does not provide physical-monitor pixel fidelity for every color/text pattern.

A 106-second native-resolution battery sample covered moving content and idle: USB powered remained true, battery remained 83%, charge counter remained 8,435,240 µAh, and reported net current was zero. Battery temperature was 26.7–26.8°C. Android thermal status stayed normal; observed current skin temperature was approximately 28°C. The panel ran at 60 Hz for the 60 fps stream. Samsung battery protection was active, and charging was paused at its limit. These readings show no reported battery discharge during this short run; they do not quantify USB input power or the PC's additional power consumption. Total power parity with a physical secondary monitor plus USB charging remains unverified, and requires an input power meter and a controlled monitor baseline. No battery protection, brightness or global refresh-rate settings were changed.

Final verification: 93 Rust tests passed (one optional graphical test ignored), Clippy with warnings denied passed, Android release assembly/lint/JVM tests passed, nine installer tests passed, and USB dialog/Quit regression passed. Installed binary/APK hashes match the release outputs; tablet reports companion 13. ADB/AOA identity, consent, scale and placement reconnect tests earlier in this report used companion 12; final companion 13 was additionally exercised with ADB/native and current-resolution motion playback. Live camera over AOA remains untested.

Regular installed session was restarted with the saved preferences; diagnostics/tracing and test windows were closed. This report accompanies the source changes.

## Review fixes — 2 October 2026

Companion 14 removes the extra application consent dialog. Android's attachment
or USB permission prompt grants access; the app verifies the OS permission and
current attachment before opening it. AOA Hello is host-requested on both initial
connection and reconnect. Historical consent tests above exercised companion 12.

Rust shutdown now aborts and joins every child before destroying capture/input
resources. Fixed-width touches reject trailing bytes. CI now enforces the Android
USB framing tests and both graphical lifecycle tests, along with ShellCheck.
Tablet and desktop share an icon, accent palette, section names and connection
labels. Light/dark Android colors and the GTK accent are generated from
`design/theme.json`; CI rejects palette drift.
