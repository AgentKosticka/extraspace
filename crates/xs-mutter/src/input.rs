//! RecordVirtual expects logical coordinates; RecordMonitor already scales input.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use futures_util::StreamExt;
use tracing::{debug, info, warn};
use zbus::Connection;

use crate::{display, CaptureSource, Result};

/// GNOME tablet settings for the `/dev/uinput` stylus (vendor:product 18d1:4ee7).
const TABLET_SETTINGS: &str =
    "org.gnome.desktop.peripherals.tablet:/org/gnome/desktop/peripherals/tablets/18d1:4ee7/";

type TabletOutput = Arc<Mutex<Option<[String; 3]>>>;

pub(crate) struct InputCoordinates {
    scale: Arc<AtomicU64>,
    stage: Arc<Mutex<Option<display::StageGeometry>>>,
    /// Output the stylus is pinned to. While set, Mutter maps the full stylus
    /// range onto that monitor, so the pen needs no scale or layout maths.
    tablet_output: TabletOutput,
    source: CaptureSource,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    watcher: Option<tokio::task::JoinHandle<()>>,
}

impl InputCoordinates {
    pub(crate) async fn new(conn: &Connection, source: &CaptureSource) -> Result<Self> {
        let mut mapping = Self {
            scale: Arc::new(AtomicU64::new(1.0f64.to_bits())),
            stage: Arc::new(Mutex::new(None)),
            tablet_output: Arc::new(Mutex::new(None)),
            source: source.clone(),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            watcher: None,
        };

        let proxy = zbus::Proxy::new_owned(
            conn.clone(),
            "org.gnome.Mutter.DisplayConfig",
            "/org/gnome/Mutter/DisplayConfig",
            "org.gnome.Mutter.DisplayConfig",
        )
        .await?;
        // Subscribe before reading, including the mode-less path where capture
        // creates the monitor later, and saved placement restores its scale.
        let mut changes = proxy.receive_signal("MonitorsChanged").await?;
        mapping.refresh(conn).await;
        let conn = conn.clone();
        let scale = Arc::clone(&mapping.scale);
        let stage = Arc::clone(&mapping.stage);
        let tablet_output = Arc::clone(&mapping.tablet_output);
        let source_clone = mapping.source.clone();
        let refresh_lock = Arc::clone(&mapping.refresh_lock);
        mapping.watcher = Some(tokio::spawn(async move {
            while changes.next().await.is_some() {
                let _guard = refresh_lock.lock().await;
                refresh(&conn, &source_clone, &scale, &stage, &tablet_output).await;
            }
        }));
        Ok(mapping)
    }

    pub(crate) async fn refresh(&self, conn: &Connection) {
        let _guard = self.refresh_lock.lock().await;
        refresh(
            conn,
            &self.source,
            &self.scale,
            &self.stage,
            &self.tablet_output,
        )
        .await;
    }

    pub(crate) fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let scale = f64::from_bits(self.scale.load(Ordering::Relaxed));
        (x / scale, y / scale)
    }

    pub(crate) fn stage_geometry(&self) -> Option<display::StageGeometry> {
        *self.stage.lock().unwrap()
    }

    /// Whether the stylus is pinned to the captured monitor.
    pub(crate) fn tablet_mapped(&self) -> bool {
        self.tablet_output.lock().unwrap().is_some()
    }
}

async fn refresh(
    conn: &Connection,
    source: &CaptureSource,
    scale: &AtomicU64,
    stage: &Mutex<Option<display::StageGeometry>>,
    tablet_output: &TabletOutput,
) {
    if matches!(source, CaptureSource::Virtual) {
        match display::virtual_input_scale(conn).await {
            Ok(current) => {
                let previous = f64::from_bits(scale.swap(current.to_bits(), Ordering::Relaxed));
                if previous != current {
                    debug!(scale = current, "virtual display input scale updated");
                }
            }
            Err(e) => warn!(error = %e, "could not refresh virtual display input scale"),
        }
    }
    if let Ok(geom) = display::stage_geometry(conn, source).await {
        *stage.lock().unwrap() = geom;
    }
    match display::tablet_output_spec(conn, source).await {
        Ok(Some(spec)) => {
            if tablet_output.lock().unwrap().as_ref() == Some(&spec) {
                return;
            }
            if set_tablet_output(&spec).await {
                info!(output = ?spec, "virtual stylus pinned to the tablet display");
                *tablet_output.lock().unwrap() = Some(spec);
            } else {
                *tablet_output.lock().unwrap() = None;
            }
        }
        Ok(None) => {
            *tablet_output.lock().unwrap() = None;
        }
        Err(e) => warn!(error = %e, "could not look up the tablet display for the stylus"),
    }
}

async fn set_tablet_output(spec: &[String; 3]) -> bool {
    let quote = |s: &str| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"));
    let value = format!(
        "[{}, {}, {}]",
        quote(&spec[0]),
        quote(&spec[1]),
        quote(&spec[2])
    );
    // Configure this device without blocking a Tokio worker on subprocesses.
    // The output write determines whether coordinates are monitor-relative;
    // do not mistake failure of an unrelated key for an unmapped device.
    let mut mapped = false;
    for (key, value) in [
        ("keep-aspect", "false"),
        ("mapping", "absolute"),
        ("output", value.as_str()),
    ] {
        let success = tokio::process::Command::new("gsettings")
            .args(["set", TABLET_SETTINGS, key, value])
            .status()
            .await
            .is_ok_and(|status| status.success());
        if key == "output" {
            mapped = success;
        }
        if !success {
            warn!(key, "could not configure virtual stylus mapping");
        }
    }
    if !mapped {
        warn!("could not pin the virtual stylus to the tablet display");
    }
    // Let Mutter apply the mapping before the next stroke.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    mapped
}

/// Forget the per-session output serial so a stale mapping never lingers.
pub(crate) fn clear_tablet_output() {
    let _ = std::process::Command::new("gsettings")
        .args(["reset", TABLET_SETTINGS, "output"])
        .status();
}

impl Drop for InputCoordinates {
    fn drop(&mut self) {
        if let Some(watcher) = &self.watcher {
            watcher.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_pixels_follow_live_logical_scale() {
        let mapping = InputCoordinates {
            scale: Arc::new(AtomicU64::new(1.0f64.to_bits())),
            stage: Arc::new(Mutex::new(None)),
            tablet_output: Arc::new(Mutex::new(None)),
            source: CaptureSource::Virtual,
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            watcher: None,
        };
        for (scale, expected) in [
            (1.0f64, (960.0, 600.0)),
            (1.25, (768.0, 480.0)),
            (1.5, (640.0, 400.0)),
            (2.0, (480.0, 300.0)),
            (1.0, (960.0, 600.0)),
        ] {
            mapping.scale.store(scale.to_bits(), Ordering::Relaxed);
            assert_eq!(mapping.map(960.0, 600.0), expected);
            assert_eq!(mapping.map(0.0, 0.0), (0.0, 0.0));
            assert_eq!(
                mapping.map(1919.0, 1199.0),
                (1919.0 / scale, 1199.0 / scale)
            );
        }
    }
}
