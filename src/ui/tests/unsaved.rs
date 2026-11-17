//! The header dot reflects both documents, never just the currently visible page.
use super::support::test_app;
use crate::{
    llm,
    transcript::Segment,
    ui::{export::ExportKind, lifecycle::WindowLifecycle, rewrite::RewriteOperation},
};
use adw::prelude::*;
use std::sync::{Arc, atomic::AtomicBool, mpsc};

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

    app.clear_transcript();
    app.reset_rewrite();
    assert!(!app.unsaved_indicator.is_visible());
    app.window.destroy();
}
