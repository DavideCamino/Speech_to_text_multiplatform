//! Queue interactions and sequential jobs use deterministic worker channels.
use super::support::test_app;
use crate::{
    transcript::Segment,
    transcription::{Event, Job, Source},
    ui::{
        app::App,
        model_controller::ModelAvailability,
        transcription_controller::{SessionSource, TranscriptionSession},
        views::buffer_text,
    },
};
use adw::prelude::*;
use std::{
    rc::Rc,
    sync::{Arc, atomic::AtomicBool, mpsc},
};

fn fake_job(app: &App) -> mpsc::SyncSender<Event> {
    app.session.borrow_mut().finish();
    let (tx, events) = mpsc::sync_channel(16);
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::File,
        job: Job {
            events,
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        },
    };
    tx
}

fn ready() -> Rc<App> {
    let app = test_app();
    app.connect();
    app.model_state.borrow_mut().availability = ModelAvailability::Available;
    app.refresh();
    app
}

fn drop_target(app: &App, index: i32) -> gtk::DropTarget {
    let row = app.main.audio_list.row_at_index(index).unwrap();
    let controllers = row.observe_controllers();
    (0..controllers.n_items())
        .find_map(|i| controllers.item(i)?.downcast::<gtk::DropTarget>().ok())
        .unwrap()
}

fn drop_file(target: &gtk::DropTarget, revision: u64, from: usize) -> bool {
    let value = gtk::glib::BoxedValue(format!("speech2text-audio:{revision}:{from}").to_value());
    target.emit_by_name::<bool>("drop", &[&value, &0.0_f64, &0.0_f64])
}

fn buttons(app: &App, index: i32) -> (gtk::Button, gtk::Button, gtk::Button) {
    let row = app.main.audio_list.row_at_index(index).unwrap();
    let remove = row.child().unwrap().last_child().unwrap();
    let down = remove.prev_sibling().unwrap();
    let up = down.prev_sibling().unwrap();
    (
        up.downcast().unwrap(),
        down.downcast().unwrap(),
        remove.downcast().unwrap(),
    )
}

pub(super) fn queue_interactions() {
    let app = ready();
    app.queue_audios(vec!["a.wav".into(), "b.wav".into(), "c.wav".into()]);
    assert!(app.main.audio_expander.property::<bool>("visible"));
    assert!(app.main.selected_audio.text().contains("3 audio files"));
    app.main.audio_expander.set_expanded(true);
    let (up, down, _) = buttons(&app, 0);
    assert!(!up.is_sensitive() && down.is_sensitive());
    down.emit_clicked();
    assert_eq!(
        app.audio_queue.borrow().selected().unwrap().to_str(),
        Some("b.wav")
    );
    let revision = app.audio_queue.borrow().revision;
    assert!(drop_file(&drop_target(&app, 2), revision, 0));
    assert_eq!(
        app.audio_queue.borrow().selected().unwrap().to_str(),
        Some("a.wav")
    );
    assert!(
        !drop_file(&drop_target(&app, 1), revision, 0),
        "stale drag rejected"
    );
    buttons(&app, 1).2.emit_clicked(); // remove c.wav
    assert_eq!(app.audio_queue.borrow().files.len(), 2);
    buttons(&app, 1).0.emit_clicked(); // move b.wav above a.wav
    assert_eq!(
        app.audio_queue.borrow().selected().unwrap().to_str(),
        Some("b.wav")
    );

    // A confirmation for the old ordering must not start a changed queue.
    app.main.transcript.buffer().set_text("Existing text");
    app.start_selected_source();
    let dialog = confirmation();
    buttons(&app, 0).1.emit_clicked();
    dialog.response("append");
    dialog.close();
    assert!(!app.session.borrow().is_running());

    app.clear_transcript();
    app.start_selected_source();
    assert!(!app.main.audio_list.is_sensitive());
    assert!(!app.move_queued_audio(0, 1));
    let revision = app.audio_queue.borrow().revision;
    assert!(!drop_file(&drop_target(&app, 1), revision, 0));
    buttons(&app, 0).2.emit_clicked(); // signal emission cannot bypass busy guard
    assert_eq!(app.audio_queue.borrow().files.len(), 2);
    app.request_cancel();
    app.session.borrow_mut().finish();
    assert!(app.audio_queue.borrow().files.is_empty());
    assert_eq!(app.main.record.label().as_deref(), Some("Record"));
    app.window.destroy();
}

fn confirmation() -> adw::MessageDialog {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<adw::MessageDialog>().ok())
        .find(|d| d.heading().as_deref() == Some("Start a new transcription?"))
        .unwrap()
}

pub(super) fn sequential_batch() {
    let app = ready();
    app.main.transcript.buffer().set_text("Existing text.");
    app.queue_audios(vec![
        "one.wav".into(),
        "empty.wav".into(),
        "three.wav".into(),
    ]);
    for file in &mut app.audio_queue.borrow_mut().files {
        file.duration = Some(60.0);
    }
    app.start_selected_source();
    let dialog = confirmation();
    dialog.response("append");
    dialog.close();
    assert!(app.session.borrow().is_running());
    assert!(app.main.file_progress.text().starts_with("File 1 of 3"));
    assert!(
        app.main
            .file_progress
            .text()
            .contains("Calculating time remaining")
    );
    let tx = fake_job(&app);
    tx.send(Event::Final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: " One.".into(),
    }]))
    .unwrap();
    tx.send(Event::Progress(0.5)).unwrap();
    app.poll_transcription();
    assert_eq!(app.main.progress.fraction(), 0.5);
    assert!(app.main.file_progress.text().contains("remaining"));
    assert!(!app.main.file_progress.text().contains("Calculating"));
    tx.send(Event::Finished(Ok(()))).unwrap();
    app.poll_transcription();
    assert_eq!(app.audio_queue.borrow().batch.as_ref().unwrap().index, 1);
    assert!(app.main.file_progress.text().starts_with("File 2 of 3"));
    assert_eq!(app.main.progress.fraction(), 0.0);
    assert_eq!(buffer_text(&app.main.transcript), "Existing text. One.");
    assert!(
        !gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<adw::MessageDialog>().ok())
            .any(
                |d| d.heading().as_deref() == Some("Start a new transcription?") && d.is_visible()
            )
    );
    let tx = fake_job(&app);
    tx.send(Event::Finished(Ok(()))).unwrap(); // empty file adds no separator
    app.poll_transcription();
    assert_eq!(buffer_text(&app.main.transcript), "Existing text. One.");
    assert!(app.main.file_progress.text().starts_with("File 3 of 3"));
    let tx = fake_job(&app);
    tx.send(Event::Provisional(" Three?".into())).unwrap();
    app.poll_transcription();
    assert_eq!(
        buffer_text(&app.main.transcript),
        "Existing text. One.\n\nThree?"
    );
    tx.send(Event::Final(vec![Segment {
        start_ms: 1000,
        end_ms: 2000,
        text: " Three.".into(),
    }]))
    .unwrap();
    tx.send(Event::Finished(Ok(()))).unwrap();
    app.poll_transcription();
    assert_eq!(
        buffer_text(&app.main.transcript),
        "Existing text. One.\n\nThree."
    );
    assert!(
        app.document
            .srt()
            .contains("00:00:01,000 --> 00:00:02,000\nThree.")
    );
    assert!(!app.session.borrow().is_running());
    assert!(app.audio_queue.borrow().files.is_empty());
    assert!(!app.main.file_progress.property::<bool>("visible"));
    assert_eq!(app.main.record.label().as_deref(), Some("Record"));
    app.window.destroy();
}

pub(super) fn batch_failures_and_cancel() {
    let app = ready();
    let root = tempfile::tempdir().unwrap();
    let paths: Vec<_> = ["one.wav", "two.wav", "three.wav"]
        .iter()
        .map(|name| {
            let path = root.path().join(name);
            std::fs::write(&path, "fixture").unwrap();
            path
        })
        .collect();
    app.queue_audios(paths.clone());
    app.start(Source::File(paths[0].clone()), false);
    let tx = fake_job(&app);
    tx.send(Event::Final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "Kept.".into(),
    }]))
    .unwrap();
    tx.send(Event::Finished(Err("decode failed".into())))
        .unwrap();
    app.poll_transcription();
    assert_eq!(app.audio_queue.borrow().batch.as_ref().unwrap().index, 1);
    assert_eq!(
        app.audio_queue
            .borrow()
            .batch
            .as_ref()
            .unwrap()
            .failures
            .len(),
        1
    );
    let tx = fake_job(&app);
    tx.send(Event::Finished(Ok(()))).unwrap();
    app.poll_transcription();
    let tx = fake_job(&app);
    tx.send(Event::Final(vec![Segment {
        start_ms: 1000,
        end_ms: 2000,
        text: "Next.".into(),
    }]))
    .unwrap();
    tx.send(Event::Finished(Ok(()))).unwrap();
    app.poll_transcription();
    assert_eq!(buffer_text(&app.main.transcript), "Kept.\n\nNext.");
    assert!(app.main.status.text().contains("errors"));
    let error = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<adw::MessageDialog>().ok())
        .find(|d| d.heading().as_deref() == Some("Some audio files failed"))
        .unwrap();
    assert!(error.body().contains("one.wav: decode failed"));
    error.close();

    app.model_state.borrow_mut().availability = ModelAvailability::Available;
    app.queue_audios(paths.clone());
    app.start(Source::File(paths[0].clone()), false);
    let tx = fake_job(&app);
    tx.send(Event::Final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "Finalized.".into(),
    }]))
    .unwrap();
    tx.send(Event::Provisional(" unfinished".into())).unwrap();
    app.poll_transcription();
    app.request_cancel();
    tx.send(Event::Finished(Ok(()))).unwrap();
    app.poll_transcription();
    assert_eq!(buffer_text(&app.main.transcript), "Finalized.");
    assert!(!app.session.borrow().is_running());
    assert!(app.audio_queue.borrow().files.is_empty());
    assert!(paths.iter().all(|path| path.exists()));
    app.window.destroy();
}

pub(super) fn single_file_time_remaining() {
    let app = ready();
    app.queue_audio("single.wav".into());
    app.start_selected_source();
    app.audio_queue.borrow_mut().files[0].duration = Some(60.0);
    let tx = fake_job(&app);
    tx.send(Event::Progress(0.5)).unwrap();
    app.poll_transcription();
    assert!(app.main.file_progress.property::<bool>("visible"));
    assert!(app.main.file_progress.text().starts_with("File 1 of 1 · "));
    assert!(app.main.file_progress.text().contains("remaining"));
    assert!(!app.main.file_progress.text().contains("Calculating"));
    app.request_cancel();
    tx.send(Event::Finished(Err("late cancellation error".into())))
        .unwrap();
    app.poll_transcription();
    assert!(app.main.status.text().starts_with("Cancelled."));
    assert!(!app.main.file_progress.property::<bool>("visible"));
    app.window.destroy();
}

pub(super) fn file_boundary_rendering() {
    for (prefix, expected) in [
        ("", "Next."),
        ("Edited.", "Edited.\n\nNext."),
        ("Edited.\n", "Edited.\n\nNext."),
        ("Edited.\n\n", "Edited.\n\nNext."),
    ] {
        let app = test_app();
        app.main.transcript.buffer().set_text(prefix);
        app.document.prepare_file_append();
        app.document.set_provisional(" Next?".into());
        app.render_transcript();
        app.document.accept_final(vec![Segment {
            start_ms: 0,
            end_ms: 1000,
            text: " Next.".into(),
        }]);
        app.document.clear_provisional();
        app.render_transcript();
        assert_eq!(buffer_text(&app.main.transcript), expected);
        app.window.destroy();
    }
}
