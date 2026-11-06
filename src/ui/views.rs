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

/// A visual hint, never buffer contents. Keep the scrolling widget/pane as
/// the overlay's main child so text selection, scrolling, and zoom are native.
fn with_placeholder(
    content: &impl IsA<gtk::Widget>,
    view: &gtk::TextView,
    hint: &str,
) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(content));
    let label = gtk::Label::builder()
        .label(hint)
        .wrap(true)
        .xalign(0.0)
        .halign(Align::Fill)
        .valign(Align::Start)
        .can_target(false)
        .margin_start(18)
        .margin_end(18)
        .margin_top(16)
        .margin_bottom(16)
        .build();
    label.add_css_class("dim-label");
    label.set_visible(view.buffer().char_count() == 0);
    overlay.add_overlay(&label);
    overlay.set_clip_overlay(&label, true);
    let weak = label.downgrade();
    view.buffer().connect_changed(move |buffer| {
        if let Some(label) = weak.upgrade() {
            label.set_visible(buffer.char_count() == 0);
        }
    });
    overlay
}

pub fn pane(title: &str, view: &impl IsA<gtk::Widget>) -> gtk::Box {
    pane_with_zoom(title, view, &Rc::new(ZoomGroup::default()))
}

fn pane_with_zoom(title: &str, view: &impl IsA<gtk::Widget>, zoom: &Rc<ZoomGroup>) -> gtk::Box {
    let container = column(8);
    if !title.is_empty() {
        let label = gtk::Label::builder()
            .label(title)
            .halign(Align::Start)
            .build();
        label.add_css_class("heading");
        container.append(&label);
    }
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
    pub context: gtk::TextView,
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
        let context = text_view(true);
        context.set_vexpand(false);
        context.set_tooltip_text(Some("Optional recognition hints: names, terminology, or topic. Keep it short; incorrect hints can bias recognition. Saved locally as plain text on exit."));
        let context_scroll = gtk::ScrolledWindow::builder()
            .child(&context)
            .min_content_height(96)
            .max_content_height(180)
            .propagate_natural_height(true)
            .vexpand(false)
            .build();
        context_scroll.add_css_class("card");
        let context_content = with_placeholder(
            &context_scroll,
            &context,
            "Optional names, terminology, or topic. For example: Rust, GTK4, Vulkan; participants Giulia and Marco. Hints can help recognition but are not instructions.",
        );
        let context_section = gtk::Expander::builder()
            .label("Context (optional)")
            .expanded(false)
            .child(&context_content)
            .build();
        root.append(&context_section);
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
            context,
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
    pub prompt_section: gtk::Expander,
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
        prompt.set_tooltip_text(Some(
            "Required instructions for the LLM. Saved locally as plain text on exit.",
        ));
        let prompt_pane = pane("", &prompt);
        prompt_pane.set_vexpand(false);
        let prompt_content = with_placeholder(
            &prompt_pane,
            &prompt,
            "Required instructions for the LLM. For example: Rewrite this team meeting transcript as clear notes, preserving facts, decisions, and action items. Do not invent information.",
        );
        let prompt_section = gtk::Expander::builder()
            .label("Sistem Prompt (required)")
            .expanded(false)
            .child(&prompt_content)
            .build();
        root.append(&prompt_section);
        let notice = gtk::Label::builder()
            .label("The system prompt and original text will be sent to the LLM server configured in Settings.")
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
            prompt_section,
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
