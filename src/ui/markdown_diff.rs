//! Pure incremental text/tag update planning in Unicode character offsets.
use super::markdown_document::Document;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UpdatePlan {
    pub(super) start: usize,
    pub(super) old_end: usize,
    pub(super) replacement: String,
    pub(super) retag_from: usize,
    new_end: usize,
}

impl UpdatePlan {
    pub(super) fn between(old: &Document, next: &Document) -> Self {
        let old_text: Vec<_> = old.text.chars().collect();
        let new_text: Vec<_> = next.text.chars().collect();
        let prefix = old_text
            .iter()
            .zip(&new_text)
            .take_while(|(a, b)| a == b)
            .count();
        // Excluding the prefix prevents an overlapping suffix for short edits.
        let suffix = old_text[prefix..]
            .iter()
            .rev()
            .zip(new_text[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let old_end = old_text.len() - suffix;
        let new_end = new_text.len() - suffix;

        // Text can stay unchanged when formatting changes (e.g. closing **).
        // A growing span retains its prefix formatting, so retag from its old
        // end rather than invalidating the entire styled region.
        let mut retag_from = prefix;
        for index in 0..old.spans.len().max(next.spans.len()) {
            let a = old.spans.get(index);
            let b = next.spans.get(index);
            if a != b {
                if let (Some(a), Some(b)) = (a, b)
                    && a.start == b.start
                    && a.style == b.style
                {
                    retag_from = retag_from.min(a.end.min(b.end));
                } else {
                    if let Some(span) = a {
                        retag_from = retag_from.min(span.start);
                    }
                    if let Some(span) = b {
                        retag_from = retag_from.min(span.start);
                    }
                }
            }
        }
        Self {
            start: prefix,
            old_end,
            new_end,
            replacement: new_text[prefix..new_end].iter().collect(),
            retag_from,
        }
    }

    pub(super) fn changes_text(&self) -> bool {
        self.start != self.old_end || self.start != self.new_end
    }

    /// Preserve endpoints in the stable prefix/suffix and clamp endpoints
    /// inside the replacement to its new length. Call separately for insert and
    /// bound marks so a reversed selection keeps its direction.
    pub(super) fn map_position(&self, offset: usize) -> usize {
        if offset <= self.start {
            offset
        } else if offset >= self.old_end {
            self.new_end + offset - self.old_end
        } else {
            self.start + (offset - self.start).min(self.new_end - self.start)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UpdatePlan;
    use crate::ui::markdown_document::{Document, Span, Style, parse_display};

    fn text(value: &str) -> Document {
        Document {
            text: value.into(),
            length: value.chars().count(),
            spans: Vec::new(),
        }
    }

    fn apply(old: &str, plan: &UpdatePlan) -> String {
        old.chars()
            .take(plan.start)
            .chain(plan.replacement.chars())
            .chain(old.chars().skip(plan.old_end))
            .collect()
    }

    #[test]
    fn replacement_ranges_and_unicode_offsets() {
        for (old, new, range, replacement) in [
            ("", "", (0, 0), ""),
            ("same", "same", (4, 4), ""),
            ("abc", "abcd", (3, 3), "d"),
            ("abc", "a☕bc", (1, 1), "☕"),
            ("a☕bc", "abc", (1, 2), ""),
            ("a☕世z", "a日本z", (1, 3), "日本"),
            ("aaaa", "aa", (2, 4), ""),
            ("aa", "aaaa", (2, 2), "aa"),
            ("all", "", (0, 3), ""),
        ] {
            let plan = UpdatePlan::between(&text(old), &text(new));
            assert_eq!((plan.start, plan.old_end), range, "{old:?} -> {new:?}");
            assert_eq!(plan.replacement, replacement);
            assert_eq!(plan.changes_text(), old != new);
            assert_eq!(apply(old, &plan), new);
        }
    }

    #[test]
    fn selection_endpoints_map_before_inside_and_after_replacement() {
        let plan = UpdatePlan::between(&text("ab12345yz"), &text("ab☕世yz"));
        assert_eq!(
            (0..=9)
                .map(|offset| plan.map_position(offset))
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 4, 4, 4, 5, 6]
        );
        assert_eq!((plan.map_position(9), plan.map_position(0)), (6, 0));
        let insertion = UpdatePlan::between(&text("ab"), &text("a世界b"));
        assert_eq!(
            insertion.map_position(1),
            1,
            "prefix boundary keeps left gravity"
        );
        assert_eq!(insertion.map_position(2), 4);
    }

    #[test]
    fn formatting_changes_are_independent_of_text_edits() {
        let old = text("abcdef");
        let mut next = text("abcdef");
        next.spans.push(Span {
            start: 0,
            end: 2,
            style: Style::Bold,
        });
        let plan = UpdatePlan::between(&old, &next);
        assert!(!plan.changes_text());
        assert_eq!(plan.retag_from, 0);
        let mut grown = text("abcdef");
        grown.spans.push(Span {
            start: 0,
            end: 4,
            style: Style::Bold,
        });
        assert_eq!(UpdatePlan::between(&next, &grown).retag_from, 2);
        assert_eq!(UpdatePlan::between(&grown, &next).retag_from, 2);
        assert_eq!(UpdatePlan::between(&grown, &old).retag_from, 0);
        grown.spans[0].style = Style::Italic;
        assert_eq!(UpdatePlan::between(&next, &grown).retag_from, 0);
        grown.spans[0].start = 1;
        assert_eq!(UpdatePlan::between(&old, &grown).retag_from, 1);
    }

    #[test]
    fn short_unicode_edits_always_reconstruct_the_target() {
        let mut values = vec![String::new()];
        for _ in 0..3 {
            let previous = values.clone();
            for prefix in previous {
                for suffix in ['a', 'b', '☕', '世'] {
                    let value = format!("{prefix}{suffix}");
                    if !values.contains(&value) {
                        values.push(value);
                    }
                }
            }
        }
        for old in &values {
            for new in &values {
                let plan = UpdatePlan::between(&text(old), &text(new));
                assert_eq!(apply(old, &plan), *new);
                let positions: Vec<_> = (0..=old.chars().count())
                    .map(|offset| plan.map_position(offset))
                    .collect();
                assert!(
                    positions
                        .iter()
                        .all(|offset| *offset <= new.chars().count())
                );
                assert!(positions.windows(2).all(|pair| pair[0] <= pair[1]));
            }
        }
    }

    #[test]
    fn streamed_markdown_plans_preserve_stable_prefixes() {
        for markdown in [
            "**Caffè ☕ 世界**",
            "[reference][id]\n\n[id]: https://example.com",
            "- [x] **done**\n\n```rust\nfn main() {}\n```",
        ] {
            let mut old = Document::default();
            for end in markdown.char_indices().map(|(i, c)| i + c.len_utf8()) {
                let next = parse_display(&markdown[..end]);
                let plan = UpdatePlan::between(&old, &next);
                assert_eq!(apply(&old.text, &plan), next.text);
                assert!(plan.retag_from <= plan.start);
                old = next;
            }
        }
    }
}
