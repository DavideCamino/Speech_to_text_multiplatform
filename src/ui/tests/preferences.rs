//! Language, recognition context, and saved prompt presentation.
use super::support::test_app;
use crate::{
    settings::Settings,
    transcription::{Event, Job},
    ui::{
        preferences,
        transcription_controller::{SessionSource, TranscriptionSession},
        views::buffer_text,
    },
};
use adw::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub(super) fn language_selection(parent: &adw::ApplicationWindow) {
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
    let app = test_app();
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
    app.window.destroy();
}

pub(super) fn recognition_context_controls() {
    let app = test_app();
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
        !app.document.is_dirty(),
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
    *app.session.borrow_mut() = TranscriptionSession::Running {
        source: SessionSource::Microphone,
        job: Job {
            events,
            stop: stop.clone(),
            cancel: cancel.clone(),
        },
    };
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
    app.window.destroy();
}

pub(super) fn prompt_placeholders_and_restore() {
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
    let app = test_app();
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
        !app.rewrite_state.borrow().operation.is_generating(),
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
    app.window.destroy();
}
