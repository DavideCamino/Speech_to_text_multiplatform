//! Microphone discovery and retryable device preferences.
use super::support::test_app;
use crate::{
    settings::Settings,
    ui::{preferences, task::background},
};
use adw::prelude::*;
use std::{sync::mpsc, time::Duration};

pub(super) fn device_discovery_states(parent: &adw::ApplicationWindow) {
    let saved = "USB ☕ microphone";
    let preferences = preferences::Preferences::new(
        parent,
        &Settings {
            device: Some(saved.into()),
            ..Settings::default()
        },
    );
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    preferences.begin_device_refresh();
    preferences.refresh_device_controls(false);
    assert!(preferences.devices_loading());
    assert!(!preferences.device.is_sensitive());
    assert!(!preferences.device_refresh.is_sensitive());
    assert_eq!(
        preferences.device_row.subtitle().as_deref(),
        Some("Loading microphones…")
    );
    // Persisting/closing while discovery is in flight must retain the name.
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    preferences.finish_device_refresh(Ok(vec!["Other microphone".into()]));
    assert!(!preferences.devices_loading());
    preferences.refresh_device_controls(false);
    assert!(preferences.device.is_sensitive());
    assert!(preferences.device_refresh.is_sensitive());
    preferences.refresh_device_controls(true);
    assert!(!preferences.device.is_sensitive());
    assert!(!preferences.device_refresh.is_sensitive());
    preferences.refresh_device_controls(false);
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    assert!(
        preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("unavailable")
    );
    let selected = preferences
        .device
        .selected_item()
        .unwrap()
        .downcast::<gtk::StringObject>()
        .unwrap();
    assert_eq!(selected.string(), format!("{saved} (unavailable)"));

    let count = preferences.device.model().unwrap().n_items();
    preferences.begin_device_refresh();
    preferences.finish_device_refresh(Err("Permission denied".into()));
    preferences.refresh_device_controls(false);
    assert!(
        preferences.device_refresh.is_sensitive(),
        "failed discovery must be retryable"
    );
    assert_eq!(preferences.device.model().unwrap().n_items(), count);
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    assert!(
        preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("Permission denied")
    );

    // Reconnecting removes the unavailable label and clears the old error;
    // duplicate native names must not create indistinguishable choices.
    preferences.begin_device_refresh();
    preferences.finish_device_refresh(Ok(vec![
        "Other microphone".into(),
        saved.into(),
        saved.into(),
    ]));
    assert_eq!(preferences.device.model().unwrap().n_items(), 3);
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    let selected = preferences
        .device
        .selected_item()
        .unwrap()
        .downcast::<gtk::StringObject>()
        .unwrap();
    assert_eq!(selected.string(), saved);
    assert!(
        !preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("Permission denied")
    );

    // Preserve the user's current choice, not the original startup setting.
    preferences.device.set_selected(1);
    preferences.begin_device_refresh();
    preferences.finish_device_refresh(Ok(vec![]));
    assert_eq!(
        preferences.selected_device().as_deref(),
        Some("Other microphone")
    );
    preferences.device.set_selected(0);
    preferences.update_device_status();
    assert!(
        preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("No microphones found")
    );
    preferences.begin_device_refresh();
    preferences.finish_device_refresh(Ok(vec![]));
    assert_eq!(preferences.selected_device(), None);
    assert_eq!(preferences.device.model().unwrap().n_items(), 1);
    preferences.window.destroy();
}

pub(super) fn discovery_disconnect_and_retry() {
    let app = test_app();
    app.preferences
        .finish_device_refresh(Ok(vec!["USB microphone".into()]));
    app.preferences.device.set_selected(1);
    let (worker, events) = mpsc::channel();
    app.preferences.begin_device_refresh();
    *app.device_discovery.borrow_mut() = Some(events);
    app.refresh();
    assert!(!app.preferences.device_refresh.is_sensitive());
    drop(worker);
    app.poll_devices();
    assert!(app.device_discovery.borrow().is_none());
    assert!(!app.preferences.devices_loading());
    assert!(app.preferences.device_refresh.is_sensitive());
    assert_eq!(
        app.preferences.selected_device().as_deref(),
        Some("USB microphone")
    );
    assert!(
        app.preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("stopped unexpectedly")
    );

    // Exercise the same completion path on retry without native device access.
    let (worker, events) = mpsc::channel();
    app.preferences.begin_device_refresh();
    *app.device_discovery.borrow_mut() = Some(events);
    app.refresh();
    assert!(!app.preferences.device_refresh.is_sensitive());
    worker
        .send(Ok(vec!["USB microphone".into(), "Other microphone".into()]))
        .unwrap();
    app.poll_devices();
    assert!(app.device_discovery.borrow().is_none());
    assert!(!app.preferences.devices_loading());
    assert!(app.preferences.device.is_sensitive() && app.preferences.device_refresh.is_sensitive());
    assert_eq!(
        app.preferences.selected_device().as_deref(),
        Some("USB microphone")
    );
    assert!(
        !app.preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("stopped unexpectedly")
    );
    app.window.destroy();
}

#[test]
fn microphone_discovery_runs_on_a_background_thread_and_reports_errors() {
    let ui_thread = std::thread::current().id();
    let events = background(move || {
        assert_ne!(std::thread::current().id(), ui_thread);
        Ok::<Vec<String>, String>(vec!["USB microphone".into()])
    });
    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        vec!["USB microphone"]
    );
    let events = background(|| Err::<Vec<String>, String>("Permission denied".into()));
    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err(),
        "Permission denied"
    );
}
