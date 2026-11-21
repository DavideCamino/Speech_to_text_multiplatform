//! The header dot reflects both documents, never just the currently visible page.
use super::support::test_app;
use crate::{
    llm,
    transcript::Segment,
    ui::{export::ExportKind, lifecycle::WindowLifecycle, rewrite::RewriteOperation},
};
use adw::prelude::*;
use std::sync::{Arc, atomic::AtomicBool, mpsc};

pub(super) fn transcript_text_presence_tracks_unicode_and_undo() {
    let app = test_app();
    app.connect();
    app.refresh();
    app.window.present();
    let buffer = app.main.transcript.buffer();
    buffer.set_enable_undo(true);
    buffer.set_text("\u{2003}\u{00a0}\n");
    let assert_text_presence = |present| {
        assert_eq!(app.document.has_text(), present);
        assert_eq!(app.unsaved_indicator.is_visible(), present);
        for button in [&app.main.copy, &app.main.txt, &app.main.rewrite] {
            assert_eq!(button.is_sensitive(), present);
        }
    };
    assert!(app.has_transcript()); // A whitespace-only buffer is not literally empty.
    assert_text_presence(false);
    buffer.begin_user_action();
    buffer.insert(&mut buffer.iter_at_offset(1), "界");
    buffer.end_user_action();
    assert_text_presence(true);
    buffer.begin_user_action();
    buffer.delete(&mut buffer.iter_at_offset(1), &mut buffer.iter_at_offset(2));
    buffer.end_user_action();
    assert_text_presence(false);
    buffer.undo();
    assert_text_presence(true);
    buffer.redo();
    assert_text_presence(false);

    app.clear_transcript();
    app.document.set_provisional("仮の結果".into());
    app.render_transcript();
    assert_text_presence(true);
    assert!(!app.document.is_dirty());
    app.document.set_provisional("\u{2003}".into());
    app.render_transcript();
    assert_text_presence(false);
    assert!(!app.document.is_dirty());

    app.document.restore(&crate::recovery::TranscriptSnapshot {
        text: "Recovered text".into(),
        ..Default::default()
    });
    app.refresh();
    assert!(app.document.has_text() && app.main.txt.is_sensitive());
    assert!(!app.unsaved_indicator.is_visible());
    app.clear_transcript();
    app.refresh();
    assert_text_presence(false);
    app.window.destroy();
}

pub(super) fn header_unsaved_indicator() {
    let app = test_app();
    app.connect();
    app.refresh();
    app.window.present();
    assert!(!app.unsaved_indicator.is_visible());
    assert_eq!(
        app.unsaved_indicator.tooltip_text().as_deref(),
        Some("Unsaved changes")
    );

    // Erased/whitespace-only edits have nothing to save, even though dirty tracking remains.
    for text in ["Draft", "", " \t\n "] {
        app.main.transcript.buffer().set_text(text);
        assert_eq!(app.unsaved_indicator.is_visible(), !text.trim().is_empty());
    }
    app.clear_transcript();
    app.render_transcript();
    app.document.set_provisional("Live speech".into());
    app.render_transcript();
    assert!(app.unsaved_indicator.is_visible());
    app.document.clear_provisional();
    app.render_transcript();
    assert!(!app.unsaved_indicator.is_visible());

    // Idle text edits update immediately. A stale export cannot hide newer edits.
    app.main.transcript.buffer().set_text("Edited transcript");
    assert!(app.unsaved_indicator.is_visible());
    app.mark_export_saved(ExportKind::TranscriptText, "Older transcript");
    assert!(app.unsaved_indicator.is_visible());
    app.mark_export_saved(ExportKind::Subtitles, "");
    assert!(app.unsaved_indicator.is_visible());
    app.mark_export_saved(ExportKind::TranscriptText, "Edited transcript");
    assert!(!app.unsaved_indicator.is_visible());

    // Recognized text can be marked saved by exporting its original subtitles.
    app.clear_transcript();
    app.document.accept_final(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: " Recognized text".into(),
    }]);
    app.render_transcript();
    assert!(app.unsaved_indicator.is_visible());
    app.mark_export_saved(ExportKind::Subtitles, &app.document.srt());
    assert!(!app.unsaved_indicator.is_visible());

    let (tx, events) = mpsc::sync_channel(8);
    app.rewrite_state.borrow_mut().operation = RewriteOperation::Generating(llm::Job {
        events,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    app.refresh();
    assert!(!app.unsaved_indicator.is_visible());
    tx.send(llm::Event::Text("# Partial rewrite".into()))
        .unwrap();
    app.poll_rewrite();
    assert!(app.unsaved_indicator.is_visible());
    tx.send(llm::Event::Finished(Ok("Done".into()))).unwrap();
    app.poll_rewrite();

    app.main.transcript.buffer().set_text("");
    assert!(app.unsaved_indicator.is_visible()); // nonempty unsaved rewrite still matters
    app.main.transcript.buffer().set_text("Another edit");
    app.mark_export_saved(ExportKind::TranscriptText, "Another edit");
    assert!(app.unsaved_indicator.is_visible()); // rewrite is still unsaved
    app.mark_export_saved(ExportKind::Rewrite, "Older rewrite");
    assert!(app.unsaved_indicator.is_visible());
    app.mark_export_saved(ExportKind::Rewrite, "# Partial rewrite");
    assert!(!app.unsaved_indicator.is_visible());

    // Exporting a rewrite must not clear an unrelated transcript edit.
    app.main.transcript.buffer().set_text("Unsaved edit");
    app.mark_export_saved(ExportKind::Rewrite, "# Partial rewrite");
    assert!(app.unsaved_indicator.is_visible());
    let snapshot = app.recovery_snapshot();
    app.lifecycle.set(WindowLifecycle::SavingRecovery);
    app.refresh();
    assert!(app.unsaved_indicator.is_visible()); // autosaving is not an export
    app.lifecycle.set(WindowLifecycle::Open);

    let restored = test_app();
    restored.restore_recovery(&snapshot);
    restored.connect();
    restored.window.present();
    assert!(restored.unsaved_indicator.is_visible());
    restored.mark_export_saved(ExportKind::TranscriptText, "Unsaved edit");
    assert!(!restored.unsaved_indicator.is_visible());
    restored.window.destroy();

    // A saved nonempty rewrite cannot make an erased transcript count as unsaved content.
    app.main.transcript.buffer().set_text("");
    assert!(!app.unsaved_indicator.is_visible());
    assert!(!app.has_unsaved_content());
    let mut empty = app.recovery_snapshot();
    empty.transcript.text = " \t\n".into();
    empty.transcript.dirty = true;
    empty.rewrite.markdown = " \n".into();
    empty.rewrite.dirty = true;
    let empty_restored = test_app();
    empty_restored.restore_recovery(&empty);
    assert!(!empty_restored.has_unsaved_content());
    assert!(!empty_restored.unsaved_indicator.property::<bool>("visible"));
    empty_restored.window.destroy();

    app.clear_transcript();
    app.reset_rewrite();
    assert!(!app.unsaved_indicator.is_visible());
    app.window.destroy();
}
