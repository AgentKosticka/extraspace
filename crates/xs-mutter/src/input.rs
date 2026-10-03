//! RecordVirtual expects logical coordinates; RecordMonitor already scales input.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use futures_util::StreamExt;
use tracing::{debug, warn};
use zbus::Connection;

use crate::{display, CaptureSource, Result};

pub(crate) struct InputCoordinates {
    scale: Arc<AtomicU64>,
    watcher: Option<tokio::task::JoinHandle<()>>,
}

impl InputCoordinates {
    pub(crate) async fn new(conn: &Connection, source: &CaptureSource) -> Result<Self> {
        let mut mapping = Self {
            scale: Arc::new(AtomicU64::new(1.0f64.to_bits())),
            watcher: None,
        };
        if !matches!(source, CaptureSource::Virtual) {
            return Ok(mapping);
        }

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
        mapping.watcher = Some(tokio::spawn(async move {
            while changes.next().await.is_some() {
                refresh(&conn, &scale).await;
            }
        }));
        Ok(mapping)
    }

    pub(crate) async fn refresh(&self, conn: &Connection) {
        refresh(conn, &self.scale).await;
    }

    pub(crate) fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let scale = f64::from_bits(self.scale.load(Ordering::Relaxed));
        (x / scale, y / scale)
    }
}

async fn refresh(conn: &Connection, scale: &AtomicU64) {
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
