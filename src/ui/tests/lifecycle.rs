//! Close confirmation, owned job cancellation, and ordered final saves.
use super::support::{test_app, wait_for_background};
use crate::{
    settings,
    transcription::{Completion, Event, Job},
    ui::{
        app::App,
        lifecycle::WindowLifecycle,
        rewrite::RewriteOperation,
        transcription_controller::{SessionSource, TranscriptionSession},
    },
};
use adw::prelude::*;
use std::{
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(super) fn closing_discards_entire_audio_batch() {
    let app = test_app();
    app.queue_audios(vec!["first.wav".into(), "second.wav".into()]);
    assert!(app.main.audio_list.row_at_index(1).is_some());
    app.audio_queue.borrow_mut().begin(0);
    let (tx, events) = mpsc::sync_channel(2);
    let cancel = Arc::new(AtomicBool::new(false));
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::File,
        job: Job {
            events,
            stop: Arc::new(AtomicBool::new(false)),
            cancel: cancel.clone(),
        },
    };
    tx.send(Event::Finished(Ok(Completion::Complete))).unwrap();
    app.begin_close();
    app.poll_transcription();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.session.borrow().is_running());
    assert!(app.audio_queue.borrow().files.is_empty());
    assert!(app.main.audio_list.first_child().is_none());
    assert!(app.audio_queue.borrow().batch.is_none());
    app.window.destroy();
}

fn close_dialog(app: &App) -> adw::MessageDialog {
    let mut dialogs = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .filter(|dialog| {
            dialog.is_visible()
                && dialog.transient_for().as_ref() == Some(app.window.upcast_ref())
                && dialog.heading().as_deref() == Some("Close Speech2Text?")
        });
    let dialog = dialogs.next().expect("close confirmation");
    assert!(dialogs.next().is_none(), "duplicate close confirmations");
    dialog
}

fn respond_to_close(dialog: &adw::MessageDialog, response: &str) {
    // Emitting the signal directly does not dismiss the dialog like a button
    // click does. Simulate that dismissal without emitting a Cancel response.
    dialog.response(response);
    dialog.destroy();
}

pub(super) fn close_confirmation_offers_autosave_for_new_worker_output() {
    use crate::{recovery, transcript::Segment};
    for cancel_updated_dialog in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(recovery::FILE_NAME);
        let mut app = test_app();
        Rc::get_mut(&mut app).unwrap().recovery_path = Some(path.clone());
        app.connect();
        let (tx, events) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        *app.session.borrow_mut() = TranscriptionSession::Running {
            source: SessionSource::Microphone,
            job: Job {
                events,
                stop: stop.clone(),
                cancel: cancel.clone(),
            },
        };
        app.window.present();
        app.window.close();
        let original = close_dialog(&app);
        assert!(!original.has_response("autosave"));
        assert!(app.close_confirmation.get());

        tx.send(Event::Final(vec![Segment {
            start_ms: 0,
            end_ms: 1000,
            text: "First words arrived".into(),
        }]))
        .unwrap();
        app.poll_transcription();
        assert!(app.has_unsaved_content());
        respond_to_close(&original, "close");
        app.window.close(); // The updated dialog must still guard close requests.
        let updated = close_dialog(&app);
        assert!(updated.has_response("autosave"));
        assert!(app.close_confirmation.get());
        assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
        assert!(app.session.borrow().is_running());
        assert!(!stop.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed));
        assert!(!path.exists());

        if cancel_updated_dialog {
            respond_to_close(&updated, "cancel");
            assert!(!app.close_confirmation.get());
            assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
            assert!(app.window.is_visible() && app.session.borrow().is_running());
            assert!(!stop.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed));
            app.discard_active_work();
            app.window.destroy();
        } else {
            respond_to_close(&updated, "autosave");
            assert!(!app.close_confirmation.get());
            assert!(cancel.load(Ordering::Relaxed));
            wait_for_background("autosave of new worker output", || {
                app.poll();
                app.lifecycle.get() == WindowLifecycle::ReadyToClose
            });
            let snapshot = recovery::load(&path).unwrap().unwrap();
            assert_eq!(snapshot.transcript.text, "First words arrived");
        }
    }
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
    assert!(close_dialog(&app).has_response("autosave"));
    respond_to_close(&close_dialog(&app), "cancel");
    assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
    assert!(!path.exists());
    app.window.close();
    respond_to_close(&close_dialog(&app), "autosave");
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
    respond_to_close(&close_dialog(&restored), "close");
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
    app.document.set_provisional("Must not be lost".into());
    app.document.render();
    let (_tx, events) = mpsc::sync_channel(1);
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::File,
        job: Job {
            events,
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        },
    };
    app.window.present();
    app.window.close();
    respond_to_close(&close_dialog(&app), "autosave");
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

pub(super) fn discard_after_preferences_failure_removes_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("recovery.json");
    let mut app = test_app();
    std::rc::Rc::get_mut(&mut app).unwrap().recovery_path = Some(path.clone());
    app.connect();
    app.main.transcript.buffer().set_text("Temporary draft");
    *app.settings_writer.borrow_mut() =
        settings::SaveWorker::with_save(|_| anyhow::bail!("preferences failure"));
    app.window.present();
    app.autosave_and_close();
    wait_for_background("preferences failure after autosave", || {
        app.poll_recovery();
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ConfirmingCloseAfterSaveFailure
    });
    assert!(path.exists());
    assert!(app.recovery_owned.get());
    preferences_failure_dialog(&app).response("cancel");
    assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
    assert!(app.window.is_sensitive() && app.main.transcript.is_editable());
    assert!(path.exists());
    *app.settings_writer.borrow_mut() = settings::SaveWorker::with_save(|_| Ok(()));
    app.close_without_saving();
    wait_for_background("discard recovery before closing", || {
        app.poll_recovery();
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ReadyToClose
    });
    assert!(!path.exists());
}

pub(super) fn empty_edits_close_without_autosave() {
    for (text, direct_autosave) in [("", false), (" \t\n", false), ("", true)] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("recovery.json");
        let mut app = test_app();
        std::rc::Rc::get_mut(&mut app).unwrap().recovery_path = Some(path.clone());
        app.connect();
        app.main
            .transcript
            .buffer()
            .set_text("Text that was erased");
        app.main.transcript.buffer().set_text(text);
        assert!(app.document.is_dirty());
        assert!(!app.has_unsaved_content());
        app.window.present();
        if direct_autosave {
            app.autosave_and_close();
        } else {
            app.window.close();
        }
        // No confirmation and no recovery write, just the normal final preferences save.
        assert_eq!(app.lifecycle.get(), WindowLifecycle::SavingBeforeClose);
        assert!(!app.close_confirmation.get());
        wait_for_background("empty content close", || {
            app.poll_preferences_save();
            app.lifecycle.get() == WindowLifecycle::ReadyToClose
        });
        assert!(!path.exists());
    }
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
    app.refresh_llm_models_with(|_, _| llm::Job {
        events,
        cancel: test_cancel.clone(),
    });
    app.window.present();
    app.window.close();
    let dialog = close_dialog(&app);
    assert!(!dialog.has_response("autosave"));
    assert!(dialog.has_response("cancel") && dialog.has_response("close"));
    respond_to_close(&dialog, "cancel");
    assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
    assert!(!cancel.load(Ordering::Relaxed) && !test_cancel.load(Ordering::Relaxed));
    app.window.close();
    respond_to_close(&close_dialog(&app), "close");
    assert!(cancel.load(Ordering::Relaxed) && test_cancel.load(Ordering::Relaxed));
    assert!(!app.session.borrow().is_running());
    assert!(matches!(
        app.rewrite_state.borrow().operation,
        RewriteOperation::Idle
    ));
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

fn preferences_failure_dialog(app: &App) -> adw::MessageDialog {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .find(|dialog| {
            dialog.is_visible()
                && dialog.transient_for().as_ref() == Some(app.window.upcast_ref())
                && dialog.heading().as_deref() == Some("Could not save preferences")
        })
        .expect("save failure is surfaced")
}

pub(super) fn failed_save_retry() {
    let app = test_app();
    app.connect();
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    *app.settings_writer.borrow_mut() = settings::SaveWorker::with_save(move |_| {
        if count.fetch_add(1, Ordering::Relaxed) == 0 {
            anyhow::bail!("simulated disk failure");
        }
        Ok(())
    });
    app.window.present();
    app.window.close();
    wait_for_background("final preferences failure", || {
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ConfirmingCloseAfterSaveFailure
    });
    let dialog = preferences_failure_dialog(&app);
    assert!(dialog.body().contains("simulated disk failure"));
    assert!(app.window.is_visible() && dialog.is_sensitive());
    dialog.response("retry");
    wait_for_background("final preferences retry", || {
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ReadyToClose
    });
    assert!(!app.window.is_visible());
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
}

pub(super) fn closing_with_defaults_preserves_malformed_preferences_backup() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(settings::FILE_NAME);
    let original = b"{\"llm_system_prompt\": \"Old instructions\"";
    std::fs::write(&path, original).unwrap();
    let (defaults, writer, warning) = settings::StartupSettings::load_in(root.path()).into_parts();
    let backup = std::fs::read_dir(root.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join(settings::FILE_NAME);
    assert!(warning.unwrap().contains(&backup.display().to_string()));
    assert!(defaults.llm_system_prompt.is_empty());
    assert!(!path.exists());
    let app = App::new(None, defaults, writer);
    app.restore_prompts();
    app.connect();
    app.window.present();
    app.window.close();
    wait_for_background("closing with fresh preferences", || {
        app.poll_preferences_save();
        app.lifecycle.get() == WindowLifecycle::ReadyToClose
    });
    assert!(!app.window.is_visible());
    let fresh: settings::Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(fresh.llm_system_prompt.is_empty());
    assert_eq!(std::fs::read(&backup).unwrap(), original);
}

pub(super) fn exit_without_updating_keeps_saved_recovery() {
    use crate::recovery;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(recovery::FILE_NAME);
    let mut app = test_app();
    Rc::get_mut(&mut app).unwrap().recovery_path = Some(path.clone());
    app.connect();
    let (attempted, attempts) = mpsc::channel();
    *app.settings_writer.borrow_mut() = settings::SaveWorker::with_save(move |_| {
        attempted.send(()).unwrap();
        anyhow::bail!("persistent preferences failure");
    });
    app.window.present();

    // A regular save failure must not offer to exit or lock the editor.
    app.persist();
    wait_for_background("ordinary preferences failure", || {
        app.poll_preferences_save();
        app.main
            .status
            .text()
            .contains("persistent preferences failure")
    });
    assert_eq!(app.lifecycle.get(), WindowLifecycle::Open);
    assert!(app.window.is_sensitive());
    attempts.try_recv().unwrap();

    app.main
        .transcript
        .buffer()
        .set_text("Saved before preferences failed");
    app.window.close();
    respond_to_close(&close_dialog(&app), "autosave");
    wait_for_background("final preferences failure after recovery save", || {
        app.poll();
        app.lifecycle.get() == WindowLifecycle::ConfirmingCloseAfterSaveFailure
    });
    attempts.try_recv().unwrap();
    let before = std::fs::read(&path).unwrap();
    preferences_failure_dialog(&app).response("exit");
    assert_eq!(app.lifecycle.get(), WindowLifecycle::ReadyToClose);
    assert!(!app.window.is_visible());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Drop the sender held by the worker: if a save was queued on exit, it
    // must be observed before the worker disconnects.
    drop(app);
    assert!(matches!(
        attempts.recv_timeout(Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}
