//! Model-library window. Filesystem/network operations belong to workers.
use adw::prelude::*;
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

pub struct Models {
    pub window: adw::Window,
    pub list: gtk::ListBox,
    pub download: gtk::Button,
    pub browse: gtk::Button,
    pub progress: gtk::ProgressBar,
    pub status: gtk::Label,
    pub directory: gtk::Label,
}

impl Models {
    pub fn new(parent: &adw::PreferencesWindow) -> Self {
        let directory = gtk::Label::builder()
            .wrap(true)
            .selectable(true)
            .xalign(0.0)
            .build();
        directory.add_css_class("dim-label");
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        let empty = gtk::Label::builder()
            .label("No models in this folder yet.")
            .margin_top(24)
            .margin_bottom(24)
            .wrap(true)
            .build();
        list.set_placeholder(Some(&empty));
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .min_content_height(180)
            .build();
        let status = gtk::Label::builder().wrap(true).xalign(0.0).build();
        let progress = gtk::ProgressBar::builder()
            .show_text(true)
            .visible(false)
            .build();
        let download = gtk::Button::with_label("Download…");
        download.add_css_class("suggested-action");
        let browse = gtk::Button::with_label("Select Local Model…");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.append(&download);
        actions.append(&browse);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        for margin in ["margin-top", "margin-bottom", "margin-start", "margin-end"] {
            content.set_property(margin, 18_i32);
        }
        content.append(&directory);
        content.append(&scroll);
        content.append(&status);
        content.append(&progress);
        content.append(&actions);
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new(
            "Whisper Models",
            "Choose a local model",
        )));
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        let window = adw::Window::builder()
            .title("Whisper Models")
            .transient_for(parent)
            .modal(true)
            .hide_on_close(true)
            .destroy_with_parent(true)
            .default_width(620)
            .default_height(480)
            .content(&toolbar)
            .build();
        Self {
            window,
            list,
            download,
            browse,
            progress,
            status,
            directory,
        }
    }

    pub fn set_entries(
        &self,
        paths: Vec<PathBuf>,
        selected: Option<&Path>,
        select: impl Fn(PathBuf) + 'static,
    ) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let select = Rc::new(select);
        for path in paths {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let label = gtk::Label::builder()
                .label(name.as_ref())
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            let button = gtk::Button::builder()
                .child(&label)
                .tooltip_text(path.to_string_lossy().as_ref())
                .build();
            if selected == Some(path.as_path()) {
                button.add_css_class("suggested-action");
            }
            let select = select.clone();
            button.connect_clicked(move |_| select(path.clone()));
            self.list.append(&button);
        }
    }
}
