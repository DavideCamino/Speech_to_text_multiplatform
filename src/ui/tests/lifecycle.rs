//! Close confirmation, owned job cancellation, and ordered final saves.
use super::support::{test_app, wait_for_background};
use crate::{
    settings,
    transcription::Job,
    ui::{
        lifecycle::WindowLifecycle,
        rewrite::RewriteOperation,
        transcription_controller::{SessionSource, TranscriptionSession},
    },
};
use adw::prelude::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(super) fn lifecycle_job_ownership() {
    use crate::llm;
    let app = test_app();
    app.connect();
    let (_tx, events) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::File,
        job: Job {
            events,
            stop: Arc::new(AtomicBool::new(false)),
            cancel: cancel.clone(),
        },
    };
    let (_tx, events) = mpsc::sync_channel(1);
    let test_cancel = Arc::new(AtomicBool::new(false));
    app.rewrite_state.borrow_mut().operation = RewriteOperation::TestingConnection(llm::Job {
        events,
        cancel: test_cancel.clone(),
    });
    app.window.present();
    app.window.close();
    let confirmation = || {
        gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
            .find(|dialog| dialog.heading().as_deref() == Some("Close Speech2Text?"))
            .expect("active work needs confirmation")
    };
    let dialog = confirmation();
    dialog.response("cancel");
    dialog.close();
    assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
    assert!(!cancel.load(Ordering::Relaxed) && !test_cancel.load(Ordering::Relaxed));
    app.window.close();
    let dialog = confirmation();
    dialog.response("close");
    dialog.close();
    assert!(cancel.load(Ordering::Relaxed) && test_cancel.load(Ordering::Relaxed));
    assert!(!app.session.borrow().is_running());
    assert!(!app.rewrite_state.borrow().operation.is_testing());
    assert_eq!(app.lifecycle.get(), WindowLifecycle::SavingBeforeClose);
    wait_for_background("window lifecycle transition", || {
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ReadyToClose
    });
}

pub(super) fn final_save_ordering() {
    let app = test_app();
    app.connect();
    // Closing queues its final snapshot, keeps GTK responsive, and waits for
    // completion instead of allowing process exit while the save is in flight.
    let (entered, started) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let ui_thread = std::thread::current().id();
    *app.settings_writer.borrow_mut() = settings::SaveWorker::with_save(move |settings| {
        assert_ne!(std::thread::current().id(), ui_thread);
        entered.send(settings.clone()).unwrap();
        resume.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    });
    // An older ordinary save must not release a window waiting for its final save.
    app.persist();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    app.poll_preferences_save();
    assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
    app.main
        .context
        .buffer()
        .set_text(" Context saved on close 世界\n ");
    // Clearing an earlier saved prompt must overwrite it with an empty value.
    app.settings.borrow_mut().llm_system_prompt = "Earlier prompt".into();
    app.rewrite.prompt.buffer().set_text("");
    app.window.present();
    app.window.close();
    assert_eq!(app.lifecycle.get(), WindowLifecycle::SavingBeforeClose);
    app.window.close(); // A second request must not enqueue another final save.
    release.send(()).unwrap(); // Finish the older save; the final save stays blocked.
    let saved = started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(saved.whisper_context, " Context saved on close 世界\n ");
    assert!(saved.llm_system_prompt.is_empty());
    app.poll_preferences_save();
    assert_eq!(app.lifecycle.get(), WindowLifecycle::SavingBeforeClose);
    assert!(!app.window.is_sensitive());
    release.send(()).unwrap();
    wait_for_background("window lifecycle transition", || {
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ReadyToClose
    });

    assert!(matches!(started.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

pub(super) fn failed_save_retry() {
    // A failed final save leaves the window usable and surfaces the error.
    let app = test_app();
    app.connect();
    *app.settings_writer.borrow_mut() =
        settings::SaveWorker::with_save(|_| anyhow::bail!("simulated disk failure"));
    app.window.present();
    app.window.close();
    wait_for_background("window lifecycle transition", || {
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::Open
    });
    assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
    assert!(app.window.is_sensitive());
    assert!(app.main.status.text().contains("simulated disk failure"));
    assert!(app.main.transcript.is_editable());
    let dialog = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .find(|dialog| dialog.heading().as_deref() == Some("Could not save preferences"))
        .expect("save failure is surfaced");
    dialog.close();
    *app.settings_writer.borrow_mut() = settings::SaveWorker::with_save(|_| Ok(()));
    app.window.close();
    wait_for_background("window lifecycle transition", || {
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ReadyToClose
    });
}
