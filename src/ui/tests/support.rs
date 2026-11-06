//! Shared test construction and bounded waits. GTK waits explicitly dispatch
//! events; worker waits deliberately do not, to avoid unrelated UI callbacks.
use crate::{
    settings::{self, Settings},
    ui::app::App,
};
use adw::prelude::*;
use gtk::glib;
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(1);

pub(in crate::ui) fn test_app() -> Rc<App> {
    App::new(
        None,
        Settings::default(),
        settings::SaveWorker::with_save(|_| Ok(())),
    )
}

/// Poll a worker condition without dispatching the GTK main context.
pub(in crate::ui) fn wait_for_background(description: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Pump GTK until a real condition holds; fail rather than silently skipping
/// layout work on a slow/headless runner. Only call on the shared GTK test thread.
pub(in crate::ui) fn wait_for_gtk(description: &str, mut condition: impl FnMut() -> bool) {
    let context = glib::MainContext::default();
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        context.iteration(false);
        if condition() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

pub(in crate::ui) fn wait_for_gtk_frames(view: &gtk::TextView, count: u32) {
    let frames = Rc::new(Cell::new(0));
    let observed = frames.clone();
    view.add_tick_callback(move |_, _| {
        observed.set(observed.get() + 1);
        if observed.get() >= count {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
    wait_for_gtk("GTK frames", || frames.get() >= count);
}

/// Final safety net after fixtures have cleaned up their own windows.
pub(super) fn destroy_windows() {
    for window in gtk::Window::list_toplevels() {
        if let Ok(window) = window.downcast::<gtk::Window>() {
            window.destroy();
        }
    }
}
