# Audit fixes

Changes are in the `extraspace-agent` checkout. The companion upgrade code is
now **15**. These changes have not been published or installed on a tablet.

| Audit item | Result |
| --- | --- |
| 1. Touch cancellation | Fixed. Track active slots and release all of them on cancel, loss of focus, or opening the lobby. Disconnect discards the local tracker as the host tears down its input session. |
| 2. Dropped compressed H.264 inputs | Fixed. Wait up to 250 ms for codec input, then terminate the connection on failure. Reconnection creates a new codec and the host explicitly requests a keyframe. Only decoded outputs may be dropped. |
| 3. Null/undersized codec input | Fixed. Validate lengths and buffers before submitting bytes; errors trigger recovery through the connection failure gate. Failed codec initialization also releases its resources. |
| 4. Mirror silently becomes Extend | Fixed. Monitor enumeration errors and an empty monitor list produce explicit errors in Mirror mode. |
| 5. Camera device ownership | Fixed through sysfs identity checks. Both setup and streaming require a virtual video device with the Extraspace card label. Unrelated physical devices and differently labelled loopbacks are rejected, including after module loading. The default remains `/dev/video10`; allocation is not dynamic. |
| 6. System uninstall | Fixed with an explicit `setup.sh --uninstall --camera --accessory` path, plus `--check` preview. Verify all selected files before deleting any, refuse administrator modifications, reload udev rules, and preserve packages/loaded modules. The user installer points to this command. |
| 7. Camera recovery instruction | Fixed: `setup.sh --camera`. |
| 8. Duplicate disconnect notifications | Fixed. All worker failures compete for one terminal transition, close every channel, and notify once. Managers cannot restart after closure. Late accepted sockets are closed. |
| 9. Cross-thread decoder publication | Fixed. Publish an immutable decoder/session pair, retain the first input until main-thread configuration completes, and discard callbacks from obsolete connections. Surface replacement and output failures initiate reconnection. |
| 10. Protocol drift | Partially addressed. Rust and Kotlin consume the same binary golden vectors covering all header message kinds/channels, touch actions, and cursor structures. JSON payloads and full protocol generation still need follow-up. |
| 11. Companion version enforcement | Fixed. CI checks PR/push history and earlier release tags, requires a higher code for Android implementation/build, protocol fixture, or public version changes, and rejects decreases. |
| 12. Misnamed review requirement | Fixed: release step now says “Require commit from main.” |
| 13. Unverifiable repository protections | Fixed documentation. Required branch policy is described as maintainer configuration, and review/immutability are explicitly external settings. Their actual GitHub state was not changed or verified. |
| 14. Linux release packaging | Deferred. A tested package/binary distribution needs a supported runtime and packaging policy. |
| 15. Published installer depends on origin | Fixed. Fetch published commits from the canonical repository; a regression test uses an unrelated origin. |
| 16. Unused permissions | Fixed. Remove INTERNET and WAKE_LOCK. |
| 17. Launcher over lock screen | Fixed. Remove unconditional showWhenLocked/turnScreenOn flags. |
| 18. Fork application identity | Deferred. Preserve the existing upgrade/signing identity until an explicit migration plan is implemented. |
| 19. Private compositor API risk | Remains. These fixes do not certify Mutter compatibility or change the existing compositor safety guards. |
| 20. Runtime test gap | Partially addressed. Add hardware-free tests for multi-finger cancellation, decoder saturation/null/capacity checks, concurrent terminal transitions, shared wire vectors, camera identity, reversible setup, and version enforcement. Real-device MediaCodec, gesture delivery, AOA recovery, and display aspect-ratio tests remain necessary. |

Validation passed:

- Stable Rust workspace tests: 97 passed; one existing hardware-dependent test ignored.
- Workspace Clippy with warnings denied.
- Rust 1.85 tests for the changed camera/core/protocol crates: 40 passed.
- Linux release build and both GTK lifecycle tests on a private display/bus.
- Android release build, lint, all nine unit tests, and APK signature verification.
- All 30 installer, release, system-setup, and version-gate tests.
- ShellCheck, shell syntax, Rust formatting, icon/theme synchronization, and whitespace checks.

Gradle uses a 768 MiB heap, two workers, no parallel projects, and an in-process
Kotlin compiler to limit local build memory. Validation after the interrupted
builds ran sequentially with a 1.5 GiB memory cap and 512 MiB swap allowance.
Physical-device behavior remains unverified for this patch.
