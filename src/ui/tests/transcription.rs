//! Incremental rendering, editable documents, and Stop/Cancel controls.
use super::support::test_app;
use crate::{
    transcript::Transcript,
    transcription::{Event, Job, Source},
    ui::{
        export::ExportKind,
        text_zoom, transcript_buffer,
        transcription_controller::{SessionSource, TranscriptionSession},
        views::{MainView, buffer_text},
    },
};
use adw::prelude::*;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub(super) fn editable_transcript() {
    use crate::transcript::Segment;
    let app = test_app();
    app.connect();
    app.refresh();
    assert!(app.main.transcript.is_editable());
    assert!(app.main.transcript.is_cursor_visible());
    assert!(!app.rewrite.source.is_editable());
    assert!(!app.rewrite.result.is_editable());
    assert!(!app.main.srt.is_sensitive());

    app.document.accept_final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "Recognized words".into(),
    }]);
    app.render_transcript();
    assert!(
        !app.document.text_edited(),
        "renderer changes are not manual edits"
    );
    let original_srt = app.document.srt();
    app.mark_export_saved(ExportKind::Subtitles, &original_srt);
    assert!(!app.document.is_dirty());
    let buffer = app.main.transcript.buffer();
    buffer.insert(&mut buffer.end_iter(), " — corrected 世界");
    let edited = buffer_text(&app.main.transcript);
    assert!(app.document.is_dirty() && app.document.text_edited());
    assert_eq!(app.document.srt(), original_srt);
    assert!(app.main.txt.is_sensitive() && app.main.copy.is_sensitive());
    app.main.copy.emit_clicked();
    let copied = gtk::glib::MainContext::default()
        .block_on(app.window.clipboard().read_text_future())
        .unwrap()
        .unwrap();
    assert_eq!(copied, edited);
    app.enter_rewrite();
    assert_eq!(buffer_text(&app.rewrite.source), edited);
    assert!(!app.rewrite.source.is_editable());
    app.stack.set_visible_child_name("main");
    buffer.insert(&mut buffer.end_iter(), "!");
    app.mark_export_saved(ExportKind::TranscriptText, &edited);
    assert!(
        app.document.is_dirty(),
        "an older TXT save must not clear newer edits"
    );
    app.mark_export_saved(ExportKind::Subtitles, &original_srt);
    assert!(
        app.document.is_dirty(),
        "saving original subtitles does not save manual edits"
    );
    app.mark_export_saved(
        ExportKind::TranscriptText,
        &buffer_text(&app.main.transcript),
    );
    assert!(!app.document.is_dirty() && !app.document.text_edited());
    buffer.insert(&mut buffer.end_iter(), " More corrections.");
    let edited_prefix = buffer_text(&app.main.transcript);

    // Append follows the original timestamp timeline, but its rendering starts
    // after the edited buffer, including text inserted at the old end mark.
    app.document.prepare_append();
    let (tx, events) = mpsc::sync_channel(8);
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::File,
        job: Job {
            events,
            stop: stop.clone(),
            cancel: cancel.clone(),
        },
    };
    app.refresh();
    assert!(!app.main.transcript.is_editable());
    assert!(!app.main.transcript.is_cursor_visible());
    stop.store(true, Ordering::Relaxed);
    cancel.store(true, Ordering::Relaxed);
    app.refresh();
    assert!(
        !app.main.transcript.is_editable(),
        "stop/cancel must wait for worker completion"
    );
    tx.send(Event::Provisional("preview".into())).unwrap();
    app.poll();
    assert_eq!(
        buffer_text(&app.main.transcript),
        format!("{edited_prefix} preview")
    );
    tx.send(Event::Final(vec![Segment {
        start_ms: 1000,
        end_ms: 2000,
        text: "next audio".into(),
    }]))
    .unwrap();
    tx.send(Event::Provisional(String::new())).unwrap();
    tx.send(Event::Finished(Ok(()))).unwrap();
    app.poll();
    assert!(app.main.transcript.is_editable());
    assert_eq!(
        buffer_text(&app.main.transcript),
        format!("{edited_prefix} next audio")
    );
    assert_eq!(app.document.end_ms(), 2000);
    let srt = app.document.srt();
    assert!(srt.starts_with(&original_srt));
    assert!(srt.contains("next audio") && !srt.contains("corrected"));
    assert!(app.document.text_edited());

    buffer.set_text("");
    assert!(!app.main.txt.is_sensitive() && !app.main.rewrite.is_sensitive());
    assert!(
        app.main.srt.is_sensitive(),
        "deleting plain text must not remove subtitles"
    );
    app.clear_transcript();
    app.render_transcript();
    assert!(!app.document.is_dirty() && !app.document.text_edited());
    assert!(!app.main.srt.is_sensitive());
    buffer.set_text("Manually entered text only");
    assert!(app.has_transcript());
    assert!(app.main.txt.is_sensitive() && app.main.rewrite.is_sensitive());
    assert!(!app.main.srt.is_sensitive());
    app.request_start(Source::File(PathBuf::from("not-opened.wav")));
    let dialog = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .find(|dialog| dialog.heading().as_deref() == Some("Start a new transcription?"))
        .expect("manual-only text requires replacement confirmation");
    dialog.response("cancel");
    dialog.close();
    assert!(!app.session.borrow().is_running());
    assert_eq!(
        buffer_text(&app.main.transcript),
        "Manually entered text only"
    );
    app.clear_transcript();
    assert!(buffer_text(&app.main.transcript).is_empty());
    assert!(!app.has_transcript());
    for (prefix, preview, recognized, expected) in [
        ("corrected", " preview", "next", "corrected next"),
        ("corrected", "preview", " next", "corrected next"),
        ("corrected\n", "preview", "next", "corrected\nnext"),
        ("", "preview", "next", "next"),
    ] {
        let buffer = gtk::TextBuffer::new(None);
        buffer.create_tag(Some("provisional"), &[]);
        let mut renderer = transcript_buffer::TranscriptBuffer::new(buffer.clone());
        let mut original = Transcript::default();
        original.append(vec![Segment {
            start_ms: 0,
            end_ms: 1000,
            text: "old".into(),
        }]);
        renderer.update(&original, "");
        buffer.set_text(prefix);
        renderer.prepare_append();
        renderer.update(&original, preview);
        original.append(vec![Segment {
            start_ms: 1000,
            end_ms: 2000,
            text: recognized.into(),
        }]);
        renderer.update(&original, "");
        assert_eq!(
            buffer.text(&buffer.start_iter(), &buffer.end_iter(), false),
            expected
        );
    }
    app.window.destroy();
}

pub(super) fn transcript_document_states() {
    use crate::transcript::Segment;
    let app = test_app();
    app.connect();
    assert!(app.document.set_provisional("preview".into()));
    assert!(!app.document.set_provisional("preview".into()));
    app.render_transcript();
    assert_eq!(buffer_text(&app.main.transcript), "preview");
    assert!(!app.document.is_dirty() && !app.document.text_edited());
    assert!(!app.document.has_segments());
    assert!(!app.document.accept_final(vec![Segment {
        start_ms: 0,
        end_ms: 0,
        text: "invalid timing".into(),
    }]));
    assert!(!app.document.is_dirty());
    assert!(app.document.accept_final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "finalized".into(),
    }]));
    app.document.clear_provisional();
    app.render_transcript();
    assert_eq!(buffer_text(&app.main.transcript), "finalized");
    assert!(app.document.is_dirty() && !app.document.text_edited());
    app.clear_transcript();
    assert!(buffer_text(&app.main.transcript).is_empty());
    assert!(!app.document.is_dirty() && !app.document.text_edited());
    app.main.transcript.buffer().set_text("manual");
    assert!(app.document.is_dirty() && app.document.text_edited());
    app.clear_transcript();
    assert!(!app.document.is_dirty() && !app.document.text_edited());
    app.window.destroy();
}

pub(super) fn transcription_session_controls() {
    let app = test_app();
    app.connect();
    let (tx, events) = mpsc::sync_channel(8);
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::Microphone,
        job: Job {
            events,
            stop: stop.clone(),
            cancel: cancel.clone(),
        },
    };
    app.refresh();
    assert!(app.main.stop.is_sensitive() && app.main.cancel.is_sensitive());
    app.main.stop.emit_clicked();
    assert!(stop.load(Ordering::Relaxed));
    assert!(!cancel.load(Ordering::Relaxed));
    app.refresh();
    assert!(!app.main.stop.is_sensitive());
    assert!(
        app.main.cancel.is_sensitive(),
        "Cancel is still available after Stop"
    );
    let status = app.main.status.text();
    tx.send(Event::Status("late worker status".into())).unwrap();
    tx.send(Event::Meter {
        seconds: 2.0,
        level: 0.8,
    })
    .unwrap();
    app.poll_transcription();
    assert_eq!(app.main.status.text(), status);
    app.main.cancel.emit_clicked();
    assert!(cancel.load(Ordering::Relaxed));
    for _ in 0..3 {
        app.refresh();
        assert!(!app.main.stop.is_sensitive() && !app.main.cancel.is_sensitive());
        assert!(!app.main.transcript.is_editable());
        assert!(app.session.borrow().is_running());
    }
    tx.send(Event::Finished(Ok(()))).unwrap();
    app.poll_transcription();
    assert!(!app.session.borrow().is_running());
    assert!(app.main.transcript.is_editable());
    assert_eq!(
        app.main.status.text(),
        "Cancelled. Completed text has been kept."
    );
    app.window.destroy();
}

pub(super) fn failed_session_restores_controls() {
    use crate::{transcript::Segment, ui::model_controller::ModelAvailability};
    let app = test_app();
    app.connect();
    app.document.accept_final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "Completed text".into(),
    }]);
    app.render_transcript();
    let (worker, events) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::File,
        job: Job {
            events,
            stop: Arc::new(AtomicBool::new(false)),
            cancel: cancel.clone(),
        },
    };
    app.refresh();
    assert!(!app.main.context.is_editable());
    assert!(!app.preferences.language.dropdown.is_sensitive());
    assert!(app.main.cancel.is_sensitive());
    worker
        .send(Event::Finished(Err("simulated decode failure".into())))
        .unwrap();
    app.poll_transcription();
    assert!(!app.session.borrow().is_running());
    assert!(
        cancel.load(Ordering::Relaxed),
        "completion releases the job"
    );
    assert!(app.main.transcript.is_editable() && app.main.context.is_editable());
    assert!(app.preferences.language.dropdown.is_sensitive());
    assert!(!app.main.stop.is_sensitive() && !app.main.cancel.is_sensitive());
    assert_eq!(buffer_text(&app.main.transcript), "Completed text");
    assert!(app.document.is_dirty());
    assert!(app.main.status.text().contains("Transcription failed"));
    let dialog = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .find(|dialog| dialog.heading().as_deref() == Some("Transcription failed"))
        .expect("transcription error is surfaced");
    dialog.close();
    // Starting another session still requires an available model after recheck.
    assert!(!app.main.open.is_sensitive());
    app.model_state.borrow_mut().availability = ModelAvailability::Available;
    app.refresh();
    assert!(app.main.open.is_sensitive() && app.main.record.is_sensitive());
    app.window.destroy();
}

pub(super) fn incremental_transcript_updates() {
    use crate::transcript::Segment;
    use crate::ui::transcript_buffer::TranscriptBuffer;

    let view = MainView::new(&Rc::new(text_zoom::ZoomGroup::default()));
    let buffer = view.transcript.buffer();
    let mut renderer = TranscriptBuffer::new(buffer.clone());
    let mut transcript = Transcript::default();
    let text = || {
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string()
    };
    let changes = Rc::new(Cell::new(0));
    let observed = changes.clone();
    buffer.connect_changed(move |_| observed.set(observed.get() + 1));
    renderer.update(&transcript, "draft");
    assert_eq!(
        text(),
        "draft",
        "no leading space before provisional-only text"
    );
    let before = changes.get();
    renderer.update(&transcript, "draft");
    transcript.append(vec![]);
    renderer.update(&transcript, "draft");
    assert_eq!(
        changes.get(),
        before,
        "unchanged and empty events must not touch GTK"
    );

    transcript.append(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "Caffè ☕".into(),
    }]);
    renderer.update(&transcript, " working");
    assert_eq!(text(), "Caffè ☕ working");
    let boundary = "Caffè ☕".chars().count() as i32;
    // Include the append boundary, where GTK's normal mark gravity would
    // otherwise expand the selection to include the newly inserted words.
    buffer.select_range(&buffer.start_iter(), &buffer.iter_at_offset(boundary));
    let deletions = Rc::new(RefCell::new(Vec::new()));
    let observed = deletions.clone();
    let handler = buffer.connect_delete_range(move |_, start, end| {
        observed.borrow_mut().push((start.offset(), end.offset()));
    });
    transcript.append(vec![Segment {
        start_ms: 1000,
        end_ms: 2000,
        text: " 世界".into(),
    }]);
    transcript.append(vec![Segment {
        start_ms: 2000,
        end_ms: 3000,
        text: " again".into(),
    }]);
    let actions = Rc::new(Cell::new(0));
    let observed = actions.clone();
    buffer.connect_begin_user_action(move |_| observed.set(observed.get() + 1));
    // One render handles several accumulated finalized segments plus the
    // latest hypothesis, just as the controller does after polling a batch.
    renderer.update(&transcript, " latest");
    assert_eq!(actions.get(), 1);
    assert_eq!(text(), "Caffè ☕ 世界 again latest");
    assert_eq!(deletions.borrow().as_slice(), &[(boundary, boundary + 8)]);
    assert_eq!(buffer.iter_at_mark(&buffer.get_insert()).offset(), 0);
    assert_eq!(
        buffer.iter_at_mark(&buffer.selection_bound()).offset(),
        boundary
    );
    let tag = buffer.tag_table().lookup("provisional").unwrap();
    assert!(!buffer.iter_at_offset(boundary + 1).has_tag(&tag));
    let finalized_end = transcript.text().chars().count() as i32;
    assert!(buffer.iter_at_offset(finalized_end + 1).has_tag(&tag));
    renderer.update(&transcript, "");
    assert_eq!(text(), transcript.text());
    assert!(
        deletions
            .borrow()
            .iter()
            .all(|(start, _)| *start >= boundary)
    );
    buffer.disconnect(handler);

    renderer.reset();
    transcript = Transcript::default();
    transcript.append(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "new".into(),
    }]);
    renderer.update(&transcript, "");
    assert_eq!(text(), "new");

    // A large committed prefix must never be deleted/reinserted by repeated
    // provisional updates. Measure touched text, not machine-dependent timing.
    renderer.reset();
    transcript = Transcript::default();
    transcript.append(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "x".repeat(100_000),
    }]);
    renderer.update(&transcript, "draft");
    let inserted = Rc::new(Cell::new(0));
    let observed = inserted.clone();
    buffer
        .connect_insert_text(move |_, _, text| observed.set(observed.get() + text.chars().count()));
    let deleted_offsets = Rc::new(RefCell::new(Vec::new()));
    let observed = deleted_offsets.clone();
    buffer.connect_delete_range(move |_, start, _| observed.borrow_mut().push(start.offset()));
    for i in 0..100 {
        renderer.update(&transcript, &format!("draft{i}"));
    }
    assert!(
        inserted.get() < 1000,
        "the finalized prefix must not be reinserted"
    );
    assert_eq!(deleted_offsets.borrow().as_slice(), &[100_000; 100]);

    // Finalized/provisional boundaries must not invent whitespace, including
    // when a hypothesis becomes final in a later polling tick.
    for (fragments, expected) in [
        (vec![" Hello", ",", " world", "!"], "Hello, world!"),
        (vec![" trans", "crip", "tion"], "transcription"),
        (vec!["你", "好", "。"], "你好。"),
        (vec!["こん", "にちは", "。"], "こんにちは。"),
        (vec![" Hello ", "world", "! "], "Hello world!"),
    ] {
        renderer.reset();
        transcript = Transcript::default();
        for (index, fragment) in fragments.iter().enumerate() {
            renderer.update(&transcript, fragment);
            let combined = format!(
                "{}{fragment}",
                transcript
                    .segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            );
            assert_eq!(text(), combined.trim());
            transcript.append(vec![Segment {
                start_ms: index as u64 * 100,
                end_ms: (index as u64 + 1) * 100,
                text: (*fragment).into(),
            }]);
            renderer.update(&transcript, "");
            assert_eq!(text(), transcript.text());
        }
        assert_eq!(text(), expected);
    }
}
