//! All recovery settings are available before connecting and after failure.
use crate::config::{scale_index, Config, SCALE_OPTIONS};
use adw::prelude::*;
use std::{cell::RefCell, rc::Rc};
use xs_core::{CameraInfo, Command, DeviceSettings, DisplayMode, EngineHandle, Event, MonitorInfo};

#[derive(Clone)]
struct Choice {
    id: Option<String>,
    label: String,
    available: bool,
}
fn fill(
    row: &adw::ComboRow,
    choices: &Rc<RefCell<Vec<Choice>>>,
    mut items: Vec<Choice>,
    saved: Option<String>,
) {
    let selected = items
        .iter()
        .position(|item| item.id == saved)
        .unwrap_or_else(|| {
            let label = format!("Unavailable: {}", saved.as_deref().unwrap_or("Automatic"));
            items.push(Choice {
                id: saved,
                label,
                available: false,
            });
            items.len() - 1
        });
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    row.set_model(Some(&gtk::StringList::new(&labels)));
    row.set_selected(selected as u32);
    *choices.borrow_mut() = items;
}
fn selected(row: &adw::ComboRow, choices: &Rc<RefCell<Vec<Choice>>>) -> Option<Choice> {
    choices.borrow().get(row.selected() as usize).cloned()
}
fn monitors(
    row: &adw::ComboRow,
    choices: &Rc<RefCell<Vec<Choice>>>,
    list: Vec<MonitorInfo>,
    saved: Option<String>,
) {
    let mut items = vec![Choice {
        id: None,
        label: "Primary monitor".into(),
        available: !list.is_empty(),
    }];
    items.extend(list.into_iter().map(|m| Choice {
        id: Some(m.connector),
        label: m.label,
        available: true,
    }));
    fill(row, choices, items, saved);
}
fn cameras(
    row: &adw::ComboRow,
    choices: &Rc<RefCell<Vec<Choice>>>,
    list: Vec<CameraInfo>,
    saved: String,
) {
    let items = list
        .into_iter()
        .map(|c| Choice {
            id: Some(c.id.clone()),
            label: format!("{} camera ({})", c.facing, c.id),
            available: true,
        })
        .collect();
    fill(row, choices, items, Some(saved));
}

pub fn present(
    parent: &adw::ApplicationWindow,
    engine: &EngineHandle,
    config: &Rc<RefCell<Config>>,
    monitor_list: &Rc<RefCell<Vec<MonitorInfo>>>,
    camera_list: &Rc<RefCell<Vec<CameraInfo>>>,
) {
    let dialog = adw::PreferencesDialog::builder()
        .title("Display & Camera Settings")
        .build();
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder().title("Display")
        .description("Apply saves these settings. A connected display restarts once to use the new settings.").build();
    let c = config.borrow().clone();
    let mode = adw::ComboRow::builder()
        .title("Mode")
        .model(&gtk::StringList::new(&["Extend", "Mirror"]))
        .build();
    mode.set_selected(u32::from(c.display_mode() == DisplayMode::Mirror));
    let source = adw::ComboRow::builder().title("Mirror source").build();
    let source_choices = Rc::new(RefCell::new(Vec::new()));
    monitors(
        &source,
        &source_choices,
        monitor_list.borrow().clone(),
        c.mirror_source.clone(),
    );
    source.set_visible(c.display_mode() == DisplayMode::Mirror);
    let source_visibility = source.clone();
    mode.connect_selected_notify(move |row| source_visibility.set_visible(row.selected() == 1));
    let labels: Vec<_> = SCALE_OPTIONS.iter().map(|s| format!("{s}×")).collect();
    let refs: Vec<_> = labels.iter().map(String::as_str).collect();
    let scale = adw::ComboRow::builder().title("Render Scale")
        .subtitle("On stock GNOME, larger values reduce stream resolution. Use Displays for desktop text size.")
        .model(&gtk::StringList::new(&refs)).build();
    scale.set_selected(scale_index(c.scale));
    let fps = adw::SpinRow::with_range(1.0, 120.0, 1.0);
    fps.set_title("Frame rate (fps)");
    fps.set_subtitle("Try 30 for lower power use; 60 for smoother motion");
    fps.set_value(c.framerate.clamp(1, 120) as f64);
    let min = adw::SpinRow::with_range(0.1, 100.0, 0.5);
    min.set_title("Minimum quality bitrate (Mbps)");
    min.set_digits(1);
    min.set_value(c.bounds().min_kbps as f64 / 1000.0);
    let max = adw::SpinRow::with_range(0.1, 100.0, 0.5);
    max.set_title("Maximum quality bitrate (Mbps)");
    max.set_digits(1);
    max.set_value(c.bounds().max_kbps as f64 / 1000.0);
    max.set_subtitle("Higher values preserve detail and use more USB bandwidth");
    group.add(&mode);
    group.add(&source);
    group.add(&scale);
    group.add(&fps);
    group.add(&min);
    group.add(&max);
    let automatic = adw::SwitchRow::builder()
        .title("Connect when Extraspace opens")
        .active(c.auto_connect)
        .build();
    group.add(&automatic);
    page.add(&group);
    let camera_group = adw::PreferencesGroup::builder().title("Camera").build();
    let enabled = adw::SwitchRow::builder()
        .title("Tablet Camera")
        .active(c.camera_enabled)
        .build();
    let camera = adw::ComboRow::builder()
        .title("Camera source")
        .subtitle("Connect a tablet to discover its cameras")
        .build();
    let camera_choices = Rc::new(RefCell::new(Vec::new()));
    cameras(
        &camera,
        &camera_choices,
        camera_list.borrow().clone(),
        c.camera_id,
    );
    camera_group.add(&enabled);
    camera_group.add(&camera);
    let setup = adw::ActionRow::builder()
        .title("Camera Setup")
        .subtitle("Install the optional virtual webcam, then allow camera access on your tablet")
        .activatable(true)
        .action_name("app.camera-setup")
        .build();
    setup.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    camera_group.add(&setup);
    page.add(&camera_group);
    let actions = adw::PreferencesGroup::new();
    let feedback = adw::ActionRow::builder()
        .title("Settings")
        .subtitle("Ready to apply")
        .build();
    actions.add(&feedback);
    let apply = gtk::Button::builder()
        .label("Apply")
        .halign(gtk::Align::End)
        .build();
    apply.add_css_class("suggested-action");
    actions.add(&apply);
    page.add(&actions);
    dialog.add(&page);
    let engine_apply = engine.clone();
    let config_apply = config.clone();
    let weak = dialog.downgrade();
    let source_apply = source.clone();
    let sources = source_choices.clone();
    let camera_apply = camera.clone();
    let camera_options = camera_choices.clone();
    apply.connect_clicked(move |_| {
        let mirror = mode.selected() == 1;
        let source = selected(&source_apply, &sources);
        if mirror && !source.as_ref().is_some_and(|item| item.available) {
            feedback.set_subtitle("Choose an available monitor, or switch to Extend.");
            return;
        }
        let camera = selected(&camera_apply, &camera_options);
        if enabled.is_active() && !camera.as_ref().is_some_and(|item| item.available) {
            feedback.set_subtitle(
                "Connect the tablet and choose an available camera, or turn Tablet Camera off.",
            );
            return;
        }
        if min.value() > max.value() {
            feedback.set_subtitle("Minimum bitrate must be no greater than maximum bitrate.");
            return;
        }
        let settings = {
            let mut c = config_apply.borrow_mut();
            c.mode = if mirror { "mirror" } else { "extend" }.into();
            c.mirror_source = source.and_then(|item| item.id);
            c.scale = SCALE_OPTIONS[scale.selected() as usize];
            c.framerate = fps.value() as u32;
            c.min_bitrate_kbps = (min.value() * 1000.0).round() as u32;
            c.max_bitrate_kbps = (max.value() * 1000.0).round() as u32;
            c.camera_enabled = enabled.is_active();
            if let Some(id) = camera.and_then(|item| item.id) {
                c.camera_id = id;
            }
            c.auto_connect = automatic.is_active();
            c.save();
            DeviceSettings {
                scale: c.scale,
                mode: c.display_mode(),
                mirror_source: c.mirror_source.clone(),
                framerate: c.framerate,
                encoder: c.encoder.clone(),
                bounds: c.bounds(),
                camera_enabled: c.camera_enabled,
                camera_id: c.camera_id.clone(),
            }
        };
        engine_apply.send(Command::Configure(settings));
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    // Refresh choices in-place while the sheet is open, without changing a valid selection.
    let mut events = engine.subscribe();
    let weak = dialog.downgrade();
    gtk::glib::spawn_future_local(async move {
        while weak.upgrade().is_some_and(|d| d.is_visible()) {
            match events.recv().await {
                Ok(Event::Monitors(list)) => {
                    let saved = selected(&source, &source_choices).and_then(|item| item.id);
                    monitors(&source, &source_choices, list, saved);
                }
                Ok(Event::Cameras(list)) => {
                    let saved = selected(&camera, &camera_choices)
                        .and_then(|item| item.id)
                        .unwrap_or_default();
                    cameras(&camera, &camera_choices, list, saved);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                _ => {}
            }
        }
    });
    dialog.present(Some(parent));
    engine.send(Command::RefreshMonitors);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut nodes = vec![widget.clone()];
        let mut child = widget.first_child();
        while let Some(node) = child {
            nodes.extend(descendants(&node));
            child = node.next_sibling();
        }
        nodes
    }

    #[test]
    #[ignore = "requires a graphical session and a private D-Bus"]
    fn offline_settings_repair_invalid_sources_and_save_controls() {
        adw::init().unwrap();
        let folder =
            std::env::temp_dir().join(format!("extraspace-recovery-ui-{}", std::process::id()));
        let old = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &folder);
        let app = adw::Application::builder()
            .application_id("io.github.tymonoman.Extraspace.RecoveryTest")
            .build();
        app.register(None::<&gtk::gio::Cancellable>).unwrap();
        let parent = adw::ApplicationWindow::builder()
            .application(&app)
            .default_width(560)
            .default_height(720)
            .build();
        parent.present();
        let engine = xs_core::spawn(xs_core::SessionConfig {
            last_device_id: Some("repair-fixture".into()),
            ..Default::default()
        });
        let config = Rc::new(RefCell::new(Config {
            auto_connect: false,
            mode: "mirror".into(),
            mirror_source: Some("disconnected-output".into()),
            ..Default::default()
        }));
        let monitors = Rc::new(RefCell::new(vec![MonitorInfo {
            connector: "DP-1".into(),
            label: "Desk monitor (Primary)".into(),
            primary: true,
        }]));
        let cameras = Rc::new(RefCell::new(vec![CameraInfo {
            id: "1".into(),
            facing: "front".into(),
            max_width: 1920,
            max_height: 1080,
        }]));
        present(&parent, &engine, &config, &monitors, &cameras);
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        let nodes = descendants(parent.upcast_ref());
        if let Some(path) = std::env::var_os("EXTRASPACE_TEST_SCREENSHOT") {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                while gtk::glib::MainContext::default().pending() {
                    gtk::glib::MainContext::default().iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let snapshot = gtk::Snapshot::new();
            gtk::WidgetPaintable::new(Some(&parent)).snapshot(
                &snapshot,
                parent.width() as f64,
                parent.height() as f64,
            );
            let node = snapshot.to_node().unwrap();
            let renderer = gtk::gsk::CairoRenderer::new();
            renderer.realize(parent.surface().as_ref()).unwrap();
            renderer
                .render_texture(&node, None)
                .save_to_png(path)
                .unwrap();
            renderer.unrealize();
        }
        let combo = |title: &str| {
            nodes
                .iter()
                .filter_map(|n| n.clone().downcast::<adw::ComboRow>().ok())
                .find(|r| r.title() == title)
                .unwrap()
        };
        let spin = |title: &str| {
            nodes
                .iter()
                .filter_map(|n| n.clone().downcast::<adw::SpinRow>().ok())
                .find(|r| r.title() == title)
                .unwrap()
        };
        let apply = nodes
            .iter()
            .filter_map(|n| n.clone().downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Apply"))
            .unwrap();
        assert!(combo("Mirror source")
            .selected_item()
            .unwrap()
            .downcast::<gtk::StringObject>()
            .unwrap()
            .string()
            .contains("Unavailable"));
        apply.emit_clicked();
        assert!(!Config::path().exists(), "invalid source must not be saved");
        combo("Mode").set_selected(0);
        let camera_enabled = nodes
            .iter()
            .filter_map(|n| n.clone().downcast::<adw::SwitchRow>().ok())
            .find(|r| r.title() == "Tablet Camera")
            .unwrap();
        camera_enabled.set_active(true);
        apply.emit_clicked();
        assert!(!Config::path().exists(), "invalid camera must not be saved");
        combo("Camera source").set_selected(0);
        combo("Mirror source").set_selected(0);
        spin("Frame rate (fps)").set_value(30.0);
        spin("Minimum quality bitrate (Mbps)").set_value(50.0);
        spin("Maximum quality bitrate (Mbps)").set_value(42.0);
        apply.emit_clicked();
        assert!(
            !Config::path().exists(),
            "reversed bitrate bounds must not be saved"
        );
        spin("Minimum quality bitrate (Mbps)").set_value(3.0);
        apply.emit_clicked();
        let saved = Config::load();
        assert_eq!(saved.display_mode(), DisplayMode::Extend);
        assert_eq!(saved.mirror_source, None);
        assert_eq!(saved.camera_id, "1");
        assert!(saved.camera_enabled);
        assert_eq!(saved.framerate, 30);
        assert_eq!(saved.bounds().min_kbps, 3000);
        assert_eq!(saved.bounds().max_kbps, 42000);
        parent.close();
        gtk::glib::MainContext::default().block_on(engine.shutdown());
        let profile =
            std::fs::read_to_string(folder.join("extraspace/device-settings.json")).unwrap();
        assert!(
            profile.contains("repair-fixture") && profile.contains("42000"),
            "offline repair must update the device profile too"
        );
        if let Some(old) = old {
            std::env::set_var("XDG_CONFIG_HOME", old);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        let _ = std::fs::remove_dir_all(folder);
    }
}
