//! Original subtitle data and editable-text export bookkeeping.
//!
//! The GTK text buffer is intentionally not reconstructed from subtitle segments
//! after a user edit. The renderer preserves those edits when appending audio.
use super::transcript_buffer::TranscriptBuffer;
use crate::transcript::{Segment, Transcript};
use gtk::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

fn count_nonblank(text: &str) -> usize {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
}

fn track_nonblank_chars(buffer: &gtk::TextBuffer) -> Rc<Cell<usize>> {
    let count = Rc::new(Cell::new(count_nonblank(&buffer.text(
        &buffer.start_iter(),
        &buffer.end_iter(),
        true,
    ))));
    // These handlers run before GTK changes the buffer and emits `changed`.
    // Track both user edits and programmatic edits, regardless of dirty state.
    let inserted = count.clone();
    buffer.connect_insert_text(move |_, _, text| {
        inserted.set(inserted.get() + count_nonblank(text));
    });
    let deleted = count.clone();
    buffer.connect_delete_range(move |buffer, start, end| {
        if start.is_start() && end.is_end() {
            deleted.set(0);
        } else if deleted.get() != 0 {
            let removed = buffer.text(start, end, true);
            deleted.set(deleted.get() - count_nonblank(&removed));
        }
    });
    count
}

pub(super) struct TranscriptState {
    original: RefCell<Transcript>,
    provisional: RefCell<String>,
    renderer: RefCell<TranscriptBuffer>,
    nonblank_chars: Rc<Cell<usize>>,
    unexported: Cell<bool>,
    text_edited: Cell<bool>,
    // Kept outside the renderer's RefCell: GTK emits changed synchronously while
    // the renderer is borrowed. The callback must be able to inspect this flag.
    rendering: Cell<bool>,
    file_boundary: Cell<bool>,
}

impl TranscriptState {
    pub(super) fn new(buffer: gtk::TextBuffer) -> Self {
        Self {
            original: RefCell::default(),
            provisional: RefCell::default(),
            nonblank_chars: track_nonblank_chars(&buffer),
            renderer: RefCell::new(TranscriptBuffer::new(buffer)),
            unexported: Cell::new(false),
            text_edited: Cell::new(false),
            rendering: Cell::new(false),
            file_boundary: Cell::new(false),
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
        self.file_boundary.set(false);
    }

    pub(super) fn snapshot(&self, text: String) -> crate::recovery::TranscriptSnapshot {
        crate::recovery::TranscriptSnapshot {
            text,
            original: self.original.borrow().clone(),
            dirty: self.is_dirty() || !self.provisional.borrow().is_empty(),
            text_edited: self.text_edited() || !self.provisional.borrow().is_empty(),
        }
    }

    pub(super) fn restore(&self, snapshot: &crate::recovery::TranscriptSnapshot) {
        let _guard = self.rendering_guard();
        self.renderer
            .borrow_mut()
            .restore(&snapshot.text, snapshot.original.segments.len());
        *self.original.borrow_mut() = snapshot.original.clone();
        self.provisional.borrow_mut().clear();
        self.unexported.set(snapshot.dirty);
        self.text_edited.set(snapshot.text_edited);
    }

    pub(super) fn prepare_append(&self) {
        self.file_boundary.set(false);
        self.renderer.borrow_mut().prepare_append();
    }

    pub(super) fn prepare_file_append(&self) {
        self.file_boundary.set(true);
        self.renderer
            .borrow_mut()
            .prepare_file_append(self.has_text());
    }

    pub(super) fn clear_provisional(&self) {
        self.provisional.borrow_mut().clear();
    }

    pub(super) fn accept_final(&self, segments: Vec<Segment>) -> bool {
        let mut original = self.original.borrow_mut();
        let previous_len = original.segments.len();
        if self.file_boundary.get() {
            original.append_file(segments);
        } else {
            original.append(segments);
        }
        let changed = original.segments.len() > previous_len;
        if changed {
            self.file_boundary.set(false);
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

    pub(super) fn has_text(&self) -> bool {
        self.nonblank_chars.get() != 0
    }

    pub(super) fn has_unsaved_text(&self) -> bool {
        self.has_text() && (self.is_dirty() || !self.provisional.borrow().trim().is_empty())
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
