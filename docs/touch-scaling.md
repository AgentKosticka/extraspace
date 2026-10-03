# Ubuntu display scaling and Android touch — 3 October 2026

Fixed and installed from `/home/l/Desktop/Extraspace/extraspace-agent`.

The companion correctly sends coordinates in the 1920×1200 video frame. Mutter's
RecordVirtual input transformation adds the stage-view origin without dividing
by the monitor scale. At the user's 125% Ubuntu scale, the corresponding logical
screen is 1536×960: a tablet centre tap reached GTK at approximately (960, 600)
instead of (768, 480). This reproduces the reported down/right displacement.

The host now divides virtual-stream touch-down, touch-motion, and absolute-pointer
coordinates by the active logical scale. A MonitorsChanged subscription refreshes
the cached scale, including mode-less monitor creation and live scale changes;
saved-layout restoration also refreshes it explicitly. Motion events only read
the cache, with no extra per-event display query. Physical monitor captures keep
their original input path because RecordMonitor already applies its own scale.
Physical-coordinate display layouts use a factor of one. The watcher is aborted
when its owning session is dropped. No Android or protocol change was required.

The upstream coordinate behavior was checked in Mutter 50.1:
[virtual stream](https://github.com/GNOME/mutter/blob/50.1/src/backends/meta-screen-cast-virtual-stream.c)
and [monitor stream](https://github.com/GNOME/mutter/blob/50.1/src/backends/meta-screen-cast-monitor-stream.c).

Validation on the connected Samsung SM-X620 over ADB:

- Reproduced the centre-tap error using the previously installed host.
- Installed the corrected release build and restarted Extraspace.
- Exercised 125%, 150%, 200%, 100%, then 125% in one connected host session.
  Three taps at quarter, centre, and three-quarter screen positions passed at
  each scale, with maximum error 0.022 logical pixels across all 15 taps.
- A real Android swipe reached the GTK drag handler at each scale, checking
  touch-motion as well as touch-down.
- All 21 xs-mutter tests and Clippy with warnings denied passed. Release build,
  formatting, and diff checks passed.
- Restored the original display arrangement and 125% scale. Application and
  device preference files are unchanged, and temporary saved test layouts were
  removed by restoring the saved profile file. The corrected host remains running.

The installed executable matches the release output (SHA-256
`345a1f1e3e569a94333b4e016cfdcb1f07f87b29b3179a44c99f0e167e769ac0`).
Ignored local diagnostics, the previous binary, and profile backups are in
`scratch/touch-scale/`. Test logs and scripts are local diagnostics, not shipped
application code. Live checks covered virtual extension over ADB; physical
mirror capture and AOA were not exercised in this validation run.
