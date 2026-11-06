//! GTK adapter for incremental Markdown display. Raw Markdown stays in rewrite
//! state; parsing, edit planning, and viewport following have separate owners.
use super::{
    markdown_diff::UpdatePlan,
    markdown_document::{Document, Style, parse_display},
    markdown_scroll::ScrollFollow,
};
use adw::prelude::*;
use gtk::pango;
use std::collections::BTreeSet;

#[cfg(test)]
use {
    super::tests::support::{wait_for_gtk, wait_for_gtk_frames},
    std::rc::Rc,
};

// Semantic styles are GTK-independent; their native presentation belongs here.
impl Style {
    fn name(&self) -> String {
        format!("md-{self:?}")
    }

    fn tag(&self, buffer: &gtk::TextBuffer) -> gtk::TextTag {
        let name = self.name();
        if let Some(tag) = buffer.tag_table().lookup(&name) {
            return tag;
        }
        let builder = gtk::TextTag::builder().name(&name);
        let tag = match self {
            Self::Heading(level) => builder
                .weight(700)
                .scale(match level {
                    1 => 1.8,
                    2 => 1.5,
                    3 => 1.3,
                    _ => 1.15,
                })
                .pixels_above_lines(8)
                .pixels_below_lines(4),
            Self::Bold => builder.weight(700),
            Self::Italic => builder.style(pango::Style::Italic),
            Self::Strike => builder.strikethrough(true),
            Self::Code => builder.family("monospace"),
            Self::CodeBlock => builder
                .family("monospace")
                .left_margin(16)
                .pixels_above_lines(6)
                .pixels_below_lines(6),
            Self::Quote(depth) => builder
                .left_margin(i32::from(*depth) * 20)
                .style(pango::Style::Italic),
            Self::List(depth) => builder.left_margin(i32::from(*depth) * 20).indent(-12),
            Self::Link => builder.underline(pango::Underline::Single),
        }
        .build();
        buffer.tag_table().add(&tag);
        tag
    }
}

pub(super) struct MarkdownBuffer {
    buffer: gtk::TextBuffer,
    rendered: Document,
    scroll: ScrollFollow,
}

impl MarkdownBuffer {
    pub fn new(view: &gtk::TextView) -> Self {
        Self {
            buffer: view.buffer(),
            rendered: Document::default(),
            scroll: ScrollFollow::default(),
        }
    }

    pub fn reset(&mut self) {
        self.scroll.reset();
        self.buffer.set_text("");
        self.rendered = Document::default();
    }

    pub fn update(&mut self, view: &gtk::TextView, markdown: &str) {
        let next = parse_display(markdown);
        if next == self.rendered {
            return;
        }
        let plan = UpdatePlan::between(&self.rendered, &next);
        let selection = (
            self.buffer.iter_at_mark(&self.buffer.get_insert()).offset(),
            self.buffer
                .iter_at_mark(&self.buffer.selection_bound())
                .offset(),
        );
        let anchor = self.scroll.capture(view, &self.buffer);
        self.buffer.begin_user_action();
        if plan.changes_text() {
            self.buffer.delete(
                &mut self.buffer.iter_at_offset(plan.start as i32),
                &mut self.buffer.iter_at_offset(plan.old_end as i32),
            );
            self.buffer.insert(
                &mut self.buffer.iter_at_offset(plan.start as i32),
                &plan.replacement,
            );
        }
        let styles: BTreeSet<_> = self
            .rendered
            .spans
            .iter()
            .chain(&next.spans)
            .map(|span| span.style.clone())
            .collect();
        for style in styles {
            self.buffer.remove_tag(
                &style.tag(&self.buffer),
                &self.buffer.iter_at_offset(plan.retag_from as i32),
                &self.buffer.end_iter(),
            );
        }
        for span in &next.spans {
            if span.end > plan.retag_from {
                self.buffer.apply_tag(
                    &span.style.tag(&self.buffer),
                    &self
                        .buffer
                        .iter_at_offset(span.start.max(plan.retag_from) as i32),
                    &self.buffer.iter_at_offset(span.end as i32),
                );
            }
        }
        self.buffer.select_range(
            &self
                .buffer
                .iter_at_offset(plan.map_position(selection.0 as usize) as i32),
            &self
                .buffer
                .iter_at_offset(plan.map_position(selection.1 as usize) as i32),
        );
        self.buffer.end_user_action();
        self.rendered = next;
        self.scroll
            .restore(view, anchor, plan.start, plan.retag_from);
    }
}

#[cfg(test)]
pub(super) fn test_markdown_buffer() {
    let view = gtk::TextView::new();
    view.set_editable(false);
    let buffer = view.buffer();
    let mut renderer = MarkdownBuffer::new(&view);
    renderer.update(&view, "# Heading\n\n**Live** output");
    assert_eq!(
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false),
        "Heading\n\nLive output"
    );
    assert!(
        buffer.start_iter().has_tag(
            &buffer
                .tag_table()
                .lookup(&Style::Heading(1).name())
                .unwrap()
        )
    );
    assert!(
        buffer
            .iter_at_offset(9)
            .has_tag(&buffer.tag_table().lookup(&Style::Bold.name()).unwrap())
    );
    buffer.select_range(&buffer.iter_at_offset(0), &buffer.iter_at_offset(7));
    let deleted = Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = deleted.clone();
    buffer.connect_delete_range(move |_, start, end| {
        observed.borrow_mut().push((start.offset(), end.offset()))
    });
    renderer.update(&view, "# Heading\n\n**Live** output grows");
    assert!(deleted.borrow().iter().all(|(start, _)| *start >= 20));
    assert_eq!(
        buffer
            .selection_bounds()
            .map(|(a, b)| (a.offset(), b.offset())),
        Some((0, 7))
    );
    renderer.reset();
    for prefix in ["**Caffè", "**Caffè ☕", "**Caffè ☕**"] {
        renderer.update(&view, prefix);
    }
    assert_eq!(
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false),
        "Caffè ☕"
    );
    assert!(
        buffer
            .start_iter()
            .has_tag(&buffer.tag_table().lookup(&Style::Bold.name()).unwrap())
    );
    let before = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
    renderer.update(&view, "**Caffè ☕**");
    assert_eq!(
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false),
        before
    );
    renderer.reset();
    assert_eq!(buffer.char_count(), 0);

    // Every UTF-8 chunk boundary must converge to the same text and tags,
    // including syntax that changes previously displayed text/formatting.
    for markdown in [
        "**Caffè ☕ 世界** and *italic*.",
        "# Heading\n\n- **one**\n- two\n\n```rust\nfn main() {}\n```",
        "[reference][id]\n\n[id]: https://example.com\n\n![alt](file:///secret)",
        "***nested*** ~~strike~~ `code` <b>literal</b>",
    ] {
        renderer.reset();
        for end in markdown.char_indices().map(|(i, c)| i + c.len_utf8()) {
            let prefix = &markdown[..end];
            renderer.update(&view, prefix);
            let expected = parse_display(prefix);
            assert_eq!(
                buffer.text(&buffer.start_iter(), &buffer.end_iter(), false),
                expected.text
            );
            for offset in 0..expected.length {
                for tag in buffer.iter_at_offset(offset as i32).tags() {
                    assert!(
                        expected.spans.iter().any(|s| s.start <= offset
                            && offset < s.end
                            && s.style.name() == tag.name().unwrap()),
                        "stale tag at {offset}: {prefix:?}"
                    );
                }
                for span in expected
                    .spans
                    .iter()
                    .filter(|s| s.start <= offset && offset < s.end)
                {
                    assert!(
                        buffer
                            .iter_at_offset(offset as i32)
                            .has_tag(&buffer.tag_table().lookup(&span.style.name()).unwrap())
                    );
                }
            }
        }
    }
    // Keep selection direction and character-based endpoints across replacement.
    renderer.reset();
    renderer.update(&view, "ab12345yz");
    buffer.select_range(&buffer.iter_at_offset(9), &buffer.iter_at_offset(3));
    renderer.update(&view, "ab☕世yz");
    assert_eq!(buffer.iter_at_mark(&buffer.get_insert()).offset(), 6);
    assert_eq!(buffer.iter_at_mark(&buffer.selection_bound()).offset(), 3);

    // Formatting-only changes must not delete/reinsert text or lose a selection.
    renderer.reset();
    renderer.update(&view, "plain");
    buffer.select_range(&buffer.iter_at_offset(4), &buffer.iter_at_offset(1));
    deleted.borrow_mut().clear();
    renderer.update(&view, "**plain**");
    assert!(deleted.borrow().is_empty());
    let bold = buffer.tag_table().lookup(&Style::Bold.name()).unwrap();
    assert!(buffer.start_iter().has_tag(&bold));
    assert_eq!(buffer.iter_at_mark(&buffer.get_insert()).offset(), 4);
    assert_eq!(buffer.iter_at_mark(&buffer.selection_bound()).offset(), 1);
    renderer.update(&view, "plain");
    assert!(deleted.borrow().is_empty());
    assert!(!buffer.start_iter().has_tag(&bold));

    // Appending to a long result must not delete/reinsert its committed prefix.
    renderer.reset();
    let prefix = format!("# Heading\n\n{}", "x".repeat(100_000));
    renderer.update(&view, &prefix);
    deleted.borrow_mut().clear();
    for i in 0..10 {
        renderer.update(&view, &format!("{prefix} tail{i}"));
    }
    assert!(deleted.borrow().iter().all(|(start, _)| *start >= 100_009));
}

#[cfg(test)]
pub(super) fn test_markdown_scrolling() {
    test_markdown_scrolling_case(false);
    test_markdown_scrolling_case(true);
}

#[cfg(test)]
fn test_markdown_scrolling_case(render_before_mapping: bool) {
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    let scroll = gtk::ScrolledWindow::builder().child(&view).build();
    let window = gtk::Window::builder()
        .child(&scroll)
        .default_width(500)
        .default_height(250)
        .build();
    let mut renderer = MarkdownBuffer::new(&view);
    let text: String = (0..80)
        .map(|i| format!("Paragraph {i}: stable content.\n\n"))
        .collect();
    if render_before_mapping {
        renderer.update(&view, &text);
    }
    window.present();
    let adjustment = scroll.vadjustment();
    wait_for_gtk("scrolling window allocation", || {
        view.is_mapped() && view.width() > 0 && view.height() > 0 && adjustment.page_size() > 0.0
    });
    wait_for_gtk_frames(&view, 2);
    if !render_before_mapping {
        renderer.update(&view, &text);
    }
    let at_bottom = || {
        adjustment.upper() > adjustment.page_size()
            && (adjustment.value() + adjustment.page_size() - adjustment.upper()).abs() < 3.0
    };
    wait_for_gtk("initial bottom follow", at_bottom);
    wait_for_gtk_frames(&view, 4);
    assert!(
        at_bottom(),
        "initial follow: value={}, page={}, upper={}",
        adjustment.value(),
        adjustment.page_size(),
        adjustment.upper()
    );
    adjustment.set_value(150.5);
    wait_for_gtk_frames(&view, 2);
    let previous = adjustment.value();
    renderer.update(&view, &format!("{text}More output."));
    wait_for_gtk_frames(&view, 4);
    assert!(
        (adjustment.value() - previous).abs() < 1.0,
        "reading viewport jumped"
    );
    adjustment.set_value(adjustment.upper() - adjustment.page_size());
    wait_for_gtk_frames(&view, 2);
    renderer.update(&view, &format!("{text}More output.\n\nAnother paragraph."));
    wait_for_gtk_frames(&view, 4);
    wait_for_gtk("appended output bottom follow", at_bottom);
    assert!((adjustment.value() + adjustment.page_size() - adjustment.upper()).abs() < 3.0);
    // A user scrolling away before the queued follow runs takes precedence.
    renderer.update(
        &view,
        &format!("{text}More output.\n\nAnother paragraph.\n\nFinal paragraph."),
    );
    adjustment.set_value(0.0);
    wait_for_gtk_frames(&view, 4);
    assert!(adjustment.value() < 1.0);
    adjustment.set_value(adjustment.upper() - adjustment.page_size());
    wait_for_gtk_frames(&view, 2);
    let mut streamed = format!("{text}More output.\n\nAnother paragraph.\n\nFinal paragraph.");
    for i in 0..10 {
        streamed.push_str(&format!("\n\nRapid batch {i}: additional output."));
        renderer.update(&view, &streamed);
        wait_for_gtk_frames(&view, 2);
    }
    wait_for_gtk_frames(&view, 4);
    wait_for_gtk("rapid batches bottom follow", at_bottom);
    assert!(
        (adjustment.value() + adjustment.page_size() - adjustment.upper()).abs() < 3.0,
        "rapid batches must keep following"
    );
    window.destroy();
}
