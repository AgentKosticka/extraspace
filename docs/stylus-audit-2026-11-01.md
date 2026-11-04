# Stylus audit and installation — 1 November 2026

Work was performed in `extraspace-agent`, on the existing
`codex/fix-ubuntu-touch-scaling` checkout. Existing uncommitted work was preserved.

## Coordinate findings

The active Samsung SM-X620 uses a 2880×1800 panel and a 1920×1200
video stream. GNOME maps the captured monitor to `(0, 120, 1536, 960)` at
125% scale. The virtual tablet is pinned to that monitor.

Synthetic Android pen taps were traced through the real USB connection and
uinput device to a fullscreen native Wayland application. The installed release
and Android companion 22 produced:

| Android panel position | Expected application position | Observed application position |
| --- | --- | --- |
| (1440, 300) | (768, 160) | (768, 160.00390625) |
| (1440, 900) | (768, 480) | (768, 480) |
| (1440, 1500) | (768, 800) | (768, 799.99609375) |

There is no progressive vertical drift in this measured path. These are
synthetic input checks, not measurements of the physical nib against the panel.

The reported offset was observed in Firefox at
<https://keyboardtester.click/pen-pressure-test.php>; the user also reported
different results between sites and aligned finger touch. A common cause for
those site differences was not established. The website and Firefox were not
modified, and no compensating global coordinate offset or scale was added.

## Cleanup and fixes

- Hover now reaches the pen tracker, enabling palm rejection throughout pen
  proximity. Host hover handling continues to avoid moving the desktop pointer.
- MOVE locates the pen pointer rather than assuming pointer slot zero belongs
  to the pen. This covers a palm entering before the pen.
- Current and historical pen samples share pressure, tilt and position extraction.
  `StylusTracker` lives in its own file instead of the finger tracker file.
- Switching pen/eraser releases the old tool at its last position before moving
  the new tool into proximity.
- A failed uinput write resets the event sequence, so recovery sends fresh
  proximity-in and tip-down reports to the recreated device.
- Monitor mapping refreshes are serialized, gsettings subprocesses run
  asynchronously, and disappearance of the captured output invalidates the
  cached pin. Missing target geometry is reported instead of falling back to
  treating the video frame as the whole desktop.

## Validation

- Rust workspace: 121 tests passed, four environment-dependent tests ignored.
- Clippy across the workspace and all targets: passed with warnings denied.
- Android release assembly and 25 JVM unit tests: passed.
- Android release lint: zero errors/fatal findings, 16 warnings and one
  informational finding in existing manifest/dependency/decoder/resource code.
- Formatting check for `xs-mutter` and `git diff --check`: passed.
- Regression coverage includes full-height mappings at 100%, 125%, 150%, and
  200%, tool-switch positions, and sequence recovery after a failed write.
- Final installed release was launched normally and streamed 1920×1200 at a
  60 fps target. Android reported companion version 22, and the final native
  coordinate checks above passed through that running installed session.

## Installed artifacts

- Host: `/home/l/.local/bin/extraspace`
- Bundled companion: `/home/l/.local/share/extraspace/extraspace.apk`
- Connected tablet: Android companion version 22, installed with `adb install -r`.
- Host SHA-256:
  `291a305317d469728b636a8d049b973c73b5d65775822882f8f75287c85de015`
- APK SHA-256:
  `fc10bc1eecc87054d8c67ddea31a154d45187b8cea4d2767f0916e92ef6c57eb`

Installed PC files match the release outputs. The updated app was left running.
The preceding host/APK and diagnostic logs are under the ignored
`scratch/stylus-audit/` directory. Source changes remain uncommitted.

If offset persists, the outstanding check is a physical pen sweep in a known
native drawing application, followed by the same sweep on the affected Firefox
page at recorded browser zoom. Website-dependent alignment has not been proven
resolved by this cleanup.
