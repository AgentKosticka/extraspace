//! Feeding the tablet's camera into a v4l2loopback device.
//!
//! The tablet sends H.264, which is decoded here and written as raw frames to
//! `/dev/video10`, where every ordinary camera consumer -- Firefox, Zoom, OBS,
//! Cheese -- picks it up as a normal webcam.
//!
//! ```text
//! appsrc -> h264parse -> avdec_h264 -> videoconvert -> videoscale -> v4l2sink
//! ```
//!
//! Decoding rather than passing H.264 through is deliberate: v4l2loopback can
//! carry encoded formats, but almost nothing consuming a webcam expects them, and
//! the whole point is to look like an ordinary camera.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app::AppSrc;
use tracing::{debug, info, warn};

/// Where `scripts/setup.sh` puts the loopback device.
pub const DEFAULT_DEVICE: &str = "/dev/video10";
const MAX_CAMERA_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GStreamer init failed: {0}")]
    Init(#[from] gst::glib::Error),

    #[error(
        "{path} does not exist. Open Camera Setup in Display & Camera Settings \
         to create the virtual webcam, then try again."
    )]
    DeviceMissing { path: PathBuf },

    #[error("{path} belongs to another video device. Open Camera Setup in Display & Camera Settings to resolve this conflict.")]
    DeviceUnowned { path: PathBuf },

    #[error(
        "{path} exists but could not be opened for writing. Is another program \
         already feeding it, or is your user missing from the 'video' group?"
    )]
    DeviceBusy { path: PathBuf },

    #[error("could not build the '{element}' element -- is its GStreamer plugin installed?")]
    ElementMissing { element: &'static str },

    #[error("pipeline error: {0}")]
    Pipeline(String),

    #[error("failed to link the pipeline: {0}")]
    Link(#[from] gst::glib::BoolError),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Decodes incoming H.264 and writes it to a v4l2loopback device.
pub struct V4l2Writer {
    pipeline: gst::Pipeline,
    source: AppSrc,
    started: bool,
    device: PathBuf,
    failure: Arc<Mutex<Option<String>>>,
}

impl V4l2Writer {
    /// Opens [`DEFAULT_DEVICE`].
    pub fn open_default() -> Result<Self> {
        Self::open(Path::new(DEFAULT_DEVICE))
    }

    pub fn open(device: &Path) -> Result<Self> {
        gst::init()?;

        if !device.exists() {
            return Err(Error::DeviceMissing {
                path: device.to_path_buf(),
            });
        }
        if !device_owned(device, Path::new("/sys/class/video4linux")) {
            return Err(Error::DeviceUnowned {
                path: device.to_path_buf(),
            });
        }
        // Fail here, with a message that says what to do, rather than letting
        // GStreamer report a generic state-change failure later.
        match std::fs::OpenOptions::new().write(true).open(device) {
            Ok(f) => drop(f),
            Err(e) => {
                debug!(error = %e, "probing the loopback device failed");
                return Err(Error::DeviceBusy {
                    path: device.to_path_buf(),
                });
            }
        }

        let pipeline = gst::Pipeline::with_name("extraspace-camera");
        let make = |name: &'static str| -> Result<gst::Element> {
            gst::ElementFactory::make(name)
                .build()
                .map_err(|_| Error::ElementMissing { element: name })
        };

        // The tablet sends Annex-B with inline SPS/PPS, and we cannot know the
        // resolution until the stream arrives, so leave caps for h264parse to
        // work out rather than asserting something that might be wrong.
        let source = AppSrc::builder()
            .name("camera-in")
            .caps(
                &gst::Caps::builder("video/x-h264")
                    .field("stream-format", "byte-stream")
                    .field("alignment", "au")
                    .build(),
            )
            .format(gst::Format::Time)
            .is_live(true)
            // Compressed reference pictures cannot be dropped safely. push()
            // fails the camera explicitly if this bounded queue is exhausted.
            .block(false)
            .max_bytes(MAX_CAMERA_BYTES)
            .build();

        let parse = make("h264parse")?;
        let decode = make("avdec_h264")?;
        let convert = make("videoconvert")?;
        let scale = make("videoscale")?;
        // Once decoded, dropping stale pictures is safe and keeps the webcam fresh.
        let raw_queue = make("queue")?;
        raw_queue.set_property("max-size-buffers", 2u32);
        raw_queue.set_property("max-size-bytes", 0u32);
        raw_queue.set_property("max-size-time", 0u64);
        raw_queue.set_property_from_str("leaky", "downstream");

        // YUY2 is the format essentially every v4l2 consumer understands.
        let caps = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("format", "YUY2")
                    .build(),
            )
            .build()
            .map_err(|_| Error::ElementMissing {
                element: "capsfilter",
            })?;

        let sink = gst::ElementFactory::make("v4l2sink")
            .property("device", device.to_string_lossy().as_ref())
            // The loopback device has no clock of its own to sync against.
            .property("sync", false)
            .build()
            .map_err(|_| Error::ElementMissing {
                element: "v4l2sink",
            })?;

        let elements = [
            source.upcast_ref(),
            &parse,
            &decode,
            &raw_queue,
            &convert,
            &scale,
            &caps,
            &sink,
        ];
        pipeline.add_many(elements)?;
        gst::Element::link_many(elements)?;

        info!(device = %device.display(), "virtual camera ready");
        Ok(Self {
            pipeline,
            source,
            started: false,
            device: device.to_path_buf(),
            failure: Arc::new(Mutex::new(None)),
        })
    }

    /// Pushes one encoded access unit.
    ///
    /// The pipeline starts lazily on the first frame: starting at construction
    /// would have `v4l2sink` announce a camera that then shows nothing, and some
    /// consumers latch onto the format they see first.
    pub fn push(&mut self, data: &[u8], pts_us: u64, _is_config: bool) -> Result<()> {
        if let Some(error) = self.failure() {
            return Err(Error::Pipeline(error));
        }
        if self
            .source
            .current_level_bytes()
            .saturating_add(data.len() as u64)
            > MAX_CAMERA_BYTES
        {
            return Err(Error::Pipeline(
                "Camera buffering is full; retry Tablet Camera".into(),
            ));
        }
        if !self.started {
            self.pipeline
                .set_state(gst::State::Playing)
                .map_err(|e| Error::Pipeline(e.to_string()))?;
            self.watch_bus();
            self.started = true;
            debug!("camera pipeline playing");
        }

        let mut buffer = gst::Buffer::from_slice(data.to_vec());
        {
            let buffer = buffer.get_mut().expect("freshly created buffer is unique");
            buffer.set_pts(gst::ClockTime::from_useconds(pts_us));
        }

        match self.source.push_buffer(buffer) {
            Ok(_) => Ok(()),
            Err(gst::FlowError::Flushing) | Err(gst::FlowError::Eos) => {
                Err(Error::Pipeline("camera pipeline stopped".into()))
            }
            Err(e) => Err(Error::Pipeline(format!("{e:?}"))),
        }
    }

    pub fn device(&self) -> &Path {
        &self.device
    }

    pub fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|failed| failed.clone())
    }

    fn watch_bus(&self) {
        let Some(bus) = self.pipeline.bus() else {
            return;
        };
        let failure = self.failure.clone();
        std::thread::spawn(move || {
            for msg in bus.iter_timed(gst::ClockTime::NONE) {
                match msg.view() {
                    gst::MessageView::Error(e) => {
                        let message = e.error().to_string();
                        if let Ok(mut failed) = failure.lock() {
                            *failed = Some(message);
                        }
                        warn!(error = %e.error(), debug = ?e.debug(), "camera pipeline error");
                        break;
                    }
                    gst::MessageView::Eos(_) => {
                        if let Ok(mut failed) = failure.lock() {
                            *failed = Some("Camera pipeline ended".into());
                        }
                        break;
                    }
                    _ => {}
                }
            }
        });
    }
}

impl Drop for V4l2Writer {
    fn drop(&mut self) {
        if let Some(bus) = self.pipeline.bus() {
            bus.set_flushing(true);
        }
        let _ = self.source.end_of_stream();
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            debug!(error = %e, "camera pipeline did not stop cleanly");
        }
    }
}

/// Whether a loopback device is present, for the UI to show a setup hint.
pub fn device_available() -> bool {
    let device = Path::new(DEFAULT_DEVICE);
    device.exists() && device_owned(device, Path::new("/sys/class/video4linux"))
}

// v4l2loopback exposes virtual video devices directly under devices/virtual/
// video4linux, with the configured card label in `name`. Physical devices cannot
// pass both checks. Fail closed when sysfs is unavailable.
fn device_owned(device: &Path, sysfs: &Path) -> bool {
    let Some(name) = device.file_name() else {
        return false;
    };
    let entry = sysfs.join(name);
    let Ok(actual) = std::fs::canonicalize(&entry) else {
        return false;
    };
    let Some(parent) = actual.parent() else {
        return false;
    };
    parent.ends_with("devices/virtual/video4linux")
        && std::fs::read_to_string(entry.join("name"))
            .is_ok_and(|label| label.trim() == "Extraspace Tablet Camera")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn isolated_writer() -> V4l2Writer {
        gst::init().unwrap();
        V4l2Writer {
            pipeline: gst::Pipeline::new(),
            source: AppSrc::builder()
                .block(false)
                .max_bytes(MAX_CAMERA_BYTES)
                .build(),
            started: true,
            device: PathBuf::new(),
            failure: Arc::new(Mutex::new(None)),
        }
    }
    #[test]
    fn compressed_camera_queue_is_bounded_without_dropping_reference_frames() {
        let mut writer = isolated_writer();
        let data = vec![0; 2 * 1024 * 1024];
        writer.push(&data, 0, false).unwrap();
        writer.push(&data, 1, false).unwrap();
        assert!(
            matches!(writer.push(&[1], 2, false), Err(Error::Pipeline(message)) if message.contains("buffering is full"))
        );
        assert_eq!(writer.source.current_level_bytes(), MAX_CAMERA_BYTES);
    }
    #[test]
    fn asynchronous_camera_bus_errors_are_visible_to_the_owner() {
        let writer = isolated_writer();
        writer.watch_bus();
        writer
            .pipeline
            .post_message(
                gst::message::Error::builder(gst::ResourceError::Write, "Simulated webcam failure")
                    .build(),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while writer.failure().is_none() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(writer
            .failure()
            .unwrap()
            .contains("Simulated webcam failure"));
    }

    #[test]
    fn rejects_unrelated_devices_and_accepts_owned_virtual_camera() {
        let root = std::env::temp_dir().join(format!("xs-camera-test-{}", std::process::id()));
        let virtual_device = root.join("devices/virtual/video4linux/video10");
        let physical_device = root.join("devices/pci/video4linux/video11");
        let class = root.join("class/video4linux");
        std::fs::create_dir_all(&virtual_device).unwrap();
        std::fs::create_dir_all(&physical_device).unwrap();
        std::fs::create_dir_all(&class).unwrap();
        symlink(&virtual_device, class.join("video10")).unwrap();
        symlink(&physical_device, class.join("video11")).unwrap();
        std::fs::write(virtual_device.join("name"), "Other loopback\n").unwrap();
        std::fs::write(physical_device.join("name"), "Extraspace Tablet Camera\n").unwrap();
        assert!(!device_owned(Path::new("/dev/video10"), &class));
        assert!(!device_owned(Path::new("/dev/video11"), &class));
        std::fs::write(virtual_device.join("name"), "Extraspace Tablet Camera\n").unwrap();
        assert!(device_owned(Path::new("/dev/video10"), &class));
        assert!(!device_owned(Path::new("/dev/video12"), &class));
        std::fs::remove_dir_all(root).unwrap();
    }
}
