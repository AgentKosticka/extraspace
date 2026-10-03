# Battery hardening and latency checks

Work is in `extraspace-agent`; it includes existing uncommitted audit fixes. Nothing has been published. Companion source upgrade code is 17.

## Changes

- The Android decoder parks on a retained signal while no pictures are pending. New input and shutdown wake it immediately. Pending output still blocks on the codec, which returns as soon as a picture is ready.
- Android pacing details are opt-in; normal streaming avoids per-frame diagnostic string construction. Host damage-driven idle gaps are debug diagnostics instead of routine warnings.
- Camera-channel lifetime uses a blocking read instead of a one-second timer. Socket shutdown explicitly interrupts blocked I/O and delivers EOF. Camera open/configure callbacks reject obsolete sessions, and failed/stopped capture releases the codec and camera workers.
- Host compressed-frame recovery waits for a notification instead of polling every 100 ms. A lost reference picture requests a fresh keyframe; compressed inputs remain intact.
- PipeWire advertises the selected maximum capture rate for both CPU and DMA-buf formats. This laptop nevertheless produced approximately 200 raw frames/second during a synthetic animation.
- A one-slot raw-frame pacer enforces the selected rate before conversion/encoding. It sends the first update immediately after idle, coalesces excess raw images, preserves the last update, and sleeps indefinitely when there is no pending damage. Nominal GStreamer caps match the selected rate. It preserves the DMA-buf GPU path and does not copy pixels.

The existing drop-only `videorate` element alone did not enforce an ongoing bound after sparse damage: its frame-count schedule accumulated credit during idle, allowing later bursts. The new pacer bases each next dispatch on actual dispatch time instead.

## Validation

- Final Rust workspace: 106 tests passed; one existing graphical hardware test ignored. Clippy passed with warnings denied. Optimized release build passed.
- Android release build, lint and 15 JVM tests passed. APK signing identity matches the installed companion. Companion 17 was installed without clearing data.
- Physical Samsung SM-X620 test: four isolated baseline-compatible H.264 frames separated by 2/5/2-second idle gaps all decoded, queue returned to zero, no drops. Closing the host camera channel ended the session; this exposed and verified the explicit socket shutdown fix.
- Tests cover retained/coalesced wakes, parked-worker shutdown, absence of idle duplicates or post-idle burst credit, final raw-image delivery, requested PipeWire maxima, and fresh-keyframe recovery after encoder output saturation.
- Camera capture is disabled in the user configuration and was not activated for testing. Live camera encode/open failure behavior remains unverified.

## Measurement limits and latency report

Short whole-laptop energy samples include the desktop, other applications, the synthetic animation, and USB charging of the tablet. They are not a controlled before/after battery-life result. Compilation was excluded from power samples.

Before the final pacer, an idle connected stream used about 1.17% of one CPU core; the whole laptop drew 17.48 W in that sample. A disconnected sample drew 17.99 W, but background loads differed, so their difference must not be interpreted as negative incremental power.

The user reported visibly worse latency while detailed GStreamer and Android tracing were enabled. Normal streaming was restored immediately; the user reported improvement but still perceived a regression. The saved original host was then restored for comparison. Android refused a version-17-to-14 downgrade, so this host comparison still uses companion 17 and is not a complete rollback. No data was removed to bypass that restriction.

A rate cap can alter frame selection during continuous motion. The latest pending raw image is selected at each dispatch, rather than building a history queue; a pending final image waits no longer than the selected frame interval absent scheduling/encoder stalls. First damage after idle is immediate. Automated tests alone do not establish unchanged perceptual or end-to-end latency. The user's latency report takes precedence over a claim of completion.

Resolution (1920×1200), configured target (60 fps), render scale, bitrate bounds, placement and tablet charging preferences are preserved. Original host and APK backups and private test logs are under `scratch/battery-hardening/`.

## Final paced candidate measurement

With per-frame tracing disabled and no build running, the 30-second synthetic animation produced 50–77 raw frames per roughly 500-ms health window. The candidate forwarded 26–30; decoder queue remained at most two, host/device stream drops were zero, and CPU pixel-copy time was zero. After the animation, capture/encode counts returned to zero rather than generating duplicates.

Two adjacent 28-second animation samples measured:

| Runtime | Whole laptop | CPU package | Extraspace CPU, one-core basis |
| --- | --- | --- | --- |
| Saved original host, companion 17 | 26.99 W | 9.51 W | 36.77% |
| Final paced candidate, companion 17 | 25.70 W | 9.20 W | 34.20% |

ChatGPT CPU usage changed from 97.32% to 71.93% between samples; this alone prevents attributing the 1.29-W whole-laptop difference to Extraspace. USB charging and the animated producer also remained active. These numbers are diagnostic observations, not a promised runtime improvement. User evaluation of the final paced candidate is pending.

Final runtime: the paced candidate host and companion 17 are installed. Normal streaming uses info logging; detailed frame tracing and health debug logging have been disabled. The saved original host restored the user's reported responsiveness; the user's evaluation of the paced candidate is still pending. Do not treat the latency requirement as verified.
