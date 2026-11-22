//! Default preference/rewrite widgets, GPU controls, menus, and licensing.
use crate::{
    settings::{self, Settings},
    transcription,
    ui::{
        app::App,
        dialogs, menu, preferences, text_zoom,
        views::{RewriteView, buffer_text},
    },
};
use adw::prelude::*;
use std::rc::Rc;

fn model_chooser_filters() {
    let chooser = dialogs::model_chooser();
    let model_filter = chooser.default_filter().expect("default model filter");
    let info = gtk::gio::FileInfo::new();
    for name in ["model.bin", "model.BIN", "model.BiN", "recording.wav"] {
        info.set_name(name);
        info.set_display_name(name);
        assert_eq!(
            model_filter.match_(&info),
            name != "recording.wav",
            "{name}"
        );
    }
    let filters = chooser.filters().expect("model chooser filters");
    let all = filters
        .item(1)
        .and_downcast::<gtk::FileFilter>()
        .expect("All files fallback");
    assert_eq!(all.name().as_deref(), Some("All files"));
    assert!(all.match_(&info));
}

pub(super) fn settings_pages_and_menu_surfaces(parent: &adw::ApplicationWindow) {
    model_chooser_filters();
    let compiled = transcription::compiled_gpu_backends();
    let settings = Settings {
        gpu_backend: compiled.last().copied(),
        ..Settings::default()
    };
    let preferences = preferences::Preferences::new(parent, &settings);
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
    assert_eq!(
        preferences.llm_refresh.label().as_deref(),
        Some("Refresh Models")
    );
    assert!(preferences.llm_refresh.is_sensitive());
    assert_eq!(preferences.llm_model_selection.n_items(), 0);
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
    preferences.window.destroy();
    menu_actions_and_license();
}

fn find_menu_button(widget: &gtk::Widget) -> Option<gtk::MenuButton> {
    if let Some(button) = widget.downcast_ref::<gtk::MenuButton>()
        && button.menu_model().is_some()
    {
        return Some(button.clone());
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if let Some(button) = find_menu_button(&widget) {
            return Some(button);
        }
        child = widget.next_sibling();
    }
    None
}

fn menu_actions_and_license() {
    let application = adw::Application::builder()
        .application_id("io.github.davidecamino.Speech2Text.MenuTests")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application
        .register(None::<&gtk::gio::Cancellable>)
        .unwrap();
    let app = App::new(
        Some(&application),
        Settings::default(),
        settings::SaveWorker::with_save(|_| Ok(())),
    );
    app.connect();
    app.window.present();

    let button = find_menu_button(&app.window.content().unwrap()).expect("Main menu button");
    let model = button.menu_model().unwrap();
    let entries = [
        ("Settings", "win.settings"),
        ("Keyboard Shortcuts", "win.shortcuts"),
        ("About Speech2Text", "win.about"),
    ];
    assert_eq!(model.n_items(), entries.len() as i32);
    let activate_entry = |index: usize| {
        let attribute = |key| {
            model
                .item_attribute_value(index as i32, key, Some(gtk::glib::VariantTy::STRING))
                .and_then(|value| value.get::<String>())
                .unwrap_or_else(|| panic!("Menu item {index} is missing {key}"))
        };
        let (label, expected_action) = entries[index];
        assert_eq!(attribute("label"), label);
        let action = attribute("action");
        assert_eq!(action, expected_action, "wrong action for {label}");
        gtk::prelude::WidgetExt::activate_action(&button, &action, None)
            .unwrap_or_else(|error| panic!("Cannot activate {action}: {error}"));
    };
    for (action, _, accelerator) in menu::SHORTCUTS {
        let expected = gtk::accelerator_parse(*accelerator).expect("Valid shortcut");
        let registered = application.accels_for_action(action);
        assert_eq!(registered.len(), 1, "accelerators for {action}");
        assert_eq!(
            gtk::accelerator_parse(&registered[0]),
            Some(expected),
            "accelerator for {action}"
        );
    }

    assert!(!app.preferences.window.is_visible());
    activate_entry(0);
    assert!(app.preferences.window.is_visible());
    app.preferences.window.set_visible(false);

    let child_windows = || {
        gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Window>().ok())
            .filter(|window| {
                window.is_visible()
                    && window.transient_for().as_ref() == Some(app.window.upcast_ref())
            })
    };
    activate_entry(1);
    let shortcuts = child_windows()
        .find_map(|window| window.downcast::<gtk::ShortcutsWindow>().ok())
        .expect("Keyboard Shortcuts action should open its window");
    shortcuts.destroy();

    activate_entry(2);
    let about = child_windows()
        .find_map(|window| window.downcast::<adw::AboutWindow>().ok())
        .expect("About action should open its window");
    assert_eq!(about.license_type(), gtk::License::Custom);
    let license = about.license();
    assert!(license.contains("project license is MIT"));
    assert!(license.contains("MPL-2.0"));
    assert!(license.contains("LGPL"));
    about.destroy();
    app.preferences.window.destroy();
    app.window.destroy();
}
