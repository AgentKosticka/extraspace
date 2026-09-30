//! Diagnostics without opening a window, and bounded logs for desktop launches.
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub fn log_path() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .map(|p| p.join("extraspace/extraspace.log"))
}

struct LogFile {
    file: File,
    size: u64,
    path: PathBuf,
}

#[derive(Clone)]
pub struct LogWriter(Option<Arc<Mutex<LogFile>>>);

impl LogWriter {
    pub fn new() -> Self {
        let file = (|| {
            let path = log_path()?;
            std::fs::create_dir_all(path.parent()?).ok()?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()?;
            let size = file.metadata().ok()?.len();
            Some(Arc::new(Mutex::new(LogFile { file, size, path })))
        })();
        Self(file)
    }
}

impl Write for LogWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(data);
        if let Some(log) = &self.0 {
            if let Ok(mut log) = log.lock() {
                if log.size + data.len() as u64 > MAX_LOG_BYTES {
                    let old = log.path.with_extension("log.1");
                    if std::fs::rename(&log.path, old).is_ok() {
                        if let Ok(file) = File::create(&log.path) {
                            log.file = file;
                            log.size = 0;
                        }
                    }
                }
                if log.file.write_all(data).is_ok() {
                    log.size += data.len() as u64;
                }
            }
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if let Some(log) = &self.0 {
            if let Ok(mut log) = log.lock() {
                let _ = log.file.flush();
            }
        }
        Ok(())
    }
}

pub fn print() {
    println!(
        "Extraspace {} (companion version {})",
        env!("CARGO_PKG_VERSION"),
        super::apk_version()
    );
    for name in ["XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE", "WAYLAND_DISPLAY"] {
        println!(
            "{name}: {}",
            std::env::var(name).unwrap_or_else(|_| "unset".into())
        );
    }
    for (program, args) in [("gnome-shell", vec!["--version"]), ("adb", vec!["version"])] {
        match Command::new(program).args(args).output() {
            Ok(out) => println!("{}", String::from_utf8_lossy(&out.stdout).trim()),
            Err(e) => println!("{program}: {e}"),
        }
    }
    match gstreamer::init() {
        Ok(()) => {
            println!("{}", gstreamer::version_string());
            for name in [
                "vah264lpenc",
                "vah264enc",
                "vapostproc",
                "x264enc",
                "openh264enc",
                "videoconvert",
                "h264parse",
            ] {
                println!(
                    "{name}: {}",
                    if gstreamer::ElementFactory::find(name).is_some() {
                        "available"
                    } else {
                        "missing"
                    }
                );
            }
        }
        Err(e) => println!("GStreamer: {e}"),
    }
    match Command::new("adb").args(["devices", "-l"]).output() {
        Ok(out) => {
            println!("ADB devices (serials omitted):");
            for line in String::from_utf8_lossy(&out.stdout)
                .lines()
                .skip(1)
                .filter(|l| !l.trim().is_empty())
            {
                let mut fields = line.split_whitespace();
                fields.next();
                let state = fields.next().unwrap_or("unknown");
                let model = fields
                    .find_map(|f| f.strip_prefix("model:"))
                    .unwrap_or("unknown model");
                println!("  {model}: {state}");
            }
            if !out.status.success() {
                println!(
                    "ADB failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
        }
        Err(e) => println!("ADB device query: {e}"),
    }
    println!(
        "Scaled modes: {} (requires verified patched Mutter and XS_MUTTER_MODES=1)",
        xs_core::modes_enabled()
    );
    println!(
        "Companion APK: {}",
        super::bundled_apk()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not bundled; using installed tablet app".into())
    );
    println!("Settings: {}", super::Config::path().display());
    if let Some(path) = log_path() {
        println!(
            "Desktop log: {} (one previous log retained, 5 MiB each)",
            path.display()
        );
    }
    println!(
        "Camera: {}",
        if std::path::Path::new("/dev/video10").exists() {
            "/dev/video10 exists"
        } else {
            "not configured (optional: scripts/setup.sh --camera)"
        }
    );
    println!("Control RTT measures the control channel. Visual latency is not measured.");
}
