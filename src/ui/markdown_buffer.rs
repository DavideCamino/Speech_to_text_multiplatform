//! Markdown rendered into one native text buffer. Raw Markdown stays in the
//! rewrite state; this module owns only its display and never opens URLs/files.
use adw::prelude::*;
use gtk::{glib, pango};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{cell::Cell, collections::BTreeSet, rc::Rc};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Style {
    Heading(u8),
    Bold,
    Italic,
    Strike,
    Code,
    CodeBlock,
    Quote(u8),
    List(u8),
    Link,
}
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct Span {
    start: usize,
    end: usize,
    style: Style,
}
#[derive(Default, Debug, PartialEq, Eq)]
struct Document {
    text: String,
    spans: Vec<Span>,
    length: usize,
}
impl Document {
    fn push(&mut self, text: &str) {
        self.length += text.chars().count();
        self.text.push_str(text);
    }
    fn breaks(&mut self, count: usize) {
        if self.text.is_empty() {
            return;
        }
        let existing = self.text.chars().rev().take_while(|c| *c == '\n').count();
        for _ in existing..count {
            self.push("\n");
        }
    }
}

fn parse(markdown: &str) -> Document {
    let mut doc = Document::default();
    let mut open = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0_u8;
    for event in Parser::new_ext(
        markdown,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        match event {
            Event::Start(tag) => {
                let end = tag.to_end();
                let style = match tag {
                    Tag::Heading { level, .. } => {
                        doc.breaks(2);
                        Some(Style::Heading(level as u8))
                    }
                    Tag::Strong => Some(Style::Bold),
                    Tag::Emphasis => Some(Style::Italic),
                    Tag::Strikethrough => Some(Style::Strike),
                    Tag::CodeBlock(_) => {
                        doc.breaks(2);
                        Some(Style::CodeBlock)
                    }
                    Tag::BlockQuote(_) => {
                        doc.breaks(2);
                        quote_depth = quote_depth.saturating_add(1);
                        Some(Style::Quote(quote_depth.min(8)))
                    }
                    Tag::List(start) => {
                        doc.breaks(1);
                        lists.push(start);
                        None
                    }
                    Tag::Item => {
                        doc.breaks(1);
                        let start = doc.length;
                        if let Some(Some(number)) = lists.last_mut() {
                            doc.push(&format!("{number}. "));
                            *number = number.saturating_add(1);
                        } else {
                            doc.push("• ");
                        }
                        open.push((end, Some((Style::List(lists.len().min(8) as u8), start))));
                        continue;
                    }
                    Tag::Link { .. } => Some(Style::Link),
                    // Image contents become inert alt text; HTML is displayed
                    // literally below, never interpreted as markup.
                    _ => None,
                };
                open.push((end, style.map(|style| (style, doc.length))));
            }
            Event::End(end) => {
                if let Some((_, Some((style, start)))) = open.pop() {
                    doc.spans.push(Span {
                        start,
                        end: doc.length,
                        style,
                    });
                }
                match end {
                    TagEnd::Paragraph => doc.breaks(if lists.is_empty() { 2 } else { 1 }),
                    TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock => doc.breaks(2),
                    TagEnd::Item => doc.breaks(1),
                    TagEnd::List(_) => {
                        lists.pop();
                        doc.breaks(if lists.is_empty() { 2 } else { 1 });
                    }
                    TagEnd::BlockQuote(_) => {
                        quote_depth = quote_depth.saturating_sub(1);
                        doc.breaks(2);
                    }
                    _ => {}
                }
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => doc.push(&text),
            Event::Code(text) => {
                let start = doc.length;
                doc.push(&text);
                doc.spans.push(Span {
                    start,
                    end: doc.length,
                    style: Style::Code,
                });
            }
            Event::SoftBreak | Event::HardBreak => doc.push("\n"),
            Event::Rule => {
                doc.breaks(2);
                doc.push("────────");
                doc.breaks(2);
            }
            Event::TaskListMarker(checked) => doc.push(if checked { "☑ " } else { "☐ " }),
            _ => {}
        }
    }
    // Remove only layout newlines, not meaningful spaces within text/code.
    doc.text.truncate(doc.text.trim_end_matches('\n').len());
    let length = doc.text.chars().count();
    doc.length = length;
    for span in &mut doc.spans {
        span.end = span.end.min(length);
    }
    doc.spans.retain(|span| span.start < span.end);
    doc.spans
        .sort_by(|a, b| (&a.start, &a.end, &a.style).cmp(&(&b.start, &b.end, &b.style)));
    doc
}

struct ScrollAnchor {
    buffer: gtk::TextBuffer,
    mark: gtk::TextMark,
}
impl Drop for ScrollAnchor {
    fn drop(&mut self) {
        self.buffer.delete_mark(&self.mark);
    }
}

pub(super) struct MarkdownBuffer {
    buffer: gtk::TextBuffer,
    rendered: Document,
    revision: Rc<Cell<u64>>,
    scroll_tick: Option<gtk::TickCallbackId>,
}

impl Drop for MarkdownBuffer {
    fn drop(&mut self) {
        self.cancel_scroll();
    }
}
impl MarkdownBuffer {
    pub fn new(view: &gtk::TextView) -> Self {
        Self {
            buffer: view.buffer(),
            rendered: Document::default(),
            revision: Rc::new(Cell::new(0)),
            scroll_tick: None,
        }
    }

    fn cancel_scroll(&mut self) {
        if let Some(tick) = self.scroll_tick.take() {
            tick.remove();
        }
    }

    pub fn reset(&mut self) {
        self.cancel_scroll();
        self.revision.set(self.revision.get() + 1);
        self.buffer.set_text("");
        self.rendered = Document::default();
    }

    pub fn update(&mut self, view: &gtk::TextView, markdown: &str) {
        let next = parse(markdown);
        if next == self.rendered {
            return;
        }
        self.cancel_scroll();
        let old: Vec<_> = self.rendered.text.chars().collect();
        let new: Vec<_> = next.text.chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let old_end = old.len() - suffix;
        let new_end = new.len() - suffix;
        let position = |offset: i32| -> i32 {
            let offset = offset as usize;
            if offset <= prefix {
                offset as i32
            } else if offset >= old_end {
                (new_end + offset - old_end) as i32
            } else {
                (prefix + (offset - prefix).min(new_end - prefix)) as i32
            }
        };
        let selection = (
            self.buffer.iter_at_mark(&self.buffer.get_insert()).offset(),
            self.buffer
                .iter_at_mark(&self.buffer.selection_bound())
                .offset(),
        );
        // Preserve a viewport anchor; do not force users back to the bottom
        // when they scroll up to read or select earlier output.
        let scroll = view.vadjustment().map(|adjustment| {
            let value = adjustment.value();
            let bottom = value + adjustment.page_size() >= adjustment.upper() - 2.0;
            let rect = view.visible_rect();
            let iter = if bottom {
                self.buffer.end_iter()
            } else {
                view.iter_at_location(rect.x(), rect.y())
                    .unwrap_or_else(|| self.buffer.start_iter())
            };
            (
                self.buffer.create_mark(None, &iter, !bottom),
                bottom,
                value,
                iter.offset() as usize,
            )
        });
        self.buffer.begin_user_action();
        if old_end != prefix || new_end != prefix {
            self.buffer.delete(
                &mut self.buffer.iter_at_offset(prefix as i32),
                &mut self.buffer.iter_at_offset(old_end as i32),
            );
            self.buffer.insert(
                &mut self.buffer.iter_at_offset(prefix as i32),
                &new[prefix..new_end].iter().collect::<String>(),
            );
        }
        // Retag only the affected suffix. Formatting can change without a text
        // change (e.g. a closing ** arrives), so compare spans independently.
        let mut retag = prefix;
        for index in 0..self.rendered.spans.len().max(next.spans.len()) {
            let a = self.rendered.spans.get(index);
            let b = next.spans.get(index);
            if a != b {
                if let (Some(a), Some(b)) = (a, b)
                    && a.start == b.start
                    && a.style == b.style
                {
                    // A growing span has unchanged formatting in its prefix.
                    retag = retag.min(a.end.min(b.end));
                } else {
                    if let Some(span) = a {
                        retag = retag.min(span.start);
                    }
                    if let Some(span) = b {
                        retag = retag.min(span.start);
                    }
                }
            }
        }
        let styles: BTreeSet<_> = self
            .rendered
            .spans
            .iter()
            .chain(&next.spans)
            .map(|s| s.style.clone())
            .collect();
        for style in styles {
            let tag = style.tag(&self.buffer);
            self.buffer.remove_tag(
                &tag,
                &self.buffer.iter_at_offset(retag as i32),
                &self.buffer.end_iter(),
            );
        }
        for span in &next.spans {
            if span.end > retag {
                self.buffer.apply_tag(
                    &span.style.tag(&self.buffer),
                    &self.buffer.iter_at_offset(span.start.max(retag) as i32),
                    &self.buffer.iter_at_offset(span.end as i32),
                );
            }
        }
        self.buffer.select_range(
            &self.buffer.iter_at_offset(position(selection.0)),
            &self.buffer.iter_at_offset(position(selection.1)),
        );
        self.buffer.end_user_action();
        self.rendered = next;
        let revision = self.revision.get() + 1;
        self.revision.set(revision);
        if let Some((mark, bottom, previous_value, anchor)) = scroll {
            if !bottom && prefix >= anchor && retag >= anchor {
                // Appending below the viewport needs no scrolling at all;
                // retaining the adjustment also preserves partial-line pixels.
                self.buffer.delete_mark(&mark);
                return;
            }
            let anchor = ScrollAnchor {
                buffer: self.buffer.clone(),
                mark,
            };
            let latest = self.revision.clone();
            let expected_value = Cell::new(previous_value);
            let previous_bounds = Cell::new(view.vadjustment().map(|a| (a.upper(), a.page_size())));
            let settled_frames = Cell::new(0);
            let started = Cell::new(None::<std::time::Instant>);
            self.scroll_tick = Some(view.add_tick_callback(move |view, _| {
                let Some(adjustment) = view.vadjustment() else {
                    return glib::ControlFlow::Break;
                };
                if latest.get() != revision {
                    return glib::ControlFlow::Break;
                }
                let bounds = (adjustment.upper(), adjustment.page_size());
                // A shrinking scroll range can clamp the value without user
                // input. Growing bounds, however, must not hide a reader's
                // upward scroll between insertion and this frame.
                let clamped = previous_bounds
                    .get()
                    .is_some_and(|(upper, page)| bounds.0 - bounds.1 < upper - page - 2.0);
                if !clamped && adjustment.value() < expected_value.get() - 2.0 {
                    return glib::ControlFlow::Break;
                }
                if !view.is_mapped() || view.height() <= 0 || adjustment.page_size() <= 0.0 {
                    return glib::ControlFlow::Continue;
                }
                if !bottom {
                    view.scroll_to_mark(&anchor.mark, 0.0, true, 0.0, 0.0);
                    return glib::ControlFlow::Break;
                }
                // Validate the actual end of the document, not just GTK's
                // provisional adjustment bounds. Under Xvfb several frames can
                // have identical bounds before the first text layout completes.
                let start = started.get().unwrap_or_else(|| {
                    let now = std::time::Instant::now();
                    started.set(Some(now));
                    now
                });
                let end = view.iter_location(&anchor.buffer.end_iter());
                // Set the adjustment directly to avoid animations that lag
                // behind rapid streamed batches and look like reader scrolling.
                let upper = adjustment.upper();
                let page = adjustment.page_size();
                adjustment.set_value((upper - page).max(0.0));
                expected_value.set(adjustment.value());
                let visible = view.visible_rect();
                let at_end = end.y() + end.height() <= visible.y() + visible.height() + 2
                    && (adjustment.value() + page - upper).abs() < 2.0;
                let unchanged = previous_bounds.replace(Some((upper, page))) == Some((upper, page));
                settled_frames.set(if at_end && unchanged {
                    settled_frames.get() + 1
                } else {
                    0
                });
                // Use elapsed time only as a safety bound, never a small frame
                // count as proof that asynchronous layout has finished.
                if settled_frames.get() >= 2 || start.elapsed() >= std::time::Duration::from_secs(2)
                {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            }));
        }
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
            let expected = parse(prefix);
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

/// Pump GTK until a real condition holds; fail rather than silently skipping
/// layout work on a slow/headless runner.
#[cfg(test)]
pub(super) fn wait_for_gtk(description: &str, mut condition: impl FnMut() -> bool) {
    let context = glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        context.iteration(false);
        if condition() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[cfg(test)]
fn wait_for_gtk_frames(view: &gtk::TextView, count: u32) {
    let frames = Rc::new(Cell::new(0));
    let observed = frames.clone();
    view.add_tick_callback(move |_, _| {
        observed.set(observed.get() + 1);
        if observed.get() >= count {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
    wait_for_gtk("GTK frames", || frames.get() >= count);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_basic_formatting_lists_unicode_and_inert_content() {
        let doc = parse(
            "# Heading\n\n**bold** *italic* ~~gone~~ `code`\n\n- one\n- two\n  - nested\n\n3. third\n4. fourth\n\n> quote\n\n```rust\nlet x = 1;\n```\n\n[link](https://example.com) ![alt](file:///secret)\n\n<script>never executed</script>",
        );
        assert!(doc.text.starts_with("Heading\n\nbold italic gone code"));
        assert!(doc.text.contains("• one\n• two\n• nested"));
        assert!(doc.text.contains("3. third\n4. fourth"));
        assert!(doc.text.contains("link alt"));
        assert!(doc.text.contains("<script>never executed</script>"));
        for style in [
            Style::Heading(1),
            Style::Bold,
            Style::Italic,
            Style::Strike,
            Style::Code,
            Style::CodeBlock,
            Style::List(2),
            Style::Quote(1),
            Style::Link,
        ] {
            assert!(
                doc.spans.iter().any(|span| span.style == style),
                "missing {style:?}"
            );
        }
        let unicode = parse("**Caffè ☕ 世界**");
        assert_eq!(unicode.text, "Caffè ☕ 世界");
        assert_eq!(unicode.spans[0].end, unicode.text.chars().count());
    }
}
