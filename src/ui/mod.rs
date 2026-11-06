//! GTK stays on the main thread. Workers communicate through typed messages;
//! no widget is ever sent to an audio, network, or inference thread.
mod actions;
mod app;
mod device_controller;
mod dialogs;
mod export;
mod lifecycle;
mod markdown_buffer;
mod markdown_diff;
mod markdown_document;
mod markdown_scroll;
mod menu;
mod model_controller;
mod models;
mod preferences;
mod rewrite;
mod task;
mod text_zoom;
mod transcript_buffer;
mod transcript_state;
mod transcription_controller;
mod views;

#[cfg(test)]
mod tests;

use self::{app::App, task::background};
use crate::settings::{self, Settings};
use adw::prelude::*;
use std::{sync::mpsc, time::Duration};

const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(30);
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(80);
const REWRITE_PANES_BREAKPOINT: i32 = 800;

pub fn build(application: &adw::Application) {
    if let Some(window) = application.active_window() {
        window.present();
        return;
    }
    let loading = adw::ApplicationWindow::builder()
        .application(application)
        .title("Speech2Text — Loading preferences…")
        .default_width(600)
        .default_height(400)
        .content(
            &gtk::Spinner::builder()
                .spinning(true)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .width_request(32)
                .height_request(32)
                .build(),
        )
        .build();
    loading.present();
    let events = background(|| Settings::load().map_err(|error| format!("{error:#}")));
    let application = application.clone();
    gtk::glib::timeout_add_local(STARTUP_POLL_INTERVAL, move || {
        if !loading.is_visible() {
            return gtk::glib::ControlFlow::Break;
        }
        let result = match events.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Preferences worker stopped unexpectedly".into())
            }
        };
        let (settings, error) = match result {
            Ok(settings) => (settings, None),
            Err(error) => (Settings::default(), Some(error)),
        };
        build_loaded(&application, settings, error);
        loading.close();
        gtk::glib::ControlFlow::Break
    });
}

fn build_loaded(application: &adw::Application, settings: Settings, load_error: Option<String>) {
    let app = App::new(Some(application), settings, settings::SaveWorker::new());
    app.restore_prompts();
    app.connect();
    app.refresh_devices();
    app.update_model_label();
    app.refresh();
    app.window.present();
    if let Some(error) = load_error {
        dialogs::message(&app.window, "Preferences could not be loaded", &error);
    }
    // This main-loop source owns the controller until the window closes. All
    // widget callbacks hold weak references, avoiding widget/controller cycles.
    gtk::glib::timeout_add_local(WORKER_POLL_INTERVAL, move || {
        if !app.window.is_visible() {
            app.discard_active_work();
            return gtk::glib::ControlFlow::Break;
        }
        app.poll();
        let orientation = if app.window.width() < REWRITE_PANES_BREAKPOINT {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        };
        app.rewrite.panes.set_orientation(orientation);
        gtk::glib::ControlFlow::Continue
    });
}
