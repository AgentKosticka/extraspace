//! StatusNotifier tray on its own runtime; all GTK operations stay on GTK's thread.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use ksni::{menu::StandardItem, TrayMethods};
use tokio::sync::{mpsc, oneshot};
use xs_core::{EngineHandle, Event, State};

use crate::window::APP_ID;

pub enum Action {
    Open,
    Connect,
    Disconnect,
    Quit,
    Available(bool),
}

struct Indicator {
    actions: mpsc::UnboundedSender<Action>,
    available: Arc<AtomicBool>,
    description: String,
}

impl ksni::Tray for Indicator {
    fn id(&self) -> String {
        APP_ID.into()
    }
    fn title(&self) -> String {
        "Extraspace".into()
    }
    fn icon_name(&self) -> String {
        APP_ID.into()
    }
    fn icon_theme_path(&self) -> String {
        // Source runs can find the same icon before it has been installed.
        std::fs::canonicalize("packaging")
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_default()
    }
    fn activate(&mut self, _: i32, _: i32) {
        let _ = self.actions.send(Action::Open);
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Extraspace".into(),
            description: self.description.clone(),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.description.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Open Extraspace".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.actions.send(Action::Open);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Connect".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.actions.send(Action::Connect);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Disconnect".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.actions.send(Action::Disconnect);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.actions.send(Action::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
    fn watcher_online(&self) {
        self.available.store(true, Ordering::Release);
        let _ = self.actions.send(Action::Available(true));
    }
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::info!(?reason, "tray unavailable; keeping the window accessible");
        self.available.store(false, Ordering::Release);
        let _ = self.actions.send(Action::Available(false));
        true // Re-register if the GNOME extension returns.
    }
}

pub struct Controller(Option<oneshot::Sender<()>>);
impl Controller {
    pub fn shutdown(&mut self) {
        if let Some(stop) = self.0.take() {
            let _ = stop.send(());
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn start(engine: EngineHandle) -> (Controller, mpsc::UnboundedReceiver<Action>) {
    let (actions, receiver) = mpsc::unbounded_channel();
    let (stop, stopped) = oneshot::channel();
    // Subscribe before the worker starts so early connection states are retained.
    let mut events = engine.subscribe();
    let worker = std::thread::Builder::new().name("xs-tray".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => { tracing::warn!(%error, "could not start tray runtime"); return; }
        };
        runtime.block_on(async move {
            let available = Arc::new(AtomicBool::new(true));
            let indicator = Indicator { actions: actions.clone(), available: available.clone(), description: "Ready".into() };
            let handle = match indicator.assume_sni_available(true).spawn().await {
                Ok(handle) => handle,
                Err(error) => { tracing::warn!(%error, "could not start tray"); return; }
            };
            let _ = actions.send(Action::Available(available.load(Ordering::Acquire)));
            tokio::pin!(stopped);
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    event = events.recv() => match event {
                        Ok(Event::State(state)) => {
                            let description = match state {
                                State::Idle => "Ready".into(),
                                State::NoTablet => "Waiting for a tablet".into(),
                                State::Unauthorized { .. } => "Allow USB debugging on the tablet".into(),
                                State::Connecting { .. } => "Connecting…".into(),
                                State::Streaming { device, .. } => format!("Connected to {device}"),
                                State::Failed { .. } => "Connection failed — open Extraspace".into(),
                            };
                            handle.update(move |tray| tray.description = description).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        _ => {}
                    }
                }
            }
            handle.shutdown().await;
        });
    });
    if let Err(error) = worker {
        tracing::warn!(%error, "could not start tray thread");
    }
    (Controller(Some(stop)), receiver)
}
