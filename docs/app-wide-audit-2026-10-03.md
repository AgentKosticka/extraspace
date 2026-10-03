# Extraspace app-wide audit — 3 October 2026

Follow-up: implementation fixes and subsequent verification are recorded in
[the second-pass report](second-pass-fixes-2026-10-03.md). The findings below
describe the audited state before those fixes.

Audited `/home/l/Desktop/Extraspace/extraspace-agent`, branch
`codex/fix-ubuntu-touch-scaling`, HEAD `16f5172`, including the pre-existing
uncommitted changes. The other two checkouts are older upstream copies.

This is a source and hardware-free runtime audit of the desktop interface,
Android companion, settings, session lifecycle, ADB/AOA transport, video/cursor
handling, camera pipeline, and installation/release paths. It does not certify
physical tablet behavior or compositor compatibility. Application sources were
not changed during this audit.

There are **12 actionable findings: seven bugs and five usability issues**.
P1 means fix promptly because a user can become stuck or lose UI control.
P2 means a concrete functional or usability problem to address next.
Source links below refer to the audited working tree.

## Bugs

### B1 · P1 · Stalled ADB writes can freeze Android's interface

**Trigger:** Use ADB, then stall the host/control reader while touching the
tablet or while its statistics timer runs.

`writeOutput()` runs the write immediately when there is no accessory descriptor.
Both touch handling and `statsTicker` call it from Android's main thread.
`FrameWriter.write()` takes a shared lock and flushes the socket synchronously,
with no bounded write timeout. A blocked write, or waiting behind a blocked pong
writer, therefore blocks Android Back, settings, touch and reconnection UI.

**Evidence:** [ConnectionManager.kt:57](/home/l/Desktop/Extraspace/extraspace-agent/android/app/src/main/kotlin/io/github/tymonoman/extraspace/ConnectionManager.kt:57),
[Protocol.kt:123](/home/l/Desktop/Extraspace/extraspace-agent/android/app/src/main/kotlin/io/github/tymonoman/extraspace/Protocol.kt:123),
[MirrorActivity.kt:78](/home/l/Desktop/Extraspace/extraspace-agent/android/app/src/main/kotlin/io/github/tymonoman/extraspace/MirrorActivity.kt:78).
The AOA path already queues outgoing work; ADB bypasses it.

**Fix:** Put writes for both transports on workers. Bound their queues, preserve
touch down/up ordering, coalesce motion and statistics where appropriate, and
close a stalled connection independently of the main thread. Verify with a
reader that intentionally stops consuming bytes.

### B2 · P1 · A failed display can hide the settings needed to repair it

**Trigger:** Save Mirror mode and disconnect/remove its source monitor, or save
a display setting that prevents the next connection from starting.

Mode, Render Scale and camera controls exist only on the running page. Failed
connections switch to the status page; the menu exposes USB and encoder settings
but no general display settings. The error can say “select a monitor or use
Extend mode” while neither action is available. Try Again repeats the same saved
configuration, including the per-device profile.

**Evidence:** [window.rs:261](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-ui/src/window.rs:261),
[window.rs:350](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-ui/src/window.rs:350),
[window.rs:719](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-ui/src/window.rs:719),
[session.rs:622](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-core/src/session.rs:622).

**Fix:** Make all display settings available before connecting and after failure.
Offer a direct recovery action for an unavailable mirror source and ensure it
updates the device profile as well as the global configuration.

### B3 · P2 · Cursor coalescing can discard shape and hotspot updates

**Trigger:** A cursor shape update followed by a position-only update arrives
before the queued Android cursor flush runs.

`CursorOverlay.submit()` replaces the entire pending message with the newest
delta. The host sends bitmap/hotspot data only on shape changes and clears its
shape-change flag after sending. The position-only delta can overwrite that
bitmap before it is installed. An initial cursor can remain invisible; a later
shape change can retain the wrong sprite/hotspot until another shape arrives.
Main-thread callback ordering does not prevent this: multiple callbacks can
already be queued before the flush is appended.

**Evidence:** [CursorOverlay.kt:52](/home/l/Desktop/Extraspace/extraspace-agent/android/app/src/main/kotlin/io/github/tymonoman/extraspace/CursorOverlay.kt:52),
[cursor.rs:177](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-video/src/cursor.rs:177).

**Fix:** Merge cursor deltas into a complete pending state, retaining the latest
bitmap and hotspot while replacing position. Test shape→move and
shape→hide→move before a single flush.

### B4 · P2 · The camera's 4 MB queue limit is ineffective

**Trigger:** Camera decoding or the virtual webcam sink stops keeping up.

The camera appsrc sets `max_bytes(4 MB)` but leaves blocking and leaking disabled,
does not react to the enough-data signal, and continues pushing buffers. The
comment promises dropping, but this configuration does not enforce that behavior.
Latency and memory can keep growing during downstream stalls.

**Runtime reproduction:** With the locally installed GStreamer, an appsrc using
these settings accepted twelve 1 MB buffers. `current-level-bytes` became
12,582,912 while `max-bytes` remained 4,194,304; every push succeeded.

**Evidence:** [xs-camera/lib.rs:106](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-camera/src/lib.rs:106).

**Fix:** Enforce bounded buffering with a cancellation-safe policy. Because this
queue contains compressed H.264, do not simply discard arbitrary reference
frames: restart/request a keyframe after loss, or put a bounded dropping queue
after decoding.

### B5 · P2 · Camera failures leave the desktop switch looking enabled

**Trigger:** Deny camera permission, use an unavailable/busy camera, encounter
unsupported capture settings, or lose the webcam pipeline.

Android reports permission denial locally but sends no camera failure/status
acknowledgement to the host. Other CameraSource startup/session failures only
log and stop. The Linux camera bus watcher likewise logs asynchronous pipeline
errors without publishing them to the session or UI. The desktop switch records
requested state, so it can remain on while no working camera exists.

**Evidence:** [MirrorActivity.kt:66](/home/l/Desktop/Extraspace/extraspace-agent/android/app/src/main/kotlin/io/github/tymonoman/extraspace/MirrorActivity.kt:66),
[CameraSource.kt:91](/home/l/Desktop/Extraspace/extraspace-agent/android/app/src/main/kotlin/io/github/tymonoman/extraspace/CameraSource.kt:91),
[xs-camera/lib.rs:203](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-camera/src/lib.rs:203).

**Fix:** Distinguish requested, permission-pending, running and failed states.
Return camera status/errors over the protocol and surface host pipeline failures
in the desktop UI with a persistent retry/setup action.

### B6 · P2 · Turning Tablet Camera off keeps the virtual webcam pipeline open

**Trigger:** Enable the camera, receive frames, then disable Tablet Camera while
leaving the display connected.

The toggle sends CameraControl to Android, but the Linux camera reader's local
`V4l2Writer` is never cleared on disable. It stays alive until the entire session
ends or a push fails. The webcam producer/device can remain open with its last
image despite the off switch, and resources remain allocated.

**Evidence:** [session.rs:415](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-core/src/session.rs:415),
[session.rs:1017](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-core/src/session.rs:1017).

**Fix:** Send enable/disable state to the camera worker and explicitly drop its
writer on disable. Gate late incoming frames so they cannot reopen the device.
Confirm that a webcam consumer sees the intended stopped state.

### B7 · P2 · A source install can pair a new host with an old APK indefinitely

**Trigger:** Upgrade host sources and companion-version, then run the source
installer with an old local APK or without supplying a replacement for the
previously installed APK.

The installer finds existing build APKs by filename or retains the installed
APK. Only the download path checks companion-version. The host compares the
tablet's version with its compiled companion-version, installs the chosen APK,
then proceeds without checking the actual installed version. A stale APK can
therefore be “upgraded” to the same old version on every connection. The unchanged
wire protocol version alone does not detect missing implementation fixes.

**Evidence:** [install.sh:70](/home/l/Desktop/Extraspace/extraspace-agent/scripts/install.sh:70),
[install.sh:91](/home/l/Desktop/Extraspace/extraspace-agent/scripts/install.sh:91),
[xs-transport/lib.rs:458](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-transport/src/lib.rs:458).

**Fix:** Inspect and validate the selected APK's package and version before
installing/pairing it with the host. Verify the installed version after upgrade;
surface an actionable incompatibility instead of repeatedly installing stale
bytes. Preserve the existing refusal to uninstall on a signing mismatch.

## Usability issues

### U1 · P2 · Selecting ADB can still change the tablet into USB accessory mode

When ADB is missing, unauthorized or sees no device, transport setup attempts
AOA discovery even for an explicit ADB selector. AOA discovery can issue the USB
mode-switch request before it checks whether that link is allowed for streaming.
This can interrupt the existing USB function and show accessory consent only to
reject the link afterward. It does not bypass the streaming selector, but the
side effect is surprising for someone who specifically chose ADB.

**Evidence:** [xs-transport/lib.rs:270](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-transport/src/lib.rs:270),
[aoa.rs:146](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-transport/src/aoa.rs:146).
**Improvement:** An explicit ADB choice should stop with the relevant ADB setup
message. Offer accessory discovery as an explicit alternative or reserve it for
Automatic/accessory selections.

### U2 · P2 · Connection and consent waits have no visible Cancel action

The Connecting page hides its button, including during APK installation and the
60-second accessory handshake wait. The engine supports interrupting setup, but
the ordinary desktop UI provides no cancel/disconnect control on this page.
Users must quit, change transport, or know about the tray menu.

**Evidence:** [window.rs:685](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-ui/src/window.rs:685),
[session.rs:437](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-core/src/session.rs:437).
**Improvement:** Add Cancel while connecting and Stop Waiting while automatic
discovery is active. Both should return to an editable idle state.

### U3 · P2 · Mirror has no source monitor picker

The backend supports `mirror_source`, but the UI never offers a way to select it.
Without a saved source, it mirrors the first enumerated connector rather than a
user-chosen monitor. Multiple-monitor users cannot reliably choose the screen
they want, and a saved disconnected connector cannot be replaced in the UI.

**Evidence:** [window.rs:261](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-ui/src/window.rs:261),
[session.rs:622](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-core/src/session.rs:622).
**Improvement:** Show a named monitor picker in Mirror mode, identify the primary
monitor, refresh it on topology changes, and explain unavailable saved sources.

### U4 · P2 · Users cannot select the tablet's front camera

Hello advertises camera IDs and facing directions; the host accepts camera_id,
but the desktop only exposes an on/off switch and defaults to ID `0`. There is
no front/back picker, and IDs are not a portable way to infer camera facing.
This makes the webcam feature frustrating for video calls and devices whose
usable camera has another ID. The host also always requests 1920×1080 at 30 fps
without negotiating a supported capture mode.

**Evidence:** [window.rs:287](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-ui/src/window.rs:287),
[session.rs:1115](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-core/src/session.rs:1115),
[DeviceInfo.kt:62](/home/l/Desktop/Extraspace/extraspace-agent/android/app/src/main/kotlin/io/github/tymonoman/extraspace/DeviceInfo.kt:62).
**Improvement:** Offer cameras by facing/name and negotiate supported encoder
surface sizes and frame-rate ranges before starting.

### U5 · P2 · Performance and quality settings require editing files

Frame rate and bitrate bounds are persisted and supported by the engine but
absent from the desktop controls. A user experiencing heat, battery drain or
poor throughput cannot try 30 fps or adjust quality through the app. Encoder
selection alone does not expose those tradeoffs.

**Evidence:** [config.rs:24](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-ui/src/config.rs:24),
[window.rs:250](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-ui/src/window.rs:250),
[session.rs:403](/home/l/Desktop/Extraspace/extraspace-agent/crates/xs-core/src/session.rs:403).
**Improvement:** Add simple frame-rate/quality controls, with more detailed
bitrate settings optional. Describe their effect and keep them available offline.

## Risks requiring device or graphical verification

These are separate from the twelve findings above; they were not reproduced on
physical hardware during this audit.

- **Aspect-ratio alignment:** The SurfaceView and diagnostic TextureView fill
  the screen; neither path sets an aspect-preserving view rectangle/transform.
  Touch and cursor mapping assume centered aspect-fit letterboxing. Test a 16:9
  mirrored monitor on a 16:10 tablet and a rotated source. Video, touch and
  cursor need the same displayed rectangle. See MirrorActivity.kt:367 and :446,
  CursorOverlay.kt:140, and activity_mirror.xml:5.
- **Decoder accepts input but presents nothing:** The host's startup gate
  verifies encoded output, not a decoded/rendered tablet frame. Pong/statistics
  traffic can keep the connection alive even if presentation never starts.
  Exercise a stalled codec and check for a bounded, actionable startup failure.
- **Cancellation after monitor creation:** Interruptible setup drops the
  connection future; explicit `Session::close()` performs physical layout
  restoration, whereas `Session::drop()` relies on D-Bus disconnection. Cancel
  at each setup phase and verify that the virtual monitor and physical layout
  are restored, including while AOA's blocking USB discovery is still running.

Additional polish: several error/setup messages refer to repository scripts that
may not be available to a desktop-launch user; many Android strings are
hardcoded English; the setup screen has no visible streaming Settings affordance
and relies on Android Back. These merit usability testing after the higher
priority fixes.

## Verification and limits

- Rust workspace tests: **108 passed, one graphical test ignored**.
- Installer/release/system-setup/version checks: **30 passed** across their four
  standalone test scripts.
- Android release JVM tests: **15 passed**, fresh test execution; zero failures/errors/skips.
- GStreamer queue reproduction: accepted **12 MB** under the declared **4 MB**
  threshold with blocking/leaking disabled.
- GTK graphical lifecycle tests were not run: this environment has no Xvfb
  executable. No physical tablet, USB function, installed app, compositor
  settings or webcam device was changed for this audit.

The initial generic Python test-discovery invocation passed the 30 unit tests
but also tried to import a standalone GTK script that requires a binary argument.
The correct four standalone unit-test commands were subsequently run and passed.
Android's first invocation lacked ANDROID_HOME; the installed SDK path was then
supplied explicitly. These were harness setup issues, not application failures.

Passing existing tests does not invalidate the findings: the current suite does
not exercise stalled ADB output, Android cursor delta merging, camera worker
enable/disable ownership, or offline recovery controls.

Recommended order: repair B1/B2, then cursor merging and camera lifecycle/state,
then APK validation and USB discovery behavior. Add the missing settings and
cancel/source selectors alongside their corresponding recovery fixes.
