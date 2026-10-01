//! Encoder settings remain reachable when a driver prevents streaming.
use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use xs_core::{
    available_encoders, Command, EncoderOption, EncoderSelection, EncodingMode, EngineHandle,
};

use crate::config::Config;

fn mode_at(index: u32) -> EncodingMode {
    match index {
        1 => EncodingMode::Cpu,
        2 => EncodingMode::Gpu,
        _ => EncodingMode::Auto,
    }
}

fn filtered(options: &[EncoderOption], mode: EncodingMode) -> Vec<EncoderOption> {
    options
        .iter()
        .filter(|o| match mode {
            EncodingMode::Auto => true,
            EncodingMode::Cpu => !o.is_gpu(),
            EncodingMode::Gpu => o.is_gpu(),
        })
        .cloned()
        .collect()
}

pub fn present(
    parent: &adw::ApplicationWindow,
    engine: &EngineHandle,
    config: &Rc<RefCell<Config>>,
) {
    let dialog = adw::PreferencesDialog::builder()
        .title("Video Encoding")
        .build();
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder()
        .title("Encode the display")
        .description("Changes reconnect an active display. Automatic tries GPU encoders first, then CPU if they cannot start.")
        .build();
    let mode = adw::ComboRow::builder()
        .title("Encode with")
        .model(&gtk::StringList::new(&["Automatic", "CPU", "GPU"]))
        .build();
    let saved = config.borrow().encoder.clone();
    mode.set_selected(match saved.mode {
        EncodingMode::Auto => 0,
        EncodingMode::Cpu => 1,
        EncodingMode::Gpu => 2,
    });
    let driver = adw::ComboRow::builder()
        .title("Encoder / Driver")
        .subtitle("Choose a detected encoder and GPU device, or let Extraspace choose")
        .build();
    driver.set_use_subtitle(false);
    let options = Rc::new(available_encoders());
    let current = Rc::new(RefCell::new(Vec::new()));
    let fill = {
        let options = options.clone();
        let current = current.clone();
        let driver = driver.clone();
        move |mode, pin: Option<&str>| {
            let choices = filtered(&options, mode);
            let mut labels = vec!["Best available".to_string()];
            labels.extend(choices.iter().map(|o| o.label.clone()));
            // Retain an unavailable saved choice visibly, rather than pretending
            // it has been changed back to Automatic when opening settings.
            let selected = if let Some(pin) = pin {
                choices
                    .iter()
                    .position(|o| o.factory == pin)
                    .map(|i| i as u32 + 1)
                    .unwrap_or_else(|| {
                        labels.push(format!("Unavailable: {pin}"));
                        labels.len() as u32 - 1
                    })
            } else {
                0
            };
            *current.borrow_mut() = choices;
            let refs: Vec<_> = labels.iter().map(String::as_str).collect();
            driver.set_model(Some(&gtk::StringList::new(&refs)));
            driver.set_selected(selected);
        }
    };
    fill(saved.mode, saved.factory.as_deref());
    mode.connect_selected_notify(move |row| fill(mode_at(row.selected()), None));
    group.add(&mode);
    group.add(&driver);
    let availability = adw::ActionRow::builder()
        .title("Available encoders")
        .subtitle(format!(
            "{} CPU · {} GPU. GPU choices require a compatible installed driver.",
            options.iter().filter(|o| !o.is_gpu()).count(),
            options.iter().filter(|o| o.is_gpu()).count()
        ))
        .build();
    group.add(&availability);
    page.add(&group);

    let actions = adw::PreferencesGroup::new();
    let apply = gtk::Button::builder()
        .label("Apply")
        .halign(gtk::Align::End)
        .build();
    apply.add_css_class("suggested-action");
    actions.add(&apply);
    page.add(&actions);
    dialog.add(&page);

    let engine = engine.clone();
    let config = config.clone();
    let close = dialog.downgrade();
    apply.connect_clicked(move |_| {
        let index = driver.selected();
        let factory = if index == 0 {
            None
        } else {
            let Some(option) = current.borrow().get(index as usize - 1).cloned() else {
                // An unavailable saved entry cannot be applied; select a usable
                // option first. Keep the dialog open with actionable feedback.
                driver.set_subtitle(
                    "This encoder is unavailable. Choose Best available or another encoder.",
                );
                return;
            };
            Some(option.factory)
        };
        let selection = EncoderSelection {
            mode: mode_at(mode.selected()),
            factory,
        };
        if selection.candidates(options.as_ref().clone()).is_err() {
            driver.set_subtitle(
                "No encoder is available in this mode. Choose another mode or install its driver.",
            );
            return;
        }
        config.borrow_mut().encoder = selection.clone();
        config.borrow().save();
        engine.send(Command::SetEncoder(selection));
        if let Some(dialog) = close.upgrade() {
            dialog.close();
        }
    });
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut result = vec![widget.clone()];
        let mut child = widget.first_child();
        while let Some(node) = child {
            result.extend(descendants(&node));
            child = node.next_sibling();
        }
        result
    }

    #[test]
    #[ignore = "requires a graphical session and a private D-Bus"]
    fn dialog_recovers_an_unavailable_pin_and_saves_the_cpu_choice() {
        adw::init().unwrap();
        let folder =
            std::env::temp_dir().join(format!("extraspace-encoding-ui-{}", std::process::id()));
        let old = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &folder);
        let app = adw::Application::builder()
            .application_id("io.github.tymonoman.Extraspace.EncodingTest")
            .build();
        app.register(None::<&gtk::gio::Cancellable>).unwrap();
        let parent = adw::ApplicationWindow::builder().application(&app).build();
        parent.present();
        let engine = xs_core::spawn(Default::default());
        let settings = Rc::new(RefCell::new(Config {
            auto_connect: false,
            encoder: EncoderSelection {
                factory: Some("unavailable-test-driver".into()),
                ..Default::default()
            },
            ..Default::default()
        }));
        present(&parent, &engine, &settings);
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        let nodes = descendants(parent.upcast_ref());
        let row = |title: &str| {
            nodes
                .iter()
                .filter_map(|n| n.clone().downcast::<adw::ComboRow>().ok())
                .find(|r| r.title() == title)
                .unwrap()
        };
        let mode = row("Encode with");
        let driver = row("Encoder / Driver");
        let selected = driver
            .selected_item()
            .unwrap()
            .downcast::<gtk::StringObject>()
            .unwrap();
        assert!(selected.string().contains("Unavailable"));
        let apply = nodes
            .iter()
            .filter_map(|n| n.clone().downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Apply"))
            .unwrap();
        apply.emit_clicked();
        assert!(
            !Config::path().exists(),
            "unavailable choice must not be saved"
        );
        mode.set_selected(1);
        assert_eq!(driver.selected(), 0);
        let cpu = filtered(&available_encoders(), EncodingMode::Cpu);
        assert!(!cpu.is_empty(), "install a CPU encoder for this test");
        driver.set_selected(1);
        apply.emit_clicked();
        let saved = Config::load();
        assert_eq!(saved.encoder.mode, EncodingMode::Cpu);
        assert_eq!(
            saved.encoder.factory.as_deref(),
            Some(cpu[0].factory.as_str())
        );
        parent.close();
        gtk::glib::MainContext::default().block_on(engine.shutdown());
        if let Some(old) = old {
            std::env::set_var("XDG_CONFIG_HOME", old);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        let _ = std::fs::remove_dir_all(folder);
    }
}
