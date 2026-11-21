//! Model discovery, generation controls, and Markdown export snapshots.
use super::support::test_app;
use crate::{
    llm,
    ui::{
        app::App, export::ExportKind, markdown_document, model_controller::ModelAvailability,
        rewrite::RewriteOperation, views::buffer_text,
    },
};
use adw::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

fn begin_model_refresh(app: &App) -> (mpsc::SyncSender<llm::Event>, Arc<AtomicBool>) {
    let (tx, events) = mpsc::sync_channel(4);
    let cancel = Arc::new(AtomicBool::new(false));
    app.refresh_llm_models_with(|_, _| llm::Job {
        events,
        cancel: cancel.clone(),
    });
    assert!(matches!(
        app.rewrite_state.borrow().operation,
        RewriteOperation::RefreshingModels(_)
    ));
    (tx, cancel)
}

fn model_list(ids: &[&str]) -> llm::Event {
    llm::Event::Models(
        ids.iter()
            .map(|&id| llm::Model {
                id: id.into(),
                needs_load: true,
                loading: false,
            })
            .collect(),
    )
}

fn finish_model_refresh(app: &App, tx: mpsc::SyncSender<llm::Event>, ids: &[&str]) {
    tx.send(model_list(ids)).unwrap();
    tx.send(llm::Event::Finished(Ok("discovery complete".into())))
        .unwrap();
    app.poll_rewrite();
    assert!(
        matches!(app.rewrite_state.borrow().operation, RewriteOperation::Idle),
        "Refresh must not start a model load"
    );
}

fn discover_models(app: &App, ids: &[&str]) {
    let (tx, _) = begin_model_refresh(app);
    finish_model_refresh(app, tx, ids);
}

pub(super) fn rewrite_generation_states() {
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
    // Discovery excludes generation, but does not block transcription.
    app.model_state.borrow_mut().availability = ModelAvailability::Available;
    let (discovery, cancel) = begin_model_refresh(&app);
    assert!(!app.busy());
    assert!(app.main.open.is_sensitive());
    assert!(!app.rewrite.run.is_sensitive() && !app.preferences.llm_refresh.is_sensitive());
    app.start_rewrite();
    app.refresh_llm_models_with(|_, _| panic!("must not start duplicate discovery"));
    assert!(!cancel.load(Ordering::Relaxed));
    finish_model_refresh(&app, discovery, &["first", "chosen"]);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.rewrite.run.is_sensitive() && app.preferences.llm_refresh.is_sensitive());
    assert_eq!(
        app.preferences.llm_status.subtitle().as_deref(),
        Some("Connected. 2 models available.")
    );

    assert!(app.llm_options().model.is_none());
    app.preferences.llm_model_selection.set_selected(1);
    app.preferences.llm_reasoning.set_selected(3);
    assert_eq!(app.llm_options().model.as_deref(), Some("chosen"));
    assert_eq!(app.llm_options().reasoning, llm::Reasoning::Medium);
    assert_eq!(
        app.settings.borrow().llm_options.model.as_deref(),
        Some("chosen")
    );
    app.persist_controls();
    assert_eq!(
        app.settings.borrow().llm_options.reasoning,
        llm::Reasoning::Medium
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
    assert!(!app.preferences.llm_model.is_sensitive());
    assert!(!app.preferences.llm_reasoning.is_sensitive());
    assert!(!app.preferences.llm_refresh.is_sensitive());
    app.refresh_llm_models_with(|_, _| panic!("must not refresh during generation"));
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

pub(super) fn refresh_models_populates_only_current_server_results() {
    let app = test_app();
    app.connect();
    let row = &app.preferences.llm_model_selection;
    let assert_list = |expected: &[&str], selected: Option<&str>| {
        let list = row.model().unwrap().downcast::<gtk::StringList>().unwrap();
        let ids: Vec<_> = (0..list.n_items())
            .map(|i| list.string(i).unwrap())
            .collect();
        assert_eq!(ids, expected);
        assert_eq!(app.llm_options().model.as_deref(), selected);
        assert_eq!(app.preferences.llm_apply.is_sensitive(), selected.is_some());
        assert_eq!(app.settings.borrow().llm_options.model.as_deref(), selected);
    };
    assert_list(&[], None);
    discover_models(&app, &["only"]);
    assert_list(&["only"], Some("only"));
    assert_eq!(row.selected(), 0);

    discover_models(&app, &["first", "only", "last"]);
    assert_list(&["first", "only", "last"], Some("only"));
    assert_eq!(row.selected(), 1);
    discover_models(&app, &["first", "last"]);
    assert_list(&["first", "last"], None);
    assert_eq!(row.selected(), gtk::INVALID_LIST_POSITION);
    let picker = app
        .preferences
        .llm_model
        .activatable_widget()
        .and_downcast::<gtk::MenuButton>()
        .unwrap();
    let scroll = picker
        .popover()
        .unwrap()
        .child()
        .and_downcast::<gtk::ScrolledWindow>()
        .unwrap();
    let list = scroll.child().and_downcast::<gtk::ListView>().unwrap();
    list.emit_by_name::<()>("activate", &[&0_u32]);
    assert_list(&["first", "last"], Some("first"));
    assert_eq!(picker.label().as_deref(), Some("first"));

    let (tx, _) = begin_model_refresh(&app);
    tx.send(model_list(&["partial response"])).unwrap();
    tx.send(llm::Event::Finished(Err("server unavailable".into())))
        .unwrap();
    app.poll_rewrite();
    assert_list(&[], None);
    assert!(
        app.preferences
            .llm_status
            .subtitle()
            .unwrap()
            .contains("server unavailable")
    );
    assert!(app.preferences.llm_refresh.is_sensitive());

    discover_models(&app, &["working"]);
    discover_models(&app, &[]);
    assert_list(&[], None);
    assert!(
        app.preferences
            .llm_status
            .subtitle()
            .unwrap()
            .contains("offers no models")
    );

    let (tx, _) = begin_model_refresh(&app);
    tx.send(model_list(&["stale result"])).unwrap();
    app.poll_rewrite(); // The list is not published before the terminal success event.
    assert_list(&[], None);
    app.preferences.llm_token.set_text("new credentials");
    tx.send(llm::Event::Finished(Ok(
        "success for the old request".into()
    )))
    .unwrap();
    app.poll_rewrite();
    assert_list(&[], None);
    assert!(
        app.preferences
            .llm_status
            .subtitle()
            .unwrap()
            .contains("Click Refresh Models")
    );
    assert!(app.preferences.llm_refresh.is_sensitive());

    app.preferences.llm_port.set_text("invalid");
    app.refresh_llm_models_with(|_, _| panic!("Invalid settings must not start discovery"));
    assert_list(&[], None);
    let error = app.llm_connection().unwrap_err().to_string();
    assert_eq!(
        app.preferences.llm_status.subtitle().as_deref(),
        Some(error.as_str())
    );
    app.window.destroy();
}

pub(super) fn model_loading_controls_and_readiness_follow_selection() {
    use crate::{
        transcription,
        ui::transcription_controller::{SessionSource, TranscriptionSession},
    };
    let app = test_app();
    app.connect();
    discover_models(&app, &["chosen", "other"]);
    app.preferences.llm_model_selection.set_selected(0);
    app.refresh();
    assert!(app.preferences.llm_apply.is_sensitive());
    let (_events, receiver) = mpsc::sync_channel(1);
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::Microphone,
        job: transcription::Job {
            events: receiver,
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        },
    };
    app.refresh();
    assert!(!app.preferences.llm_apply.is_sensitive());
    assert!(app.preferences.llm_refresh.is_sensitive());
    app.apply_llm_with(|_, _, _| panic!("Apply must not start while transcribing"));
    app.session.borrow_mut().finish();
    app.refresh();

    let start_load = || {
        app.preferences.window.present();
        let (tx, events) = mpsc::sync_channel(2);
        let expected_server = app.llm_connection().unwrap();
        let expected_model = app.llm_options().model;
        let before_apply = app.settings_writer.borrow().submitted_count();
        app.apply_llm_with(move |server, _, options| {
            assert_eq!(server, expected_server);
            assert_eq!(options.model, expected_model);
            llm::Job {
                events,
                cancel: Arc::new(AtomicBool::new(false)),
            }
        });
        assert!(app.rewrite_state.borrow().operation.is_loading());
        assert_eq!(
            app.settings_writer.borrow().submitted_count(),
            before_apply + 1
        );
        assert_eq!(app.settings.borrow().llm, app.llm_connection().unwrap());
        assert!(!app.preferences.window.is_visible());
        tx
    };
    let complete_load = |tx: mpsc::SyncSender<llm::Event>| {
        tx.send(llm::Event::Finished(Ok("load completed".into())))
            .unwrap();
        app.poll_rewrite();
        assert!(!app.busy());
        assert!(app.rewrite.run.is_sensitive());
    };
    app.window.present();
    app.preferences.window.present();
    app.preferences.llm_port.set_text("invalid");
    let before_invalid = app.settings_writer.borrow().submitted_count();
    app.apply_llm_with(|_, _, _| panic!("Invalid settings must not start a load"));
    assert!(app.preferences.window.is_visible());
    assert_eq!(
        app.settings_writer.borrow().submitted_count(),
        before_invalid
    );
    app.preferences.llm_port.set_text("8080");
    discover_models(&app, &["chosen", "other"]);
    app.preferences.llm_model_selection.set_selected(0);
    let load = start_load();
    app.preferences.window.present();
    assert!(app.preferences.window.is_visible());
    for widget in [
        app.preferences.llm_host.upcast_ref::<gtk::Widget>(),
        app.preferences.llm_port.upcast_ref(),
        app.preferences.llm_https.upcast_ref(),
        app.preferences.llm_token.upcast_ref(),
        app.preferences.llm_refresh.upcast_ref(),
        app.preferences.llm_model.upcast_ref(),
        app.preferences.llm_reasoning.upcast_ref(),
        app.preferences.llm_apply.upcast_ref(),
        app.rewrite.run.upcast_ref(),
    ] {
        assert!(!widget.is_sensitive());
    }
    app.refresh_llm_models_with(|_, _| panic!("Refresh must not replace the load job"));
    assert!(app.rewrite_state.borrow().operation.is_loading());
    complete_load(load);
    let assert_ready = |expected: &str| {
        assert_eq!(app.main.status.text(), expected);
        assert_eq!(app.rewrite.status.text(), expected);
        assert_eq!(
            app.preferences.llm_status.subtitle().as_deref(),
            Some(expected)
        );
    };
    assert_ready("Model ready: chosen");

    app.preferences.llm_reasoning.set_selected(1);
    assert_ready("Model ready: chosen"); // Reasoning does not select another model.
    app.main.status.set_text("Unrelated progress");
    app.preferences.llm_host.set_text("localhost");
    assert_eq!(app.main.status.text(), "Unrelated progress");
    assert!(
        app.rewrite
            .status
            .text()
            .contains("Connection settings changed")
    );
    assert!(
        app.preferences
            .llm_status
            .subtitle()
            .unwrap()
            .contains("Click Refresh Models")
    );
    assert!(app.llm_options().model.is_none());
    discover_models(&app, &["chosen", "other"]);
    app.preferences.llm_model_selection.set_selected(0);
    complete_load(start_load());
    assert_ready("Model ready: chosen");
    app.preferences.llm_model_selection.set_selected(1);
    assert!(app.main.status.text().contains("LLM settings changed"));

    let stale_load = start_load();
    // Normal editing is disabled. Simulate an out-of-band change to make
    // sure even an already queued completion cannot mark it as ready.
    app.preferences.llm_model_selection.set_selected(0);
    complete_load(stale_load);
    assert!(app.main.status.text().contains("LLM settings changed"));
    assert!(
        !app.preferences
            .llm_status
            .subtitle()
            .unwrap()
            .contains("Model ready")
    );
    complete_load(start_load());
    assert_ready("Model ready: chosen");
    app.preferences.llm_token.set_text("changed-in-session");
    assert!(
        app.main
            .status
            .text()
            .contains("Connection settings changed")
    );
    app.window.destroy();
}

pub(super) fn failed_generation_restores_controls() {
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
    assert!(app.preferences.llm_refresh.is_sensitive() && app.preferences.llm_token.is_sensitive());
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
