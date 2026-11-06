//! Model library presentation, selection, and stale availability checks.
use super::support::{test_app, wait_for_background};
use crate::{
    model,
    settings::{self, Settings},
    ui::{model_controller::ModelAvailability, models, preferences, task::background},
};
use adw::prelude::*;
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::mpsc, time::Duration};

pub(super) fn model_library_window(parent: &adw::ApplicationWindow) {
    let selected = PathBuf::from("models").join("ggml-tiny.bin");
    let preferences = preferences::Preferences::new(
        parent,
        &Settings {
            model: Some(selected.clone()),
            ..Settings::default()
        },
    );
    assert_eq!(preferences.model_label.text(), "ggml-tiny.bin");
    assert_eq!(
        preferences.select_model.tooltip_text().as_deref(),
        Some(selected.to_string_lossy().as_ref())
    );
    preferences.set_model(None);
    assert_eq!(preferences.model_label.text(), "No model selected");
    assert!(preferences.select_model.tooltip_text().is_none());
    preferences.set_model(Some(&selected));
    assert_eq!(preferences.model_label.text(), "ggml-tiny.bin");
    assert_eq!(
        preferences.select_model.tooltip_text().as_deref(),
        Some(selected.to_string_lossy().as_ref())
    );
    assert_eq!(
        preferences.select_model.child(),
        Some(preferences.model_label.clone().upcast())
    );
    let models = models::Models::new(&preferences.window);
    assert_eq!(models.download.label().as_deref(), Some("Download…"));
    assert_eq!(
        models.browse.label().as_deref(),
        Some("Select Local Model…")
    );
    let other = PathBuf::from("models").join("ggml-small.bin");
    let app = test_app();
    let (saved, snapshots) = mpsc::channel();
    *app.settings_writer.borrow_mut() = settings::SaveWorker::with_save(move |settings| {
        saved.send(settings.model).unwrap();
        Ok(())
    });
    let directory = tempfile::tempdir().unwrap();
    let accepted = directory.path().join("ggml-small.bin");
    std::fs::write(&accepted, b"test model").unwrap();
    // The shared acceptance path updates presentation and queues one save.
    app.accept_model(accepted.clone());
    assert_eq!(app.settings.borrow().model.as_ref(), Some(&accepted));
    assert_eq!(app.preferences.model_label.text(), "ggml-small.bin");
    assert_eq!(
        app.preferences.select_model.tooltip_text().as_deref(),
        Some(accepted.to_string_lossy().as_ref())
    );
    assert_eq!(
        snapshots.recv_timeout(Duration::from_secs(5)).unwrap(),
        Some(accepted)
    );
    wait_for_background("accepted model save/check", || {
        app.settings_writer.borrow_mut().poll().is_some()
    });
    assert!(matches!(
        snapshots.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(app.model_state.borrow().availability.is_checking());
    wait_for_background("accepted model save/check", || {
        app.poll_model_check();
        !app.model_state.borrow().availability.is_checking()
    });
    assert!(app.model_state.borrow().availability.is_available());
    app.window.destroy();

    let chosen = Rc::new(RefCell::new(None));
    let observed = chosen.clone();
    models.set_entries(
        vec![selected.clone(), other.clone()],
        Some(&selected),
        move |path| {
            *observed.borrow_mut() = Some(path);
        },
    );
    let first = models
        .list
        .row_at_index(0)
        .unwrap()
        .child()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    assert!(first.has_css_class("suggested-action"));
    let second = models
        .list
        .row_at_index(1)
        .unwrap()
        .child()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    second.emit_clicked();
    assert_eq!(*chosen.borrow(), Some(other));
    assert!(models.list.row_at_index(2).is_none());
    models.set_entries(vec![], None, |_| {});
    assert!(models.list.row_at_index(0).is_none());
    models.window.destroy();
    preferences.window.destroy();
}

pub(super) fn replacement_confirmation() {
    let app = test_app();
    app.connect();
    // Asking to replace a model must not start work until explicit approval.
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join(model::CATALOG[0].file);
    std::fs::write(&target, b"old model").unwrap();
    app.ask_replace_download(0, target.clone());
    assert!(!app.model_state.borrow().operation.is_busy());
    let dialog = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .find(|dialog| dialog.heading().as_deref() == Some("Replace existing model?"))
        .expect("replacement confirmation");
    assert_eq!(dialog.default_response().as_deref(), Some("cancel"));
    dialog.response("cancel");
    assert!(!app.model_state.borrow().operation.is_busy());
    assert_eq!(std::fs::read(&target).unwrap(), b"old model");
    dialog.close();
    app.window.destroy();
}

pub(super) fn replaced_scan_discards_stale_results() {
    let app = test_app();
    let (old_worker, old_scan) = mpsc::channel();
    app.model_state.borrow_mut().scan = Some(old_scan);
    old_worker.send(Ok(vec![PathBuf::from("old.bin")])).unwrap();

    // Replacing the receiver must discard even an already queued old result.
    let (new_worker, new_scan) = mpsc::channel();
    app.model_state.borrow_mut().scan = Some(new_scan);
    app.refresh();
    app.poll_model_scan();
    assert!(app.model_state.borrow().scan.is_some());
    assert!(app.models.list.row_at_index(0).is_none());
    assert!(!app.models.list.is_sensitive());

    new_worker.send(Ok(vec![PathBuf::from("new.bin")])).unwrap();
    app.poll_model_scan();
    assert!(app.model_state.borrow().scan.is_none());
    assert!(app.models.list.is_sensitive());
    assert!(app.models.list.row_at_index(1).is_none());
    let button = app
        .models
        .list
        .row_at_index(0)
        .unwrap()
        .child()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    assert_eq!(button.tooltip_text().as_deref(), Some("new.bin"));
    assert!(
        old_worker
            .send(Ok(vec![PathBuf::from("late.bin")]))
            .is_err()
    );
    app.poll_model_scan();
    assert_eq!(
        app.models.list.row_at_index(0).unwrap().child(),
        Some(button.upcast())
    );
    assert!(app.models.list.row_at_index(1).is_none());
    app.window.destroy();
}

pub(super) fn model_availability_states() {
    let app = test_app();
    app.connect();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model.bin");
    std::fs::write(&path, b"test model").unwrap();
    app.settings.borrow_mut().model = Some(path.clone());
    app.update_model_label();
    assert!(!app.main.open.is_sensitive());
    wait_for_background("model availability check", || {
        app.poll_model_check();
        !app.model_state.borrow().availability.is_checking()
    });
    assert!(app.main.open.is_sensitive());

    // Idle refreshes must use cached state, not stat the model again.
    std::fs::remove_file(&path).unwrap();
    for _ in 0..100 {
        app.refresh();
    }
    assert!(app.main.open.is_sensitive());
    assert!(!app.model_state.borrow().availability.is_checking());
    app.check_model();
    wait_for_background("model availability check", || {
        app.poll_model_check();
        !app.model_state.borrow().availability.is_checking()
    });
    assert!(!app.main.open.is_sensitive());

    // A stalled old check must not block polling or replace a newer result.
    let (release, resume) = mpsc::sync_channel(1);
    app.model_state.borrow_mut().availability =
        ModelAvailability::Checking(background(move || {
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
            false
        }));
    for _ in 0..100 {
        app.poll_model_check();
        app.refresh();
    }
    std::fs::write(&path, b"test model").unwrap();
    app.update_model_label();
    wait_for_background("model availability check", || {
        app.poll_model_check();
        !app.model_state.borrow().availability.is_checking()
    });
    release.send(()).unwrap();
    app.poll_model_check();
    assert!(app.model_state.borrow().availability.is_available());
    assert!(app.main.open.is_sensitive());
    // An idle rewrite poll should not reset widget sensitivity at all.
    app.main.open.set_sensitive(false);
    app.poll_rewrite();
    assert!(!app.main.open.is_sensitive());
    app.refresh();
    assert!(app.main.open.is_sensitive());

    app.window.destroy();
}
