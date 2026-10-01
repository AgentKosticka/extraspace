//! The main window.
//!
//! Two faces, switched by a [`gtk::Stack`]:
//!
//! * a full-bleed [`adw::StatusPage`] whenever there is nothing to control --
//!   no tablet, not authorised, connecting, or failed;
//! * an [`adw::PreferencesPage`] once a session is live.
//!
//! Failure states get first-class treatment rather than a generic error dialog.
//! The two most likely first-run problems -- no tablet, and USB debugging not
//! authorised -- are entirely fixable by the user, so each gets its own page
//! saying exactly which buttons to press.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use xs_core::{Command, DisplayMode, EngineHandle, Event, State, Stats};

use crate::config::{scale_index, Config, SCALE_OPTIONS};
use crate::tray;

pub const APP_ID: &str = "io.github.tymonoman.Extraspace";

struct Widgets {
    stack: gtk::Stack,
    status: adw::StatusPage,
    status_button: gtk::Button,
    spinner: gtk::Spinner,
    window_title: adw::WindowTitle,
    toasts: adw::ToastOverlay,

    display_switch: adw::SwitchRow,
    mode_row: adw::ComboRow,
    scale_row: adw::ComboRow,
    camera_switch: adw::SwitchRow,

    stat_resolution: adw::ActionRow,
    stat_bitrate: adw::ActionRow,
    stat_fps: adw::ActionRow,
    stat_latency: adw::ActionRow,
    stat_encoder: adw::ActionRow,
    updating_display: Cell<bool>,
}

pub fn build(app: &adw::Application, engine: EngineHandle, config: Rc<RefCell<Config>>) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Extraspace")
        .default_width(560)
        .default_height(720)
        .width_request(360)
        .height_request(400)
        .build();

    let window_title = adw::WindowTitle::new("Extraspace", "Not connected");

    let menu = gio_menu();
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Main Menu")
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&window_title)
        .build();
    header.pack_end(&menu_button);

    let (page, widgets) = build_content(window_title.clone());

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&widgets.toasts));
    widgets.toasts.set_child(Some(&page));
    window.set_content(Some(&toolbar));

    let widgets = Rc::new(widgets);
    let quitting = Rc::new(Cell::new(false));
    let tray_available = Rc::new(Cell::new(false));
    let (tray_controller, mut tray_actions) = tray::start(engine.clone());
    let tray_controller = Rc::new(RefCell::new(tray_controller));
    wire_actions(
        app,
        &window,
        &quitting,
        &tray_available,
        &config,
        &widgets.toasts,
    );
    let encoding = gtk::gio::SimpleAction::new("video-encoding", None);
    let parent = window.clone();
    let settings = config.clone();
    let video_engine = engine.clone();
    encoding
        .connect_activate(move |_, _| crate::encoding::present(&parent, &video_engine, &settings));
    app.add_action(&encoding);
    wire_controls(&widgets, &engine, &config);
    listen_to_engine(&widgets, &engine, &config);

    let engine_on_close = engine.clone();
    let closing = Rc::new(Cell::new(false));
    let config_on_close = config.clone();
    let available_on_close = tray_available.clone();
    let quitting_on_close = quitting.clone();
    let toasts = widgets.toasts.clone();
    window.connect_close_request(move |window| {
        if closing.get() {
            return glib::Propagation::Proceed;
        }
        if !quitting_on_close.get() && config_on_close.borrow().keep_running_in_tray {
            if available_on_close.get() {
                tracing::info!("window closed to tray; session continues");
                window.set_visible(false);
            } else {
                toasts.add_toast(adw::Toast::new(
                    "Tray unavailable. Enable GNOME’s AppIndicator extension, or use Quit.",
                ));
            }
            return glib::Propagation::Stop;
        }
        quitting_on_close.set(true);
        closing.set(true);
        let window = window.clone();
        let engine = engine_on_close.clone();
        let tray_controller = tray_controller.clone();
        glib::spawn_future_local(async move {
            engine.shutdown().await;
            tray_controller.borrow_mut().shutdown();
            window.close();
        });
        glib::Propagation::Stop
    });

    {
        let window = window.clone();
        let app = app.clone();
        let engine = engine.clone();
        let toasts = widgets.toasts.clone();
        glib::spawn_future_local(async move {
            while let Some(action) = tray_actions.recv().await {
                match action {
                    tray::Action::Open => {
                        tracing::info!("window restored from tray");
                        window.present();
                    }
                    tray::Action::Connect => engine.send(Command::Connect),
                    tray::Action::Disconnect => engine.send(Command::Disconnect),
                    tray::Action::Quit => app.activate_action("quit", None),
                    tray::Action::Available(available) => {
                        tray_available.set(available);
                        if let Some(action) = app.lookup_action("hide-window") {
                            if let Some(action) = action.downcast_ref::<gtk::gio::SimpleAction>() {
                                action.set_enabled(available);
                            }
                        }
                        if !available && !window.is_visible() && !quitting.get() {
                            tracing::info!("tray disappeared; restoring hidden window");
                            window.present();
                            toasts.add_toast(adw::Toast::new(
                                "Tray support disappeared; Extraspace is still running.",
                            ));
                        }
                    }
                }
            }
            tray_available.set(false);
            if !quitting.get() && !window.is_visible() {
                window.present();
                toasts.add_toast(adw::Toast::new(
                    "Tray stopped; Extraspace is still running.",
                ));
            }
        });
    }

    // Look for a tablet straight away; the user opened the app to use it.
    if config.borrow().auto_connect {
        engine.send(Command::Connect);
    }

    window.present();
}

fn build_content(window_title: adw::WindowTitle) -> (gtk::Widget, Widgets) {
    let toasts = adw::ToastOverlay::new();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(200)
        .build();

    // ---- the "nothing to control yet" face ----
    let status_button = gtk::Button::builder()
        .label("Check Again")
        .halign(gtk::Align::Center)
        .build();
    status_button.add_css_class("pill");
    status_button.add_css_class("suggested-action");

    let spinner = gtk::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .build();

    let status_extra = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    status_extra.append(&spinner);
    status_extra.append(&status_button);

    let status = adw::StatusPage::builder()
        .icon_name("computer-symbolic")
        .title("Looking for your tablet")
        .child(&status_extra)
        .build();
    stack.add_named(&status, Some("status"));

    // ---- the live control face ----
    let prefs = adw::PreferencesPage::new();

    let display_group = adw::PreferencesGroup::builder()
        .title("Display")
        .description("Use the tablet as an extra monitor")
        .build();

    let display_switch = adw::SwitchRow::builder()
        .title("Extra Display")
        .subtitle("Stream your desktop to the tablet")
        .build();
    display_group.add(&display_switch);

    let mode_model = gtk::StringList::new(&["Extend", "Mirror"]);
    let mode_row = adw::ComboRow::builder()
        .title("Mode")
        .subtitle("Extend adds a new monitor; Mirror copies an existing one")
        .model(&mode_model)
        .build();
    display_group.add(&mode_row);

    let scale_labels: Vec<String> = SCALE_OPTIONS.iter().map(|s| format!("{s}×")).collect();
    let scale_refs: Vec<&str> = scale_labels.iter().map(String::as_str).collect();
    let scale_row = adw::ComboRow::builder()
        .title("Scale")
        .subtitle("GNOME UI scale on native panel pixels")
        .model(&gtk::StringList::new(&scale_refs))
        .build();
    display_group.add(&scale_row);
    let encoding_row = adw::ActionRow::builder()
        .title("Video Encoding")
        .subtitle("Choose Automatic, CPU or GPU and the encoder driver")
        .activatable(true)
        .action_name("app.video-encoding")
        .build();
    encoding_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    display_group.add(&encoding_row);
    prefs.add(&display_group);

    let camera_group = adw::PreferencesGroup::builder()
        .title("Camera")
        .description("Expose the tablet camera to Linux as a normal webcam")
        .build();
    let camera_switch = adw::SwitchRow::builder()
        .title("Tablet Camera")
        .subtitle("Appears as “Extraspace Tablet Camera”")
        .build();
    camera_group.add(&camera_switch);
    prefs.add(&camera_group);

    let stats_group = adw::PreferencesGroup::builder().title("Statistics").build();
    let stat_resolution = stat_row("Resolution", "video-display-symbolic");
    let stat_bitrate = stat_row("Bitrate", "network-transmit-symbolic");
    let stat_fps = stat_row("Decoded Frame Rate", "preferences-system-time-symbolic");
    stat_fps.set_tooltip_text(Some(
        "Frames released by the tablet decoder. An idle desktop can produce zero frames.",
    ));
    let stat_latency = stat_row("Control RTT", "network-wireless-symbolic");
    stat_latency.set_tooltip_text(Some("USB control-message round trip. Capture, encoding, decoding and display delay are not included; visual latency is not measured."));
    let stat_encoder = stat_row("Encoder", "applications-multimedia-symbolic");
    for row in [
        &stat_resolution,
        &stat_bitrate,
        &stat_fps,
        &stat_latency,
        &stat_encoder,
    ] {
        stats_group.add(row);
    }
    prefs.add(&stats_group);

    stack.add_named(&prefs, Some("running"));
    stack.set_visible_child_name("status");

    let widgets = Widgets {
        stack: stack.clone(),
        status,
        status_button,
        spinner,
        window_title,
        toasts,
        display_switch,
        mode_row,
        scale_row,
        camera_switch,
        stat_resolution,
        stat_bitrate,
        stat_fps,
        stat_latency,
        stat_encoder,
        updating_display: Cell::new(false),
    };
    (stack.upcast(), widgets)
}

fn stat_row(title: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle("—").build();
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    row.set_subtitle_selectable(true);
    row
}

fn gio_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();
    menu.append(
        Some("Keep Running in _Tray"),
        Some("app.keep-running-in-tray"),
    );
    menu.append(Some("_Hide Window"), Some("app.hide-window"));
    menu.append(Some("_Video Encoding"), Some("app.video-encoding"));
    menu.append(Some("_About Extraspace"), Some("app.about"));
    menu.append(Some("_Quit"), Some("app.quit"));
    menu
}

fn wire_actions(
    app: &adw::Application,
    window: &adw::ApplicationWindow,
    quitting: &Rc<Cell<bool>>,
    available: &Rc<Cell<bool>>,
    config: &Rc<RefCell<Config>>,
    toasts: &adw::ToastOverlay,
) {
    let background = gtk::gio::SimpleAction::new_stateful(
        "keep-running-in-tray",
        None,
        &config.borrow().keep_running_in_tray.to_variant(),
    );
    let settings = config.clone();
    let tray_available = available.clone();
    let messages = toasts.clone();
    background.connect_change_state(move |action, value| {
        let Some(enabled) = value.and_then(|v| v.get::<bool>()) else {
            return;
        };
        settings.borrow_mut().keep_running_in_tray = enabled;
        settings.borrow().save();
        action.set_state(&enabled.to_variant());
        if enabled && !tray_available.get() {
            messages.add_toast(adw::Toast::new(
                "Enable GNOME’s AppIndicator extension to use the tray.",
            ));
        }
    });
    // Menu activation toggles a stateful boolean action.
    background.connect_activate(|action, _| {
        if let Some(current) = action.state().and_then(|v| v.get::<bool>()) {
            action.change_state(&(!current).to_variant());
        }
    });
    app.add_action(&background);

    let hide = gtk::gio::SimpleAction::new("hide-window", None);
    hide.set_enabled(false);
    let parent = window.clone();
    let tray_available = available.clone();
    hide.connect_activate(move |_, _| {
        if tray_available.get() {
            tracing::info!("window hidden to tray; session continues");
            parent.set_visible(false);
        }
    });
    app.add_action(&hide);

    let close = gtk::gio::SimpleAction::new("close-window", None);
    let parent = window.clone();
    close.connect_activate(move |_, _| parent.close());
    app.add_action(&close);
    app.set_accels_for_action("app.close-window", &["<primary>w"]);
    let about = gtk::gio::SimpleAction::new("about", None);
    let parent = window.clone();
    about.connect_activate(move |_, _| {
        adw::AboutDialog::builder()
            .application_name("Extraspace")
            .application_icon(APP_ID)
            .developer_name("Tymonoman")
            .version(env!("CARGO_PKG_VERSION"))
            .website("https://github.com/AgentKosticka/extraspace")
            .issue_url("https://github.com/AgentKosticka/extraspace/issues")
            .license_type(gtk::License::Gpl30)
            .comments("Use an Android tablet as an extra display and webcam, over USB.")
            .build()
            .present(Some(&parent));
    });
    app.add_action(&about);

    let quit = gtk::gio::SimpleAction::new("quit", None);
    let window = window.clone();
    let quitting = quitting.clone();
    quit.connect_activate(move |_, _| {
        quitting.set(true);
        window.close();
    });
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<primary>q"]);
}

fn wire_controls(widgets: &Rc<Widgets>, engine: &EngineHandle, config: &Rc<RefCell<Config>>) {
    // Reflect stored settings before connecting handlers, so restoring state
    // does not immediately fire a command back at the engine.
    {
        let c = config.borrow();
        widgets.scale_row.set_selected(scale_index(c.scale));
        widgets
            .mode_row
            .set_selected(if c.display_mode() == DisplayMode::Mirror {
                1
            } else {
                0
            });
        widgets.camera_switch.set_active(c.camera_enabled);
        widgets.display_switch.set_active(true);
    }
    update_scale_subtitle(widgets, config);

    {
        let engine = engine.clone();
        let widgets_ref = Rc::clone(widgets);
        widgets.display_switch.connect_active_notify(move |row| {
            if widgets_ref.updating_display.get() {
                return;
            }
            engine.send(if row.is_active() {
                Command::Connect
            } else {
                Command::Disconnect
            });
        });
    }

    {
        let engine = engine.clone();
        let config = Rc::clone(config);
        let widgets_ref = Rc::clone(widgets);
        widgets.scale_row.connect_selected_notify(move |row| {
            let Some(&scale) = SCALE_OPTIONS.get(row.selected() as usize) else {
                return;
            };
            config.borrow_mut().scale = scale;
            config.borrow().save();
            update_scale_subtitle(&widgets_ref, &config);
            engine.send(Command::SetScale(scale));
        });
    }

    {
        let engine = engine.clone();
        let config = Rc::clone(config);
        widgets.mode_row.connect_selected_notify(move |row| {
            let mode = if row.selected() == 1 {
                DisplayMode::Mirror
            } else {
                DisplayMode::Extend
            };
            config.borrow_mut().mode = if row.selected() == 1 {
                "mirror"
            } else {
                "extend"
            }
            .into();
            config.borrow().save();
            engine.send(Command::SetMode(mode));
        });
    }

    {
        let engine = engine.clone();
        let config = Rc::clone(config);
        widgets.camera_switch.connect_active_notify(move |row| {
            let enabled = row.is_active();
            let camera_id = {
                let mut c = config.borrow_mut();
                c.camera_enabled = enabled;
                c.camera_id.clone()
            };
            config.borrow().save();
            engine.send(Command::SetCamera { enabled, camera_id });
        });
    }

    {
        let engine = engine.clone();
        widgets.status_button.connect_clicked(move |_| {
            engine.send(Command::Connect);
        });
    }
}

fn update_scale_subtitle(widgets: &Rc<Widgets>, config: &Rc<RefCell<Config>>) {
    let scale = config.borrow().scale;
    let description = if xs_core::modes_enabled() {
        "GNOME scale on panel pixels"
    } else {
        "smaller framebuffer, upscaled on the tablet"
    };
    widgets
        .scale_row
        .set_subtitle(&format!("{scale}× — {description}"));
}

fn listen_to_engine(widgets: &Rc<Widgets>, engine: &EngineHandle, config: &Rc<RefCell<Config>>) {
    let mut events = engine.subscribe();
    let widgets = Rc::clone(widgets);
    let config = Rc::clone(config);

    glib::spawn_future_local(async move {
        loop {
            match events.recv().await {
                Ok(Event::State(state)) => apply_state(&widgets, &state, &config),
                Ok(Event::Stats(stats)) => apply_stats(&widgets, &stats),
                Ok(Event::Warning(message)) => {
                    widgets.toasts.add_toast(adw::Toast::new(&message));
                }
                // Lagged means the UI fell behind a burst; the next state event
                // resyncs it, so there is nothing useful to do but continue.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn apply_state(widgets: &Rc<Widgets>, state: &State, config: &Rc<RefCell<Config>>) {
    if matches!(state, State::Idle | State::Streaming { .. }) {
        widgets.updating_display.set(true);
        widgets
            .display_switch
            .set_active(matches!(state, State::Streaming { .. }));
        widgets.updating_display.set(false);
    }
    let show_status = |icon: &str, title: &str, description: &str, button: Option<&str>| {
        widgets.status.set_icon_name(Some(icon));
        widgets.status.set_title(title);
        widgets.status.set_description(Some(description));
        widgets.spinner.set_visible(false);
        widgets.spinner.stop();
        match button {
            Some(label) => {
                widgets.status_button.set_label(label);
                widgets.status_button.set_visible(true);
            }
            None => widgets.status_button.set_visible(false),
        }
        widgets.stack.set_visible_child_name("status");
    };

    match state {
        State::Idle => {
            widgets.window_title.set_subtitle("Not connected");
            show_status(
                "video-display-symbolic",
                "Ready",
                "Turn on Extra Display to start streaming to your tablet.",
                Some("Connect"),
            );
        }

        State::NoTablet => {
            widgets.window_title.set_subtitle("No tablet");
            show_status(
                "phone-disconnected-symbolic",
                "No tablet found",
                "Connect your tablet over USB, then enable USB debugging:\n\n\
                 1.  Settings → About tablet → tap “Build number” seven times\n\
                 2.  Settings → System → Developer options → USB debugging",
                Some("Check Again"),
            );
        }

        State::Unauthorized { device } => {
            widgets.window_title.set_subtitle("Not authorised");
            show_status(
                "dialog-password-symbolic",
                "Allow USB debugging",
                &format!(
                    "{device} is connected but has not granted permission yet.\n\n\
                     Unlock the tablet and tap “Allow” on the USB debugging prompt. \
                     Tick “Always allow from this computer” so you are not asked again.",
                ),
                Some("Try Again"),
            );
        }

        State::Connecting { step } => {
            widgets.window_title.set_subtitle("Connecting…");
            widgets
                .status
                .set_icon_name(Some("content-loading-symbolic"));
            widgets.status.set_title("Connecting");
            widgets.status.set_description(Some(step));
            widgets.status_button.set_visible(false);
            widgets.spinner.set_visible(true);
            widgets.spinner.start();
            widgets.stack.set_visible_child_name("status");
        }

        State::Streaming {
            device,
            width,
            height,
            encoder,
        } => {
            widgets.window_title.set_subtitle(device);
            widgets.spinner.stop();
            let scale = xs_core::clamp_ui_scale(config.borrow().scale);
            let description = if xs_core::modes_enabled() {
                format!("{scale}× GNOME scale")
            } else {
                "upscaled to the tablet panel".into()
            };
            widgets
                .stat_resolution
                .set_subtitle(&format!("{width} × {height} stream — {description}"));
            widgets.stat_encoder.set_subtitle(encoder);
            widgets.stack.set_visible_child_name("running");
        }

        State::Failed { message } => {
            widgets.window_title.set_subtitle("Error");
            show_status(
                "dialog-warning-symbolic",
                "Something went wrong",
                message,
                Some("Try Again"),
            );
        }
    }
}

fn apply_stats(widgets: &Rc<Widgets>, stats: &Stats) {
    widgets
        .stat_bitrate
        .set_subtitle(&xs_core::format_bitrate(stats.bitrate_kbps));
    widgets
        .stat_fps
        .set_subtitle(&format!("{:.0} fps", stats.fps));
    // Bound to a local: a `format!` temporary inside the call would not outlive it.
    let latency = if stats.control_rtt_ms > 0.0 {
        format!("{:.1} ms", stats.control_rtt_ms)
    } else {
        "measuring…".to_owned()
    };
    widgets.stat_latency.set_subtitle(&latency);
}
