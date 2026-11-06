//! Viewport anchoring and deferred bottom-follow for streamed Markdown.
//! Owns temporary marks and tick callbacks, but never parses or edits text.
use adw::prelude::*;
use gtk::glib;
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

const SCROLL_TOLERANCE: f64 = 2.0;
const SETTLED_FRAMES: u32 = 2;
const LAYOUT_SAFETY_BOUND: Duration = Duration::from_secs(2);

pub(super) struct ViewportAnchor {
    buffer: gtk::TextBuffer,
    mark: gtk::TextMark,
    bottom: bool,
    previous_value: f64,
    offset: usize,
}

impl Drop for ViewportAnchor {
    fn drop(&mut self) {
        self.buffer.delete_mark(&self.mark);
    }
}

#[derive(Default)]
pub(super) struct ScrollFollow {
    revision: Rc<Cell<u64>>,
    tick: Option<gtk::TickCallbackId>,
}

impl Drop for ScrollFollow {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl ScrollFollow {
    fn cancel(&mut self) {
        if let Some(tick) = self.tick.take() {
            tick.remove();
        }
    }

    pub(super) fn reset(&mut self) {
        self.cancel();
        self.revision.set(self.revision.get() + 1);
    }

    /// Capture before editing, replacing any follow queued for an older update.
    pub(super) fn capture(
        &mut self,
        view: &gtk::TextView,
        buffer: &gtk::TextBuffer,
    ) -> Option<ViewportAnchor> {
        self.cancel();
        view.vadjustment().map(|adjustment| {
            let value = adjustment.value();
            let bottom = value + adjustment.page_size() >= adjustment.upper() - SCROLL_TOLERANCE;
            let rect = view.visible_rect();
            let iter = if bottom {
                buffer.end_iter()
            } else {
                view.iter_at_location(rect.x(), rect.y())
                    .unwrap_or_else(|| buffer.start_iter())
            };
            ViewportAnchor {
                buffer: buffer.clone(),
                mark: buffer.create_mark(None, &iter, !bottom),
                bottom,
                previous_value: value,
                offset: iter.offset() as usize,
            }
        })
    }

    pub(super) fn restore(
        &mut self,
        view: &gtk::TextView,
        anchor: Option<ViewportAnchor>,
        text_changed_from: usize,
        tags_changed_from: usize,
    ) {
        let revision = self.revision.get() + 1;
        self.revision.set(revision);
        let Some(anchor) = anchor else {
            return;
        };
        if !anchor.bottom
            && text_changed_from >= anchor.offset
            && tags_changed_from >= anchor.offset
        {
            // Appending below the viewport needs no scrolling. Retaining the
            // adjustment also preserves partial-line pixels. Drop removes mark.
            return;
        }
        let latest = self.revision.clone();
        let expected_value = Cell::new(anchor.previous_value);
        let previous_bounds = Cell::new(view.vadjustment().map(|a| (a.upper(), a.page_size())));
        let settled_frames = Cell::new(0);
        let started = Cell::new(None::<Instant>);
        self.tick = Some(view.add_tick_callback(move |view, _| {
            let Some(adjustment) = view.vadjustment() else {
                return glib::ControlFlow::Break;
            };
            if latest.get() != revision {
                return glib::ControlFlow::Break;
            }
            let bounds = (adjustment.upper(), adjustment.page_size());
            // A shrinking scroll range can clamp the value without user input.
            // Growing bounds must not hide upward scrolling before this frame.
            let clamped = previous_bounds
                .get()
                .is_some_and(|(upper, page)| bounds.0 - bounds.1 < upper - page - SCROLL_TOLERANCE);
            if !clamped && adjustment.value() < expected_value.get() - SCROLL_TOLERANCE {
                return glib::ControlFlow::Break;
            }
            if !view.is_mapped() || view.height() <= 0 || adjustment.page_size() <= 0.0 {
                return glib::ControlFlow::Continue;
            }
            if !anchor.bottom {
                view.scroll_to_mark(&anchor.mark, 0.0, true, 0.0, 0.0);
                return glib::ControlFlow::Break;
            }
            // Validate the actual document end, not provisional adjustment
            // bounds: several identical frames may precede initial text layout.
            let start = started.get().unwrap_or_else(|| {
                let now = Instant::now();
                started.set(Some(now));
                now
            });
            let end = view.iter_location(&anchor.buffer.end_iter());
            // Direct adjustment avoids animations lagging behind streamed batches
            // and being mistaken for the reader scrolling upward.
            let upper = adjustment.upper();
            let page = adjustment.page_size();
            adjustment.set_value((upper - page).max(0.0));
            expected_value.set(adjustment.value());
            let visible = view.visible_rect();
            let at_end = end.y() + end.height()
                <= visible.y() + visible.height() + SCROLL_TOLERANCE as i32
                && (adjustment.value() + page - upper).abs() < SCROLL_TOLERANCE;
            let unchanged = previous_bounds.replace(Some((upper, page))) == Some((upper, page));
            settled_frames.set(if at_end && unchanged {
                settled_frames.get() + 1
            } else {
                0
            });
            // Time is a safety bound, never a substitute for layout settling.
            if settled_frames.get() >= SETTLED_FRAMES || start.elapsed() >= LAYOUT_SAFETY_BOUND {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        }));
    }
}

/// Invoked from the single GTK test thread; never initialize GTK in a #[test].
#[cfg(test)]
pub(super) fn test_scroll_lifecycle() {
    let view = gtk::TextView::new();
    let _scrolled = gtk::ScrolledWindow::builder().child(&view).build();
    let buffer = view.buffer();
    buffer.set_text("Pending layout before mapping");

    // Before mapping, a follow tick cannot settle on its own. Reset/drop must
    // release its anchor immediately, without waiting for a frame to run.
    for dropping in [false, true] {
        let mut follow = ScrollFollow::default();
        let anchor = follow.capture(&view, &buffer).unwrap();
        assert!(anchor.bottom);
        let mark = anchor.mark.clone();
        follow.restore(&view, Some(anchor), 0, 0);
        assert!(!mark.is_deleted());
        if dropping {
            drop(follow);
        } else {
            follow.reset();
        }
        assert!(mark.is_deleted(), "cancelled follow must remove its anchor");
    }

    let mut follow = ScrollFollow::default();
    let anchor = follow.capture(&view, &buffer).unwrap();
    let old_mark = anchor.mark.clone();
    follow.restore(&view, Some(anchor), 0, 0);
    let replacement = follow.capture(&view, &buffer).unwrap();
    assert!(
        old_mark.is_deleted(),
        "new updates cancel obsolete callbacks"
    );
    let new_mark = replacement.mark.clone();
    drop(replacement);
    assert!(
        new_mark.is_deleted(),
        "even an unapplied capture releases its mark"
    );

    // An append below a reader's anchor releases the mark without queuing any
    // callback. Real fractional adjustment preservation is tested with layout
    // in markdown_buffer::test_markdown_scrolling.
    let mark = buffer.create_mark(None, &buffer.start_iter(), true);
    let anchor = ViewportAnchor {
        buffer: buffer.clone(),
        mark: mark.clone(),
        bottom: false,
        previous_value: 150.5,
        offset: 0,
    };
    follow.restore(&view, Some(anchor), usize::MAX, usize::MAX);
    assert!(mark.is_deleted());
    assert!(follow.tick.is_none());
}
