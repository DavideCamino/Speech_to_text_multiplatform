use adw::prelude::*;
use gtk::{Align, Orientation};

pub fn column(spacing: i32) -> gtk::Box {
    gtk::Box::new(Orientation::Vertical, spacing)
}

pub fn text_view(editable: bool) -> gtk::TextView {
    gtk::TextView::builder()
        .editable(editable)
        .cursor_visible(editable)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(18)
        .right_margin(18)
        .top_margin(16)
        .bottom_margin(16)
        .vexpand(true)
        .hexpand(true)
        .build()
}

pub fn pane(title: &str, view: &gtk::TextView) -> gtk::Box {
    let container = column(8);
    let label = gtk::Label::builder()
        .label(title)
        .halign(Align::Start)
        .build();
    label.add_css_class("heading");
    container.append(&label);
    let scroll = gtk::ScrolledWindow::builder()
        .child(view)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(160)
        .build();
    scroll.add_css_class("card");
    container.append(&scroll);
    container
}

pub struct MainView {
    pub root: gtk::Box,
    pub open: gtk::Button,
    pub record: gtk::Button,
    pub stop: gtk::Button,
    pub cancel: gtk::Button,
    pub transcript: gtk::TextView,
    pub status: gtk::Label,
    pub progress: gtk::ProgressBar,
    pub meter: gtk::LevelBar,
    pub copy: gtk::Button,
    pub txt: gtk::Button,
    pub srt: gtk::Button,
    pub rewrite: gtk::Button,
}

impl MainView {
    pub fn new() -> Self {
        let root = column(12);
        root.set_margin_top(18);
        root.set_margin_bottom(18);
        root.set_margin_start(18);
        root.set_margin_end(18);
        let actions = gtk::Box::new(Orientation::Horizontal, 8);
        let open = gtk::Button::with_label("Open Audio…");
        open.add_css_class("suggested-action");
        let record = gtk::Button::with_label("Record");
        let stop = gtk::Button::with_label("Stop");
        let cancel = gtk::Button::with_label("Cancel");
        for button in [&open, &record, &stop, &cancel] {
            actions.append(button);
        }
        let meter = gtk::LevelBar::builder()
            .min_value(0.0)
            .max_value(1.0)
            .hexpand(true)
            .valign(Align::Center)
            .build();
        meter.set_tooltip_text(Some("Microphone input level"));
        actions.append(&meter);
        root.append(&actions);
        let transcript = text_view(false);
        let provisional = gtk::TextTag::builder()
            .name("provisional")
            .foreground("#888888")
            .build();
        transcript.buffer().tag_table().add(&provisional);
        root.append(&pane("Transcript", &transcript));
        let status = gtk::Label::builder()
            .label("Select a model in Settings, then open an audio file or record.")
            .halign(Align::Start)
            .wrap(true)
            .build();
        status.add_css_class("dim-label");
        root.append(&status);
        let progress = gtk::ProgressBar::new();
        root.append(&progress);
        let footer = gtk::Box::new(Orientation::Horizontal, 8);
        let copy = gtk::Button::with_label("Copy");
        let txt = gtk::Button::with_label("Save TXT…");
        let srt = gtk::Button::with_label("Save Subtitles…");
        let rewrite = gtk::Button::with_label("Rewrite →");
        rewrite.add_css_class("suggested-action");
        for button in [&copy, &txt, &srt, &rewrite] {
            footer.append(button);
        }
        root.append(&footer);
        Self {
            root,
            open,
            record,
            stop,
            cancel,
            transcript,
            status,
            progress,
            meter,
            copy,
            txt,
            srt,
            rewrite,
        }
    }
}

pub struct RewriteView {
    pub root: gtk::Box,
    pub back: gtk::Button,
    pub run: gtk::Button,
    pub source: gtk::TextView,
    pub result: gtk::TextView,
    pub copy: gtk::Button,
    pub txt: gtk::Button,
    pub markdown: gtk::Button,
    pub panes: gtk::Box,
}

impl RewriteView {
    pub fn new() -> Self {
        let root = column(12);
        root.set_margin_top(18);
        root.set_margin_bottom(18);
        root.set_margin_start(18);
        root.set_margin_end(18);
        let back = gtk::Button::with_label("← Transcription");
        back.set_halign(Align::Start);
        root.append(&back);
        let prompt = text_view(true);
        prompt.set_vexpand(false);
        prompt.buffer().set_text("Context: this audio is a team meeting about a software project. Rewrite the transcript as clear meeting notes, preserving facts, decisions, and action items. Do not invent information.");
        let prompt_pane = pane("Instructions", &prompt);
        prompt_pane.set_vexpand(false);
        root.append(&prompt_pane);
        let notice = gtk::Label::builder()
            .label("Demo: Rewrite copies the original text unchanged. No LLM is connected.")
            .wrap(true)
            .halign(Align::Start)
            .build();
        notice.add_css_class("dim-label");
        root.append(&notice);
        let run = gtk::Button::with_label("Rewrite (Demo)");
        run.add_css_class("suggested-action");
        run.set_halign(Align::End);
        root.append(&run);
        let source = text_view(false);
        let result = text_view(false);
        let panes = gtk::Box::new(Orientation::Horizontal, 12);
        panes.set_homogeneous(true);
        panes.set_vexpand(true);
        panes.append(&pane("Original", &source));
        panes.append(&pane("Result", &result));
        root.append(&panes);
        let footer = gtk::Box::new(Orientation::Horizontal, 8);
        let copy = gtk::Button::with_label("Copy Result");
        let txt = gtk::Button::with_label("Save TXT…");
        let markdown = gtk::Button::with_label("Save Markdown…");
        for button in [&copy, &txt, &markdown] {
            footer.append(button);
        }
        root.append(&footer);
        Self {
            root,
            back,
            run,
            source,
            result,
            copy,
            txt,
            markdown,
            panes,
        }
    }
}

pub fn buffer_text(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}
