use super::*;

#[test]
fn settings_pages_and_menu_surfaces_build() {
    if gtk::init().is_err() {
        eprintln!("Skipping GTK surface test: no display available");
        return;
    }
    adw::init().expect("libadwaita initialization");
    let parent = adw::ApplicationWindow::builder().build();
    let preferences = preferences::Preferences::new(&parent, &Settings::default());
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
