//! Incremental rendering: finalized text is never rewritten during a job.
use crate::transcript::Transcript;
use gtk::prelude::*;

pub(super) struct TranscriptBuffer {
    buffer: gtk::TextBuffer,
    // Left gravity keeps the mark before new text until we explicitly move it.
    finalized_end: gtk::TextMark,
    rendered_segments: usize,
    provisional: String,
    // Keep boundary whitespace without displaying trailing whitespace. It is
    // emitted only when another finalized/provisional fragment follows it.
    trailing_whitespace: String,
}

impl TranscriptBuffer {
    pub fn new(buffer: gtk::TextBuffer) -> Self {
        let finalized_end = buffer.create_mark(None, &buffer.end_iter(), true);
        Self {
            buffer,
            finalized_end,
            rendered_segments: 0,
            provisional: String::new(),
            trailing_whitespace: String::new(),
        }
    }

    /// Only a deliberate transcript replacement may clear the entire buffer.
    pub fn reset(&mut self) {
        self.buffer.set_text("");
        self.buffer
            .move_mark(&self.finalized_end, &self.buffer.end_iter());
        self.rendered_segments = 0;
        self.provisional.clear();
        self.trailing_whitespace.clear();
    }

    pub fn update(&mut self, transcript: &Transcript, provisional: &str) {
        let new_segments = &transcript.segments[self.rendered_segments..];
        if new_segments.is_empty() && self.provisional == provisional {
            return;
        }
        let new_text = new_segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>();
        self.rendered_segments = transcript.segments.len();
        if new_text.is_empty() && self.provisional == provisional {
            return;
        }

        let boundary = self.buffer.iter_at_mark(&self.finalized_end).offset();
        // GTK's selection marks have right gravity. Explicitly preserve a
        // selection in finalized text, including one ending at the boundary,
        // so appending does not expand it. Keep its direction as well.
        let selection = self.buffer.selection_bounds().and_then(|(_, end)| {
            (end.offset() <= boundary).then(|| {
                (
                    self.buffer.iter_at_mark(&self.buffer.get_insert()).offset(),
                    self.buffer
                        .iter_at_mark(&self.buffer.selection_bound())
                        .offset(),
                )
            })
        });
        self.buffer.begin_user_action();
        if !self.provisional.is_empty() {
            let mut start = self.buffer.iter_at_mark(&self.finalized_end);
            let mut end = self.buffer.end_iter();
            self.buffer.delete(&mut start, &mut end);
        }
        if !new_text.is_empty() {
            let mut text = std::mem::take(&mut self.trailing_whitespace);
            text.push_str(&new_text);
            let text = if boundary == 0 {
                text.trim_start()
            } else {
                &text
            };
            let committed = text.trim_end();
            self.trailing_whitespace.push_str(&text[committed.len()..]);
            let mut end = self.buffer.end_iter();
            self.buffer.insert(&mut end, committed);
            // Plain insertions can inherit adjacent tags. Finalized text must
            // never retain the provisional foreground tag.
            let start = self.buffer.iter_at_mark(&self.finalized_end);
            self.buffer.remove_tag_by_name("provisional", &start, &end);
            self.buffer.move_mark(&self.finalized_end, &end);
        }
        if !provisional.is_empty() {
            let mut end = self.buffer.end_iter();
            let text = format!("{}{provisional}", self.trailing_whitespace);
            let text = if end.offset() == 0 {
                text.trim_start()
            } else {
                &text
            };
            self.buffer
                .insert_with_tags_by_name(&mut end, text.trim_end(), &["provisional"]);
        }
        self.provisional.clear();
        self.provisional.push_str(provisional);
        if let Some((insert, bound)) = selection {
            self.buffer.select_range(
                &self.buffer.iter_at_offset(insert),
                &self.buffer.iter_at_offset(bound),
            );
        }
        self.buffer.end_user_action();
    }
}
