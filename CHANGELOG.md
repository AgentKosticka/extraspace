# Changelog

## 0.2.0

- Android tablet display and camera companion with ADB and USB accessory transports,
  remembered device/display settings and an offline display/touch check.
- Unified tablet/Ubuntu setup styling and connection labels; Android system USB
  permission opens the accessory without an additional app confirmation.
- Join canceled session tasks before teardown; single host-requested AOA handshake;
  strict fixed-width touch decoding and accurate transport documentation.
- CI runs Android USB framing tests, both GTK lifecycle tests, installer/release
  interruption tests and ShellCheck alongside the Rust MSRV/stable matrix.
- Verified draft publication to immutable versioned releases; pinned stable
  installation; development APKs remain Actions artifacts.
- Required main-branch checks and a staged application identity migration roadmap.
