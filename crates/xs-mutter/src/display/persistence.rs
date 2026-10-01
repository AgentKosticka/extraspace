//! Keep layout identity independent of Mutter's per-process virtual serials.
//!
//! Restore placement and the tablet logical scale after capture produces a frame.
//! Physical modes, scales, transforms and enabled outputs must match the profile.
//! This avoids unconfigured CRTCs / missing stage views during live capture.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Placement {
    x: i32,
    y: i32,
    scale: f64,
    transform: u32,
    primary: bool,
    members: Vec<MonitorSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Profile {
    device: String,
    layout_mode: u32,
    modes: Vec<(MonitorSpec, String)>,
    placements: Vec<Placement>,
}

fn normalized(spec: &MonitorSpec) -> MonitorSpec {
    if is_virtual(spec) {
        (
            "@extraspace".into(),
            "MetaVendor".into(),
            String::new(),
            String::new(),
        )
    } else {
        spec.clone()
    }
}

fn profile(device: &str, state: &CurrentState) -> Option<Profile> {
    let (_, monitors, logical, props) = state;
    // Do not adopt somebody else's screen-cast, disabled output, or clone group.
    if monitors.iter().filter(|(s, ..)| is_virtual(s)).count() != 1
        || logical.iter().any(|(.., members, _)| members.len() != 1)
        || logical.len() != monitors.len()
    {
        return None;
    }
    let mut modes = Vec::new();
    for monitor in monitors {
        modes.push((normalized(&monitor.0), current_mode_id(monitor)?.to_owned()));
    }
    modes.sort();
    let mut placements: Vec<_> = logical
        .iter()
        .map(|(x, y, scale, transform, primary, members, _)| Placement {
            x: *x,
            y: *y,
            scale: *scale,
            transform: *transform,
            primary: *primary,
            members: members.iter().map(normalized).collect(),
        })
        .collect();
    placements.sort_by(|a, b| a.members.cmp(&b.members));
    Some(Profile {
        device: device.into(),
        modes,
        placements,
        layout_mode: prop_u32(props, "layout-mode").unwrap_or(LAYOUT_LOGICAL),
    })
}

fn is_virtual_identity(spec: &MonitorSpec) -> bool {
    spec.0 == "@extraspace"
}

fn same_geometry(a: &Profile, b: &Profile) -> bool {
    a.device == b.device
        && a.layout_mode == b.layout_mode
        && a.modes == b.modes
        && a.placements.len() == b.placements.len()
        && a.placements.iter().zip(&b.placements).all(|(a, b)| {
            a.members == b.members
                && (a.members.iter().all(is_virtual_identity) || a.scale == b.scale)
                && a.transform == b.transform
        })
}

fn path() -> Option<std::path::PathBuf> {
    dirs_config_dir().map(|d| d.join("extraspace/monitor-layouts.json"))
}

fn load(path: &std::path::Path) -> Result<Vec<Profile>> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| Error::DisplayLayout(format!("invalid layout profiles: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(Error::DisplayLayout(format!(
            "reading layout profiles: {e}"
        ))),
    }
}

pub(crate) async fn remember(conn: &Connection, device: &str) -> Result<()> {
    let state = get_current_state(conn).await?;
    let Some(current) = profile(device, &state) else {
        return Ok(());
    };
    let Some(path) = path() else {
        return Ok(());
    };
    let mut profiles = load(&path)?;
    if profiles.iter().any(|p| p == &current) {
        return Ok(());
    }
    profiles.retain(|p| !same_geometry(p, &current));
    profiles.push(current);
    if profiles.len() > 16 {
        profiles.drain(..profiles.len() - 16);
    }
    let io = |e| Error::DisplayLayout(format!("saving layout profiles: {e}"));
    std::fs::create_dir_all(path.parent().unwrap()).map_err(io)?;
    let text =
        serde_json::to_string_pretty(&profiles).map_err(|e| Error::DisplayLayout(e.to_string()))?;
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(io)?;
    std::fs::rename(&temp, &path).map_err(io)?;
    info!("remembered tablet placement");
    Ok(())
}

pub(crate) async fn restore(conn: &Connection, device: &str) -> Result<()> {
    let Some(path) = path() else {
        return Ok(());
    };
    let profiles = load(&path)?;
    let state = get_current_state(conn).await?;
    let Some(current) = profile(device, &state) else {
        return Ok(());
    };
    let Some(saved) = profiles.iter().rev().find(|p| same_geometry(p, &current)) else {
        return Ok(());
    };
    if saved == &current {
        return Ok(());
    }
    let (serial, monitors, logical, _) = state;
    let layout = DisplayLayout {
        monitors,
        logical,
        properties: HashMap::new(),
    };
    let mut applied = build_applied_existing_layout(&layout, &layout.monitors)?;
    for (x, y, scale, transform, primary, members) in &mut applied {
        let identities: Vec<_> = members
            .iter()
            .filter_map(|(connector, ..)| {
                layout
                    .monitors
                    .iter()
                    .find(|(s, ..)| s.0 == *connector)
                    .map(|(s, ..)| normalized(s))
            })
            .collect();
        let Some(placement) = saved.placements.iter().find(|p| p.members == identities) else {
            return Ok(());
        };
        if *transform != placement.transform {
            return Ok(());
        }
        if identities.iter().all(is_virtual_identity) {
            let connector = &members[0].0;
            let mode_id = &members[0].1;
            let mode = layout
                .monitors
                .iter()
                .find(|(s, ..)| &s.0 == connector)
                .and_then(|(_, modes, _)| modes.iter().find(|m| &m.0 == mode_id));
            if !mode.is_some_and(|m| supported_scale(m, placement.scale)) {
                return Ok(());
            }
            *scale = placement.scale;
        } else if *scale != placement.scale {
            return Ok(());
        }
        *x = placement.x;
        *y = placement.y;
        *primary = placement.primary;
    }
    conn.call_method(
        Some("org.gnome.Mutter.DisplayConfig"),
        "/org/gnome/Mutter/DisplayConfig",
        Some("org.gnome.Mutter.DisplayConfig"),
        "ApplyMonitorsConfig",
        &(
            serial,
            APPLY_TEMPORARY,
            applied,
            HashMap::<String, Value<'static>>::new(),
        ),
    )
    .await?;
    info!("restored tablet placement and GNOME scale");
    Ok(())
}

fn supported_scale(mode: &Mode, scale: f64) -> bool {
    scale.is_finite() && scale >= 1.0 && mode.5.iter().any(|s| (*s - scale).abs() < 0.0001)
}

/// Adopt a legacy ADB serial profile once; UUID profiles always win.
pub(crate) fn migrate(old: &str, new: &str) -> Result<()> {
    if old == new {
        return Ok(());
    }
    let Some(path) = path() else {
        return Ok(());
    };
    let mut profiles = load(&path)?;
    if profiles.iter().any(|p| p.device == new) {
        return Ok(());
    }
    let adopted: Vec<_> = profiles
        .iter()
        .filter(|p| p.device == old)
        .cloned()
        .map(|mut p| {
            p.device = new.into();
            p
        })
        .collect();
    if adopted.is_empty() {
        return Ok(());
    }
    profiles.extend(adopted);
    let text =
        serde_json::to_string_pretty(&profiles).map_err(|e| Error::DisplayLayout(e.to_string()))?;
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temp, text)
        .and_then(|_| std::fs::rename(temp, path))
        .map_err(|e| Error::DisplayLayout(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(serial: &str, physical_mode: &str) -> Profile {
        let physical = (
            "eDP-1".into(),
            "AUO".into(),
            "panel".into(),
            "serial".into(),
        );
        let virtual_spec = (
            "Meta-1".into(),
            "MetaVendor".into(),
            "Virtual remote monitor".into(),
            serial.into(),
        );
        let monitors = vec![
            super::super::tests::active_monitor(physical.clone(), physical_mode, 1920, 1080),
            super::super::tests::active_monitor(virtual_spec.clone(), "1920x1200@60", 1920, 1200),
        ];
        let logical = vec![
            super::super::tests::logical_monitor(0, 0, true, physical),
            super::super::tests::logical_monitor(1920, 0, false, virtual_spec),
        ];
        profile("test-tablet", &(1, monitors, logical, HashMap::new())).unwrap()
    }
    #[test]
    fn restore_only_accepts_scales_advertised_by_the_current_mode() {
        let m = (
            "mode".into(),
            1920,
            1200,
            60.0,
            1.0,
            vec![1.0, 1.5, 2.0],
            HashMap::new(),
        );
        assert!(supported_scale(&m, 1.5));
        assert!(!supported_scale(&m, 1.75));
        assert!(!supported_scale(&m, f64::NAN));
    }
    #[test]
    fn compositor_serial_changes_do_not_lose_identity() {
        assert_eq!(
            sample("0x000001", "1920x1080"),
            sample("0x000009", "1920x1080")
        );
    }
    #[test]
    fn positions_can_change_but_geometry_and_device_must_match() {
        let a = sample("1", "1920x1080");
        let mut b = a.clone();
        b.placements[0].x = 300;
        assert!(same_geometry(&a, &b));
        b.placements[0].scale = 1.5;
        assert!(same_geometry(&a, &b)); // virtual scale is restored
        b.placements[1].scale = 1.5;
        assert!(!same_geometry(&a, &b)); // physical scale must still match
        b = a.clone();
        b.placements[0].transform = 1;
        assert!(!same_geometry(&a, &b));
        b = a.clone();
        b.device = "other".into();
        assert!(!same_geometry(&a, &b));
        assert!(!same_geometry(&a, &sample("1", "1280x720")));
    }
}
