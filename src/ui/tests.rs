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
    let parent = adw::ApplicationWindow::builder().build();
    device_discovery_states(&parent);
    model_library_window(&parent);
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
    let menu = menu::button();
    assert_eq!(menu.menu_model().unwrap().n_items(), 3);
    for (_, _, accelerator) in menu::SHORTCUTS {
        assert!(gtk::accelerator_parse(*accelerator).is_some());
    }
    menu::shortcuts(&parent);
    menu::about(&parent);
    for window in gtk::Window::list_toplevels() {
        if let Ok(window) = window.downcast::<gtk::Window>() {
            window.destroy();
        }
    }
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
    let events = discover_devices(move || {
        assert_ne!(std::thread::current().id(), ui_thread);
        Ok(vec!["USB microphone".into()])
    });
    assert_eq!(
        events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        vec!["USB microphone"]
    );
    let events = discover_devices(|| Err("Permission denied".into()));
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

    let view = MainView::new();
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
    renderer.update(&transcript, "working");
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
        text: "世界".into(),
    }]);
    transcript.append(vec![Segment {
        start_ms: 2000,
        end_ms: 3000,
        text: "again".into(),
    }]);
    let actions = Rc::new(Cell::new(0));
    let observed = actions.clone();
    buffer.connect_begin_user_action(move |_| observed.set(observed.get() + 1));
    // One render handles several accumulated finalized segments plus the
    // latest hypothesis, just as the controller does after polling a batch.
    renderer.update(&transcript, "latest");
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
}
