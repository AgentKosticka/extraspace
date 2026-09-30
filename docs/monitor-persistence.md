# GNOME monitor persistence and crash avoidance

The reference `sal0-h` history added full panel scaling, then restricted explicit
`RecordVirtual` modes to a locally patched Mutter. Later commits restored physical
layouts on startup/teardown, but only for that patched build. Startup also removed
every saved virtual configuration from `monitors.xml`. That protects against
missing saved modes, but discards the arrangement users make in GNOME Displays.

The running Ubuntu compositor is stock Mutter 50.1-0ubuntu2.4. Its source and
Ubuntu's patch series were inspected alongside both Extraspace repositories.
There are several separate hazards:

* `get_specs` assumes an assigned virtual CRTC mode when explicit modes are used.
* The mode-less path creates the monitor after a PipeWire consumer negotiates a
  format. A second update can dereference a missing CRTC and reload monitors.
* Monitor-change and cursor callbacks assume a stage view exists. Disabling an
  output during capture can invalidate that assumption.

The existing patched-Mutter gate is retained. A successful node announcement is
not enough to authorize a layout operation: the consumer must have produced an
encoded frame. PipeWire dimensions remain pinned for the whole stream. Neither
the `XS_MUTTER_MODES` opt-in nor the Arch patch marker is treated as proof that a
stock Ubuntu compositor is patched.

## Persistence in this fork

The app periodically reads `DisplayConfig.GetCurrentState`, and samples again
before disconnect. It atomically stores at most 16 profiles in its own settings
directory. A profile keys the tablet's ADB identity, all physical monitor specs,
active mode IDs, layout mode, scales, transforms and group membership. The virtual
spec is normalized, because its connector/serial is an implementation detail of
the compositor, not a stable tablet identity.

After a real captured frame, an exact geometry match may restore only x/y and
primary selection with a temporary `ApplyMonitorsConfig`. The active modes,
scales, transforms, monitor properties and enabled output set are taken from the
current state. Every connected output must be active, with exactly one virtual
output and no clone groups. There is no attempt to enable an inactive CRTC,
change the stream size, or disable an output on stock Mutter. If the geometry
changed, a new arrangement must be made and saved for that geometry.

This is intentionally narrower than restoring arbitrary GNOME settings. It was
verified on the provided Ubuntu/Samsung setup, including left placement,
repeated recreation, companion-process loss and ADB transport loss. It is not a
substitute for fixing Mutter's missing guards, and cannot guarantee compositor
behavior across untested releases.

`monitors.xml` filtering now uses an XML parser and keeps compatible virtual
configurations, physical-only configurations, whitespace and comments. A virtual
configuration is incompatible if it disables the virtual output or pins a different
framebuffer size, refresh rate, scale or rotation than the stream can support.
Before removing one, the existing file is backed up and the replacement is written
by rename. Invalid/unknown XML or a concurrent file change aborts the operation
rather than risking a partial rewrite. The backup is kept until the user removes it.

Mutter reads its configuration store into memory; editing `monitors.xml` does not
invalidate every cached configuration in a running compositor. This filtering is
an additional safety measure, not the sole crash workaround. Do not remove the
mode gate or start updating capture dimensions live just because an XML backup
exists.

## Source references

* [Mutter 50.1 virtual stream source](https://github.com/GNOME/mutter/blob/50.1/src/backends/meta-screen-cast-virtual-stream-src.c)
* [Mutter 50.1 monitor config store](https://github.com/GNOME/mutter/blob/50.1/src/backends/meta-monitor-config-store.c)
* [Mutter 50.1 config manager](https://github.com/GNOME/mutter/blob/50.1/src/backends/meta-monitor-config-manager.c)
* [Ubuntu's exact source package](https://launchpad.net/ubuntu/+source/mutter/50.1-0ubuntu2.4)
* [Existing crash report and patch notes](../packaging/mutter-patch/UPSTREAM-REPORT.md)
