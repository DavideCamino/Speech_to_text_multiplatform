use super::*;

#[test]
fn settings_pages_and_menu_surfaces_build() {
    if gtk::init().is_err() {
        eprintln!("Skipping GTK surface test: no display available");
        return;
    }
    adw::init().expect("libadwaita initialization");
    // Keep all GTK checks on this one test thread: GTK cannot be initialized
    // independently by parallel Rust test threads.
    incremental_transcript_updates();
    markdown_buffer::test_markdown_buffer();
    markdown_buffer::test_markdown_scrolling();
    text_zoom::test_markdown_zoom();
    text_zoom::test_font_zoom();
    text_zoom::test_shared_recognized_zoom();
    font_zoom_panes();
    let parent = adw::ApplicationWindow::builder().build();
    device_discovery_states(&parent);
    language_selection(&parent);
    recognition_context_controls(&parent);
    prompt_placeholders_and_restore(&parent);
    model_library_window(&parent);
    rewrite_generation_states(&parent);
    editable_transcript();
    background_filesystem_states();
    let compiled = transcription::compiled_gpu_backends();
    let settings = Settings {
        gpu_backend: compiled.last().copied(),
        ..Settings::default()
    };
    let preferences = preferences::Preferences::new(&parent, &settings);
    assert_eq!(
        preferences.gpu_backend.model().unwrap().n_items(),
        compiled.len() as u32 + 1
    );
    assert_eq!(preferences.gpu_backend.selected(), compiled.len() as u32);
    // Test the toggle and busy-state sensitivity without initializing a driver.
    preferences.gpu.set_sensitive(true);
    preferences.gpu.set_active(true);
    assert!(preferences.gpu_backend.is_sensitive());
    preferences.gpu.set_active(false);
    assert!(!preferences.gpu_backend.is_sensitive());
    preferences.gpu.set_active(true);
    preferences.gpu.set_sensitive(false);
    assert!(!preferences.gpu_backend.is_sensitive());
    preferences.window.set_visible_page_name("stt");
    assert_eq!(
        preferences.window.visible_page_name().as_deref(),
        Some("stt")
    );
    preferences.window.set_visible_page_name("llm");
    assert_eq!(
        preferences.window.visible_page_name().as_deref(),
        Some("llm")
    );
    assert_eq!(preferences.llm_host.text(), "127.0.0.1");
    assert_eq!(preferences.llm_port.text(), "8080");
    assert!(!preferences.llm_https.is_active());
    assert!(preferences.llm_token.text().is_empty());
    assert!(preferences.llm_test.is_sensitive());
    let rewrite = RewriteView::new(&Rc::new(text_zoom::ZoomGroup::default()));
    assert_eq!(rewrite.run.label().as_deref(), Some("Rewrite"));
    assert!(buffer_text(&rewrite.prompt).is_empty());
    assert_eq!(
        rewrite.prompt_section.label().as_deref(),
        Some("System Prompt (required)")
    );
    assert!(!rewrite.prompt_section.is_expanded());
    assert!(!rewrite.stop.is_sensitive());
    rewrite
        .renderer
        .borrow_mut()
        .update(&rewrite.result, "# Heading\n\n**Live** output");
    assert_eq!(buffer_text(&rewrite.result), "Heading\n\nLive output");
    assert!(!rewrite.result.is_editable());
    let menu = menu::button();
    assert_eq!(menu.menu_model().unwrap().n_items(), 3);
    for (_, _, accelerator) in menu::SHORTCUTS {
        assert!(gtk::accelerator_parse(*accelerator).is_some());
    }
    menu::shortcuts(&parent);
    menu::about(&parent);
    let about = gtk::Window::list_toplevels()
        .into_iter()
        .find_map(|window| window.downcast::<adw::AboutWindow>().ok())
        .expect("About window");
    assert_eq!(about.license_type(), gtk::License::Custom);
    let license = about.license();
    assert!(license.contains("project license is MIT"));
    assert!(license.contains("MPL-2.0"));
    assert!(license.contains("LGPL"));
    for window in gtk::Window::list_toplevels() {
        if let Ok(window) = window.downcast::<gtk::Window>() {
            window.destroy();
        }
    }
}

fn language_selection(parent: &adw::ApplicationWindow) {
    fn popup() -> (adw::MessageDialog, gtk::Entry) {
        let dialog = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
            .find(|dialog| dialog.heading().as_deref() == Some("Custom language"))
            .expect("custom language popup");
        let entry = dialog
            .extra_child()
            .unwrap()
            .first_child()
            .unwrap()
            .downcast::<gtk::Entry>()
            .unwrap();
        (dialog, entry)
    }
    let app = test_app(parent);
    let language = &app.preferences.language;
    assert_eq!(language.dropdown.selected(), 5);
    let model = language.dropdown.model().unwrap();
    let names: Vec<_> = (0..model.n_items())
        .map(|i| {
            model
                .item(i)
                .unwrap()
                .downcast::<gtk::StringObject>()
                .unwrap()
                .string()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        [
            "English",
            "Italian",
            "French",
            "Spanish",
            "German",
            "Auto",
            "Custom…"
        ]
    );
    for (index, code) in [
        Some("en"),
        Some("it"),
        Some("fr"),
        Some("es"),
        Some("de"),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        language.dropdown.set_selected(index as u32);
        app.persist_controls();
        assert_eq!(language.selected_language().as_deref(), code);
        assert_eq!(app.settings.borrow().language.as_deref(), code);
    }
    language.dropdown.set_selected(0);
    language.dropdown.set_selected(6);
    let (dialog, entry) = popup();
    assert!(!dialog.is_response_enabled("apply"));
    entry.set_text("not-a-language");
    assert!(!dialog.is_response_enabled("apply"));
    assert_eq!(language.selected_language().as_deref(), Some("en"));
    dialog.response("cancel");
    assert_eq!(language.dropdown.selected(), 0);
    assert_eq!(language.selected_language().as_deref(), Some("en"));
    dialog.close();

    language.dropdown.set_selected(6);
    let (dialog, entry) = popup();
    entry.set_text(" PT ");
    assert!(dialog.is_response_enabled("apply"));
    dialog.response("apply");
    assert_eq!(language.dropdown.selected(), 6);
    assert_eq!(language.selected_language().as_deref(), Some("pt"));
    app.persist_controls();
    assert_eq!(app.settings.borrow().language.as_deref(), Some("pt"));
    dialog.close();
    let edit = language
        .dropdown
        .next_sibling()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    assert!(edit.property::<bool>("visible"));
    edit.emit_clicked();
    let (dialog, entry) = popup();
    assert_eq!(entry.text(), "pt");
    entry.set_text("ja");
    dialog.close();
    assert_eq!(language.selected_language().as_deref(), Some("pt"));

    // Reopening Custom remembers the previous code; Cancel restores the preset.
    language.dropdown.set_selected(1);
    language.dropdown.set_selected(6);
    let (dialog, entry) = popup();
    assert_eq!(entry.text(), "pt");
    entry.set_text("ja");
    dialog.response("cancel");
    assert_eq!(language.dropdown.selected(), 1);
    assert_eq!(language.selected_language().as_deref(), Some("it"));
    dialog.close();
    language.dropdown.set_selected(6);
    let (dialog, entry) = popup();
    entry.set_text(" FR ");
    dialog.response("apply");
    assert_eq!(language.dropdown.selected(), 2);
    dialog.close();
    language.dropdown.set_selected(6);
    let (dialog, entry) = popup();
    entry.set_text("AUTO");
    dialog.response("apply");
    assert_eq!(language.dropdown.selected(), 5);
    assert_eq!(language.selected_language(), None);
    dialog.close();

    // Even a programmatic Apply cannot commit an invalid or busy-state choice.
    language.dropdown.set_selected(6);
    let (dialog, entry) = popup();
    entry.set_text("invalid");
    dialog.response("apply");
    assert_eq!(language.dropdown.selected(), 5);
    dialog.close();
    language.dropdown.set_selected(6);
    let (dialog, entry) = popup();
    entry.set_text("ja");
    language.set_sensitive(false);
    assert!(!language.dropdown.is_sensitive());
    dialog.response("apply");
    assert_eq!(language.selected_language(), None);
    dialog.close();
    language.set_sensitive(true);

    for (saved, index, expected) in [
        (None, 5, None),
        (Some("en"), 0, Some("en")),
        (Some("it"), 1, Some("it")),
        (Some("fr"), 2, Some("fr")),
        (Some("es"), 3, Some("es")),
        (Some("de"), 4, Some("de")),
        (Some("auto"), 5, None),
        (Some("pt"), 6, Some("pt")),
        (Some(" JA "), 6, Some("ja")),
    ] {
        let settings = Settings {
            language: saved.map(str::to_owned),
            ..Settings::default()
        };
        let preferences = preferences::Preferences::new(parent, &settings);
        assert_eq!(preferences.language.dropdown.selected(), index);
        assert_eq!(
            preferences.language.selected_language().as_deref(),
            expected
        );
        preferences.window.destroy();
    }
    app.preferences.window.destroy();
}

fn recognition_context_controls(parent: &adw::ApplicationWindow) {
    let app = test_app(parent);
    app.connect_transcript_editing();
    app.refresh();
    let section = app
        .main
        .root
        .first_child()
        .unwrap()
        .downcast::<gtk::Expander>()
        .unwrap();
    assert_eq!(section.label().as_deref(), Some("Context (optional)"));
    assert!(!section.is_expanded());
    assert!(app.main.context.is_editable());
    assert!(app.recognition_context().is_empty());
    let rewrite_instructions = buffer_text(&app.rewrite.prompt);
    section.set_expanded(true);
    app.main
        .context
        .buffer()
        .set_text(" \nGiulia, GTK4, Rust\n日本語 terminology  ");
    let snapshot = app.recognition_context();
    assert_eq!(snapshot, "Giulia, GTK4, Rust\n日本語 terminology");
    assert!(
        !app.dirty.get(),
        "context is not an unsaved transcript edit"
    );
    app.clear_transcript();
    assert_eq!(
        app.recognition_context(),
        snapshot,
        "replacing audio retains session context"
    );
    app.main
        .context
        .buffer()
        .set_text("Different context for the next job");
    assert_eq!(
        snapshot, "Giulia, GTK4, Rust\n日本語 terminology",
        "a captured job snapshot is owned"
    );
    assert_eq!(buffer_text(&app.rewrite.prompt), rewrite_instructions);

    let (tx, events) = mpsc::sync_channel(8);
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    *app.job.borrow_mut() = Some(Job {
        events,
        stop: stop.clone(),
        cancel: cancel.clone(),
    });
    app.refresh();
    assert!(!app.main.context.is_editable());
    assert!(!app.main.context.is_cursor_visible());
    section.set_expanded(false);
    assert!(
        section.is_sensitive(),
        "the box can still be collapsed during recognition"
    );
    stop.store(true, Ordering::Relaxed);
    cancel.store(true, Ordering::Relaxed);
    app.refresh();
    assert!(
        !app.main.context.is_editable(),
        "shutdown must finish before editing resumes"
    );
    tx.send(Event::Finished(Ok(()))).unwrap();
    app.poll();
    assert!(app.main.context.is_editable());
    assert!(app.main.context.is_cursor_visible());
    assert_eq!(
        app.recognition_context(),
        "Different context for the next job"
    );
    app.main.context.buffer().set_text(" \n\t ");
    assert!(app.recognition_context().is_empty());
    assert_eq!(buffer_text(&app.rewrite.prompt), rewrite_instructions);
    app.preferences.window.destroy();
}

fn prompt_placeholders_and_restore(parent: &adw::ApplicationWindow) {
    fn hint(view: &gtk::TextView) -> gtk::Label {
        view.ancestor(gtk::Overlay::static_type())
            .unwrap()
            .downcast::<gtk::Overlay>()
            .unwrap()
            .last_child()
            .unwrap()
            .downcast::<gtk::Label>()
            .unwrap()
    }
    let app = test_app(parent);
    app.restore_prompts();
    let context_hint = hint(&app.main.context);
    let prompt_hint = hint(&app.rewrite.prompt);
    for (view, label) in [
        (&app.main.context, &context_hint),
        (&app.rewrite.prompt, &prompt_hint),
    ] {
        assert!(buffer_text(view).is_empty());
        assert!(label.property::<bool>("visible"));
        assert!(label.has_css_class("dim-label"));
        assert!(!label.can_target());
        assert!(!label.text().is_empty());
        view.buffer().set_text("Typed text 世界");
        assert!(!label.property::<bool>("visible"));
        assert_eq!(buffer_text(view), "Typed text 世界");
        view.buffer().set_text("");
        assert!(label.property::<bool>("visible"));
    }
    assert!(!app.rewrite.prompt_section.is_expanded());
    assert!(
        app.recognition_context().is_empty(),
        "the hint must not become Whisper input"
    );
    app.rewrite.source.buffer().set_text("Original text");
    app.start_rewrite();
    assert!(
        app.rewrite_state.borrow().job.is_none(),
        "the LLM hint does not satisfy the required prompt"
    );
    {
        let mut settings = app.settings.borrow_mut();
        settings.whisper_context = " Giulia, GTK4\n日本語 ".into();
        settings.llm_system_prompt =
            " Preserve facts and decisions.\nDo not invent information. ".into();
    }
    app.restore_prompts();
    assert_eq!(buffer_text(&app.main.context), " Giulia, GTK4\n日本語 ");
    assert_eq!(
        buffer_text(&app.rewrite.prompt),
        " Preserve facts and decisions.\nDo not invent information. "
    );
    assert!(!context_hint.property::<bool>("visible"));
    assert!(!prompt_hint.property::<bool>("visible"));
    // Edits, including clearing a saved value, are captured only on close.
    app.main.context.buffer().set_text("Updated context");
    app.rewrite.prompt.buffer().set_text("");
    app.persist_controls();
    assert_eq!(
        app.settings.borrow().whisper_context,
        " Giulia, GTK4\n日本語 "
    );
    assert!(!app.settings.borrow().llm_system_prompt.is_empty());
    {
        let mut settings = app.settings.borrow_mut();
        settings.whisper_context.clear();
        settings.llm_system_prompt.clear();
    }
    app.restore_prompts();
    assert!(buffer_text(&app.main.context).is_empty());
    assert!(buffer_text(&app.rewrite.prompt).is_empty());
    assert!(context_hint.property::<bool>("visible"));
    assert!(prompt_hint.property::<bool>("visible"));
    app.preferences.window.destroy();
}

fn font_zoom_panes() {
    fn zoom_controller(view: &impl IsA<gtk::Widget>) -> Option<gtk::EventControllerScroll> {
        let pane = view.as_ref().parent().unwrap().parent().unwrap();
        let controllers = pane.observe_controllers();
        (0..controllers.n_items())
            .filter_map(|index| {
                controllers
                    .item(index)?
                    .downcast::<gtk::EventControllerScroll>()
                    .ok()
            })
            .find(|controller| controller.name().as_deref() == Some("text-font-zoom"))
    }
    let recognized_zoom = Rc::new(text_zoom::ZoomGroup::default());
    let main = MainView::new(&recognized_zoom);
    let rewrite = RewriteView::new(&recognized_zoom);
    for view in [
        &main.transcript,
        &rewrite.source,
        &rewrite.prompt,
        &rewrite.result,
    ] {
        let controller = zoom_controller(view).expect("plain-text pane zoom controller");
        assert_eq!(
            controller.propagation_phase(),
            gtk::PropagationPhase::Capture
        );
        assert_eq!(
            controller.flags(),
            gtk::EventControllerScrollFlags::VERTICAL
        );
        // Synthetic scroll signals without Ctrl must propagate normally.
        assert!(!controller.emit_by_name::<bool>("scroll", &[&0.0_f64, &-1.0_f64]));
    }
}

fn test_app(parent: &adw::ApplicationWindow) -> Rc<App> {
    let settings = Settings::default();
    let recognized_zoom = Rc::new(text_zoom::ZoomGroup::default());
    let main = MainView::new(&recognized_zoom);
    let preferences = preferences::Preferences::new(parent, &settings);
    let models = models::Models::new(&preferences.window);
    Rc::new(App {
        window: parent.clone(),
        stack: gtk::Stack::new(),
        transcript_buffer: RefCell::new(transcript_buffer::TranscriptBuffer::new(
            main.transcript.buffer(),
        )),
        main,
        preferences,
        models,
        model_scan: RefCell::default(),
        model_check: RefCell::default(),
        model_available: Cell::new(false),
        download_check: RefCell::default(),
        settings_writer: RefCell::new(settings::SaveWorker::with_save(|_| Ok(()))),
        closing: Cell::new(false),
        rewrite: RewriteView::new(&recognized_zoom),
        rewrite_state: RefCell::default(),
        settings: RefCell::new(settings),
        transcript: RefCell::default(),
        provisional: RefCell::default(),
        job: RefCell::default(),
        download: RefCell::default(),
        device_discovery: RefCell::default(),
        recording: Cell::new(false),
        dirty: Cell::new(false),
        text_dirty: Cell::new(false),
        rendering_transcript: Cell::new(false),
        rewrite_dirty: Cell::new(false),
        allow_close: Cell::new(false),
    })
}

fn rewrite_generation_states(parent: &adw::ApplicationWindow) {
    use crate::llm;
    let app = test_app(parent);
    // Asking to replace a model must not start work until explicit approval.
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join(model::CATALOG[0].file);
    std::fs::write(&target, b"old model").unwrap();
    app.ask_replace_download(0, target.clone());
    assert!(app.download.borrow().is_none());
    let dialog = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .find(|dialog| dialog.heading().as_deref() == Some("Replace existing model?"))
        .expect("replacement confirmation");
    assert_eq!(dialog.default_response().as_deref(), Some("cancel"));
    dialog.response("cancel");
    assert!(app.download.borrow().is_none());
    assert_eq!(std::fs::read(&target).unwrap(), b"old model");
    dialog.close();

    app.stack.add_named(&app.main.root, Some("main"));
    app.stack.add_named(&app.rewrite.root, Some("rewrite"));
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
        assert!(app.rewrite_state.borrow().job.is_none());
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
    assert!(app.rewrite_state.borrow().job.is_none());
    assert_eq!(
        app.rewrite.status.text(),
        "Original text must not be empty."
    );
    app.rewrite.prompt_section.set_expanded(false);
    app.preferences.llm_port.set_text("not-a-port");
    assert!(app.llm_connection().is_err());
    app.preferences.llm_port.set_text("8080");
    assert_eq!(app.llm_connection().unwrap().port, 8080);
    let (tx, events) = mpsc::sync_channel(8);
    let cancel = Arc::new(AtomicBool::new(false));
    app.rewrite_state.borrow_mut().job = Some(llm::Job {
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
    tx.send(llm::Event::Text("# Live\n\n**Partial** output".into()))
        .unwrap();
    app.poll_rewrite();
    assert_eq!(buffer_text(&app.rewrite.result), "Live\n\nPartial output");
    assert!(app.rewrite_dirty.get());
    assert!(!app.rewrite.copy.is_sensitive());
    cancel.store(true, Ordering::Relaxed);
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
    assert_eq!(app.rewrite_markdown(), "# Live\n\n**Partial** output");
    assert!(app.rewrite.copy.is_sensitive());
    assert!(app.rewrite.run.is_sensitive());
    assert!(app.rewrite.prompt.is_editable());
    assert_eq!(buffer_text(&app.rewrite.prompt), saved_prompt);
    assert!(!app.rewrite.stop.is_sensitive());
    app.reset_rewrite();
    assert!(!app.rewrite_dirty.get());
    assert!(app.rewrite_markdown().is_empty());
    assert!(buffer_text(&app.rewrite.result).is_empty());
    assert!(!app.rewrite.copy.is_sensitive());
}

fn editable_transcript() {
    use crate::transcript::Segment;
    let parent = adw::ApplicationWindow::builder().build();
    let app = test_app(&parent);
    app.stack.add_named(&app.main.root, Some("main"));
    app.stack.add_named(&app.rewrite.root, Some("rewrite"));
    app.connect();
    app.refresh();
    assert!(app.main.transcript.is_editable());
    assert!(app.main.transcript.is_cursor_visible());
    assert!(!app.rewrite.source.is_editable());
    assert!(!app.rewrite.result.is_editable());
    assert!(!app.main.srt.is_sensitive());

    app.transcript.borrow_mut().append(vec![Segment {
        start_ms: 0,
        end_ms: 1000,
        text: "Recognized words".into(),
    }]);
    app.dirty.set(true);
    app.render_transcript();
    assert!(
        !app.text_dirty.get(),
        "renderer changes are not manual edits"
    );
    let original_srt = app.transcript.borrow().srt();
    app.mark_export_saved(ExportKind::Subtitles, &original_srt);
    assert!(!app.dirty.get());
    let buffer = app.main.transcript.buffer();
    buffer.insert(&mut buffer.end_iter(), " — corrected 世界");
    let edited = buffer_text(&app.main.transcript);
    assert!(app.dirty.get() && app.text_dirty.get());
    assert_eq!(app.transcript.borrow().srt(), original_srt);
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
        app.dirty.get(),
        "an older TXT save must not clear newer edits"
    );
    app.mark_export_saved(ExportKind::Subtitles, &original_srt);
    assert!(
        app.dirty.get(),
        "saving original subtitles does not save manual edits"
    );
    app.mark_export_saved(
        ExportKind::TranscriptText,
        &buffer_text(&app.main.transcript),
    );
    assert!(!app.dirty.get() && !app.text_dirty.get());
    buffer.insert(&mut buffer.end_iter(), " More corrections.");
    let edited_prefix = buffer_text(&app.main.transcript);

    // Append follows the original timestamp timeline, but its rendering starts
    // after the edited buffer, including text inserted at the old end mark.
    app.transcript_buffer.borrow_mut().prepare_append();
    let (tx, events) = mpsc::sync_channel(8);
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    *app.job.borrow_mut() = Some(Job {
        events,
        stop: stop.clone(),
        cancel: cancel.clone(),
    });
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
    assert_eq!(app.transcript.borrow().end_ms(), 2000);
    let srt = app.transcript.borrow().srt();
    assert!(srt.starts_with(&original_srt));
    assert!(srt.contains("next audio") && !srt.contains("corrected"));
    assert!(app.text_dirty.get());

    buffer.set_text("");
    assert!(!app.main.txt.is_sensitive() && !app.main.rewrite.is_sensitive());
    assert!(
        app.main.srt.is_sensitive(),
        "deleting plain text must not remove subtitles"
    );
    app.clear_transcript();
    app.render_transcript();
    assert!(!app.dirty.get() && !app.text_dirty.get());
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
    assert!(app.job.borrow().is_none());
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
    parent.destroy();
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "background operation timed out"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn background_filesystem_states() {
    let parent = adw::ApplicationWindow::builder().build();
    let app = test_app(&parent);
    app.connect();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model.bin");
    std::fs::write(&path, b"test model").unwrap();
    app.settings.borrow_mut().model = Some(path.clone());
    app.update_model_label();
    assert!(!app.main.open.is_sensitive());
    wait_for(|| {
        app.poll_model_check();
        app.model_check.borrow().is_none()
    });
    assert!(app.main.open.is_sensitive());

    // Idle refreshes must use cached state, not stat the model again.
    std::fs::remove_file(&path).unwrap();
    for _ in 0..100 {
        app.refresh();
    }
    assert!(app.main.open.is_sensitive());
    assert!(app.model_check.borrow().is_none());
    app.check_model();
    wait_for(|| {
        app.poll_model_check();
        app.model_check.borrow().is_none()
    });
    assert!(!app.main.open.is_sensitive());

    // A stalled old check must not block polling or replace a newer result.
    let (release, resume) = mpsc::sync_channel(1);
    *app.model_check.borrow_mut() = Some(background(move || {
        resume.recv_timeout(Duration::from_secs(5)).unwrap();
        false
    }));
    for _ in 0..100 {
        app.poll_model_check();
        app.refresh();
    }
    std::fs::write(&path, b"test model").unwrap();
    app.update_model_label();
    wait_for(|| {
        app.poll_model_check();
        app.model_check.borrow().is_none()
    });
    release.send(()).unwrap();
    app.poll_model_check();
    assert!(app.model_available.get());
    assert!(app.main.open.is_sensitive());
    // An idle rewrite poll should not reset widget sensitivity at all.
    app.main.open.set_sensitive(false);
    app.poll_rewrite();
    assert!(!app.main.open.is_sensitive());
    app.refresh();
    assert!(app.main.open.is_sensitive());

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
    app.main
        .context
        .buffer()
        .set_text(" Context saved on close 世界\n ");
    // Clearing an earlier saved prompt must overwrite it with an empty value.
    app.settings.borrow_mut().llm_system_prompt = "Earlier prompt".into();
    app.rewrite.prompt.buffer().set_text("");
    app.window.present();
    app.window.close();
    let saved = started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(saved.whisper_context, " Context saved on close 世界\n ");
    assert!(saved.llm_system_prompt.is_empty());
    app.poll_preferences_save();
    assert!(app.closing.get());
    assert!(!app.allow_close.get());
    assert!(!app.window.is_sensitive());
    release.send(()).unwrap();
    wait_for(|| {
        app.poll_preferences_save();
        app.allow_close.get()
    });

    // A failed final save leaves the window usable and surfaces the error.
    let parent = adw::ApplicationWindow::builder().build();
    let app = test_app(&parent);
    app.connect();
    *app.settings_writer.borrow_mut() =
        settings::SaveWorker::with_save(|_| anyhow::bail!("simulated disk failure"));
    app.window.present();
    app.window.close();
    wait_for(|| {
        app.poll_preferences_save();
        !app.closing.get()
    });
    assert!(!app.allow_close.get());
    assert!(app.window.is_sensitive());
    assert!(app.main.status.text().contains("simulated disk failure"));
}

fn model_library_window(parent: &adw::ApplicationWindow) {
    let selected = PathBuf::from("models").join("ggml-tiny.bin");
    let preferences = preferences::Preferences::new(
        parent,
        &Settings {
            model: Some(selected.clone()),
            ..Settings::default()
        },
    );
    assert_eq!(preferences.model_label.text(), "ggml-tiny.bin");
    assert_eq!(
        preferences.select_model.tooltip_text().as_deref(),
        Some(selected.to_string_lossy().as_ref())
    );
    preferences.set_model(None);
    assert_eq!(preferences.model_label.text(), "No model selected");
    assert!(preferences.select_model.tooltip_text().is_none());
    preferences.set_model(Some(&selected));
    assert_eq!(preferences.model_label.text(), "ggml-tiny.bin");
    assert_eq!(
        preferences.select_model.tooltip_text().as_deref(),
        Some(selected.to_string_lossy().as_ref())
    );
    assert_eq!(
        preferences.select_model.child(),
        Some(preferences.model_label.clone().upcast())
    );
    let models = models::Models::new(&preferences.window);
    assert_eq!(models.download.label().as_deref(), Some("Download…"));
    assert_eq!(
        models.browse.label().as_deref(),
        Some("Select Local Model…")
    );
    let other = PathBuf::from("models").join("ggml-small.bin");
    let app = test_app(parent);
    let (saved, snapshots) = mpsc::channel();
    *app.settings_writer.borrow_mut() = settings::SaveWorker::with_save(move |settings| {
        saved.send(settings.model).unwrap();
        Ok(())
    });
    let directory = tempfile::tempdir().unwrap();
    let accepted = directory.path().join("ggml-small.bin");
    std::fs::write(&accepted, b"test model").unwrap();
    // The shared acceptance path updates presentation and queues one save.
    app.accept_model(accepted.clone());
    assert_eq!(app.settings.borrow().model.as_ref(), Some(&accepted));
    assert_eq!(app.preferences.model_label.text(), "ggml-small.bin");
    assert_eq!(
        app.preferences.select_model.tooltip_text().as_deref(),
        Some(accepted.to_string_lossy().as_ref())
    );
    assert_eq!(
        snapshots.recv_timeout(Duration::from_secs(5)).unwrap(),
        Some(accepted)
    );
    wait_for(|| app.settings_writer.borrow_mut().poll().is_some());
    assert!(matches!(
        snapshots.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(app.model_check.borrow().is_some());
    wait_for(|| {
        app.poll_model_check();
        app.model_check.borrow().is_none()
    });
    assert!(app.model_available.get());
    app.preferences.window.destroy();

    let chosen = Rc::new(RefCell::new(None));
    let observed = chosen.clone();
    models.set_entries(
        vec![selected.clone(), other.clone()],
        Some(&selected),
        move |path| {
            *observed.borrow_mut() = Some(path);
        },
    );
    let first = models
        .list
        .row_at_index(0)
        .unwrap()
        .child()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    assert!(first.has_css_class("suggested-action"));
    let second = models
        .list
        .row_at_index(1)
        .unwrap()
        .child()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    second.emit_clicked();
    assert_eq!(*chosen.borrow(), Some(other));
    assert!(models.list.row_at_index(2).is_none());
    models.set_entries(vec![], None, |_| {});
    assert!(models.list.row_at_index(0).is_none());
}

#[test]
fn microphone_discovery_runs_on_a_background_thread_and_reports_errors() {
    let ui_thread = std::thread::current().id();
    let events = background(move || {
        assert_ne!(std::thread::current().id(), ui_thread);
        Ok::<Vec<String>, String>(vec!["USB microphone".into()])
    });
    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        vec!["USB microphone"]
    );
    let events = background(|| Err::<Vec<String>, String>("Permission denied".into()));
    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err(),
        "Permission denied"
    );
}

fn device_discovery_states(parent: &adw::ApplicationWindow) {
    let saved = "USB ☕ microphone";
    let preferences = preferences::Preferences::new(
        parent,
        &Settings {
            device: Some(saved.into()),
            ..Settings::default()
        },
    );
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    preferences.begin_device_refresh();
    preferences.refresh_device_controls(false);
    assert!(preferences.devices_loading());
    assert!(!preferences.device.is_sensitive());
    assert!(!preferences.device_refresh.is_sensitive());
    assert_eq!(
        preferences.device_row.subtitle().as_deref(),
        Some("Loading microphones…")
    );
    // Persisting/closing while discovery is in flight must retain the name.
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    preferences.finish_device_refresh(Ok(vec!["Other microphone".into()]));
    assert!(!preferences.devices_loading());
    preferences.refresh_device_controls(false);
    assert!(preferences.device.is_sensitive());
    assert!(preferences.device_refresh.is_sensitive());
    preferences.refresh_device_controls(true);
    assert!(!preferences.device.is_sensitive());
    assert!(!preferences.device_refresh.is_sensitive());
    preferences.refresh_device_controls(false);
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    assert!(
        preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("unavailable")
    );
    let selected = preferences
        .device
        .selected_item()
        .unwrap()
        .downcast::<gtk::StringObject>()
        .unwrap();
    assert_eq!(selected.string(), format!("{saved} (unavailable)"));

    let count = preferences.device.model().unwrap().n_items();
    preferences.begin_device_refresh();
    preferences.finish_device_refresh(Err("Permission denied".into()));
    preferences.refresh_device_controls(false);
    assert!(
        preferences.device_refresh.is_sensitive(),
        "failed discovery must be retryable"
    );
    assert_eq!(preferences.device.model().unwrap().n_items(), count);
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    assert!(
        preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("Permission denied")
    );

    // Reconnecting removes the unavailable label and clears the old error;
    // duplicate native names must not create indistinguishable choices.
    preferences.begin_device_refresh();
    preferences.finish_device_refresh(Ok(vec![
        "Other microphone".into(),
        saved.into(),
        saved.into(),
    ]));
    assert_eq!(preferences.device.model().unwrap().n_items(), 3);
    assert_eq!(preferences.selected_device().as_deref(), Some(saved));
    let selected = preferences
        .device
        .selected_item()
        .unwrap()
        .downcast::<gtk::StringObject>()
        .unwrap();
    assert_eq!(selected.string(), saved);
    assert!(
        !preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("Permission denied")
    );

    // Preserve the user's current choice, not the original startup setting.
    preferences.device.set_selected(1);
    preferences.begin_device_refresh();
    preferences.finish_device_refresh(Ok(vec![]));
    assert_eq!(
        preferences.selected_device().as_deref(),
        Some("Other microphone")
    );
    preferences.device.set_selected(0);
    preferences.update_device_status();
    assert!(
        preferences
            .device_row
            .subtitle()
            .unwrap()
            .contains("No microphones found")
    );
    preferences.begin_device_refresh();
    preferences.finish_device_refresh(Ok(vec![]));
    assert_eq!(preferences.selected_device(), None);
    assert_eq!(preferences.device.model().unwrap().n_items(), 1);
}

fn incremental_transcript_updates() {
    use super::transcript_buffer::TranscriptBuffer;
    use crate::transcript::Segment;

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
