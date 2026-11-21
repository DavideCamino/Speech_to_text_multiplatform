//! Queue interactions and sequential jobs use deterministic worker channels.
use super::support::{test_app, wait_for_background};
use crate::{
    transcript::Segment,
    transcription::{Completion, Event, Job, Source},
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
    app.queue_audios((0..64).map(|i| format!("{i}.wav").into()).collect());
    assert!(app.main.audio_list.row_at_index(63).is_some());
    app.request_cancel();
    assert!(app.audio_queue.borrow().files.is_empty());
    assert!(app.main.audio_list.first_child().is_none());
    assert!(!app.main.audio_expander.is_visible());

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
    let tx = fake_job(&app);
    app.request_cancel();
    assert!(app.main.audio_list.row_at_index(1).is_some());
    tx.send(Event::Finished(Ok(Completion::Complete))).unwrap();
    app.poll_transcription();
    assert!(app.audio_queue.borrow().files.is_empty());
    assert!(app.main.audio_list.first_child().is_none());
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
    app.document.accept_final(vec![Segment {
        start_ms: 0,
        end_ms: 2000,
        text: "Existing text.".into(),
    }]);
    app.document.render();
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
    tx.send(Event::FileFinished(Ok(Completion::Complete)))
        .unwrap();
    app.poll_transcription();
    assert!(app.session.borrow().is_running());
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
    // The same worker/channel completes the empty file and starts the next.
    tx.send(Event::FileFinished(Ok(Completion::Complete)))
        .unwrap();
    app.poll_transcription();
    assert_eq!(buffer_text(&app.main.transcript), "Existing text. One.");
    assert!(app.main.file_progress.text().starts_with("File 3 of 3"));
    tx.send(Event::Provisional(" Three?".into())).unwrap();
    app.poll_transcription();
    assert_eq!(
        buffer_text(&app.main.transcript),
        "Existing text. One.\n\nThree?"
    );
    tx.send(Event::Final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: " Three.".into(),
    }]))
    .unwrap();
    tx.send(Event::FileFinished(Ok(Completion::Complete)))
        .unwrap();
    tx.send(Event::Finished(Ok(Completion::Complete))).unwrap();
    app.poll_transcription();
    assert_eq!(
        buffer_text(&app.main.transcript),
        "Existing text. One.\n\nThree."
    );
    assert!(
        app.document
            .srt()
            .contains("00:00:03,000 --> 00:00:04,000\nThree.")
    );
    assert!(!app.session.borrow().is_running());
    assert!(app.audio_queue.borrow().files.is_empty());
    assert!(app.main.audio_list.first_child().is_none());
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
    tx.send(Event::FileFinished(Err("decode failed".into())))
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
    tx.send(Event::FileFinished(Ok(Completion::Complete)))
        .unwrap();
    tx.send(Event::Final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "Next.".into(),
    }]))
    .unwrap();
    tx.send(Event::FileFinished(Ok(Completion::Complete)))
        .unwrap();
    tx.send(Event::Finished(Ok(Completion::Complete))).unwrap();
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
    tx.send(Event::FileFinished(Ok(Completion::IncompleteFile)))
        .unwrap();
    tx.send(Event::Final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "Already queued.".into(),
    }]))
    .unwrap();
    tx.send(Event::Provisional(" unfinished".into())).unwrap();
    app.request_cancel();
    tx.send(Event::Finished(Ok(Completion::Complete))).unwrap();
    app.poll_transcription();
    assert!(batch_dialogs(&app).is_empty());
    assert_eq!(
        buffer_text(&app.main.transcript),
        "Finalized.\n\nAlready queued."
    );
    assert_eq!(app.document.end_ms(), 2000);
    assert!(!app.session.borrow().is_running());
    assert!(app.audio_queue.borrow().files.is_empty());
    assert!(paths.iter().all(|path| path.exists()));
    app.window.destroy();
}

fn batch_dialogs(app: &App) -> Vec<adw::MessageDialog> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .filter(|dialog| {
            dialog.is_visible() && dialog.transient_for().as_ref() == Some(app.window.upcast_ref())
        })
        .collect()
}

pub(super) fn incomplete_batch_warns_only_after_final_file() {
    let app = ready();
    let paths = [
        "clipped-one.wav",
        "broken.wav",
        "clipped-two.wav",
        "complete.wav",
    ]
    .map(std::path::PathBuf::from);
    app.queue_audios(paths.to_vec());
    app.start(Source::File(paths[0].clone()), false);
    let tx = fake_job(&app);
    for (i, (result, text)) in [
        (Ok(Completion::IncompleteFile), "First readable part."),
        (Err("decode failed".into()), ""),
        (Ok(Completion::IncompleteFile), "Second readable part."),
        (Ok(Completion::Complete), "Complete file."),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(app.audio_queue.borrow().selected(), Some(&paths[i]));
        if !text.is_empty() {
            tx.send(Event::Final(vec![Segment {
                start_ms: 0,
                end_ms: 1000,
                text: text.into(),
            }]))
            .unwrap();
        }
        tx.send(Event::FileFinished(result)).unwrap();
        app.poll_transcription();
        assert!(batch_dialogs(&app).is_empty());
    }
    tx.send(Event::Finished(Ok(Completion::Complete))).unwrap();
    app.poll_transcription();
    let dialogs = batch_dialogs(&app);
    assert_eq!(dialogs.len(), 1);
    let warning = &dialogs[0];
    assert_eq!(warning.heading().as_deref(), Some("Incomplete audio files"));
    let body = warning.body();
    assert!(body.contains("clipped-one.wav") && body.contains("clipped-two.wav"));
    assert!(body.contains("broken.wav: decode failed"));
    assert!(!body.contains("complete.wav"));
    assert_eq!(
        buffer_text(&app.main.transcript),
        "First readable part.\n\nSecond readable part.\n\nComplete file."
    );
    assert!(app.audio_queue.borrow().files.is_empty());
    assert!(!app.session.borrow().is_running());
    warning.close();
    app.window.destroy();
}

pub(super) fn batch_model_failure_ends_the_queue_once() {
    let app = ready();
    app.queue_audios(vec!["one.wav".into(), "two.wav".into()]);
    app.start_selected_source(); // No model configured: the real worker fails before opening audio.
    wait_for_background("batch setup failure", || {
        app.poll_transcription();
        !app.session.borrow().is_running()
    });
    let dialogs = batch_dialogs(&app);
    assert_eq!(dialogs.len(), 1);
    assert_eq!(
        dialogs[0].heading().as_deref(),
        Some("Transcription failed")
    );
    assert!(
        dialogs[0]
            .body()
            .contains("Select or download a Whisper model first")
    );
    assert!(app.audio_queue.borrow().files.is_empty());
    dialogs[0].close();
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

pub(super) fn large_transcript_edit_and_file_boundary() {
    let app = test_app();
    app.connect();
    let buffer = app.main.transcript.buffer();
    let prefix = "長い transcript with Unicode.\n".repeat(16_384);
    buffer.set_text(&prefix);
    for _ in 0..32 {
        buffer.insert(&mut buffer.end_iter(), "字");
        let mut end = buffer.end_iter();
        let mut start = end;
        assert!(start.backward_char());
        buffer.delete(&mut start, &mut end);
        assert!(app.document.has_text());
        assert!(app.main.txt.is_sensitive());
    }
    app.document.prepare_file_append();
    app.document.accept_final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: " New file.".into(),
    }]);
    app.render_transcript();
    assert_eq!(
        buffer_text(&app.main.transcript),
        format!("{prefix}\nNew file.")
    );
    app.window.destroy();
}

pub(super) fn file_boundary_rendering() {
    for (prefix, expected) in [
        ("", "Next."),
        ("\u{2003}", "\u{2003}Next."),
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
