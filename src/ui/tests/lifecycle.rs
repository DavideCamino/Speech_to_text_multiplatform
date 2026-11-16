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

fn close_dialog() -> adw::MessageDialog {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .find(|dialog| dialog.heading().as_deref() == Some("Close Speech2Text?"))
        .expect("close confirmation")
}

pub(super) fn autosave_and_recovery() {
    use crate::{recovery, transcript::Segment, ui::views::buffer_text};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(recovery::FILE_NAME);
    let mut app = test_app();
    std::rc::Rc::get_mut(&mut app).unwrap().recovery_path = Some(path.clone());
    app.connect();
    app.document.accept_final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: " Original words".into(),
    }]);
    app.document.render();
    app.main
        .transcript
        .buffer()
        .set_text("Edited transcript 世界");
    app.main.context.buffer().set_text("Recognition vocabulary");
    let rewrite = recovery::RewriteSnapshot {
        source: "Original rewrite source".into(),
        prompt: "Rewrite instructions".into(),
        markdown: "# Partial result".into(),
        dirty: true,
    };
    app.rewrite_state.borrow_mut().restore(&rewrite);
    app.rewrite.source.buffer().set_text(&rewrite.source);
    app.rewrite.prompt.buffer().set_text(&rewrite.prompt);
    app.stack.set_visible_child_name("rewrite");
    app.window.present();
    app.window.close();
    assert!(close_dialog().has_response("autosave"));
    close_dialog().response("cancel");
    assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
    assert!(!path.exists());
    app.window.close();
    close_dialog().response("autosave");
    assert_eq!(app.lifecycle.get(), WindowLifecycle::SavingRecovery);
    assert!(!app.window.is_sensitive());
    app.window.close(); // repeated close cannot interrupt the write
    wait_for_background("autosave and final preferences", || {
        app.poll_recovery();
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ReadyToClose
    });
    let saved = recovery::load(&path).unwrap().unwrap();
    assert_eq!(saved.transcript.text, "Edited transcript 世界");
    assert!(saved.transcript.dirty && saved.transcript.text_edited);
    assert_eq!(saved.rewrite.markdown, "# Partial result");
    assert!(path.exists());

    let mut restored = test_app();
    std::rc::Rc::get_mut(&mut restored).unwrap().recovery_path = Some(path.clone());
    restored.restore_recovery(&saved);
    restored.connect();
    assert!(path.exists()); // restoration itself must not delete synchronously
    assert!(restored.document.is_dirty() && restored.rewrite_state.borrow().is_dirty());
    assert_eq!(
        buffer_text(&restored.main.transcript),
        saved.transcript.text
    );
    assert_eq!(restored.document.srt(), saved.transcript.original.srt());
    assert_eq!(buffer_text(&restored.rewrite.source), rewrite.source);
    assert_eq!(buffer_text(&restored.rewrite.prompt), rewrite.prompt);
    assert_eq!(restored.rewrite_markdown(), rewrite.markdown);
    assert_eq!(
        buffer_text(&restored.main.context),
        "Recognition vocabulary"
    );
    assert_eq!(
        restored.stack.visible_child_name().as_deref(),
        Some("rewrite")
    );
    assert!(!restored.session.borrow().is_running());
    assert!(!restored.rewrite_state.borrow().operation.is_generating());
    restored.consume_recovery();
    wait_for_background("recovery deletion", || {
        restored.poll_recovery();
        restored.lifecycle.get() == WindowLifecycle::Open
    });
    assert!(!path.exists());
    restored.document.prepare_append();
    restored.document.accept_final(vec![Segment {
        start_ms: 1000,
        end_ms: 2000,
        text: " appended".into(),
    }]);
    restored.document.render();
    assert_eq!(
        buffer_text(&restored.main.transcript),
        "Edited transcript 世界 appended"
    );
    // Explicit discard must not create a recovery file.
    restored.window.present();
    restored.window.close();
    close_dialog().response("close");
    wait_for_background("discard close", || {
        restored.poll_preferences_save();
        restored.lifecycle.get() == WindowLifecycle::ReadyToClose
    });
    assert!(!path.exists());
}

pub(super) fn failed_autosave_keeps_content() {
    use crate::ui::views::buffer_text;
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("not-a-directory");
    std::fs::write(&blocked, "blocking file").unwrap();
    let mut app = test_app();
    std::rc::Rc::get_mut(&mut app).unwrap().recovery_path = Some(blocked.join("recovery.json"));
    app.connect();
    app.main.transcript.buffer().set_text("Must not be lost");
    app.window.present();
    app.window.close();
    close_dialog().response("autosave");
    wait_for_background("autosave failure", || {
        app.poll_recovery();
        app.lifecycle.get() == WindowLifecycle::Open
    });
    assert_eq!(buffer_text(&app.main.transcript), "Must not be lost");
    assert!(app.document.is_dirty());
    assert!(app.window.is_visible() && app.window.is_sensitive());
    let failure = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .find(|dialog| dialog.heading().as_deref() == Some("Could not autosave"))
        .unwrap();
    failure.response("close");
    wait_for_background("close after failed autosave", || {
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ReadyToClose
    });
}

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
