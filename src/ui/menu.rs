//! Header menu and application information/help surfaces.
use adw::prelude::*;

/// Action, description, accelerator. Also used to populate the shortcut viewer.
pub const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("win.open", "Open an audio file", "<Primary>o"),
    ("win.record", "Start recording", "<Primary>r"),
    ("win.stop", "Stop recording", "<Primary>period"),
    ("win.save", "Save transcript as TXT", "<Primary>s"),
    ("win.settings", "Settings", "<Primary>comma"),
    ("win.shortcuts", "Keyboard shortcuts", "<Primary>question"),
    ("win.close", "Close window", "<Primary>w"),
];

pub fn button() -> gtk::MenuButton {
    let menu = gtk::gio::Menu::new();
    menu.append(Some("Settings"), Some("win.settings"));
    menu.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    menu.append(Some("About Speech2Text"), Some("win.about"));
    gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text("Main Menu")
        .menu_model(&menu)
        .build()
}

pub fn shortcuts(parent: &adw::ApplicationWindow) {
    // GTK 4.12 supports the builder format; programmatic add_section/add_group
    // APIs require GTK 4.14. Keep compatibility with our existing minimum.
    let entries: String = SHORTCUTS
        .iter()
        .map(|(_, title, accelerator)| {
            let accelerator = accelerator.replace('<', "&lt;").replace('>', "&gt;");
            format!(
                r#"<child><object class="GtkShortcutsShortcut">
            <property name="title">{title}</property>
            <property name="accelerator">{accelerator}</property>
        </object></child>"#
            )
        })
        .collect();
    let xml = format!(
        r#"<interface>
        <object class="GtkShortcutsWindow" id="shortcuts">
            <property name="modal">true</property>
            <child><object class="GtkShortcutsSection">
                <property name="section-name">general</property>
                <property name="title">Speech2Text</property>
                <child><object class="GtkShortcutsGroup">
                    <property name="title">General</property>
                    {entries}
                </object></child>
            </object></child>
        </object>
    </interface>"#
    );
    let builder = gtk::Builder::from_string(&xml);
    let window: gtk::ShortcutsWindow = builder
        .object("shortcuts")
        .expect("shortcut window definition");
    window.set_transient_for(Some(parent));
    window.set_destroy_with_parent(true);
    window.present();
}

pub fn about(parent: &adw::ApplicationWindow) {
    let window = adw::AboutWindow::builder()
        .transient_for(parent)
        .modal(true)
        .destroy_with_parent(true)
        .application_name("Speech2Text")
        .application_icon("io.github.davidecamino.Speech2Text")
        .developer_name("Davide Camino")
        .developers(["Davide Camino"])
        .website(env!("CARGO_PKG_HOMEPAGE"))
        .issue_url("https://github.com/DavideCamino/Speech_to_text_multiplatform/issues")
        .copyright("© 2026 Davide Camino and Speech2Text contributors")
        .version(env!("CARGO_PKG_VERSION"))
        .license_type(gtk::License::MitX11)
        .comments("Private, on-device speech recognition with Whisper.\nBuilt with Rust, GTK, and libadwaita.\n\nThe rewrite feature is a demonstration; no LLM is connected.")
        .build();
    window.present();
}
