//! Make a private Intel driver installation available to every app entry point.
//!
//! libva loads the driver later, but its dependencies need LD_LIBRARY_PATH from
//! process startup. Re-exec before GTK/GStreamer initialization instead of
//! mutating the environment of a process which may already have library threads.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn environment(
    library_dir: &Path,
    cache_dir: &Path,
    have_driver: bool,
    get: impl Fn(&str) -> Option<OsString>,
) -> Option<Vec<(&'static str, OsString)>> {
    // An explicit driver choice also identifies an already prepared re-exec.
    // Preserve callers' system/custom driver configurations and the old launcher.
    if !have_driver || get("LIBVA_DRIVERS_PATH").is_some() || get("LIBVA_DRIVER_NAME").is_some() {
        return None;
    }
    let mut libraries = library_dir.as_os_str().to_owned();
    if let Some(existing) = get("LD_LIBRARY_PATH").filter(|p| !p.is_empty()) {
        libraries.push(":");
        libraries.push(existing);
    }
    Some(vec![
        (
            "LIBVA_DRIVERS_PATH",
            library_dir.join("dri").into_os_string(),
        ),
        ("LIBVA_DRIVER_NAME", "iHD".into()),
        ("LD_LIBRARY_PATH", libraries),
        (
            "GST_REGISTRY",
            get("GST_REGISTRY")
                .unwrap_or_else(|| cache_dir.join("intel-va-registry.bin").into_os_string()),
        ),
    ])
}

fn xdg_dir(key: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
}

pub fn prepare() -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;

    let Some(data_dir) = xdg_dir("XDG_DATA_HOME", ".local/share") else {
        return Ok(());
    };
    let Some(cache_dir) = xdg_dir("XDG_CACHE_HOME", ".cache") else {
        return Ok(());
    };
    let library_dir = data_dir.join("extraspace/intel-va/usr/lib/x86_64-linux-gnu");
    let cache_dir = cache_dir.join("extraspace");
    let have_driver = library_dir.join("dri/iHD_drv_video.so").is_file()
        && library_dir.join("libigdgmm.so.12").is_file();
    let Some(environment) = environment(&library_dir, &cache_dir, have_driver, |name| {
        std::env::var_os(name)
    }) else {
        return Ok(());
    };
    std::fs::create_dir_all(&cache_dir)?;
    // exec preserves the PID and arguments, including --diagnostics. The new
    // process sees LIBVA_DRIVER_NAME, so this branch cannot re-exec in a loop.
    let error = std::process::Command::new(std::env::current_exe()?)
        .args(std::env::args_os().skip(1))
        .envs(environment)
        .exec();
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(have_driver: bool, values: &[(&str, &str)]) -> Option<Vec<(&'static str, OsString)>> {
        environment(
            Path::new("/data with spaces/intel-va/lib"),
            Path::new("/cache/extraspace"),
            have_driver,
            |name| values.iter().find(|v| v.0 == name).map(|v| v.1.into()),
        )
    }

    #[test]
    fn direct_launch_prepares_driver_and_separate_registry() {
        let values = plan(true, &[]).unwrap();
        assert!(values.contains(&(
            "LIBVA_DRIVERS_PATH",
            "/data with spaces/intel-va/lib/dri".into()
        )));
        assert!(values.contains(&("LIBVA_DRIVER_NAME", "iHD".into())));
        assert!(values.contains(&(
            "GST_REGISTRY",
            "/cache/extraspace/intel-va-registry.bin".into()
        )));
    }

    #[test]
    fn no_private_driver_leaves_system_discovery_alone() {
        assert!(plan(false, &[]).is_none());
    }

    #[test]
    fn explicit_driver_and_prepared_reexec_are_preserved() {
        assert!(plan(true, &[("LIBVA_DRIVER_NAME", "other")]).is_none());
        assert!(plan(true, &[("LIBVA_DRIVERS_PATH", "/custom")]).is_none());
        let prepared = plan(true, &[]).unwrap();
        assert!(
            environment(Path::new("/lib"), Path::new("/cache"), true, |name| {
                prepared.iter().find(|v| v.0 == name).map(|v| v.1.clone())
            })
            .is_none()
        );
    }

    #[test]
    fn existing_library_paths_and_registry_are_retained() {
        let values = plan(
            true,
            &[
                ("LD_LIBRARY_PATH", "/other:/last"),
                ("GST_REGISTRY", "/chosen"),
            ],
        )
        .unwrap();
        assert!(values.contains(&(
            "LD_LIBRARY_PATH",
            "/data with spaces/intel-va/lib:/other:/last".into()
        )));
        assert!(values.contains(&("GST_REGISTRY", "/chosen".into())));
    }
}
