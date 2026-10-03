# Extraspace second-pass fixes — 3 October 2026

All twelve findings from [the app-wide audit](app-wide-audit-2026-10-03.md) now
have implementation fixes in this checkout. Existing uncommitted work was
preserved. These changes have not been installed on the tablet or published.
The companion version was raised from 17 to **18**.

## Findings addressed

| Finding | Result |
|---|---|
| B1: Android freezes on stalled ADB output | Control and camera output use workers. Submission is nonblocking, queues are bounded, and a separate watchdog closes stalled connections. Adjacent motion/statistics can coalesce; gesture boundaries remain ordered. |
| B2: inaccessible recovery settings | Display & Camera Settings is available offline, while connecting, and after failure. Applying repairs saves global and per-device settings; a connected display restarts once. |
| B3: lost cursor shape/hotspot | Android merges cursor deltas and retains bitmap/hotspot changes through movement and hide/show bursts. |
| B4: ineffective camera queue limit | Compressed camera buffering explicitly rejects pushes beyond 4 MB. A downstream queue can safely discard old decoded pictures. Overflow produces a camera failure rather than silently breaking H.264 references. |
| B5: camera failures look enabled | Permission, startup and runtime errors return over a new camera-status message. Host pipeline errors reach the desktop. Pending/running/failed states have explanations; failure turns the camera off and releases its producer. Failed control writes close the connection before another frame can use a partially written stream. |
| B6: camera remains open after disabling | One worker owns the webcam sink, drops it on disable and ignores late frames. Request changes reset buffered frames and require a fresh keyframe. Android callbacks and queued frames belong to their original camera generation. |
| B7: stale host/APK pairing | The source installer checks the selected or retained APK's package and actual version before replacing app files. The host checks bundled APKs and verifies the installed version after an upgrade. It preserves the refusal to uninstall on a signing mismatch. |
| U1: ADB selection changes USB mode | Explicit ADB returns the relevant ADB result before accessory discovery. |
| U2: missing Cancel | Connecting offers Cancel; discovery/authorization offers Stop Waiting. Both return to an editable idle state. |
| U3: missing mirror picker | A refreshed list names active physical monitors and identifies the primary. Missing saved sources remain explicitly unavailable. Automatic source selection follows the primary. |
| U4: missing camera picker/support negotiation | Front/back/external camera choices use advertised IDs. Android finds a shared Camera2 surface size, frame duration/fps range and H.264 encoder mode, with supported focus and bitrate controls. |
| U5: hidden performance controls | Frame rate and minimum/maximum quality bitrate are editable in the app, with explanations and validation. |

## Additional audit risks and polish

- Both Android video surfaces use a centered aspect-fit rectangle. Video, touch
  and cursor placement share its bounds; black-bar taps are ignored.
- A decoder that produces no first picture fails after eight seconds. Pending
  input without output also fails after three seconds. An idle desktop without
  pending work stays connected.
- Cancelled/failed compositor setup retains its intermediate D-Bus session and
  physical-layout snapshot for explicit cleanup. Cancellation after a completed
  session also awaits its normal close. Blocking accessory discovery checks a
  cancellation flag before further probing and mode-switch requests.
- Android has a visible streaming Settings button. Main status/device/camera
  messages use string resources.
- Camera Setup is reachable from the desktop and gives manual setup guidance
  as well as the source-script option. Camera provisioning now includes the
  required H.264 decoder package on Ubuntu/Debian and Arch.
- Graphical verification caught and fixed an unescaped ampersand in a settings
  row. The settings dialog was rendered and visually checked at 560×720.

## Verification

- Rust workspace: **113 passed**; the four tests requiring a private bus or
  graphical session were then run explicitly and **all four passed** (117 total).
  The intermediate-session test uses a fake D-Bus service and confirms Stop is
  sent before the owning connection is dropped, including idempotent cleanup.
- Android release JVM tests: **22 passed**, zero failures, errors or skips.
- Installer/APK/release/system-setup/version tests: **35 passed**.
- Desktop lifecycle scripts passed for offline USB, display-settings and camera
  setup dialogs, quitting with dialogs open, closing to tray, tray disappearance, startup without a tray,
  accessible fallback, and explicit Quit.
- Rust Clippy with warnings denied, formatting, shell syntax/ShellCheck, icon
  and theme synchronization, and whitespace checks passed.
- Android release build and lint passed (**zero lint errors**, 16 existing
  advisory warnings). APK signature and package/version **18** verified.
- Desktop release build passed. Both release artifacts remain local; no
  installation or publishing was performed.

New regression coverage includes a stalled output worker and watchdog, bounded
output overload, gesture ordering through motion coalescing, cursor bursts,
aspect fit and decoder presentation deadlines, webcam disable/late frames,
asynchronous camera failure, actual compressed queue limits, APK manifest
validation, and recovery/settings persistence.

## Physical verification still needed

No physical tablet, USB function, virtual webcam device, installed application,
or GNOME display configuration was changed during this pass. Hardware-free
checks cannot certify Camera2/codec behavior, USB stalls on this tablet, or
Mutter's actual monitor-removal and layout-restoration behavior.

Before publishing, exercise front and back camera capture, permission denial,
disable/re-enable with a webcam consumer, and a blocked reader on both USB
methods. Mirror a 16:9 monitor and a portrait source on the tablet and check
pointer/touch alignment. Cancel at each compositor setup phase and confirm the
virtual monitor disappears and the physical arrangement is restored. The
companion must be upgraded to version 18 to receive the Android fixes.
