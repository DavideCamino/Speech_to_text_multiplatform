//! Original subtitle data and editable-text export bookkeeping.
//!
//! The GTK text buffer is intentionally not reconstructed from subtitle segments
//! after a user edit. The renderer preserves those edits when appending audio.
use super::transcript_buffer::TranscriptBuffer;
use crate::transcript::{Segment, Transcript};
use std::cell::{Cell, RefCell};

pub(super) struct TranscriptState {
    original: RefCell<Transcript>,
    provisional: RefCell<String>,
    renderer: RefCell<TranscriptBuffer>,
    unexported: Cell<bool>,
    text_edited: Cell<bool>,
    // Kept outside the renderer's RefCell: GTK emits changed synchronously while
    // the renderer is borrowed. The callback must be able to inspect this flag.
    rendering: Cell<bool>,
}

impl TranscriptState {
    pub(super) fn new(buffer: gtk::TextBuffer) -> Self {
        Self {
            original: RefCell::default(),
            provisional: RefCell::default(),
            renderer: RefCell::new(TranscriptBuffer::new(buffer)),
            unexported: Cell::new(false),
            text_edited: Cell::new(false),
            rendering: Cell::new(false),
        }
    }

    pub(super) fn render(&self) {
        let _guard = self.rendering_guard();
        self.renderer
            .borrow_mut()
            .update(&self.original.borrow(), &self.provisional.borrow());
    }

    pub(super) fn reset(&self) {
        let _guard = self.rendering_guard();
        *self.original.borrow_mut() = Transcript::default();
        self.renderer.borrow_mut().reset();
        self.provisional.borrow_mut().clear();
        self.unexported.set(false);
        self.text_edited.set(false);
    }

    pub(super) fn prepare_append(&self) {
        self.renderer.borrow_mut().prepare_append();
    }

    pub(super) fn clear_provisional(&self) {
        self.provisional.borrow_mut().clear();
    }

    pub(super) fn accept_final(&self, segments: Vec<Segment>) -> bool {
        let mut original = self.original.borrow_mut();
        let previous_len = original.segments.len();
        original.append(segments);
        let changed = original.segments.len() > previous_len;
        if changed {
            self.unexported.set(true);
        }
        changed
    }

    pub(super) fn set_provisional(&self, text: String) -> bool {
        let mut provisional = self.provisional.borrow_mut();
        if *provisional == text {
            return false;
        }
        *provisional = text;
        true
    }

    /// Returns false for programmatic rendering, without borrowing the renderer.
    pub(super) fn record_user_edit(&self) -> bool {
        if self.rendering.get() {
            return false;
        }
        self.unexported.set(true);
        self.text_edited.set(true);
        true
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.unexported.get()
    }

    pub(super) fn text_edited(&self) -> bool {
        self.text_edited.get()
    }

    pub(super) fn has_segments(&self) -> bool {
        !self.original.borrow().segments.is_empty()
    }

    pub(super) fn end_ms(&self) -> u64 {
        self.original.borrow().end_ms()
    }

    pub(super) fn srt(&self) -> String {
        self.original.borrow().srt()
    }

    pub(super) fn mark_text_saved(&self, current: &str, snapshot: &str) {
        if current == snapshot {
            self.unexported.set(false);
            self.text_edited.set(false);
        }
    }

    pub(super) fn mark_subtitles_saved(&self, snapshot: &str) {
        if !self.text_edited() && self.srt() == snapshot {
            self.unexported.set(false);
        }
    }

    fn rendering_guard(&self) -> RenderingGuard<'_> {
        let previous = self.rendering.replace(true);
        RenderingGuard {
            flag: &self.rendering,
            previous,
        }
    }
}

// Restore even if rendering unwinds, and preserve an outer rendering guard.
struct RenderingGuard<'a> {
    flag: &'a Cell<bool>,
    previous: bool,
}

impl Drop for RenderingGuard<'_> {
    fn drop(&mut self) {
        self.flag.set(self.previous);
    }
}
