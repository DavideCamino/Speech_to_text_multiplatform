//! GTK-independent Markdown interpretation. No URLs, images, or HTML are opened.
//!
//! Display and TXT export deliberately retain different layout policies. In
//! particular, TXT preserves the legacy compact spacing and disabled extensions;
//! changing those outputs is not part of the renderer refactor.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Style {
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

/// All offsets are Unicode character offsets, never UTF-8 byte positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) style: Style,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct Document {
    pub(super) text: String,
    pub(super) spans: Vec<Span>,
    pub(super) length: usize,
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

    fn finish(mut self, policy: OutputPolicy) -> Self {
        // Display removes only layout newlines, preserving meaningful code
        // spaces. TXT historically trims all trailing whitespace.
        let end = match policy {
            OutputPolicy::Display => self.text.trim_end_matches('\n').len(),
            OutputPolicy::PlainText => self.text.trim_end().len(),
        };
        self.text.truncate(end);
        self.length = self.text.chars().count();
        for span in &mut self.spans {
            span.end = span.end.min(self.length);
        }
        self.spans.retain(|span| span.start < span.end);
        self.spans
            .sort_by(|a, b| (&a.start, &a.end, &a.style).cmp(&(&b.start, &b.end, &b.style)));
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputPolicy {
    Display,
    PlainText,
}

impl OutputPolicy {
    fn options(self) -> Options {
        match self {
            Self::Display => Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
            Self::PlainText => Options::empty(),
        }
    }
}

#[derive(Default)]
struct DisplayLayout {
    // Unstyled entries are essential: every Start/End pair participates in
    // nesting, even when its tag does not add a visual style.
    open: Vec<Option<(Style, usize)>>,
    lists: Vec<Option<u64>>,
    quote_depth: u8,
}

impl DisplayLayout {
    fn start(&mut self, doc: &mut Document, tag: Tag<'_>) {
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
                self.quote_depth = self.quote_depth.saturating_add(1);
                Some(Style::Quote(self.quote_depth.min(8)))
            }
            Tag::List(start) => {
                doc.breaks(1);
                self.lists.push(start);
                None
            }
            Tag::Item => {
                doc.breaks(1);
                let start = doc.length;
                if let Some(Some(number)) = self.lists.last_mut() {
                    doc.push(&format!("{number}. "));
                    *number = number.saturating_add(1);
                } else {
                    doc.push("• ");
                }
                self.open
                    .push(Some((Style::List(self.lists.len().min(8) as u8), start)));
                return;
            }
            Tag::Link { .. } => Some(Style::Link),
            // Image contents become inert alt text. Destinations are ignored.
            _ => None,
        };
        self.open.push(style.map(|style| (style, doc.length)));
    }

    fn end(&mut self, doc: &mut Document, end: TagEnd) {
        if let Some(Some((style, start))) = self.open.pop() {
            doc.spans.push(Span {
                start,
                end: doc.length,
                style,
            });
        }
        match end {
            TagEnd::Paragraph => doc.breaks(if self.lists.is_empty() { 2 } else { 1 }),
            TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock => doc.breaks(2),
            TagEnd::Item => doc.breaks(1),
            TagEnd::List(_) => {
                self.lists.pop();
                doc.breaks(if self.lists.is_empty() { 2 } else { 1 });
            }
            TagEnd::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
                doc.breaks(2);
            }
            _ => {}
        }
    }
}

pub(super) fn parse_display(markdown: &str) -> Document {
    parse(markdown, OutputPolicy::Display)
}

pub(super) fn plain_text(markdown: &str) -> String {
    parse(markdown, OutputPolicy::PlainText).text
}

fn parse(markdown: &str, policy: OutputPolicy) -> Document {
    let display = policy == OutputPolicy::Display;
    let mut doc = Document::default();
    let mut layout = DisplayLayout::default();
    for event in Parser::new_ext(markdown, policy.options()) {
        match event {
            Event::Start(tag) if display => layout.start(&mut doc, tag),
            Event::End(end) if display => layout.end(&mut doc, end),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::BlockQuote(_),
            ) if !doc.text.ends_with('\n') => doc.push("\n"),
            Event::Text(text) => doc.push(&text),
            Event::Html(text) | Event::InlineHtml(text) if display => doc.push(&text),
            Event::Code(text) => {
                let start = doc.length;
                doc.push(&text);
                if display {
                    doc.spans.push(Span {
                        start,
                        end: doc.length,
                        style: Style::Code,
                    });
                }
            }
            Event::SoftBreak | Event::HardBreak => doc.push("\n"),
            Event::Rule => {
                if display {
                    doc.breaks(2);
                    doc.push("────────");
                    doc.breaks(2);
                } else {
                    doc.push("\n");
                }
            }
            Event::TaskListMarker(checked) if display => {
                doc.push(if checked { "☑ " } else { "☐ " })
            }
            _ => {}
        }
    }
    doc.finish(policy)
}

#[cfg(test)]
mod tests {
    use super::{Style, parse_display, plain_text};

    const POLICY_CASES: &[(&str, &str, &str)] = &[
        ("", "", ""),
        (
            "# Heading\n\n**bold** and `code`",
            "Heading\n\nbold and code",
            "Heading\nbold and code",
        ),
        (
            "first\nline\n\nnext  \nline",
            "first\nline\n\nnext\nline",
            "first\nline\nnext\nline",
        ),
        // Legacy TXT does not insert a break when entering a nested tight list.
        (
            "- one\n- two\n  - nested\n\n3. third\n4. fourth",
            "• one\n• two\n• nested\n\n3. third\n4. fourth",
            "one\ntwonested\nthird\nfourth",
        ),
        (
            "- [x] done\n- [ ] todo\n\n~~gone~~",
            "• ☑ done\n• ☐ todo\n\ngone",
            "[x] done\n[ ] todo\n~~gone~~",
        ),
        (
            "> quote\n\n---\n\nafter",
            "quote\n\n────────\n\nafter",
            "quote\n\nafter",
        ),
        ("```rust\nlet x = 1;  \n```", "let x = 1;  ", "let x = 1;"),
        (
            "[label](https://example.com) ![alt](file:///secret)",
            "label alt",
            "label alt",
        ),
        ("<b>literal</b>", "<b>literal</b>", "literal"),
        ("<script>hidden</script>", "<script>hidden</script>", ""),
        ("**Caffè ☕ 世界", "**Caffè ☕ 世界", "**Caffè ☕ 世界"),
        ("**Caffè ☕ 世界**", "Caffè ☕ 世界", "Caffè ☕ 世界"),
        ("```rust\nfn main()", "fn main()", "fn main()"),
    ];

    #[test]
    fn display_output_policy() {
        for &(markdown, display, _) in POLICY_CASES {
            assert_eq!(parse_display(markdown).text, display, "{markdown:?}");
        }
    }

    #[test]
    fn plain_text_output_policy() {
        for &(markdown, _, plain) in POLICY_CASES {
            assert_eq!(plain_text(markdown), plain, "{markdown:?}");
        }
    }

    #[test]
    fn plain_text_export() {
        assert_eq!(
            plain_text("# Heading\n\n**bold** and `code`"),
            "Heading\nbold and code"
        );
        assert_eq!(
            plain_text("![alt](file:///secret) [label](https://example.com)"),
            "alt label"
        );
    }

    #[test]
    fn parses_basic_formatting_lists_unicode_and_inert_content() {
        let doc = parse_display(
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
        let unicode = parse_display("**Caffè ☕ 世界**");
        assert_eq!(unicode.text, "Caffè ☕ 世界");
        assert_eq!(unicode.spans[0].end, unicode.text.chars().count());
    }

    #[test]
    fn streamed_documents_have_valid_character_spans() {
        for &(markdown, _, _) in POLICY_CASES {
            for end in markdown.char_indices().map(|(i, c)| i + c.len_utf8()) {
                let doc = parse_display(&markdown[..end]);
                assert_eq!(doc.length, doc.text.chars().count());
                for span in &doc.spans {
                    assert!(span.start < span.end && span.end <= doc.length);
                }
                assert!(doc.spans.windows(2).all(|pair| {
                    (&pair[0].start, &pair[0].end, &pair[0].style)
                        <= (&pair[1].start, &pair[1].end, &pair[1].style)
                }));
            }
        }
    }
}
