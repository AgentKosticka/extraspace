//! Host preferences per companion installation. UUIDs are identifiers, not credentials.
use crate::{BitrateBounds, DisplayMode, EncoderSelection, SessionConfig};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    pub scale: f64,
    pub mode: DisplayMode,
    pub mirror_source: Option<String>,
    pub framerate: u32,
    pub encoder: EncoderSelection,
    pub bounds: BitrateBounds,
    pub camera_enabled: bool,
    pub camera_id: String,
}
impl Default for DeviceSettings {
    fn default() -> Self {
        Self::from_config(&SessionConfig::default())
    }
}
impl DeviceSettings {
    pub fn from_config(c: &SessionConfig) -> Self {
        Self {
            scale: c.scale,
            mode: c.mode,
            mirror_source: c.mirror_source.clone(),
            framerate: c.framerate,
            encoder: c.encoder.clone(),
            bounds: c.bounds,
            camera_enabled: c.camera_enabled,
            camera_id: c.camera_id.clone(),
        }
    }
    fn apply(&self, c: &mut SessionConfig) {
        c.scale = crate::clamp_ui_scale(self.scale);
        c.mode = self.mode;
        c.mirror_source = self.mirror_source.clone();
        c.framerate = self.framerate.clamp(1, 120);
        c.encoder = self.encoder.clone();
        c.bounds = BitrateBounds {
            min_kbps: self.bounds.min_kbps.min(self.bounds.max_kbps).max(100),
            max_kbps: self.bounds.max_kbps.max(self.bounds.min_kbps).max(100),
        };
        c.camera_enabled = self.camera_enabled;
        c.camera_id = self.camera_id.clone();
    }
}

pub(crate) fn identity(hello: &xs_proto::Hello, serial: &str) -> String {
    // Strict ASCII shape: never use arbitrary peer strings as identities or paths.
    if let Some(id) = hello.device_id.as_deref().filter(|id| valid_uuid(id)) {
        format!("uuid:{}", id.to_ascii_lowercase())
    } else {
        serial.to_owned()
    }
}
fn valid_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|p| p.join("extraspace/device-settings.json"))
}
fn load(path: &std::path::Path) -> anyhow::Result<BTreeMap<String, DeviceSettings>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(serde_json::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn restore(config: &mut SessionConfig, id: &str) {
    let Some(path) = path() else {
        return;
    };
    match load(&path) {
        Ok(settings) => {
            if let Some(s) = settings.get(id) {
                s.apply(config);
            }
        }
        Err(e) => warn!(error = %e, "could not read device settings"),
    }
}
pub(crate) fn remember(config: &SessionConfig, id: &str) {
    let Some(path) = path() else {
        return;
    };
    let save = || -> anyhow::Result<()> {
        let mut settings = load(&path)?;
        settings.insert(id.into(), DeviceSettings::from_config(config));
        std::fs::create_dir_all(path.parent().unwrap())?;
        let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&temp, serde_json::to_string_pretty(&settings)?)?;
        std::fs::rename(temp, &path)?;
        Ok(())
    };
    if let Err(e) = save() {
        warn!(error = %e, "could not save device settings");
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_requires_a_uuid_and_is_transport_independent() {
        let mut h: xs_proto::Hello = serde_json::from_str(r#"{"protocol_version":1,"device_name":"Tablet","android_release":"15","width":2000,"height":1200,"density_dpi":160,"refresh_rate":60,"cameras":[]}"#).unwrap();
        assert_eq!(identity(&h, "legacy"), "legacy");
        h.device_id = Some("../bad".into());
        assert_eq!(identity(&h, "legacy"), "legacy");
        h.device_id = Some("15BB50C4-82D8-4628-8F06-16C6A871DA11".into());
        assert_eq!(identity(&h, "adb"), identity(&h, "aoa"));
    }
    #[test]
    fn device_settings_do_not_overwrite_transport_or_apk_and_keep_devices_separate() {
        let first = DeviceSettings {
            scale: 2.0,
            mode: DisplayMode::Mirror,
            camera_enabled: true,
            ..Default::default()
        };
        let second = DeviceSettings {
            scale: 1.25,
            ..Default::default()
        };
        let profiles = BTreeMap::from([("one", first), ("two", second)]);
        let json = serde_json::to_string(&profiles).unwrap();
        let profiles: BTreeMap<String, DeviceSettings> = serde_json::from_str(&json).unwrap();
        let mut c = SessionConfig {
            apk_version: 77,
            ..Default::default()
        };
        profiles["one"].apply(&mut c);
        assert_eq!(c.scale, 2.0);
        assert!(c.camera_enabled);
        assert_eq!(c.apk_version, 77);
        profiles["two"].apply(&mut c);
        assert_eq!(c.scale, 1.25);
        assert!(!c.camera_enabled);
    }
}
