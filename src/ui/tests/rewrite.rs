//! Connection tests, generation controls, and Markdown export snapshots.
use super::support::test_app;
use crate::ui::{
    export::ExportKind, markdown_document, model_controller::ModelAvailability,
    rewrite::RewriteOperation, views::buffer_text,
};
use adw::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub(super) fn rewrite_generation_states() {
    use crate::llm;
    let app = test_app();
    app.connect();
    app.rewrite
        .prompt
        .buffer()
        .set_text("Rewrite as clear notes without inventing information.");
    let saved_prompt = buffer_text(&app.rewrite.prompt);
    assert!(!app.rewrite.prompt_section.is_expanded());
    app.rewrite.prompt_section.set_expanded(true);
    app.rewrite.prompt_section.set_expanded(false);
    assert_eq!(buffer_text(&app.rewrite.prompt), saved_prompt);
    app.rewrite.source.buffer().set_text("Original transcript");
    for empty in ["", " \n\t "] {
        app.rewrite.prompt.buffer().set_text(empty);
        app.rewrite.prompt_section.set_expanded(false);
        app.start_rewrite();
        assert!(!app.rewrite_state.borrow().operation.is_generating());
        assert!(app.rewrite.prompt_section.is_expanded());
        assert!(
            app.rewrite
                .status
                .text()
                .contains("System Prompt (required) must not be empty")
        );
    }
    app.rewrite.prompt.buffer().set_text(&saved_prompt);
    app.rewrite.source.buffer().set_text("");
    app.start_rewrite();
    assert!(!app.rewrite_state.borrow().operation.is_generating());
    assert_eq!(
        app.rewrite.status.text(),
        "Original text must not be empty."
    );
    app.rewrite.prompt_section.set_expanded(false);
    app.preferences.llm_port.set_text("not-a-port");
    assert!(app.llm_connection().is_err());
    app.preferences.llm_port.set_text("8080");
    assert_eq!(app.llm_connection().unwrap().port, 8080);
    // A connection test excludes generation, but does not block transcription.
    app.model_state.borrow_mut().availability = ModelAvailability::Available;
    let (tested, events) = mpsc::sync_channel(8);
    let test_cancel = Arc::new(AtomicBool::new(false));
    app.rewrite_state.borrow_mut().operation = RewriteOperation::TestingConnection(llm::Job {
        events,
        cancel: test_cancel.clone(),
    });
    app.refresh();
    assert!(!app.busy());
    assert!(app.main.open.is_sensitive());
    assert!(!app.rewrite.run.is_sensitive() && !app.preferences.llm_test.is_sensitive());
    app.start_rewrite();
    app.test_llm();
    assert!(app.rewrite_state.borrow().operation.is_testing());
    assert!(!test_cancel.load(Ordering::Relaxed));
    tested
        .send(llm::Event::Finished(Ok("Connected".into())))
        .unwrap();
    app.poll_rewrite();
    assert!(test_cancel.load(Ordering::Relaxed));
    assert!(app.rewrite.run.is_sensitive() && app.preferences.llm_test.is_sensitive());
    assert_eq!(
        app.preferences.llm_status.subtitle().as_deref(),
        Some("Connected")
    );

    let (tx, events) = mpsc::sync_channel(8);
    let cancel = Arc::new(AtomicBool::new(false));
    app.rewrite_state.borrow_mut().operation = RewriteOperation::Generating(llm::Job {
        events,
        cancel: cancel.clone(),
    });
    app.refresh();
    assert!(app.busy());
    assert!(!app.rewrite.run.is_sensitive());
    assert!(!app.rewrite.prompt.is_editable());
    assert!(app.rewrite.prompt_section.is_sensitive());
    app.rewrite.prompt_section.set_expanded(true);
    assert!(app.rewrite.prompt_section.is_expanded());
    assert!(!app.rewrite.prompt.is_editable());
    app.rewrite.prompt_section.set_expanded(false);
    assert_eq!(buffer_text(&app.rewrite.prompt), saved_prompt);
    assert!(!app.rewrite.back.is_sensitive());
    assert!(app.rewrite.stop.is_sensitive());
    assert!(!app.preferences.llm_token.is_sensitive());
    app.test_llm();
    assert!(app.rewrite_state.borrow().operation.is_generating());
    tx.send(llm::Event::Text("# Live\n\n**Partial** output".into()))
        .unwrap();
    app.poll_rewrite();
    assert_eq!(buffer_text(&app.rewrite.result), "Live\n\nPartial output");
    assert!(app.rewrite_state.borrow().is_dirty());
    assert!(!app.rewrite.copy.is_sensitive());
    let older = app.rewrite_markdown();
    tx.send(llm::Event::Text("!".into())).unwrap();
    app.poll_rewrite();
    app.mark_export_saved(ExportKind::Rewrite, &older);
    assert!(app.rewrite_state.borrow().is_dirty());
    app.rewrite.stop.emit_clicked();
    assert!(cancel.load(Ordering::Relaxed));
    app.refresh();
    assert!(!app.rewrite.stop.is_sensitive());
    assert!(app.rewrite_state.borrow().operation.is_generating());
    tx.send(llm::Event::Finished(Err("Cancelled".into())))
        .unwrap();
    app.poll_rewrite();
    assert!(!app.busy());
    assert!(
        app.rewrite
            .status
            .text()
            .contains("Partial output has been kept")
    );
    assert_eq!(app.rewrite_markdown(), "# Live\n\n**Partial** output!");
    assert!(app.rewrite.copy.is_sensitive());
    assert!(app.rewrite.run.is_sensitive());
    assert!(app.rewrite.prompt.is_editable());
    assert_eq!(buffer_text(&app.rewrite.prompt), saved_prompt);
    assert!(!app.rewrite.stop.is_sensitive());
    // Display has richer block spacing, TXT uses its explicit compact policy,
    // and Copy/Markdown snapshots must still use the authoritative raw source.
    let raw = app.rewrite_markdown();
    let txt = markdown_document::plain_text(&raw);
    assert_eq!(txt, "Live\nPartial output!");
    assert_eq!(buffer_text(&app.rewrite.result), "Live\n\nPartial output!");
    app.rewrite.copy.emit_clicked();
    let copied = gtk::glib::MainContext::default()
        .block_on(app.window.clipboard().read_text_future())
        .unwrap()
        .unwrap();
    assert_eq!(copied, raw);
    app.mark_export_saved(ExportKind::Rewrite, &txt);
    assert!(app.rewrite_state.borrow().is_dirty());
    app.mark_export_saved(ExportKind::Rewrite, &older);
    assert!(app.rewrite_state.borrow().is_dirty());
    app.mark_export_saved(ExportKind::Rewrite, &app.rewrite_markdown());
    assert!(!app.rewrite_state.borrow().is_dirty());
    app.reset_rewrite();
    assert!(!app.rewrite_state.borrow().is_dirty());
    assert!(app.rewrite_markdown().is_empty());
    assert!(buffer_text(&app.rewrite.result).is_empty());
    assert!(!app.rewrite.copy.is_sensitive());
    app.window.destroy();
}

pub(super) fn failed_generation_restores_controls() {
    use crate::llm;
    let app = test_app();
    app.connect();
    let (worker, events) = mpsc::sync_channel(2);
    let cancel = Arc::new(AtomicBool::new(false));
    app.rewrite_state.borrow_mut().operation = RewriteOperation::Generating(llm::Job {
        events,
        cancel: cancel.clone(),
    });
    app.refresh();
    assert!(!app.rewrite.run.is_sensitive());
    assert!(!app.rewrite.prompt.is_editable());
    worker
        .send(llm::Event::Text("**Partial** output".into()))
        .unwrap();
    worker
        .send(llm::Event::Finished(Err("simulated server failure".into())))
        .unwrap();
    app.poll_rewrite();
    assert!(!app.busy());
    assert!(!app.rewrite_state.borrow().operation.is_generating());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.rewrite.run.is_sensitive() && app.rewrite.back.is_sensitive());
    assert!(app.rewrite.prompt.is_editable());
    assert!(!app.rewrite.stop.is_sensitive());
    assert!(app.preferences.llm_test.is_sensitive() && app.preferences.llm_token.is_sensitive());
    assert!(app.rewrite.copy.is_sensitive() && app.rewrite.txt.is_sensitive());
    assert_eq!(app.rewrite_markdown(), "**Partial** output");
    assert_eq!(buffer_text(&app.rewrite.result), "Partial output");
    assert!(app.rewrite_state.borrow().is_dirty());
    assert!(
        app.rewrite
            .status
            .text()
            .contains("simulated server failure")
    );
    app.window.destroy();
}
