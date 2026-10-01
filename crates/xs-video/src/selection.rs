//! Persisted encoder policy and discovery of supported, installed backends.
use gst::prelude::*;
use gstreamer as gst;
use serde::{Deserialize, Serialize};

use crate::Encoder;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncodingMode {
    Cpu,
    Gpu,
    #[default]
    #[serde(other)]
    Auto,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EncoderSelection {
    pub mode: EncodingMode,
    /// None selects the best working encoder in the chosen mode.
    pub factory: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EncoderOption {
    pub factory: String,
    pub label: String,
    pub encoder: Encoder,
    /// VA conversion must run on the same device as encoding.
    pub postproc: Option<String>,
}

impl EncoderOption {
    pub fn is_gpu(&self) -> bool {
        self.encoder.is_gpu()
    }

    pub fn build(&self, kbps: u32, fps: u32) -> Result<gst::Element, gst::glib::BoolError> {
        self.encoder.build_factory(&self.factory, kbps, fps)
    }
}

fn numbered(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(suffix))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

fn supported(name: &str, plugin: &str) -> Option<Encoder> {
    match (name, plugin) {
        ("x264enc", "x264") => Some(Encoder::X264),
        ("openh264enc", "openh264") => Some(Encoder::OpenH264),
        ("vah264lpenc", "va") => Some(Encoder::VaH264Lp),
        ("vah264enc", "va") => Some(Encoder::VaH264),
        ("nvh264enc", "nvcodec") => Some(Encoder::NvH264),
        (_, "va") if numbered(name, "varenderD", "h264lpenc") => Some(Encoder::VaH264Lp),
        (_, "va") if numbered(name, "varenderD", "h264enc") => Some(Encoder::VaH264),
        (_, "nvcodec") if numbered(name, "nvh264device", "enc") => Some(Encoder::NvH264),
        _ => None,
    }
}

fn device_path(factory: &gst::ElementFactory) -> Option<String> {
    let element = factory.create().build().ok()?;
    element.find_property("device-path")?;
    element.property::<Option<String>>("device-path")
}

/// Discover factories registered by working drivers, including every VA render
/// device and NVIDIA device variant. An installed .so alone is not sufficient.
pub fn available_encoders() -> Vec<EncoderOption> {
    if gst::init().is_err() {
        return Vec::new();
    }
    let factories: Vec<gst::ElementFactory> = gst::Registry::get()
        .features(gst::ElementFactory::static_type())
        .iter()
        .filter_map(|f| f.clone().downcast().ok())
        .collect();
    let postprocs: Vec<(String, Option<String>)> = factories
        .iter()
        .filter(|f| {
            f.plugin_name().as_deref() == Some("va")
                && (f.name() == "vapostproc"
                    || numbered(f.name().as_str(), "varenderD", "postproc"))
        })
        .map(|f| (f.name().to_string(), device_path(f)))
        .collect();
    let mut options = Vec::new();
    for factory in &factories {
        let name = factory.name();
        let Some(encoder) = supported(
            name.as_str(),
            factory.plugin_name().as_deref().unwrap_or(""),
        ) else {
            continue;
        };
        let va = matches!(encoder, Encoder::VaH264 | Encoder::VaH264Lp);
        let device = if va { device_path(factory) } else { None };
        let postproc = if va {
            // Do not cross devices, even when both can encode H.264.
            let Some(path) = device.as_ref() else {
                continue;
            };
            let Some((name, _)) = postprocs
                .iter()
                .find(|(_, have)| have.as_ref() == Some(path))
            else {
                continue;
            };
            Some(name.clone())
        } else {
            None
        };
        let driver = factory
            .metadata("long-name")
            .unwrap_or(encoder.human_name())
            .rsplit(" in ")
            .next()
            .unwrap_or(encoder.human_name());
        let label = if let Some(path) = device {
            let backend = std::env::var("LIBVA_DRIVER_NAME")
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| format!(" ({v})"))
                .unwrap_or_default();
            format!(
                "{}{} · {} · {}",
                encoder.human_name(),
                backend,
                driver,
                path
            )
        } else if encoder.is_gpu() {
            format!("{} · {} · {}", encoder.human_name(), driver, name)
        } else {
            encoder.human_name().to_string()
        };
        options.push(EncoderOption {
            factory: name.to_string(),
            label,
            encoder,
            postproc,
        });
    }
    options.sort_by_key(|o| {
        (
            match o.encoder {
                Encoder::VaH264Lp => 0,
                Encoder::VaH264 => 1,
                Encoder::NvH264 => 2,
                Encoder::X264 => 3,
                Encoder::OpenH264 => 4,
            },
            o.factory.clone(),
        )
    });
    options
}

impl EncoderSelection {
    /// A pinned encoder is strict. Auto may fall back across CPU/GPU; CPU and
    /// GPU modes stay within their respective classes.
    pub fn candidates(&self, options: Vec<EncoderOption>) -> Result<Vec<EncoderOption>, String> {
        let mut candidates: Vec<_> = options
            .into_iter()
            .filter(|o| match self.mode {
                EncodingMode::Auto => true,
                EncodingMode::Cpu => !o.is_gpu(),
                EncodingMode::Gpu => o.is_gpu(),
            })
            .collect();
        if let Some(factory) = &self.factory {
            candidates.retain(|o| &o.factory == factory);
        }
        if candidates.is_empty() {
            Err(match &self.factory {
                Some(factory) => format!("The selected encoder ({factory}) is unavailable in this mode. Choose another encoder in Video Encoding."),
                None => format!("No supported {:?} encoder is available. Install its GStreamer plugin and graphics driver, or choose another mode in Video Encoding.", self.mode),
            })
        } else {
            Ok(candidates)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn option(factory: &str, encoder: Encoder) -> EncoderOption {
        EncoderOption {
            factory: factory.into(),
            label: factory.into(),
            encoder,
            postproc: None,
        }
    }
    fn options() -> Vec<EncoderOption> {
        vec![
            option("vah264lpenc", Encoder::VaH264Lp),
            option("varenderD129h264enc", Encoder::VaH264),
            option("x264enc", Encoder::X264),
        ]
    }
    #[test]
    fn modes_and_pins_are_strict() {
        assert_eq!(
            EncoderSelection::default()
                .candidates(options())
                .unwrap()
                .len(),
            3
        );
        for (mode, gpu, count) in [(EncodingMode::Cpu, false, 1), (EncodingMode::Gpu, true, 2)] {
            let selected = EncoderSelection {
                mode,
                factory: None,
            }
            .candidates(options())
            .unwrap();
            assert_eq!(selected.len(), count);
            assert!(selected.iter().all(|o| o.is_gpu() == gpu));
        }
        let pin = EncoderSelection {
            mode: EncodingMode::Gpu,
            factory: Some("varenderD129h264enc".into()),
        };
        assert_eq!(
            pin.candidates(options()).unwrap()[0].factory,
            "varenderD129h264enc"
        );
        assert!(EncoderSelection {
            mode: EncodingMode::Cpu,
            ..pin
        }
        .candidates(options())
        .is_err());
        assert!(EncoderSelection {
            factory: Some("missing".into()),
            ..Default::default()
        }
        .candidates(options())
        .is_err());
    }
    #[test]
    fn only_known_plugin_factory_pairs_are_accepted() {
        assert_eq!(
            supported("varenderD129h264lpenc", "va"),
            Some(Encoder::VaH264Lp)
        );
        assert_eq!(
            supported("nvh264device2enc", "nvcodec"),
            Some(Encoder::NvH264)
        );
        for (name, plugin) in [
            ("x264enc", "fake"),
            ("vulkanh264enc", "vulkan"),
            ("varenderDxxh264enc", "va"),
            ("varenderDh264enc", "va"),
            ("fakesink", "coreelements"),
        ] {
            assert_eq!(supported(name, plugin), None);
        }
    }
}
