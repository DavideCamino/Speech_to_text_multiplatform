//! Default preference/rewrite widgets, GPU controls, menus, and licensing.
use crate::{
    settings::Settings,
    transcription,
    ui::{
        menu, preferences, text_zoom,
        views::{RewriteView, buffer_text},
    },
};
use adw::prelude::*;
use std::rc::Rc;

pub(super) fn settings_pages_and_menu_surfaces(parent: &adw::ApplicationWindow) {
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
    menu::shortcuts(parent);
    menu::about(parent);
    let about = gtk::Window::list_toplevels()
        .into_iter()
        .find_map(|window| window.downcast::<adw::AboutWindow>().ok())
        .expect("About window");
    assert_eq!(about.license_type(), gtk::License::Custom);
    let license = about.license();
    assert!(license.contains("project license is MIT"));
    assert!(license.contains("MPL-2.0"));
    assert!(license.contains("LGPL"));
    let shortcuts = gtk::Window::list_toplevels()
        .into_iter()
        .find_map(|window| window.downcast::<gtk::ShortcutsWindow>().ok())
        .expect("Shortcuts window");
    shortcuts.destroy();
    about.destroy();
    preferences.window.destroy();
}
