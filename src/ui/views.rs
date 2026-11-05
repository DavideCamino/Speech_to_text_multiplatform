use super::text_zoom::ZoomGroup;
use adw::prelude::*;
use gtk::{Align, Orientation};
use std::{cell::RefCell, rc::Rc};

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

pub fn pane(title: &str, view: &impl IsA<gtk::Widget>) -> gtk::Box {
    pane_with_zoom(title, view, &Rc::new(ZoomGroup::default()))
}

fn pane_with_zoom(title: &str, view: &impl IsA<gtk::Widget>, zoom: &Rc<ZoomGroup>) -> gtk::Box {
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
    if let Some(text) = view.as_ref().downcast_ref::<gtk::TextView>() {
        super::text_zoom::install(text, &container, zoom);
        text.set_tooltip_text(Some("Ctrl + mouse wheel: change font size"));
    }
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
    pub(super) fn new(recognized_zoom: &Rc<ZoomGroup>) -> Self {
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
        let transcript = text_view(true);
        let provisional = gtk::TextTag::builder()
            .name("provisional")
            .foreground("#888888")
            .build();
        transcript.buffer().tag_table().add(&provisional);
        root.append(&pane_with_zoom("Transcript", &transcript, recognized_zoom));
        transcript.set_tooltip_text(Some("Edit when transcription is stopped. Copy, TXT, and Rewrite use this text; subtitles keep the original recognition. Ctrl + mouse wheel: change font size"));
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
        srt.set_tooltip_text(Some(
            "Export the original recognized text and timestamps, without manual transcript edits",
        ));
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
    pub prompt: gtk::TextView,
    pub result: gtk::TextView,
    pub renderer: RefCell<super::markdown_buffer::MarkdownBuffer>,
    pub stop: gtk::Button,
    pub status: gtk::Label,
    pub copy: gtk::Button,
    pub txt: gtk::Button,
    pub markdown: gtk::Button,
    pub panes: gtk::Box,
}

impl RewriteView {
    pub(super) fn new(recognized_zoom: &Rc<ZoomGroup>) -> Self {
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
            .label("Instructions and original text will be sent to the LLM server configured in Settings.")
            .wrap(true)
            .halign(Align::Start)
            .build();
        notice.add_css_class("dim-label");
        root.append(&notice);
        let run = gtk::Button::with_label("Rewrite");
        run.add_css_class("suggested-action");
        run.set_halign(Align::End);
        root.append(&run);
        let stop = gtk::Button::with_label("Stop Generation");
        stop.set_halign(Align::End);
        stop.set_sensitive(false);
        root.append(&stop);
        let status = gtk::Label::builder()
            .label("Ready")
            .wrap(true)
            .halign(Align::Start)
            .build();
        root.append(&status);
        let source = text_view(false);
        let result = text_view(false);
        let renderer = RefCell::new(super::markdown_buffer::MarkdownBuffer::new(&result));
        let panes = gtk::Box::new(Orientation::Horizontal, 12);
        panes.set_homogeneous(true);
        panes.set_vexpand(true);
        panes.append(&pane_with_zoom("Original", &source, recognized_zoom));
        panes.append(&pane("Result", &result));
        root.append(&panes);
        let footer = gtk::Box::new(Orientation::Horizontal, 8);
        let copy = gtk::Button::with_label("Copy Markdown");
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
            renderer,
            prompt,
            stop,
            status,
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
