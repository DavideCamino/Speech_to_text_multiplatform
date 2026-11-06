//! Shared production constructor and weak callback ownership.
use super::support::wait_for_background;
use crate::{
    settings::{self, Settings},
    ui::{app::App, menu, model_controller::ModelAvailability, views::buffer_text},
};
use adw::prelude::*;
use std::{path::PathBuf, rc::Rc, sync::mpsc, time::Duration};

pub(super) fn shared_app_construction() {
    let (saved, snapshots) = mpsc::channel();
    let settings = Settings {
        language: Some("it".into()),
        model: Some(PathBuf::from("models/ggml-base.bin")),
        whisper_context: "Saved recognition context".into(),
        llm_system_prompt: "Saved rewrite prompt".into(),
        ..Settings::default()
    };
    let writer = settings::SaveWorker::with_save(move |settings| {
        saved.send(settings).unwrap();
        Ok(())
    });
    let app = App::new(None, settings, writer);
    assert!(app.window.application().is_none());
    assert!(!app.window.is_visible());
    assert!(app.window.content().is_some());
    assert_eq!(
        app.stack.child_by_name("main"),
        Some(app.main.root.clone().upcast())
    );
    assert_eq!(
        app.stack.child_by_name("rewrite"),
        Some(app.rewrite.root.clone().upcast())
    );
    assert_eq!(app.stack.visible_child_name().as_deref(), Some("main"));
    assert_eq!(
        app.preferences.language.selected_language().as_deref(),
        Some("it")
    );
    assert_eq!(app.preferences.model_label.text(), "ggml-base.bin");
    assert!(app.device_discovery.borrow().is_none());
    assert!(app.model_state.borrow().scan.is_none());
    assert!(!app.model_state.borrow().availability.is_checking());
    assert!(!app.model_state.borrow().operation.is_busy());
    assert!(!app.busy());
    assert!(matches!(
        snapshots.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(buffer_text(&app.main.context).is_empty());
    assert!(buffer_text(&app.rewrite.prompt).is_empty());
    app.restore_prompts();
    assert_eq!(buffer_text(&app.main.context), "Saved recognition context");
    assert_eq!(buffer_text(&app.rewrite.prompt), "Saved rewrite prompt");
    app.main.progress.set_fraction(0.42);
    assert_eq!(app.models.progress.fraction(), 0.42);
    app.persist();
    let saved = snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(saved.language.as_deref(), Some("it"));
    assert_eq!(saved.whisper_context, "Saved recognition context");
    wait_for_background("constructor settings save", || {
        app.settings_writer.borrow_mut().poll().is_some()
    });
    app.window.destroy();
    let weak = Rc::downgrade(&app);
    drop(app);
    assert!(
        weak.upgrade().is_none(),
        "construction must not retain the controller"
    );

    let application = adw::Application::builder()
        .application_id("io.github.davidecamino.Speech2Text.Tests")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application
        .register(None::<&gtk::gio::Cancellable>)
        .unwrap();
    let app = App::new(
        Some(&application),
        Settings::default(),
        settings::SaveWorker::with_save(|_| Ok(())),
    );
    assert_eq!(app.window.application(), Some(application.clone().upcast()));
    app.connect();
    app.refresh();
    let open = app.window.lookup_action("open").unwrap();
    assert!(!open.is_enabled());
    app.model_state.borrow_mut().availability = ModelAvailability::Available;
    app.refresh();
    assert!(open.is_enabled());
    for (action, _, _) in menu::SHORTCUTS {
        assert!(!application.accels_for_action(action).is_empty());
    }
    // Keep widgets alive after dropping App to catch strong callback cycles.
    let window = app.window.clone();
    let weak = Rc::downgrade(&app);
    drop(app);
    assert!(
        weak.upgrade().is_none(),
        "callbacks must not retain the controller"
    );
    window.destroy();
}
