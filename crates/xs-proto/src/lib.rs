//! The extraspace wire protocol, shared by the Linux daemon and the Android app.
//!
//! ADB uses separate control/touch, video and camera sockets so a large video
//! frame cannot queue a latency-sensitive touch. Android Open Accessory (AOA)
//! multiplexes those channels over one USB bulk stream. The channel byte routes
//! each frame to its consumer; it is required routing information on AOA and
//! validates the stream on ADB. AOA sends Hello only in response to HelloRequest;
//! ADB sends Hello when the control socket connects.
//!
//! Every frame carries a fixed 20-byte header, little-endian:
//!
//! ```text
//!  0..4   magic  u32   always MAGIC
//!  4      channel u8   Channel
//!  5      kind    u8   message kind, interpreted per-channel
//!  6..8   flags   u16  Flags bitfield
//!  8..12  len     u32  payload length
//! 12..20  pts_us  u64  presentation timestamp, microseconds
//! ```
//!
//! The Kotlin side mirrors this in `Protocol.kt`. Shared golden vectors under
//! `protocol/` check both implementations. Magic only detects a
//! different protocol identifier; it cannot detect every layout mismatch.

use serde::{Deserialize, Serialize};

/// Reads as the ASCII bytes `XSPA` on the wire (little-endian).
pub const MAGIC: u32 = 0x4150_5358;

/// Bytes in a frame header.
pub const HEADER_LEN: usize = 20;

/// Refuse absurd frames early rather than trying to allocate them.
pub const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

/// Allowed USB methods. Auto permits both; ADB is preferred for its independent
/// channels, which avoid queuing input behind video on the accessory bulk link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    #[default]
    Auto,
    Adb,
    Accessory,
}

impl TransportMode {
    pub fn allows(self, method: Self) -> bool {
        method != Self::Auto && (self == Self::Auto || self == method)
    }

    pub fn select(self, peer: Self) -> Result<Self, &'static str> {
        match (self, peer) {
            (Self::Auto, Self::Auto) => Ok(Self::Adb),
            (Self::Auto, mode) | (mode, Self::Auto) => Ok(mode),
            (a, b) if a == b => Ok(a),
            _ => Err("Incompatible connection methods: one app allows only ADB and the other only USB accessory. Select a shared method or Automatic in both apps."),
        }
    }

    /// ADB may discover an accessory-only choice. Accessory fallback must still
    /// be allowed by both selectors, including when ADB discovery is unavailable.
    pub fn select_for_link(self, peer: Self, link: Self) -> Result<Self, &'static str> {
        let selected = self.select(peer)?;
        match link {
            Self::Adb => Ok(selected),
            Self::Accessory if self.allows(link) && peer.allows(link) => Ok(link),
            Self::Accessory => Err("Incompatible connection methods: the shared method is ADB, but the device is in USB accessory mode. Reconnect with USB debugging enabled."),
            Self::Auto => Err("Discovery requires a concrete connection method"),
        }
    }
}

/// Default TCP ports, forwarded over adb. Chosen to sit just above scrcpy's 27183
/// so the two can run side by side.
pub mod ports {
    pub const CONTROL: u16 = 27183;
    pub const VIDEO: u16 = 27184;
    pub const CAMERA: u16 = 27185;
}

/// Which logical stream a frame belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Channel {
    Control = 0,
    Touch = 1,
    VideoDown = 2,
    CameraUp = 3,
}

impl Channel {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Control,
            1 => Self::Touch,
            2 => Self::VideoDown,
            3 => Self::CameraUp,
            _ => return None,
        })
    }
}

/// Frame flags.
pub mod flags {
    /// Payload is a keyframe (IDR). Set on video/camera frames.
    pub const KEYFRAME: u16 = 1 << 0;
    /// Payload is codec configuration (SPS/PPS), not a displayable frame.
    pub const CODEC_CONFIG: u16 = 1 << 1;
    /// More of this video frame follows in the next frame on the channel.
    /// Only the USB accessory bridge splits frames, so control can interleave.
    pub const FRAGMENT: u16 = 1 << 2;
}

/// Message kinds on [`Channel::Control`].
///
/// Hello / VideoConfig / Stats / CameraControl / Error are JSON: they are rare
/// and being able to read them in a log is worth more than the bytes saved.
/// [`ControlKind::Cursor`] is binary -- it can arrive at the panel refresh rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControlKind {
    /// Device -> host, first message: identifies the tablet.
    Hello = 0,
    /// Host -> device: display stream is about to start with these parameters.
    VideoConfig = 1,
    /// Device -> host, periodic: decode health, drives adaptive bitrate.
    Stats = 2,
    /// Host -> device: begin/end camera capture.
    CameraControl = 3,
    /// Either direction: liveness probe, echoed back with the same `pts_us`.
    Ping = 4,
    Pong = 5,
    /// Either direction: fatal error, connection is about to close.
    Error = 6,
    /// Host -> device: cursor overlay. Binary; see [`CursorMessage`].
    Cursor = 7,
    /// Host -> accessory: request Hello on initial connection or reconnection.
    HelloRequest = 8,
    /// Host -> accessory: stop displaying without physically unplugging USB.
    SessionEnd = 9,
    /// Device -> host: camera permission, capture and error state.
    CameraStatus = 10,
    /// Host -> device: open the companion settings screen on the tablet.
    OpenSettings = 11,
}

impl ControlKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Hello,
            1 => Self::VideoConfig,
            2 => Self::Stats,
            3 => Self::CameraControl,
            4 => Self::Ping,
            5 => Self::Pong,
            6 => Self::Error,
            7 => Self::Cursor,
            8 => Self::HelloRequest,
            9 => Self::SessionEnd,
            10 => Self::CameraStatus,
            11 => Self::OpenSettings,
            _ => return None,
        })
    }
}

/// Flags for [`CursorMessage`].
pub mod cursor_flags {
    pub const VISIBLE: u8 = 1 << 0;
    pub const POSITION: u8 = 1 << 1;
    pub const HOTSPOT: u8 = 1 << 2;
    pub const BITMAP: u8 = 1 << 3;
}

/// Packed BGRA cursor sprite, `width * 4` bytes per row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorBitmap {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u8>,
}

/// Host -> device cursor overlay update.
///
/// ```text
/// 0      flags  u8    cursor_flags
/// [if POSITION] x i32  y i32     hotspot location in stream pixels
/// [if HOTSPOT]  hx i16 hy i16    hotspot offset inside the sprite
/// [if BITMAP]   w u16  h u16     then w*h*4 BGRA pixels
/// ```
///
/// Position-only updates must not carry a bitmap. The tablet keeps the last
/// sprite until a new one arrives or the cursor is hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorMessage {
    pub visible: bool,
    pub position: Option<(i32, i32)>,
    pub hotspot: Option<(i16, i16)>,
    pub bitmap: Option<CursorBitmap>,
}

impl CursorMessage {
    pub fn hide() -> Self {
        Self {
            visible: false,
            position: None,
            hotspot: None,
            bitmap: None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut flags = 0u8;
        if self.visible {
            flags |= cursor_flags::VISIBLE;
        }
        if self.position.is_some() {
            flags |= cursor_flags::POSITION;
        }
        if self.hotspot.is_some() {
            flags |= cursor_flags::HOTSPOT;
        }
        if self.bitmap.is_some() {
            flags |= cursor_flags::BITMAP;
        }
        let mut len = 1;
        if self.position.is_some() {
            len += 8;
        }
        if self.hotspot.is_some() {
            len += 4;
        }
        if let Some(bitmap) = &self.bitmap {
            len += 4 + bitmap.pixels.len();
        }
        let mut buf = Vec::with_capacity(len);
        buf.push(flags);
        if let Some((x, y)) = self.position {
            buf.extend_from_slice(&x.to_le_bytes());
            buf.extend_from_slice(&y.to_le_bytes());
        }
        if let Some((hx, hy)) = self.hotspot {
            buf.extend_from_slice(&hx.to_le_bytes());
            buf.extend_from_slice(&hy.to_le_bytes());
        }
        if let Some(bitmap) = &self.bitmap {
            buf.extend_from_slice(&bitmap.width.to_le_bytes());
            buf.extend_from_slice(&bitmap.height.to_le_bytes());
            buf.extend_from_slice(&bitmap.pixels);
        }
        buf
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None;
        }
        let flags = buf[0];
        let mut i = 1;
        let position = if flags & cursor_flags::POSITION != 0 {
            if i + 8 > buf.len() {
                return None;
            }
            let x = i32::from_le_bytes(buf[i..i + 4].try_into().ok()?);
            let y = i32::from_le_bytes(buf[i + 4..i + 8].try_into().ok()?);
            i += 8;
            Some((x, y))
        } else {
            None
        };
        let hotspot = if flags & cursor_flags::HOTSPOT != 0 {
            if i + 4 > buf.len() {
                return None;
            }
            let hx = i16::from_le_bytes(buf[i..i + 2].try_into().ok()?);
            let hy = i16::from_le_bytes(buf[i + 2..i + 4].try_into().ok()?);
            i += 4;
            Some((hx, hy))
        } else {
            None
        };
        let bitmap = if flags & cursor_flags::BITMAP != 0 {
            if i + 4 > buf.len() {
                return None;
            }
            let width = u16::from_le_bytes(buf[i..i + 2].try_into().ok()?);
            let height = u16::from_le_bytes(buf[i + 2..i + 4].try_into().ok()?);
            i += 4;
            let pixels_len = (width as usize)
                .checked_mul(height as usize)?
                .checked_mul(4)?;
            if i + pixels_len != buf.len() {
                return None;
            }
            Some(CursorBitmap {
                width,
                height,
                pixels: buf[i..].to_vec(),
            })
        } else if i != buf.len() {
            return None;
        } else {
            None
        };
        Some(Self {
            visible: flags & cursor_flags::VISIBLE != 0,
            position,
            hotspot,
            bitmap,
        })
    }
}

/// First message from the tablet. The host uses `width`/`height`/`density` to size
/// the virtual monitor to the panel exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    /// The Android selector, independent of the link used for discovery.
    #[serde(default)]
    pub transport: TransportMode,
    pub device_name: String,
    /// Installation UUID shared across ADB and accessory transports. Older apps omit it.
    #[serde(default)]
    pub device_id: Option<String>,
    pub android_release: String,
    pub width: u32,
    pub height: u32,
    pub density_dpi: u32,
    pub refresh_rate: f64,
    /// Camera ids the tablet can offer, e.g. `["0", "1"]`.
    pub cameras: Vec<CameraInfo>,
}

pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraInfo {
    pub id: String,
    /// `"back"`, `"front"` or `"external"`.
    pub facing: String,
    pub max_width: u32,
    pub max_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
    /// Always `"h264"` for now; present so the tablet can reject what it cannot decode.
    pub codec: String,
}

/// Periodic health report that drives the adaptive controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    /// Frames waiting in the decoder input queue. Sustained growth means the
    /// tablet cannot keep up and bitrate should come down.
    pub decode_queue_depth: u32,
    pub frames_decoded: u64,
    pub frames_dropped: u64,
    /// Host PTS of the most recent frame reported rendered on the decoder surface.
    pub last_frame_pts_us: u64,
    /// Device clock reported by the codec for that surface render, in microseconds.
    /// This does not measure physical display scanout; callbacks may be delayed.
    pub rendered_at_us: u64,
    /// Latest or rolling-average hardware decode time on the tablet, in microseconds.
    #[serde(default)]
    pub decode_time_us: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraControl {
    pub enabled: bool,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraState {
    Off,
    Pending,
    Running,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraStatus {
    pub state: CameraState,
    pub message: String,
}

/// Discriminator in [`Header::kind`] on [`Channel::Touch`].
pub const TOUCH_KIND_FINGER: u8 = 0;
pub const TOUCH_KIND_STYLUS: u8 = 1;

/// A single touch point, sent on [`Channel::Touch`] with `kind == TOUCH_KIND_FINGER`.
///
/// Encoded as a fixed 21-byte payload rather than JSON: these arrive at up to
/// 120 Hz per finger and the parse cost matters.
///
/// ```text
/// 0      action u8   TouchAction
/// 1..5   slot   u32
/// 5..13  x      f64  stream coordinates
/// 13..21 y      f64
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchEvent {
    pub action: TouchAction,
    pub slot: u32,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TouchAction {
    Down = 0,
    Motion = 1,
    Up = 2,
}

impl TouchAction {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Down,
            1 => Self::Motion,
            2 => Self::Up,
            _ => return None,
        })
    }
}

pub const TOUCH_PAYLOAD_LEN: usize = 21;

impl TouchEvent {
    pub fn encode(&self) -> [u8; TOUCH_PAYLOAD_LEN] {
        let mut buf = [0u8; TOUCH_PAYLOAD_LEN];
        buf[0] = self.action as u8;
        buf[1..5].copy_from_slice(&self.slot.to_le_bytes());
        buf[5..13].copy_from_slice(&self.x.to_le_bytes());
        buf[13..21].copy_from_slice(&self.y.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() != TOUCH_PAYLOAD_LEN {
            return None;
        }
        Some(Self {
            action: TouchAction::from_u8(buf[0])?,
            slot: u32::from_le_bytes(buf[1..5].try_into().ok()?),
            x: f64::from_le_bytes(buf[5..13].try_into().ok()?),
            y: f64::from_le_bytes(buf[13..21].try_into().ok()?),
        })
    }
}

/// Action for a stylus event sent on [`Channel::Touch`] with `kind == TOUCH_KIND_STYLUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StylusAction {
    Hover = 0,
    Down = 1,
    Motion = 2,
    Up = 3,
    HoverExit = 4,
}

impl StylusAction {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Hover,
            1 => Self::Down,
            2 => Self::Motion,
            3 => Self::Up,
            4 => Self::HoverExit,
            _ => return None,
        })
    }
}

/// Active stylus end (pen tip vs. eraser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StylusTool {
    Pen = 0,
    Eraser = 1,
}

impl StylusTool {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Pen,
            1 => Self::Eraser,
            _ => return None,
        })
    }
}

/// Bitmask flags for [`StylusEvent::buttons`].
pub mod stylus_buttons {
    pub const PRIMARY: u8 = 1 << 0;
    pub const SECONDARY: u8 = 1 << 1;
    pub const TERTIARY: u8 = 1 << 2;
}

pub const STYLUS_PAYLOAD_LEN: usize = 32;

/// High-frequency stylus packet with hover, pressure, tilt, and barrel buttons.
///
/// ```text
/// 0      action   u8   StylusAction
/// 1      tool     u8   StylusTool
/// 2      buttons  u8   stylus_buttons bitmask
/// 3      reserved u8   0
/// 4..12  x        f64  normalized 0.0..=1.0
/// 12..20 y        f64  normalized 0.0..=1.0
/// 20..24 pressure f32  normalized 0.0..=1.0
/// 24..28 tilt_x   f32  normalized -1.0..=1.0 (-90..=+90 deg)
/// 28..32 tilt_y   f32  normalized -1.0..=1.0 (-90..=+90 deg)
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StylusEvent {
    pub action: StylusAction,
    pub tool: StylusTool,
    pub buttons: u8,
    pub x: f64,
    pub y: f64,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
}

impl StylusEvent {
    pub fn encode(&self) -> [u8; STYLUS_PAYLOAD_LEN] {
        let mut buf = [0u8; STYLUS_PAYLOAD_LEN];
        buf[0] = self.action as u8;
        buf[1] = self.tool as u8;
        buf[2] = self.buttons;
        buf[3] = 0;
        buf[4..12].copy_from_slice(&self.x.to_le_bytes());
        buf[12..20].copy_from_slice(&self.y.to_le_bytes());
        buf[20..24].copy_from_slice(&self.pressure.to_le_bytes());
        buf[24..28].copy_from_slice(&self.tilt_x.to_le_bytes());
        buf[28..32].copy_from_slice(&self.tilt_y.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() != STYLUS_PAYLOAD_LEN || buf[3] != 0 {
            return None;
        }
        let action = StylusAction::from_u8(buf[0])?;
        let tool = StylusTool::from_u8(buf[1])?;
        let buttons = buf[2];
        let x = f64::from_le_bytes(buf[4..12].try_into().ok()?);
        let y = f64::from_le_bytes(buf[12..20].try_into().ok()?);
        let pressure = f32::from_le_bytes(buf[20..24].try_into().ok()?);
        let tilt_x = f32::from_le_bytes(buf[24..28].try_into().ok()?);
        let tilt_y = f32::from_le_bytes(buf[28..32].try_into().ok()?);
        if !x.is_finite()
            || !y.is_finite()
            || !pressure.is_finite()
            || !tilt_x.is_finite()
            || !tilt_y.is_finite()
        {
            return None;
        }
        Some(Self {
            action,
            tool,
            buttons,
            x,
            y,
            pressure: pressure.clamp(0.0, 1.0),
            tilt_x: tilt_x.clamp(-1.0, 1.0),
            tilt_y: tilt_y.clamp(-1.0, 1.0),
        })
    }
}

/// Parsed frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub channel: Channel,
    pub kind: u8,
    pub flags: u16,
    pub len: u32,
    pub pts_us: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtoError {
    #[error("bad magic {0:#010x}: peer is not speaking extraspace, or the stream desynced")]
    BadMagic(u32),
    #[error("unknown channel {0}")]
    UnknownChannel(u8),
    #[error("payload of {0} bytes exceeds the {MAX_PAYLOAD} byte limit")]
    PayloadTooLarge(u32),
    #[error("need {needed} bytes for a header, got {got}")]
    Short { needed: usize, got: usize },
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        buf[4] = self.channel as u8;
        buf[5] = self.kind;
        buf[6..8].copy_from_slice(&self.flags.to_le_bytes());
        buf[8..12].copy_from_slice(&self.len.to_le_bytes());
        buf[12..20].copy_from_slice(&self.pts_us.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Result<Self, ProtoError> {
        if buf.len() < HEADER_LEN {
            return Err(ProtoError::Short {
                needed: HEADER_LEN,
                got: buf.len(),
            });
        }
        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != MAGIC {
            return Err(ProtoError::BadMagic(magic));
        }
        let channel = Channel::from_u8(buf[4]).ok_or(ProtoError::UnknownChannel(buf[4]))?;
        let len = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        if len > MAX_PAYLOAD {
            return Err(ProtoError::PayloadTooLarge(len));
        }
        Ok(Self {
            channel,
            kind: buf[5],
            flags: u16::from_le_bytes(buf[6..8].try_into().unwrap()),
            len,
            pts_us: u64::from_le_bytes(buf[12..20].try_into().unwrap()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_transport_selector_matrix() {
        let rows: Vec<_> = include_str!("../../../protocol/transport-selection.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .collect();
        assert_eq!(rows.len(), 9);
        let parse =
            |value: &str| serde_json::from_str::<TransportMode>(&format!("\"{value}\"")).unwrap();
        for row in rows {
            let fields: Vec<_> = row.split('\t').collect();
            let selected = parse(fields[0]).select(parse(fields[1]));
            if fields[2] == "incompatible" {
                assert!(
                    selected
                        .unwrap_err()
                        .starts_with("Incompatible connection methods:"),
                    "{row}"
                );
            } else {
                assert_eq!(selected.unwrap(), parse(fields[2]), "{row}");
            }
        }
        assert!(serde_json::from_str::<TransportMode>("\"bogus\"").is_err());
    }

    #[test]
    fn accessory_fallback_never_bypasses_an_adb_only_selector() {
        for host in [
            TransportMode::Auto,
            TransportMode::Adb,
            TransportMode::Accessory,
        ] {
            for device in [
                TransportMode::Auto,
                TransportMode::Adb,
                TransportMode::Accessory,
            ] {
                let actual = host.select_for_link(device, TransportMode::Accessory);
                if host.allows(TransportMode::Accessory) && device.allows(TransportMode::Accessory)
                {
                    assert_eq!(actual.unwrap(), TransportMode::Accessory);
                } else {
                    assert!(actual
                        .unwrap_err()
                        .starts_with("Incompatible connection methods:"));
                }
            }
        }
        assert_eq!(
            TransportMode::Auto
                .select_for_link(TransportMode::Accessory, TransportMode::Adb)
                .unwrap(),
            TransportMode::Accessory
        );
        assert_eq!(
            TransportMode::Accessory
                .select_for_link(TransportMode::Auto, TransportMode::Adb)
                .unwrap(),
            TransportMode::Accessory
        );
    }

    #[test]
    fn header_roundtrips() {
        let h = Header {
            channel: Channel::VideoDown,
            kind: 7,
            flags: flags::KEYFRAME,
            len: 4096,
            pts_us: 1_234_567_890,
        };
        assert_eq!(Header::decode(&h.encode()).unwrap(), h);
    }

    #[test]
    fn header_rejects_foreign_data() {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..4].copy_from_slice(&0xdead_beefu32.to_le_bytes());
        assert_eq!(Header::decode(&buf), Err(ProtoError::BadMagic(0xdead_beef)));
    }

    #[test]
    fn header_rejects_oversized_payload() {
        let h = Header {
            channel: Channel::Control,
            kind: 0,
            flags: 0,
            len: MAX_PAYLOAD + 1,
            pts_us: 0,
        };
        assert_eq!(
            Header::decode(&h.encode()),
            Err(ProtoError::PayloadTooLarge(MAX_PAYLOAD + 1))
        );
    }

    #[test]
    fn touch_roundtrips_with_subpixel_precision() {
        let t = TouchEvent {
            action: TouchAction::Motion,
            slot: 3,
            x: 1234.5678,
            y: 987.6543,
        };
        assert_eq!(TouchEvent::decode(&t.encode()).unwrap(), t);
    }

    #[test]
    fn touch_rejects_truncated_and_extended_payloads() {
        let event = TouchEvent {
            action: TouchAction::Down,
            slot: 0,
            x: 1.0,
            y: 2.0,
        };
        let encoded = event.encode();
        for length in 0..TOUCH_PAYLOAD_LEN {
            assert!(TouchEvent::decode(&encoded[..length]).is_none());
        }
        let mut extended = encoded.to_vec();
        extended.push(0);
        assert!(TouchEvent::decode(&extended).is_none());
    }

    #[test]
    fn magic_reads_as_xspa_on_the_wire() {
        assert_eq!(&MAGIC.to_le_bytes(), b"XSPA");
    }

    #[test]
    fn cursor_hide_is_one_zero_byte() {
        let msg = CursorMessage::hide();
        let bytes = msg.encode();
        assert_eq!(bytes, [0]);
        assert_eq!(CursorMessage::decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn cursor_move_does_not_carry_a_bitmap() {
        let msg = CursorMessage {
            visible: true,
            position: Some((1316, 10)),
            hotspot: None,
            bitmap: None,
        };
        let decoded = CursorMessage::decode(&msg.encode()).unwrap();
        assert_eq!(decoded, msg);
        assert!(decoded.bitmap.is_none());
    }

    #[test]
    fn cursor_sprite_roundtrips() {
        let msg = CursorMessage {
            visible: true,
            position: Some((40, 50)),
            hotspot: Some((3, 4)),
            bitmap: Some(CursorBitmap {
                width: 2,
                height: 1,
                pixels: vec![1, 2, 3, 255, 4, 5, 6, 128],
            }),
        };
        assert_eq!(CursorMessage::decode(&msg.encode()).unwrap(), msg);
    }

    #[test]
    fn stylus_roundtrips_with_pressure_and_tilt() {
        let s = StylusEvent {
            action: StylusAction::Motion,
            tool: StylusTool::Pen,
            buttons: stylus_buttons::PRIMARY | stylus_buttons::SECONDARY,
            x: 0.5,
            y: 0.25,
            pressure: 0.75,
            tilt_x: 0.125,
            tilt_y: -0.25,
        };
        assert_eq!(StylusEvent::decode(&s.encode()).unwrap(), s);
    }

    #[test]
    fn stylus_rejects_invalid_and_non_finite_payloads() {
        let s = StylusEvent {
            action: StylusAction::Hover,
            tool: StylusTool::Eraser,
            buttons: 0,
            x: 0.25,
            y: 0.75,
            pressure: 0.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
        };
        let encoded = s.encode();
        for length in 0..STYLUS_PAYLOAD_LEN {
            assert!(StylusEvent::decode(&encoded[..length]).is_none());
        }
        let mut bad_reserved = encoded;
        bad_reserved[3] = 1;
        assert!(StylusEvent::decode(&bad_reserved).is_none());
        let mut bad_action = encoded;
        bad_action[0] = 99;
        assert!(StylusEvent::decode(&bad_action).is_none());
    }
}

#[cfg(test)]
mod golden_tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn shared_wire_vectors() {
        assert_eq!(PROTOCOL_VERSION, 2);
        assert_eq!(
            [
                Channel::Control as u8,
                Channel::Touch as u8,
                Channel::VideoDown as u8,
                Channel::CameraUp as u8
            ],
            [0, 1, 2, 3]
        );
        assert_eq!(
            [
                TouchAction::Down as u8,
                TouchAction::Motion as u8,
                TouchAction::Up as u8
            ],
            [0, 1, 2]
        );
        assert_eq!(
            [
                StylusAction::Hover as u8,
                StylusAction::Down as u8,
                StylusAction::Motion as u8,
                StylusAction::Up as u8,
                StylusAction::HoverExit as u8
            ],
            [0, 1, 2, 3, 4]
        );
        assert_eq!([StylusTool::Pen as u8, StylusTool::Eraser as u8], [0, 1]);
        assert_eq!(
            [
                ControlKind::Hello as u8,
                ControlKind::VideoConfig as u8,
                ControlKind::Stats as u8,
                ControlKind::CameraControl as u8,
                ControlKind::Ping as u8,
                ControlKind::Pong as u8,
                ControlKind::Error as u8,
                ControlKind::Cursor as u8,
                ControlKind::HelloRequest as u8,
                ControlKind::SessionEnd as u8,
                ControlKind::CameraStatus as u8,
                ControlKind::OpenSettings as u8
            ],
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
        );
        assert_eq!(
            [flags::KEYFRAME, flags::CODEC_CONFIG, flags::FRAGMENT],
            [1, 2, 4]
        );
        assert_eq!(
            [
                cursor_flags::VISIBLE,
                cursor_flags::POSITION,
                cursor_flags::HOTSPOT,
                cursor_flags::BITMAP
            ],
            [1, 2, 4, 8]
        );
        for line in include_str!("../../../protocol/golden-vectors.tsv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let f: Vec<_> = line.split('\t').collect();
            let bytes = hex(f[2]);
            match f[0] {
                "header" => {
                    let expected = Header {
                        channel: Channel::from_u8(f[3].parse().unwrap()).unwrap(),
                        kind: f[4].parse().unwrap(),
                        flags: f[5].parse().unwrap(),
                        len: f[6].parse().unwrap(),
                        pts_us: f[7].parse().unwrap(),
                    };
                    assert_eq!(Header::decode(&bytes).unwrap(), expected, "{}", f[1]);
                    assert_eq!(expected.encode().as_slice(), bytes, "{}", f[1]);
                    if expected.channel == Channel::Control {
                        assert_eq!(
                            ControlKind::from_u8(expected.kind).unwrap() as u8,
                            expected.kind
                        );
                    }
                }
                "touch" => {
                    let expected = TouchEvent {
                        action: TouchAction::from_u8(f[3].parse().unwrap()).unwrap(),
                        slot: f[4].parse().unwrap(),
                        x: f[5].parse().unwrap(),
                        y: f[6].parse().unwrap(),
                    };
                    assert_eq!(TouchEvent::decode(&bytes).unwrap(), expected, "{}", f[1]);
                    assert_eq!(expected.encode().as_slice(), bytes, "{}", f[1]);
                }
                "stylus" => {
                    let expected = StylusEvent {
                        action: StylusAction::from_u8(f[3].parse().unwrap()).unwrap(),
                        tool: StylusTool::from_u8(f[4].parse().unwrap()).unwrap(),
                        buttons: f[5].parse().unwrap(),
                        x: f[6].parse().unwrap(),
                        y: f[7].parse().unwrap(),
                        pressure: f[8].parse().unwrap(),
                        tilt_x: f[9].parse().unwrap(),
                        tilt_y: f[10].parse().unwrap(),
                    };
                    assert_eq!(StylusEvent::decode(&bytes).unwrap(), expected, "{}", f[1]);
                    assert_eq!(expected.encode().as_slice(), bytes, "{}", f[1]);
                }
                "cursor" => {
                    let expected = CursorMessage {
                        visible: f[3].parse().unwrap(),
                        position: (f[4] != "-")
                            .then(|| (f[4].parse().unwrap(), f[5].parse().unwrap())),
                        hotspot: (f[6] != "-")
                            .then(|| (f[6].parse().unwrap(), f[7].parse().unwrap())),
                        bitmap: (f[8] != "-").then(|| CursorBitmap {
                            width: f[8].parse().unwrap(),
                            height: f[9].parse().unwrap(),
                            pixels: hex(f[10]),
                        }),
                    };
                    assert_eq!(CursorMessage::decode(&bytes).unwrap(), expected, "{}", f[1]);
                    assert_eq!(expected.encode(), bytes, "{}", f[1]);
                }
                unknown => panic!("unknown golden vector {unknown}"),
            }
        }
    }
}
