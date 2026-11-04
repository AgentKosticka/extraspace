//! Android Open Accessory: a single bulk stream carrying the existing XSPA frames.
//! Auto prefers an authorized ADB device when both selectors allow it.
use crate::{Adb, Device, DeviceState, Error, Result, Transport, TransportStream};
use rusb::{DeviceHandle, Direction, GlobalContext, TransferType};
use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use xs_proto::{flags, Channel, ControlKind, Header, HEADER_LEN};

const USB_PACKET: usize = 16 * 1024;
/// Video payloads are split into fragments of this size so that control frames
/// (cursor moves, pings) wait at most one fragment, not a whole keyframe.
const VIDEO_FRAGMENT: usize = USB_PACKET;
const IO_TIMEOUT: Duration = Duration::from_millis(250);
const SETUP_TIMEOUT: Duration = Duration::from_secs(1);
const MANUFACTURER: &str = "Extraspace";
const MODEL: &str = "Extraspace Display";

pub(crate) struct AccessoryHandle {
    stopped: Arc<AtomicBool>,
    sockets: Vec<UnixStream>,
    usb: Arc<DeviceHandle<GlobalContext>>,
    output: u8,
    writer_lock: Arc<WriterGate>,
}

/// Exclusive access to the single bulk OUT endpoint, with control frames
/// served before video. Waiting is a condition variable, never a spin.
#[derive(Default)]
struct WriterGate {
    state: Mutex<GateState>,
    changed: Condvar,
}

#[derive(Default)]
struct GateState {
    busy: bool,
    control_waiting: usize,
}

struct GateGuard<'a>(&'a WriterGate);

impl WriterGate {
    /// Waits for the endpoint. Video also yields to any waiting control frame.
    /// Wakes periodically so a stopped bridge does not wait forever.
    fn acquire(&self, control: bool, stopped: &AtomicBool) -> std::io::Result<GateGuard<'_>> {
        fn poisoned<T>(_: T) -> std::io::Error {
            std::io::Error::other("USB writer lock poisoned")
        }
        let mut state = self.state.lock().map_err(poisoned)?;
        if control {
            state.control_waiting += 1;
        }
        while state.busy || (!control && state.control_waiting > 0) {
            if stopped.load(Ordering::Relaxed) {
                if control {
                    state.control_waiting -= 1;
                }
                return Err(std::io::ErrorKind::ConnectionAborted.into());
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .map_err(poisoned)?
                .0;
        }
        if control {
            state.control_waiting -= 1;
        }
        state.busy = true;
        Ok(GateGuard(self))
    }
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.busy = false;
        }
        self.0.changed.notify_all();
    }
}

/// Splits one video frame into wire frames of at most [`VIDEO_FRAGMENT`]
/// payload bytes. All but the last carry [`flags::FRAGMENT`]; the last keeps
/// the original flags, so the receiver delivers on it.
fn video_fragments(
    header: &Header,
    payload: &[u8],
) -> Vec<([u8; HEADER_LEN], std::ops::Range<usize>)> {
    if payload.len() <= VIDEO_FRAGMENT {
        return vec![(header.encode(), 0..payload.len())];
    }
    let mut out = Vec::with_capacity(payload.len().div_ceil(VIDEO_FRAGMENT));
    let mut start = 0;
    while start < payload.len() {
        let end = (start + VIDEO_FRAGMENT).min(payload.len());
        let last = end == payload.len();
        let piece = Header {
            flags: if last {
                header.flags
            } else {
                header.flags | flags::FRAGMENT
            },
            len: (end - start) as u32,
            ..*header
        };
        out.push((piece.encode(), start..end));
        start = end;
    }
    out
}
impl AccessoryHandle {
    pub(crate) async fn disconnect(self: &Arc<Self>) {
        self.close(); // unblock pending handshake writers before taking the USB lock
        let this = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            // The bridge is already stopped; take the endpoint unconditionally.
            let never = AtomicBool::new(false);
            let _guard = this.writer_lock.acquire(true, &never);
            let head = Header {
                channel: Channel::Control,
                kind: ControlKind::SessionEnd as u8,
                flags: 0,
                len: 0,
                pts_us: 0,
            }
            .encode();
            let _ = this.usb.write_bulk(this.output, &head, IO_TIMEOUT);
        })
        .await;
        self.close();
    }
    pub(crate) fn close(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        for socket in &self.sockets {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}
impl Drop for AccessoryHandle {
    fn drop(&mut self) {
        self.close();
    }
}

fn usb_error(e: rusb::Error) -> Error {
    Error::Accessory(if e == rusb::Error::Access {
        "USB permission denied. Install packaging/70-extraspace-accessory.rules in /etc/udev/rules.d, reload udev rules, and reconnect the cable. The initial Android USB device must also be accessible to your user (android-sdk-platform-tools-common on Ubuntu).".into()
    } else {
        e.to_string()
    })
}
fn accessory(vid: u16, pid: u16) -> bool {
    vid == 0x18d1 && [0x2d00, 0x2d01].contains(&pid)
}

fn discovery_active(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(Error::Accessory("USB discovery cancelled".into()));
    }
    Ok(())
}
struct DiscoveryGuard(Arc<AtomicBool>);
impl Drop for DiscoveryGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Whether a device is already in accessory mode, e.g. left there by a host
/// process that exited. Its companion then listens on USB, not on ADB.
pub(crate) fn accessory_present() -> bool {
    rusb::devices().is_ok_and(|devices| {
        devices.iter().any(|device| {
            device
                .device_descriptor()
                .is_ok_and(|d| accessory(d.vendor_id(), d.product_id()))
        })
    })
}

fn open_usb(cancelled: &AtomicBool) -> Result<(DeviceHandle<GlobalContext>, u8, u8, String)> {
    discovery_active(cancelled)?;
    let devices = rusb::devices().map_err(usb_error)?;
    let mut ready = Vec::new();
    let mut candidates = Vec::new();
    for device in devices.iter() {
        discovery_active(cancelled)?;
        let descriptor = device.device_descriptor().map_err(usb_error)?;
        if accessory(descriptor.vendor_id(), descriptor.product_id()) {
            ready.push(device);
            continue;
        }
        // Probe Android-like MTP/ADB interfaces only, never HID, disks or hubs.
        if device.config_descriptor(0).ok().is_some_and(|c| {
            c.interfaces().any(|i| {
                i.descriptors().any(|d| {
                    (d.class_code() == 6 && d.sub_class_code() == 1)
                        || (d.class_code() == 255
                            && d.sub_class_code() == 0x42
                            && d.protocol_code() == 1)
                })
            })
        }) {
            candidates.push(device);
        }
    }
    if ready.len() > 1 {
        return Err(Error::Accessory(
            "Multiple accessory devices connected; connect one tablet at a time.".into(),
        ));
    }
    let device = if let Some(device) = ready.pop() {
        device
    } else {
        let mut supported = Vec::new();
        let mut denied = false;
        for device in candidates {
            discovery_active(cancelled)?;
            let handle = match device.open() {
                Ok(h) => h,
                Err(rusb::Error::Access) => {
                    denied = true;
                    continue;
                }
                Err(_) => continue,
            };
            let mut protocol = [0u8; 2];
            if handle
                .read_control(0xc0, 51, 0, 0, &mut protocol, SETUP_TIMEOUT)
                .ok()
                == Some(2)
                && u16::from_le_bytes(protocol) >= 1
            {
                supported.push((device, handle));
            }
        }
        if supported.len() > 1 {
            return Err(Error::Accessory(
                "Multiple AOA-capable devices found; connect one tablet at a time.".into(),
            ));
        }
        let Some((device, handle)) = supported.pop() else {
            return Err(if denied {
                usb_error(rusb::Error::Access)
            } else {
                Error::Adb(crate::adb::Error::NoDevice)
            });
        };
        let bus = device.bus_number();
        let ports = device.port_numbers().map_err(usb_error)?;
        for (index, value) in [
            MANUFACTURER,
            MODEL,
            "USB second display",
            "1",
            "https://github.com/AgentKosticka/extraspace",
            "extraspace-host",
        ]
        .iter()
        .enumerate()
        {
            discovery_active(cancelled)?;
            let mut bytes = value.as_bytes().to_vec();
            bytes.push(0);
            let written = handle
                .write_control(0x40, 52, 0, index as u16, &bytes, SETUP_TIMEOUT)
                .map_err(usb_error)?;
            if written != bytes.len() {
                return Err(Error::Accessory(
                    "Incomplete accessory identification transfer".into(),
                ));
            }
        }
        discovery_active(cancelled)?;
        handle
            .write_control(0x40, 53, 0, 0, &[], SETUP_TIMEOUT)
            .map_err(usb_error)?;
        drop(handle);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            discovery_active(cancelled)?;
            let found = rusb::devices().map_err(usb_error)?.iter().find(|d| {
                d.bus_number() == bus
                    && d.port_numbers().ok().as_ref() == Some(&ports)
                    && d.device_descriptor()
                        .ok()
                        .is_some_and(|v| accessory(v.vendor_id(), v.product_id()))
            });
            if let Some(d) = found {
                break d;
            }
            if Instant::now() >= deadline {
                return Err(Error::Accessory(
                    "Tablet did not enter accessory mode; reconnect the cable and try ADB.".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    discovery_active(cancelled)?;
    let descriptor = device.device_descriptor().map_err(usb_error)?;
    let handle = device.open().map_err(usb_error)?;
    let serial = handle
        .read_serial_number_string_ascii(&descriptor)
        .unwrap_or_else(|_| format!("usb-{}-{}", device.bus_number(), device.address()));
    let config = device.active_config_descriptor().map_err(usb_error)?;
    for interface in config.interfaces() {
        for d in interface.descriptors() {
            // Google's accessory interface is 0. ADB, if present, is interface 1.
            if d.interface_number() != 0 || d.setting_number() != 0 {
                continue;
            }
            let input = d.endpoint_descriptors().find(|e| {
                e.transfer_type() == TransferType::Bulk && e.direction() == Direction::In
            });
            let output = d.endpoint_descriptors().find(|e| {
                e.transfer_type() == TransferType::Bulk && e.direction() == Direction::Out
            });
            if let (Some(input), Some(output)) = (input, output) {
                handle
                    .claim_interface(d.interface_number())
                    .map_err(usb_error)?;
                return Ok((handle, input.address(), output.address(), serial));
            }
        }
    }
    Err(Error::Accessory("No accessory bulk endpoints found".into()))
}

pub(crate) async fn connect(mode: crate::TransportMode) -> Result<Transport> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _guard = DiscoveryGuard(cancelled.clone());
    let (usb, input, output, serial) = tokio::task::spawn_blocking(move || open_usb(&cancelled))
        .await
        .map_err(|e| Error::Accessory(e.to_string()))??;
    let mut transport = bridge(usb, input, output, serial)?;
    crate::FrameWriter::new(&mut transport.control)
        .write_frame(
            Channel::Control,
            ControlKind::HelloRequest as u8,
            0,
            0,
            serde_json::json!({ "transport": mode })
                .to_string()
                .as_bytes(),
        )
        .await?;
    Ok(transport)
}
fn pair() -> Result<(TransportStream, UnixStream)> {
    let (host, bridge) = UnixStream::pair()?;
    host.set_nonblocking(true)?;
    bridge.set_write_timeout(Some(Duration::from_secs(2)))?;
    Ok((Box::new(tokio::net::UnixStream::from_std(host)?), bridge))
}
fn bridge(
    usb: DeviceHandle<GlobalContext>,
    input: u8,
    output: u8,
    serial: String,
) -> Result<Transport> {
    let (control, control_bridge) = pair()?;
    let (video, video_bridge) = pair()?;
    let (camera, camera_bridge) = pair()?;
    let cleanup = vec![
        control_bridge.try_clone()?,
        video_bridge.try_clone()?,
        camera_bridge.try_clone()?,
    ];
    let read_control = control_bridge.try_clone()?;
    let stopped = Arc::new(AtomicBool::new(false));
    let usb = Arc::new(usb);
    let lock = Arc::new(WriterGate::default());
    let session = Arc::new(AccessoryHandle {
        stopped: stopped.clone(),
        sockets: cleanup,
        usb: usb.clone(),
        output,
        writer_lock: lock.clone(),
    });
    for (is_control, mut stream) in [(true, control_bridge), (false, video_bridge)] {
        let usb = usb.clone();
        let stopped = stopped.clone();
        let lock = lock.clone();
        let fail_sockets = session
            .sockets
            .iter()
            .map(UnixStream::try_clone)
            .collect::<std::io::Result<Vec<_>>>()?;
        std::thread::spawn(move || {
            let result = (|| -> std::io::Result<()> {
                while !stopped.load(Ordering::Relaxed) {
                    let (head, payload) = read_frame(&mut stream)?;
                    let header = Header::decode(&head).map_err(std::io::Error::other)?;
                    let awaiting_permission = header.channel == Channel::Control
                        && header.kind == ControlKind::HelloRequest as u8;
                    let consent_deadline = Instant::now() + Duration::from_secs(60);
                    let pieces = if is_control {
                        vec![(head, 0..payload.len())]
                    } else {
                        video_fragments(&header, &payload)
                    };
                    for (piece_head, range) in pieces {
                        // Released between fragments, so a control frame waits
                        // at most one fragment behind video.
                        let _guard = lock.acquire(is_control, &stopped)?;
                        // One buffer per piece: a separate 20-byte header write
                        // would cost its own USB transfer on every frame.
                        let mut wire = Vec::with_capacity(HEADER_LEN + range.len());
                        wire.extend_from_slice(&piece_head);
                        wire.extend_from_slice(&payload[range]);
                        {
                            for chunk in wire.chunks(USB_PACKET + HEADER_LEN) {
                                let mut rest = chunk;
                                while !rest.is_empty() {
                                    if stopped.load(Ordering::Relaxed) {
                                        return Err(std::io::ErrorKind::ConnectionAborted.into());
                                    }
                                    let n = match usb.write_bulk(output, rest, IO_TIMEOUT) {
                                        Ok(n) => n,
                                        // Android does not consume the endpoint until Allow.
                                        // rusb returns Ok(n) for partial timeout transfers, so
                                        // this retry never resends bytes already transferred.
                                        Err(rusb::Error::Timeout)
                                            if awaiting_permission
                                                && Instant::now() < consent_deadline =>
                                        {
                                            continue
                                        }
                                        Err(e) => return Err(std::io::Error::other(e)),
                                    };
                                    if n == 0 {
                                        return Err(std::io::ErrorKind::WriteZero.into());
                                    }
                                    rest = &rest[n..];
                                }
                            }
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                tracing::debug!(error = %e, "accessory writer closed");
            }
            stopped.store(true, Ordering::Relaxed);
            for s in fail_sockets {
                let _ = s.shutdown(Shutdown::Both);
            }
        });
    }
    let fail_sockets = session
        .sockets
        .iter()
        .map(UnixStream::try_clone)
        .collect::<std::io::Result<Vec<_>>>()?;
    std::thread::spawn(move || {
        let result = route_incoming(
            &mut BulkReader {
                usb,
                endpoint: input,
                stopped: stopped.clone(),
                buf: vec![0; USB_PACKET],
                pos: 0,
                end: 0,
            },
            read_control,
            camera_bridge,
        );
        if let Err(e) = result {
            tracing::debug!(error = %e, "accessory reader closed");
        }
        stopped.store(true, Ordering::Relaxed);
        for s in fail_sockets {
            let _ = s.shutdown(Shutdown::Both);
        }
    });
    Ok(Transport {
        hello: None,
        device: Device {
            serial,
            state: DeviceState::Ready,
            model: Some("USB_Accessory".into()),
        },
        control,
        video,
        camera,
        adb: Adb::none(),
        simulated: false,
        accessory: Some(session),
    })
}
fn read_frame(reader: &mut impl Read) -> std::io::Result<([u8; HEADER_LEN], Vec<u8>)> {
    let mut head = [0; HEADER_LEN];
    reader.read_exact(&mut head)?;
    let header = Header::decode(&head).map_err(std::io::Error::other)?;
    let mut payload = vec![0; header.len as usize];
    reader.read_exact(&mut payload)?;
    Ok((head, payload))
}
fn route_incoming(
    reader: &mut impl Read,
    mut control: impl Write,
    mut camera: impl Write,
) -> std::io::Result<()> {
    loop {
        let (head, payload) = read_frame(reader)?;
        let header = Header::decode(&head).map_err(std::io::Error::other)?;
        let dest: &mut dyn Write = match header.channel {
            Channel::Control | Channel::Touch => &mut control,
            Channel::CameraUp => &mut camera,
            _ => return Err(std::io::Error::other("unexpected accessory channel")),
        };
        dest.write_all(&head)?;
        dest.write_all(&payload)?;
    }
}
struct BulkReader {
    usb: Arc<DeviceHandle<GlobalContext>>,
    endpoint: u8,
    stopped: Arc<AtomicBool>,
    buf: Vec<u8>,
    pos: usize,
    end: usize,
}
impl Read for BulkReader {
    fn read(&mut self, dest: &mut [u8]) -> std::io::Result<usize> {
        if dest.is_empty() {
            return Ok(0);
        }
        while self.pos == self.end {
            if self.stopped.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::ConnectionAborted.into());
            }
            match self.usb.read_bulk(self.endpoint, &mut self.buf, IO_TIMEOUT) {
                Ok(n) => {
                    self.pos = 0;
                    self.end = n;
                }
                Err(rusb::Error::Timeout) => continue,
                Err(e) => return Err(std::io::Error::other(e)),
            }
        }
        let n = dest.len().min(self.end - self.pos);
        dest[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}
#[cfg(test)]
mod tests {
    use super::{video_fragments, WriterGate, VIDEO_FRAGMENT};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use xs_proto::{flags, Channel, Header};

    #[test]
    fn large_video_frames_split_and_keep_flags_on_the_last_piece() {
        let header = Header {
            channel: Channel::VideoDown,
            kind: 0,
            flags: flags::KEYFRAME,
            len: (VIDEO_FRAGMENT * 2 + 5) as u32,
            pts_us: 42,
        };
        let pieces = video_fragments(&header, &vec![0; VIDEO_FRAGMENT * 2 + 5]);
        let decoded: Vec<_> = pieces
            .iter()
            .map(|(h, r)| (Header::decode(h).unwrap(), r.clone()))
            .collect();
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].0.flags, flags::KEYFRAME | flags::FRAGMENT);
        assert_eq!(decoded[1].0.flags, flags::KEYFRAME | flags::FRAGMENT);
        assert_eq!(decoded[2].0.flags, flags::KEYFRAME);
        assert_eq!(decoded[2].1, VIDEO_FRAGMENT * 2..VIDEO_FRAGMENT * 2 + 5);
        assert!(decoded
            .iter()
            .all(|(h, r)| h.len as usize == r.len() && h.pts_us == 42));
        // A frame that fits is sent unchanged.
        let small = video_fragments(&Header { len: 5, ..header }, &[0; 5]);
        assert_eq!(Header::decode(&small[0].0).unwrap().flags, flags::KEYFRAME);
    }

    #[test]
    fn waiting_control_goes_before_waiting_video() {
        let gate = Arc::new(WriterGate::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let held = gate.acquire(false, &stopped).unwrap();
        let (order_tx, order) = mpsc::channel();
        let spawn = |control: bool, delay: u64| {
            let (gate, stopped, order_tx) = (gate.clone(), stopped.clone(), order_tx.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(delay));
                let _guard = gate.acquire(control, &stopped).unwrap();
                order_tx.send(control).unwrap();
            })
        };
        // Video queues first, then control; control must still win.
        let video = spawn(false, 0);
        let control = spawn(true, 50);
        std::thread::sleep(Duration::from_millis(150));
        drop(held);
        assert!(order.recv_timeout(Duration::from_secs(1)).unwrap());
        assert!(!order.recv_timeout(Duration::from_secs(1)).unwrap());
        video.join().unwrap();
        control.join().unwrap();
    }

    #[test]
    fn stopping_releases_a_waiting_writer() {
        let gate = WriterGate::default();
        let stopped = AtomicBool::new(false);
        let _held = gate.acquire(true, &stopped).unwrap();
        stopped.store(true, Ordering::Relaxed);
        assert!(gate.acquire(false, &stopped).is_err());
    }

    #[test]
    fn cancelled_discovery_stops_before_probing_or_switching_usb() {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = super::DiscoveryGuard(cancelled.clone());
        drop(guard);
        assert!(
            matches!(super::open_usb(&cancelled), Err(crate::Error::Accessory(message)) if message.contains("cancelled"))
        );
    }

    use super::*;
    fn frame(channel: Channel, payload: &[u8]) -> Vec<u8> {
        let mut v = Header {
            channel,
            kind: 0,
            flags: 0,
            len: payload.len() as u32,
            pts_us: 42,
        }
        .encode()
        .to_vec();
        v.extend_from_slice(payload);
        v
    }
    #[test]
    fn multiplexed_frames_route_to_the_correct_stream_without_losing_boundaries() {
        let control = frame(Channel::Control, b"hello");
        let touch = frame(Channel::Touch, &[0; 21]);
        let camera = frame(Channel::CameraUp, &[1; 33000]);
        let mut input =
            std::io::Cursor::new([control.clone(), camera.clone(), touch.clone()].concat());
        let mut c = Vec::new();
        let mut v = Vec::new();
        assert!(route_incoming(&mut input, &mut c, &mut v).is_err()); // EOF after complete frames
        assert_eq!(c, [control, touch].concat());
        assert_eq!(v, camera);
    }
    #[test]
    fn malformed_accessory_data_is_rejected_before_allocating_a_payload() {
        assert!(read_frame(&mut std::io::Cursor::new(vec![255; HEADER_LEN])).is_err());
        let h = Header {
            channel: Channel::Control,
            kind: 0,
            flags: 0,
            len: u32::MAX,
            pts_us: 0,
        }
        .encode();
        assert!(read_frame(&mut std::io::Cursor::new(h)).is_err());
        assert!(!accessory(0x18d1, 0x2d02));
        assert!(accessory(0x18d1, 0x2d01));
    }
}
