//! Stylus input injection via `/dev/uinput` graphics tablet (`ID_INPUT_TABLET=1`,
//! `zwp_tablet_tool_v2`) with full pressure (`0.0..=1.0`) and tilt (`-90..=90` deg)
//! support, without mouse pointer emulation or hover pointer tracking.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::fd::AsRawFd;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{debug, warn};
use xs_proto::{stylus_buttons, StylusAction, StylusEvent, StylusTool};

use crate::display::StageGeometry;

// Linux input event types and codes (`linux/input-event-codes.h`).
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;

const SYN_REPORT: u16 = 0x00;

const BTN_TOOL_PEN: u16 = 0x140;
const BTN_TOOL_RUBBER: u16 = 0x141;
const BTN_TOUCH: u16 = 0x14a;
const BTN_STYLUS: u16 = 0x14b;
const BTN_STYLUS2: u16 = 0x14c;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_PRESSURE: u16 = 0x18;
const ABS_TILT_X: u16 = 0x1a;
const ABS_TILT_Y: u16 = 0x1b;

const INPUT_PROP_POINTER: u16 = 0x00;

const ABS_COORD_MAX: i32 = 65_535;
const ABS_PRESSURE_MAX: i32 = 4_095;
const ABS_TILT_MIN: i32 = -90;
const ABS_TILT_MAX: i32 = 90;

// Linux uinput ioctl numbers on 64-bit / x86_64 / aarch64 (`linux/uinput.h`).
const UI_DEV_CREATE: libc_ioctl::Ioctl = 0x5501;
const UI_DEV_DESTROY: libc_ioctl::Ioctl = 0x5502;
const UI_DEV_SETUP: libc_ioctl::Ioctl = 0x405c_5503;
const UI_ABS_SETUP: libc_ioctl::Ioctl = 0x401c_5504;
const UI_SET_EVBIT: libc_ioctl::Ioctl = 0x4004_5564;
const UI_SET_KEYBIT: libc_ioctl::Ioctl = 0x4004_5565;
const UI_SET_ABSBIT: libc_ioctl::Ioctl = 0x4004_5567;
const UI_SET_PROPBIT: libc_ioctl::Ioctl = 0x4004_556e;

const BUS_VIRTUAL: u16 = 0x06;

mod libc_ioctl {
    pub type Ioctl = std::ffi::c_ulong;
    unsafe extern "C" {
        pub fn ioctl(fd: std::ffi::c_int, request: Ioctl, ...) -> std::ffi::c_int;
        #[cfg(not(test))]
        pub fn getuid() -> std::ffi::c_uint;
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InputAbsinfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UinputAbsSetup {
    code: u16,
    absinfo: InputAbsinfo,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawInputEvent {
    pub tv_sec: i64,
    pub tv_usec: i64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

/// Quantized single-frame tablet state ready to be written to `/dev/uinput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UinputStylusReport {
    pub abs_x: i32,
    pub abs_y: i32,
    pub pressure: i32,
    pub tilt_x: i32,
    pub tilt_y: i32,
    pub tool_pen: bool,
    pub tool_rubber: bool,
    pub touching: bool,
    pub barrel_primary: bool,
    pub barrel_secondary: bool,
}

impl UinputStylusReport {
    pub(crate) fn from_event(event: StylusEvent, stage: StageGeometry) -> Self {
        let stage_x = stage.monitor_x + (event.x / stage.monitor_scale.max(0.25));
        let stage_y = stage.monitor_y + (event.y / stage.monitor_scale.max(0.25));
        let abs_x = ((stage_x / stage.stage_width.max(1.0)) * ABS_COORD_MAX as f64)
            .round()
            .clamp(0.0, ABS_COORD_MAX as f64) as i32;
        let abs_y = ((stage_y / stage.stage_height.max(1.0)) * ABS_COORD_MAX as f64)
            .round()
            .clamp(0.0, ABS_COORD_MAX as f64) as i32;

        let touching = matches!(event.action, StylusAction::Down | StylusAction::Motion);
        let in_proximity = touching;
        let pressure = if touching {
            (event.pressure.clamp(0.0, 1.0) * ABS_PRESSURE_MAX as f32)
                .round()
                .max(1.0) as i32
        } else {
            0
        };
        let tilt_x = (event.tilt_x.clamp(-1.0, 1.0) * ABS_TILT_MAX as f32)
            .round()
            .clamp(ABS_TILT_MIN as f32, ABS_TILT_MAX as f32) as i32;
        let tilt_y = (event.tilt_y.clamp(-1.0, 1.0) * ABS_TILT_MAX as f32)
            .round()
            .clamp(ABS_TILT_MIN as f32, ABS_TILT_MAX as f32) as i32;

        Self {
            abs_x,
            abs_y,
            pressure,
            tilt_x,
            tilt_y,
            tool_pen: in_proximity && event.tool == StylusTool::Pen,
            tool_rubber: in_proximity && event.tool == StylusTool::Eraser,
            touching,
            barrel_primary: in_proximity && (event.buttons & stylus_buttons::PRIMARY != 0),
            barrel_secondary: in_proximity && (event.buttons & stylus_buttons::SECONDARY != 0),
        }
    }

    pub(crate) fn to_events(self) -> [RawInputEvent; 11] {
        let ev = |type_: u16, code: u16, value: i32| RawInputEvent {
            tv_sec: 0,
            tv_usec: 0,
            type_,
            code,
            value,
        };
        [
            ev(EV_ABS, ABS_X, self.abs_x),
            ev(EV_ABS, ABS_Y, self.abs_y),
            ev(EV_ABS, ABS_PRESSURE, self.pressure),
            ev(EV_ABS, ABS_TILT_X, self.tilt_x),
            ev(EV_ABS, ABS_TILT_Y, self.tilt_y),
            ev(EV_KEY, BTN_TOOL_PEN, i32::from(self.tool_pen)),
            ev(EV_KEY, BTN_TOOL_RUBBER, i32::from(self.tool_rubber)),
            ev(EV_KEY, BTN_TOUCH, i32::from(self.touching)),
            ev(EV_KEY, BTN_STYLUS, i32::from(self.barrel_primary)),
            ev(EV_KEY, BTN_STYLUS2, i32::from(self.barrel_secondary)),
            ev(EV_SYN, SYN_REPORT, 0),
        ]
    }
}

/// Sequences `SYN_REPORT` frames for `libinput` and `Mutter`:
/// - `libinput` (`src/evdev-tablet.c`) treats non-zero `ABS_PRESSURE` in the same
///   `SYN_REPORT` frame as `BTN_TOOL_PEN 0 -> 1` as a hardware resting pressure offset.
///   Therefore `PROXIMITY_IN` is always emitted in its own `SYN_REPORT` frame with
///   `ABS_PRESSURE = 0` and `BTN_TOUCH = 0` before tip-down.
/// - `Mutter` (`meta-wayland-tablet-tool.c`) emits `zwp_tablet_tool_v2.down` on
///   `CLUTTER_BUTTON_PRESS` (`BTN_TOUCH 0 -> 1`) and emits `zwp_tablet_tool_v2.pressure`
///   on `CLUTTER_MOTION` (`LIBINPUT_EVENT_TABLET_TOOL_AXIS`), so tip-down is followed
///   by an axis frame carrying the exact contact pressure.
/// - Hovering out of contact does not move any desktop pointer; lifting the tip emits
///   `TIP_UP` followed by `PROXIMITY_OUT`.
#[derive(Debug, Default)]
pub(crate) struct UinputStylusSequence {
    active_tool: Option<StylusTool>,
    touching: bool,
    last_abs_x: i32,
    last_abs_y: i32,
}

impl UinputStylusSequence {
    pub(crate) fn translate(
        &mut self,
        event: StylusEvent,
        stage: StageGeometry,
    ) -> Vec<UinputStylusReport> {
        let mut out = Vec::with_capacity(3);
        match event.action {
            StylusAction::Hover => {
                // Do not track hover with a desktop cursor; release if a stroke was active.
                self.release_into(&mut out);
            }
            StylusAction::HoverExit => {
                self.release_into(&mut out);
            }
            StylusAction::Down | StylusAction::Motion => {
                let target = UinputStylusReport::from_event(event, stage);

                if self.active_tool != Some(event.tool) {
                    if self.active_tool.is_some() {
                        self.release_into(&mut out);
                    }
                    // Frame 1: Proximity-In at contact coordinates with zero pressure.
                    out.push(UinputStylusReport {
                        abs_x: target.abs_x,
                        abs_y: target.abs_y,
                        pressure: 0,
                        tilt_x: target.tilt_x,
                        tilt_y: target.tilt_y,
                        tool_pen: event.tool == StylusTool::Pen,
                        tool_rubber: event.tool == StylusTool::Eraser,
                        touching: false,
                        barrel_primary: false,
                        barrel_secondary: false,
                    });
                    self.active_tool = Some(event.tool);
                    self.touching = false;
                }

                // Release the old tool at its own last position before moving
                // the replacement tool (e.g. when turning the pen over).
                self.last_abs_x = target.abs_x;
                self.last_abs_y = target.abs_y;

                if !self.touching {
                    // Frame 2: Tip-Down (`BTN_TOUCH = 1`) with pre-axis pressure so the
                    // subsequent axis frame has a distinct `ABS_PRESSURE` value that passes
                    // the kernel's `input_handle_abs_event` deduplication filter.
                    let pre_pressure = if target.pressure > 1 {
                        target.pressure - 1
                    } else {
                        2
                    };
                    out.push(UinputStylusReport {
                        pressure: pre_pressure,
                        ..target
                    });
                    // Frame 3: Axis update (`LIBINPUT_EVENT_TABLET_TOOL_AXIS` -> `CLUTTER_MOTION`)
                    // delivering the exact pressure and tilt to `zwp_tablet_tool_v2`.
                    out.push(target);
                    self.touching = true;
                } else {
                    out.push(target);
                }
            }
            StylusAction::Up => {
                let released = UinputStylusReport::from_event(event, stage);
                self.last_abs_x = released.abs_x;
                self.last_abs_y = released.abs_y;
                self.release_into(&mut out);
            }
        }
        out
    }

    fn release_into(&mut self, out: &mut Vec<UinputStylusReport>) {
        if let Some(tool) = self.active_tool {
            if self.touching {
                out.push(UinputStylusReport {
                    abs_x: self.last_abs_x,
                    abs_y: self.last_abs_y,
                    pressure: 0,
                    tilt_x: 0,
                    tilt_y: 0,
                    tool_pen: tool == StylusTool::Pen,
                    tool_rubber: tool == StylusTool::Eraser,
                    touching: false,
                    barrel_primary: false,
                    barrel_secondary: false,
                });
                self.touching = false;
            }
            out.push(UinputStylusReport {
                abs_x: self.last_abs_x,
                abs_y: self.last_abs_y,
                pressure: 0,
                tilt_x: 0,
                tilt_y: 0,
                tool_pen: false,
                tool_rubber: false,
                touching: false,
                barrel_primary: false,
                barrel_secondary: false,
            });
            self.active_tool = None;
        }
    }
}

#[cfg(not(test))]
fn ensure_uinput_access() {
    static ATTEMPTED: AtomicBool = AtomicBool::new(false);
    if ATTEMPTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let uid = unsafe { libc_ioctl::getuid() };
    let script = format!(
        "printf '%s\\n' \
         '# Extraspace virtual stylus (/dev/uinput): grant access to active local seat users and plugdev.' \
         'KERNEL==\"uinput\", SUBSYSTEM==\"misc\", OPTIONS+=\"static_node=uinput\", TAG+=\"uaccess\", GROUP=\"plugdev\", MODE=\"0660\"' \
         > /etc/udev/rules.d/70-extraspace-uinput.rules && \
         chmod 0644 /etc/udev/rules.d/70-extraspace-uinput.rules && \
         udevadm control --reload-rules && \
         udevadm trigger --sysname-match=uinput && \
         (chgrp plugdev /dev/uinput 2>/dev/null || true) && \
         chmod 0660 /dev/uinput && \
         (setfacl -m u:{uid}:rw /dev/uinput 2>/dev/null || true)"
    );
    match std::process::Command::new("pkexec")
        .args(["sh", "-c", &script])
        .status()
    {
        Ok(status) if status.success() => {
            debug!("configured /dev/uinput permissions via pkexec");
        }
        Ok(status) => {
            warn!(
                ?status,
                "pkexec /dev/uinput permission setup did not succeed"
            );
        }
        Err(e) => {
            warn!(error = %e, "could not invoke pkexec to configure /dev/uinput permissions");
        }
    }
}

pub(crate) struct UinputStylus {
    file: File,
}

impl UinputStylus {
    pub(crate) fn try_open() -> Option<Self> {
        let file = match OpenOptions::new().write(true).open("/dev/uinput") {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                #[cfg(not(test))]
                {
                    ensure_uinput_access();
                    OpenOptions::new().write(true).open("/dev/uinput").ok()?
                }
                #[cfg(test)]
                {
                    return None;
                }
            }
            Err(_) => return None,
        };
        let fd = file.as_raw_fd();

        // Safety: standard Linux uinput ioctls on a valid character-device fd.
        unsafe {
            if libc_ioctl::ioctl(fd, UI_SET_EVBIT, std::ffi::c_int::from(EV_KEY)) < 0
                || libc_ioctl::ioctl(fd, UI_SET_EVBIT, std::ffi::c_int::from(EV_ABS)) < 0
                || libc_ioctl::ioctl(
                    fd,
                    UI_SET_PROPBIT,
                    std::ffi::c_int::from(INPUT_PROP_POINTER),
                ) < 0
            {
                return None;
            }
            for key in [
                BTN_TOOL_PEN,
                BTN_TOOL_RUBBER,
                BTN_TOUCH,
                BTN_STYLUS,
                BTN_STYLUS2,
            ] {
                if libc_ioctl::ioctl(fd, UI_SET_KEYBIT, std::ffi::c_int::from(key)) < 0 {
                    return None;
                }
            }
            for (code, min, max, res) in [
                (ABS_X, 0, ABS_COORD_MAX, 100),
                (ABS_Y, 0, ABS_COORD_MAX, 100),
                (ABS_PRESSURE, 0, ABS_PRESSURE_MAX, 0),
                (ABS_TILT_X, ABS_TILT_MIN, ABS_TILT_MAX, 57),
                (ABS_TILT_Y, ABS_TILT_MIN, ABS_TILT_MAX, 57),
            ] {
                if libc_ioctl::ioctl(fd, UI_SET_ABSBIT, std::ffi::c_int::from(code)) < 0 {
                    return None;
                }
                let abs = UinputAbsSetup {
                    code,
                    absinfo: InputAbsinfo {
                        value: 0,
                        minimum: min,
                        maximum: max,
                        fuzz: 0,
                        flat: 0,
                        resolution: res,
                    },
                };
                if libc_ioctl::ioctl(fd, UI_ABS_SETUP, &abs as *const UinputAbsSetup) < 0 {
                    return None;
                }
            }

            let mut name = [0u8; 80];
            let label = b"Extraspace Stylus";
            name[..label.len()].copy_from_slice(label);
            let setup = UinputSetup {
                id: InputId {
                    bustype: BUS_VIRTUAL,
                    vendor: 0x18d1,
                    product: 0x4ee7,
                    version: 1,
                },
                name,
                ff_effects_max: 0,
            };
            if libc_ioctl::ioctl(fd, UI_DEV_SETUP, &setup as *const UinputSetup) < 0
                || libc_ioctl::ioctl(fd, UI_DEV_CREATE) < 0
            {
                return None;
            }
        }
        // Allow udev and Mutter's libinput seat to attach to the new evdev node.
        std::thread::sleep(std::time::Duration::from_millis(60));
        debug!("opened /dev/uinput virtual stylus with pressure and tilt support");
        Some(Self { file })
    }

    pub(crate) fn emit(&mut self, report: UinputStylusReport) -> std::io::Result<()> {
        let events = report.to_events();
        // Safety: `RawInputEvent` is `#[repr(C)]` with no uninitialized padding.
        let bytes = unsafe {
            std::slice::from_raw_parts(events.as_ptr() as *const u8, std::mem::size_of_val(&events))
        };
        self.file.write_all(bytes)
    }
}

impl Drop for UinputStylus {
    fn drop(&mut self) {
        unsafe {
            let _ = libc_ioctl::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY);
        }
    }
}

pub(crate) struct StylusInput {
    uinput: Option<UinputStylus>,
    sequence: UinputStylusSequence,
}

impl StylusInput {
    pub(crate) fn new() -> Self {
        Self {
            uinput: UinputStylus::try_open(),
            sequence: UinputStylusSequence::default(),
        }
    }

    /// Emits native pressure/tilt tablet reports via `/dev/uinput`.
    pub(crate) fn handle(&mut self, event: StylusEvent, stage: StageGeometry) {
        if self.uinput.is_none() {
            self.uinput = UinputStylus::try_open();
        }
        let Some(uinput) = self.uinput.as_mut() else {
            warn!("dropping stylus event because /dev/uinput is unavailable");
            return;
        };
        for report in self.sequence.translate(event, stage) {
            if let Err(e) = uinput.emit(report) {
                warn!(error = %e, "failed to write stylus report to /dev/uinput");
                self.uinput = None;
                // A recreated evdev device has no proximity or tip state. Its
                // first motion must emit a fresh proximity-in/down sequence.
                self.sequence = UinputStylusSequence::default();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(x: f64, y: f64, tool: StylusTool) -> StylusEvent {
        StylusEvent {
            action: StylusAction::Down,
            tool,
            buttons: 0,
            x,
            y,
            pressure: 0.5,
            tilt_x: 0.0,
            tilt_y: 0.0,
        }
    }

    fn video_frame() -> StageGeometry {
        StageGeometry {
            monitor_x: 0.0,
            monitor_y: 0.0,
            monitor_scale: 1.0,
            stage_width: 1920.0,
            stage_height: 1200.0,
        }
    }

    #[test]
    fn pinned_and_desktop_mapping_align_from_top_to_bottom_at_fractional_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let desktop = StageGeometry {
                monitor_x: 0.0,
                monitor_y: 120.0,
                monitor_scale: scale,
                stage_width: 1920.0 / scale + 1920.0,
                stage_height: (1200.0 / scale + 120.0).max(1080.0),
            };
            for y in [0.0, 1.0, 200.0, 600.0, 1000.0, 1199.0] {
                for x in [0.0, 960.0, 1919.0] {
                    let event = contact(x, y, StylusTool::Pen);
                    let pinned = UinputStylusReport::from_event(event, video_frame());
                    let unpinned = UinputStylusReport::from_event(event, desktop);
                    // Model Mutter's inverse mapping in both supported modes.
                    // Check the full height: a center-only assertion hides drift.
                    let pinned_x = pinned.abs_x as f64 / ABS_COORD_MAX as f64 * 1920.0;
                    let pinned_y = pinned.abs_y as f64 / ABS_COORD_MAX as f64 * 1200.0;
                    let stage_x =
                        unpinned.abs_x as f64 / ABS_COORD_MAX as f64 * desktop.stage_width;
                    let stage_y =
                        unpinned.abs_y as f64 / ABS_COORD_MAX as f64 * desktop.stage_height;
                    assert!((pinned_x - x).abs() < 0.06);
                    assert!((pinned_y - y).abs() < 0.06);
                    assert!(((stage_x - desktop.monitor_x) * scale - x).abs() < 0.06);
                    assert!(((stage_y - desktop.monitor_y) * scale - y).abs() < 0.06);
                }
            }
        }
    }

    #[test]
    fn tool_change_releases_old_tool_before_moving_to_the_new_position() {
        let mut sequence = UinputStylusSequence::default();
        let old = sequence.translate(contact(100.0, 200.0, StylusTool::Pen), video_frame());
        let swapped = sequence.translate(contact(900.0, 800.0, StylusTool::Eraser), video_frame());
        assert_eq!(swapped.len(), 5);
        for release in &swapped[..2] {
            assert_eq!((release.abs_x, release.abs_y), (old[2].abs_x, old[2].abs_y));
            assert!(!release.touching);
        }
        assert!(swapped[2].tool_rubber);
        assert!(!swapped[2].touching);
        assert!(swapped[4].touching);
        assert_ne!(swapped[4].abs_y, old[2].abs_y);
    }

    #[test]
    fn write_failure_requires_fresh_proximity_and_tip_down_on_recovery() {
        let mut input = StylusInput {
            uinput: Some(UinputStylus {
                file: OpenOptions::new().write(true).open("/dev/full").unwrap(),
            }),
            sequence: UinputStylusSequence::default(),
        };
        let down = contact(100.0, 200.0, StylusTool::Pen);
        input.sequence.translate(down, video_frame());
        let motion = StylusEvent {
            action: StylusAction::Motion,
            ..down
        };
        input.handle(motion, video_frame());
        assert!(input.uinput.is_none());
        let recovered = input.sequence.translate(motion, video_frame());
        assert_eq!(recovered.len(), 3);
        assert!(recovered[0].tool_pen);
        assert!(!recovered[0].touching);
        assert_eq!(recovered[0].pressure, 0);
        assert!(recovered[1].touching);
    }

    #[test]
    fn uinput_report_normalizes_stage_coords_pressure_and_tilt() {
        let stage = StageGeometry {
            monitor_x: 1920.0,
            monitor_y: 0.0,
            monitor_scale: 1.5,
            stage_width: 3456.0,
            stage_height: 1080.0,
        };
        let event = StylusEvent {
            action: StylusAction::Motion,
            tool: StylusTool::Pen,
            buttons: stylus_buttons::PRIMARY,
            x: 1152.0,
            y: 720.0,
            pressure: 0.5,
            tilt_x: 0.25,
            tilt_y: -0.5,
        };
        let report = UinputStylusReport::from_event(event, stage);
        // Logical on virtual monitor: (1152 / 1.5, 720 / 1.5) = (768, 480).
        // Global stage: (1920 + 768, 480) = (2688, 480) in 3456x1080.
        assert_eq!(
            report.abs_x,
            ((2688.0 / 3456.0) * ABS_COORD_MAX as f64).round() as i32
        );
        assert_eq!(
            report.abs_y,
            ((480.0 / 1080.0) * ABS_COORD_MAX as f64).round() as i32
        );
        assert_eq!(report.pressure, 2048);
        assert_eq!(report.tilt_x, 23);
        assert_eq!(report.tilt_y, -45);
        assert!(report.tool_pen);
        assert!(!report.tool_rubber);
        assert!(report.touching);
        assert!(report.barrel_primary);
        assert!(!report.barrel_secondary);
    }

    #[test]
    fn sequence_separates_proximity_in_tip_down_axis_and_release_without_hover_cursor() {
        let stage = StageGeometry {
            monitor_x: 0.0,
            monitor_y: 0.0,
            monitor_scale: 1.0,
            stage_width: 1920.0,
            stage_height: 1200.0,
        };
        let mut seq = UinputStylusSequence::default();

        // Hover while out of contact emits nothing (never moves the desktop cursor).
        let hover_reports = seq.translate(
            StylusEvent {
                action: StylusAction::Hover,
                tool: StylusTool::Pen,
                buttons: 0,
                x: 100.0,
                y: 200.0,
                pressure: 0.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
            },
            stage,
        );
        assert!(hover_reports.is_empty());

        // Down emits: (1) Proximity-In with pressure 0, (2) Tip-Down, (3) Axis frame with exact pressure.
        let down_reports = seq.translate(
            StylusEvent {
                action: StylusAction::Down,
                tool: StylusTool::Pen,
                buttons: 0,
                x: 960.0,
                y: 600.0,
                pressure: 0.25,
                tilt_x: 0.5,
                tilt_y: -0.25,
            },
            stage,
        );
        assert_eq!(down_reports.len(), 3);
        assert!(down_reports[0].tool_pen);
        assert!(!down_reports[0].touching);
        assert_eq!(down_reports[0].pressure, 0);

        assert!(down_reports[1].tool_pen);
        assert!(down_reports[1].touching);
        assert_eq!(down_reports[1].pressure, 1023);

        assert!(down_reports[2].tool_pen);
        assert!(down_reports[2].touching);
        assert_eq!(down_reports[2].pressure, 1024);
        assert_eq!(down_reports[2].tilt_x, 45);
        assert_eq!(down_reports[2].tilt_y, -23);

        // Motion emits a single axis frame with the updated pressure.
        let motion_reports = seq.translate(
            StylusEvent {
                action: StylusAction::Motion,
                tool: StylusTool::Pen,
                buttons: stylus_buttons::PRIMARY,
                x: 1000.0,
                y: 620.0,
                pressure: 0.75,
                tilt_x: 0.5,
                tilt_y: -0.25,
            },
            stage,
        );
        assert_eq!(motion_reports.len(), 1);
        assert!(motion_reports[0].touching);
        assert_eq!(motion_reports[0].pressure, 3071);
        assert!(motion_reports[0].barrel_primary);

        // Up emits Tip-Up followed by Proximity-Out.
        let up_reports = seq.translate(
            StylusEvent {
                action: StylusAction::Up,
                tool: StylusTool::Pen,
                buttons: 0,
                x: 1000.0,
                y: 620.0,
                pressure: 0.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
            },
            stage,
        );
        assert_eq!(up_reports.len(), 2);
        assert!(up_reports[0].tool_pen);
        assert!(!up_reports[0].touching);
        assert_eq!(up_reports[0].pressure, 0);
        assert!(!up_reports[1].tool_pen);
        assert!(!up_reports[1].touching);
        assert_eq!(up_reports[1].pressure, 0);
    }
}
