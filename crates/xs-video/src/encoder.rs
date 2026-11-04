//! Picking an H.264 encoder at runtime.
//!
//! There is no single encoder we can rely on being present, and the differences
//! between them are not cosmetic -- they disagree on the *units* of the `bitrate`
//! property, which is an easy way to ship a stream 1000x the intended size.
//!
//! Measured on the development machine (i5-11400F / RTX 2060, 2000x1200 @ 60):
//!
//! | element         | fps  | bitrate honoured | ships with Fedora |
//! |-----------------|------|------------------|-------------------|
//! | `x264enc`       | 292  | yes (+-1%)       | no, plugins-ugly  |
//! | `openh264enc`   | 108  | yes              | yes               |
//! | `vulkanh264enc` | ~96  | **no**           | yes               |
//!
//! `vulkanh264enc` is deliberately excluded: it reports CBR support but produces
//! byte-identical output at 5, 15 and 40 Mbit/s, which makes adaptive control
//! impossible and pins the link at roughly 48 Mbit/s.

use gst::prelude::*;
use gstreamer as gst;
use tracing::{debug, info, warn};

/// An H.264 or H.265 (HEVC) encoder we know how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    /// Preferred software encoder: fastest, most accurate rate control.
    X264,
    /// Fallback that ships with Fedora, so the app works with no extra installs.
    OpenH264,
    /// VA-API H.264 on the GPU's low-power (VDENC) engine.
    VaH264Lp,
    /// VA-API H.264 on the general encode engine, where VDENC is absent.
    VaH264,
    /// NVIDIA NVENC H.264 using the installed nvcodec driver.
    NvH264,
    /// VA-API HEVC (H.265) on the GPU's low-power engine.
    VaH265Lp,
    /// VA-API HEVC (H.265) on the general encode engine.
    VaH265,
    /// NVIDIA NVENC HEVC (H.265) using the installed nvcodec driver.
    NvH265,
}

impl Encoder {
    pub fn element_name(self) -> &'static str {
        match self {
            Self::X264 => "x264enc",
            Self::OpenH264 => "openh264enc",
            Self::VaH264Lp => "vah264lpenc",
            Self::VaH264 => "vah264enc",
            Self::NvH264 => "nvh264enc",
            Self::VaH265Lp => "vah265lpenc",
            Self::VaH265 => "vah265enc",
            Self::NvH265 => "nvh265enc",
        }
    }

    pub fn human_name(self) -> &'static str {
        match self {
            Self::X264 => "x264 (CPU)",
            Self::OpenH264 => "OpenH264 (CPU)",
            Self::VaH264Lp => "VA-API (GPU, low-power engine)",
            Self::VaH264 => "VA-API (GPU)",
            Self::NvH264 => "NVIDIA NVENC (GPU)",
            Self::VaH265Lp => "VA-API HEVC (GPU, low-power engine)",
            Self::VaH265 => "VA-API HEVC (GPU)",
            Self::NvH265 => "NVIDIA NVENC HEVC (GPU)",
        }
    }

    /// Whether this encoder produces HEVC (`h265`) rather than AVC (`h264`).
    pub fn is_hevc(self) -> bool {
        matches!(self, Self::VaH265Lp | Self::VaH265 | Self::NvH265)
    }

    /// Wire codec name sent in `VideoConfig`.
    pub fn codec_name(self) -> &'static str {
        if self.is_hevc() {
            "h265"
        } else {
            "h264"
        }
    }

    /// GStreamer bitstream parser element name matching this encoder.
    pub fn parser_name(self) -> &'static str {
        if self.is_hevc() {
            "h265parse"
        } else {
            "h264parse"
        }
    }

    /// GStreamer caps media type for the parsed byte-stream.
    pub fn caps_media_type(self) -> &'static str {
        if self.is_hevc() {
            "video/x-h265"
        } else {
            "video/x-h264"
        }
    }

    /// Whether this encoder consumes GPU surfaces rather than system memory.
    ///
    /// At panel resolution a frame is 13 MB, and the measured cost of the whole
    /// software chain is dominated by moving those bytes rather than by the
    /// encode itself, so a GPU encoder is only worth having if the frames reach
    /// it without a trip through the CPU.
    pub fn is_gpu(self) -> bool {
        matches!(
            self,
            Self::VaH264Lp
                | Self::VaH264
                | Self::NvH264
                | Self::VaH265Lp
                | Self::VaH265
                | Self::NvH265
        )
    }

    /// Encoders in descending order of preference.
    pub const PREFERENCE: [Encoder; 2] = [Encoder::X264, Encoder::OpenH264];

    /// GPU encoders in descending order of preference. The low-power engine is
    /// first: it is a separate fixed-function block, so it leaves the render
    /// engine free for the compositing that produced the frame.
    pub const GPU_PREFERENCE: [Encoder; 2] = [Encoder::VaH264Lp, Encoder::VaH264];

    /// Whether this encoder's element is registered in the local GStreamer install.
    pub fn is_available(self) -> bool {
        gst::ElementFactory::find(self.element_name()).is_some()
            && gst::ElementFactory::find(self.parser_name()).is_some()
    }

    /// Best encoder present on this machine.
    pub fn detect() -> Option<Self> {
        let found = Self::PREFERENCE.into_iter().find(|e| e.is_available());
        match found {
            Some(Self::X264) => info!("using x264enc"),
            Some(Self::OpenH264) => warn!(
                "x264enc not found, falling back to openh264enc. For lower CPU use and better \
                 quality install gstreamer1-plugins-ugly."
            ),
            // `PREFERENCE` holds only software encoders, so nothing else can
            // come out of this search.
            _ => {}
        }
        found
    }

    /// Best GPU encoder present, or `None` when there is no VA-API plugin.
    pub fn detect_gpu() -> Option<Self> {
        let found = Self::GPU_PREFERENCE.into_iter().find(|e| e.is_available());
        if let Some(encoder) = found {
            info!(element = encoder.element_name(), "using GPU encoder");
        }
        found
    }

    /// `bitrate` is kbit/s for x264enc and the GPU encoders but bit/s for
    /// openh264enc. Getting this wrong is a 1000x error, so the conversion lives
    /// in exactly one place.
    fn bitrate_property_value(self, kbps: u32) -> u32 {
        match self {
            Self::X264
            | Self::VaH264Lp
            | Self::VaH264
            | Self::NvH264
            | Self::VaH265Lp
            | Self::VaH265
            | Self::NvH265 => kbps,
            Self::OpenH264 => kbps.saturating_mul(1000),
        }
    }

    /// Builds the encoder element configured for low-latency streaming.
    pub fn build(self, kbps: u32, framerate: u32) -> Result<gst::Element, gst::glib::BoolError> {
        self.build_factory(self.element_name(), kbps, framerate)
    }

    /// Configure only a supported factory, including a device-specific variant.
    pub(crate) fn build_factory(
        self,
        factory: &str,
        kbps: u32,
        framerate: u32,
    ) -> Result<gst::Element, gst::glib::BoolError> {
        let element = gst::ElementFactory::make(factory).build()?;
        let bitrate = self.bitrate_property_value(kbps);
        set_checked(&element, "bitrate", &bitrate.to_string())?;
        let gop = keyframe_interval_frames(framerate).to_string();
        let x264_vbv_ms = (1000 * FRAME_CAP_FRAMES / framerate.max(1))
            .max(1)
            .to_string();
        let properties: &[(&str, &str)] = match self {
            Self::X264 => &[
                ("tune", "zerolatency"),
                ("speed-preset", "veryfast"),
                ("sliced-threads", "true"),
                ("threads", "2"),
                ("byte-stream", "true"),
                ("key-int-max", &gop),
                // Bound burst size to the frame cap; ms of the bitrate.
                ("vbv-buf-capacity", &x264_vbv_ms),
                // Refresh a moving column of intra blocks instead of sending a
                // whole IDR each GOP: no periodic size spike to cross USB.
                // The first frame and requested recoveries are still IDR.
                ("intra-refresh", "true"),
            ],
            Self::OpenH264 => &[
                ("rate-control", "bitrate"),
                ("complexity", "low"),
                ("gop-size", &gop),
            ],
            Self::VaH264Lp | Self::VaH264 | Self::VaH265Lp | Self::VaH265 => &[
                ("rate-control", "cbr"),
                ("key-int-max", &gop),
                ("b-frames", "0"),
                ("ref-frames", "1"),
            ],
            Self::NvH264 | Self::NvH265 => &[
                ("rc-mode", "cbr"),
                ("gop-size", &gop),
                ("bframes", "0"),
                ("rc-lookahead", "0"),
                ("zerolatency", "true"),
                ("repeat-sequence-header", "true"),
            ],
        };
        for (name, value) in properties {
            set_checked(&element, name, value)?;
        }
        // No multi-slice encoding: frames are sent whole (h264parse alignment=au),
        // so slices cannot overlap with transfer. Measured on Intel VA with an
        // Exynos decoder, four slices gave no latency gain and cost quality.
        self.apply_frame_cap(&element, kbps, framerate);
        debug!(encoder = factory, kbps, framerate, "encoder built");
        Ok(element)
    }

    pub fn set_bitrate(
        self,
        element: &gst::Element,
        kbps: u32,
        framerate: u32,
    ) -> Result<(), gst::glib::BoolError> {
        set_checked(
            element,
            "bitrate",
            &self.bitrate_property_value(kbps).to_string(),
        )?;
        self.apply_frame_cap(element, kbps, framerate);
        Ok(())
    }

    /// Caps the rate-control buffer to [`FRAME_CAP_FRAMES`] frames of bitrate.
    ///
    /// Left at its default, the buffer lets a keyframe or full-screen change
    /// grow to many frames' worth, and every byte of it must cross USB before
    /// the tablet can decode. A small buffer trades a briefly softer picture
    /// for a bounded transmission time. Best effort: a driver without the
    /// property keeps its default.
    fn apply_frame_cap(self, element: &gst::Element, kbps: u32, framerate: u32) {
        let kbits = frame_cap_kbits(kbps, framerate);
        let property = match self {
            // In kbit; the VA range tops out at 1023.
            Self::VaH264Lp | Self::VaH264 | Self::VaH265Lp | Self::VaH265 => {
                Some(("cpb-size", kbits.min(1023)))
            }
            Self::NvH264 | Self::NvH265 => Some(("vbv-buffer-size", kbits)),
            // x264 takes milliseconds at build time; OpenH264 has no control.
            Self::X264 | Self::OpenH264 => None,
        };
        if let Some((name, value)) = property {
            if let Err(e) = set_checked(element, name, &value.to_string()) {
                debug!(error = %e, property = name, "encoder has no frame-size cap");
            }
        }
    }
}

/// Rate-control buffer, in frames of the target bitrate.
const FRAME_CAP_FRAMES: u32 = 2;

fn frame_cap_kbits(kbps: u32, framerate: u32) -> u32 {
    (kbps.saturating_mul(FRAME_CAP_FRAMES) / framerate.max(1)).max(1)
}

/// Driver versions can expose different properties/ranges. Turn incompatibility
/// into a recoverable error rather than a GObject property-set panic.
fn set_checked(element: &gst::Element, name: &str, text: &str) -> Result<(), gst::glib::BoolError> {
    let pspec = element
        .find_property(name)
        .ok_or_else(|| gst::glib::bool_error!("{} has no {} property", element.name(), name))?;
    if !pspec.flags().contains(gst::glib::ParamFlags::WRITABLE)
        || pspec
            .flags()
            .contains(gst::glib::ParamFlags::CONSTRUCT_ONLY)
    {
        return Err(gst::glib::bool_error!("{} is not writable", name));
    }
    if let Some(spec) = pspec.downcast_ref::<gst::glib::ParamSpecUInt>() {
        let value = text
            .parse::<u32>()
            .map_err(|_| gst::glib::bool_error!("invalid {}", name))?;
        if value < spec.minimum() || value > spec.maximum() {
            return Err(gst::glib::bool_error!(
                "{} must be between {} and {}",
                name,
                spec.minimum(),
                spec.maximum()
            ));
        }
    } else if let Some(spec) = pspec.downcast_ref::<gst::glib::ParamSpecInt>() {
        let value = text
            .parse::<i32>()
            .map_err(|_| gst::glib::bool_error!("invalid {}", name))?;
        if value < spec.minimum() || value > spec.maximum() {
            return Err(gst::glib::bool_error!(
                "{} must be between {} and {}",
                name,
                spec.minimum(),
                spec.maximum()
            ));
        }
    }
    let value = gst::glib::Value::deserialize(text, pspec.value_type())?;
    element.set_property_from_value(name, &value);
    Ok(())
}

/// Periodic safety keyframe interval in frames (~10 seconds at nominal fps).
///
/// Initial tablet attachment and any dropped reference frame explicitly trigger
/// an immediate IDR via `request_keyframe()`. Forcing an IDR every 24 frames
/// (400 ms at 60 fps) starved P-frames and caused periodic USB transfer spikes.
fn keyframe_interval_frames(nominal_framerate: u32) -> u32 {
    const TARGET_SECONDS: u32 = 10;
    nominal_framerate.saturating_mul(TARGET_SECONDS).max(24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_units_differ_between_encoders() {
        // The whole reason this conversion is centralised.
        assert_eq!(Encoder::X264.bitrate_property_value(15_000), 15_000);
        assert_eq!(Encoder::OpenH264.bitrate_property_value(15_000), 15_000_000);
        // The VA encoders document their bitrate in kbps, like x264, so the
        // adaptive controller needs no special case for them.
        assert_eq!(Encoder::VaH264Lp.bitrate_property_value(15_000), 15_000);
        assert_eq!(Encoder::VaH264.bitrate_property_value(15_000), 15_000);
        assert_eq!(Encoder::VaH265Lp.bitrate_property_value(15_000), 15_000);
    }

    #[test]
    fn frame_cap_is_two_frames_of_bitrate() {
        assert_eq!(frame_cap_kbits(30_000, 90), 666);
        assert_eq!(frame_cap_kbits(30_000, 60), 1000);
        assert_eq!(frame_cap_kbits(1, 120), 1);
        assert_eq!(frame_cap_kbits(30_000, 0), 60_000);
    }

    #[test]
    fn hardware_backends_are_gpu() {
        assert!(Encoder::VaH264Lp.is_gpu());
        assert!(Encoder::VaH264.is_gpu());
        assert!(Encoder::VaH265Lp.is_gpu());
        assert!(Encoder::VaH265.is_gpu());
        assert!(!Encoder::X264.is_gpu());
        assert!(!Encoder::OpenH264.is_gpu());
        assert!(Encoder::NvH264.is_gpu());
        assert!(Encoder::NvH265.is_gpu());
    }

    #[test]
    fn bitrate_conversion_cannot_overflow() {
        assert_eq!(Encoder::OpenH264.bitrate_property_value(u32::MAX), u32::MAX);
    }

    #[test]
    fn x264_is_preferred_over_openh264() {
        assert_eq!(Encoder::PREFERENCE[0], Encoder::X264);
    }

    #[test]
    fn keyframe_interval_uses_long_gop_since_attach_and_drops_force_idr() {
        assert_eq!(keyframe_interval_frames(60), 600);
        assert_eq!(keyframe_interval_frames(30), 300);
        assert_eq!(keyframe_interval_frames(2), 24);
    }

    #[test]
    fn keyframe_interval_is_never_degenerate() {
        for fps in [0, 1, 2, 5, 60, 240] {
            assert!(keyframe_interval_frames(fps) >= 24, "fps {fps}");
        }
    }

    #[test]
    fn incompatible_properties_return_errors_instead_of_panicking() {
        gst::init().unwrap();
        let element = gst::ElementFactory::make("fakesink").build().unwrap();
        assert!(set_checked(&element, "nonexistent", "1").is_err());
        assert!(set_checked(&element, "num-buffers", "999999999999").is_err());
        assert!(set_checked(&element, "sync", "not-a-boolean").is_err());
        set_checked(&element, "sync", "false").unwrap();
        assert!(!element.property::<bool>("sync"));
    }

    #[test]
    fn software_encoders_build_and_accept_bitrate_changes_when_installed() {
        gst::init().unwrap();
        for encoder in Encoder::PREFERENCE {
            if !encoder.is_available() {
                continue;
            }
            let element = encoder.build(15_000, 60).unwrap();
            encoder.set_bitrate(&element, 10_000, 60).unwrap();
            assert_eq!(
                element.property::<u32>("bitrate"),
                encoder.bitrate_property_value(10_000)
            );
            if encoder == Encoder::X264 {
                assert!(encoder.set_bitrate(&element, u32::MAX, 60).is_err());
            }
        }
    }
}
