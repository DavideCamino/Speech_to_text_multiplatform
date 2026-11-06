//! Background microphone discovery and completion handling.
use super::{app::App, task::background};
use std::sync::mpsc::{self, Receiver};

pub(super) type DeviceDiscovery = Receiver<Result<Vec<String>, String>>;

impl App {
    pub(super) fn refresh_devices(&self) {
        if self.busy() || self.device_discovery.borrow().is_some() {
            return;
        }
        self.preferences.begin_device_refresh();
        *self.device_discovery.borrow_mut() = Some(background(|| {
            crate::audio::input_devices().map_err(|error| format!("{error:#}"))
        }));
    }

    pub(super) fn poll_devices(&self) {
        let devices =
            self.device_discovery
                .borrow()
                .as_ref()
                .and_then(|events| match events.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                        "Microphone discovery worker stopped unexpectedly".into(),
                    )),
                });
        if let Some(result) = devices {
            self.device_discovery.borrow_mut().take();
            self.preferences.finish_device_refresh(result);
            self.refresh();
        }
    }
}
