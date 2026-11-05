//! Session-only font zoom with optionally shared sizes. Never changes text or tags.
use adw::prelude::*;
use gtk::{gdk, glib};
use std::{cell::Cell, rc::Rc};

const MIN_POINTS: f64 = 8.0;
const MAX_POINTS: f64 = 36.0;
// Surface-unit deltas are pixels, unlike wheel-unit deltas (wheel notches).
const SURFACE_PIXELS_PER_STEP: f64 = 60.0;

/// A provider is installed only on members of this group. Sharing it keeps
/// their size synchronized, including members hidden in another stack page.
#[derive(Default)]
pub(super) struct ZoomGroup {
    provider: gtk::CssProvider,
    points: Cell<Option<f64>>,
}

pub(super) struct FontZoom {
    view: glib::WeakRef<gtk::TextView>,
    group: Rc<ZoomGroup>,
    // Gesture accumulation belongs to the pane, not the shared size.
    pending: Cell<f64>,
}

impl FontZoom {
    fn scroll(
        &self,
        modifiers: gdk::ModifierType,
        dy: f64,
        unit: gdk::ScrollUnit,
    ) -> glib::Propagation {
        if !modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            self.pending.set(0.0);
            return glib::Propagation::Proceed;
        }
        if !dy.is_finite() || dy == 0.0 {
            return glib::Propagation::Proceed;
        }
        let Some(view) = self.view.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let divisor = if unit == gdk::ScrollUnit::Surface {
            SURFACE_PIXELS_PER_STEP
        } else {
            1.0
        };
        let delta = -dy / divisor;
        // Reversing direction must not first pay off an old partial gesture.
        let previous = self.pending.get();
        let pending = if previous * delta < 0.0 {
            delta
        } else {
            previous + delta
        };
        let steps = pending.trunc();
        self.pending.set(pending - steps);
        if steps != 0.0 {
            let current = self
                .group
                .points
                .get()
                .unwrap_or_else(|| font_points(&view));
            let next = (current + steps).clamp(MIN_POINTS, MAX_POINTS);
            self.group.points.set(Some(next));
            self.group
                .provider
                .load_from_string(&format!("textview {{ font-size: {next}pt; }}"));
        }
        // Consume Ctrl+scroll even for partial steps or at a size limit, so the
        // surrounding ScrolledWindow does not also move the document.
        glib::Propagation::Stop
    }
}

fn font_points(view: &gtk::TextView) -> f64 {
    let Some(description) = view.pango_context().font_description() else {
        return 11.0;
    };
    let size = f64::from(description.size()) / f64::from(gtk::pango::SCALE);
    let points = if description.is_size_absolute() {
        let dpi = gtk::Settings::for_display(&view.display()).gtk_xft_dpi();
        let dpi = if dpi > 0 {
            f64::from(dpi) / 1024.0
        } else {
            96.0
        };
        size * 72.0 / dpi
    } else {
        size
    };
    if points > 0.0 { points } else { 11.0 }
}

pub(super) fn install(
    view: &gtk::TextView,
    pane: &gtk::Box,
    group: &Rc<ZoomGroup>,
) -> Rc<FontZoom> {
    // Widget-local registration affects only explicitly linked text views,
    // never the prompt, unrelated panes, or application chrome.
    // Do not override the theme's font until the user actually zooms.
    #[allow(deprecated)]
    view.style_context()
        .add_provider(&group.provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    let zoom = Rc::new(FontZoom {
        view: view.downgrade(),
        group: group.clone(),
        pending: Cell::new(0.0),
    });
    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    controller.set_name(Some("text-font-zoom"));
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let state = zoom.clone();
    controller.connect_scroll(move |controller, _, dy| {
        state.scroll(controller.current_event_state(), dy, controller.unit())
    });
    let state = zoom.clone();
    controller.connect_scroll_begin(move |_| state.pending.set(0.0));
    let state = zoom.clone();
    controller.connect_scroll_end(move |_| state.pending.set(0.0));
    // Capture on the pane ancestor, before the ScrolledWindow can consume it.
    pane.add_controller(controller);
    zoom
}

#[cfg(test)]
pub(super) fn test_shared_recognized_zoom() {
    use super::views::{MainView, RewriteView};
    let group = Rc::new(ZoomGroup::default());
    let main = MainView::new(&group);
    let rewrite = RewriteView::new(&group);
    let stack = gtk::Stack::new();
    stack.add_named(&main.root, Some("main"));
    stack.add_named(&rewrite.root, Some("rewrite"));
    let window = gtk::Window::builder().child(&stack).build();
    window.present();
    let context = glib::MainContext::default();
    let wait_for_font = |view: &gtk::TextView, target: f64| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            while context.pending() {
                context.iteration(false);
            }
            if (font_points(view) - target).abs() < 0.1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "font size {} did not become {target}",
                font_points(view)
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    while context.pending() {
        context.iteration(false);
    }
    // Exercise the same scroll handler as the installed controllers, then
    // verify the real views' providers, including after switching stack pages.
    let main_zoom = FontZoom {
        view: main.transcript.downgrade(),
        group: group.clone(),
        pending: Cell::new(0.0),
    };
    let original_zoom = FontZoom {
        view: rewrite.source.downgrade(),
        group: group.clone(),
        pending: Cell::new(0.0),
    };
    let ctrl = gdk::ModifierType::CONTROL_MASK;
    main_zoom.scroll(ctrl, -3.0, gdk::ScrollUnit::Wheel);
    let size = group.points.get().unwrap();
    wait_for_font(&main.transcript, size);
    stack.set_visible_child_name("rewrite");
    wait_for_font(&rewrite.source, size);
    let prompt_size = font_points(&rewrite.prompt);
    original_zoom.scroll(ctrl, 1.0, gdk::ScrollUnit::Wheel);
    let size = group.points.get().unwrap();
    wait_for_font(&rewrite.source, size);
    assert!((font_points(&rewrite.prompt) - prompt_size).abs() < 0.1);
    stack.set_visible_child_name("main");
    wait_for_font(&main.transcript, size);
    assert!((font_points(&main.transcript) - font_points(&rewrite.source)).abs() < 0.1);
    window.destroy();
}

#[cfg(test)]
pub(super) fn test_markdown_zoom() {
    let view = gtk::TextView::new();
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.append(&view);
    let group = Rc::new(ZoomGroup::default());
    let zoom = install(&view, &pane, &group);
    let mut renderer = super::markdown_buffer::MarkdownBuffer::new(&view);
    renderer.update(&view, "# Heading\n\nBody text");
    let window = gtk::Window::builder().child(&pane).build();
    window.present();
    use super::markdown_buffer::wait_for_gtk;
    wait_for_gtk("Markdown zoom initial layout", || {
        view.is_mapped()
            && view.width() > 0
            && view.height() > 0
            && view.iter_location(&view.buffer().start_iter()).height()
                > view
                    .iter_location(&view.buffer().iter_at_offset(9))
                    .height()
    });
    let heading_height = view.iter_location(&view.buffer().start_iter()).height();
    zoom.scroll(
        gdk::ModifierType::CONTROL_MASK,
        -6.0,
        gdk::ScrollUnit::Wheel,
    );
    let target = group.points.get().unwrap();
    wait_for_gtk("Markdown zoom font and layout", || {
        (font_points(&view) - target).abs() < 0.1
            && view.iter_location(&view.buffer().start_iter()).height() > heading_height
    });
    let enlarged_heading = view.iter_location(&view.buffer().start_iter()).height();
    let body_height = view
        .iter_location(&view.buffer().iter_at_offset(9))
        .height();
    assert!(enlarged_heading > heading_height);
    assert!(
        enlarged_heading > body_height,
        "heading must retain relative sizing after zoom"
    );
    assert_eq!(super::views::buffer_text(&view), "Heading\n\nBody text");
    window.destroy();
}

#[cfg(test)]
pub(super) fn test_font_zoom() {
    let view = gtk::TextView::new();
    let other = gtk::TextView::new();
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.append(&view);
    pane.append(&other);
    let window = gtk::Window::builder().child(&pane).build();
    window.present();
    let context = glib::MainContext::default();
    while context.pending() {
        context.iteration(false);
    }
    let group = Rc::new(ZoomGroup::default());
    let zoom = install(&view, &pane, &group);
    let ctrl = gdk::ModifierType::CONTROL_MASK;
    let wheel = gdk::ScrollUnit::Wheel;
    let surface = gdk::ScrollUnit::Surface;
    view.buffer().set_text("Hello, 世界!\nOriginal text.");
    let tag = view
        .buffer()
        .create_tag(Some("provisional"), &[("foreground", &"#888888")])
        .unwrap();
    view.buffer()
        .apply_tag(&tag, &view.buffer().start_iter(), &view.buffer().end_iter());
    let before = view.pango_context().font_description();
    let other_before = other.pango_context().font_description();
    let base = font_points(&view);

    assert_eq!(
        zoom.scroll(gdk::ModifierType::empty(), -1.0, wheel),
        glib::Propagation::Proceed
    );
    assert_eq!(group.points.get(), None);
    assert_eq!(view.pango_context().font_description(), before);
    assert_eq!(zoom.scroll(ctrl, -1.0, wheel), glib::Propagation::Stop);
    assert_eq!(
        group.points.get(),
        Some((base + 1.0).clamp(MIN_POINTS, MAX_POINTS))
    );
    zoom.scroll(ctrl, 1.0, wheel);
    let current = group.points.get().unwrap();
    for _ in 0..3 {
        zoom.scroll(ctrl, -15.0, surface);
        assert_eq!(group.points.get(), Some(current));
    }
    zoom.scroll(ctrl, -15.0, surface);
    assert_eq!(group.points.get(), Some((current + 1.0).min(MAX_POINTS)));
    for _ in 0..100 {
        zoom.scroll(ctrl, -1.0, wheel);
    }
    assert_eq!(group.points.get(), Some(MAX_POINTS));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while (font_points(&view) - MAX_POINTS).abs() >= 0.1 {
        while context.pending() {
            context.iteration(false);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "CSS must resize the actual text font; got {}",
            font_points(&view)
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(zoom.scroll(ctrl, -1.0, wheel), glib::Propagation::Stop);
    // Establish the selection after mapping/font validation. Initial window
    // focus events can otherwise collapse it independently of zoom handling.
    view.buffer().select_range(
        &view.buffer().start_iter(),
        &view.buffer().iter_at_offset(5),
    );
    for _ in 0..100 {
        zoom.scroll(ctrl, 1.0, wheel);
    }
    assert_eq!(group.points.get(), Some(MIN_POINTS));
    assert_eq!(zoom.scroll(ctrl, 1.0, wheel), glib::Propagation::Stop);
    assert_eq!(
        zoom.scroll(ctrl, f64::NAN, surface),
        glib::Propagation::Proceed
    );
    assert_eq!(other.pango_context().font_description(), other_before);
    assert_eq!(
        view.buffer().text(
            &view.buffer().start_iter(),
            &view.buffer().end_iter(),
            false
        ),
        "Hello, 世界!\nOriginal text."
    );
    assert!(view.buffer().start_iter().has_tag(&tag));
    let (start, end) = view.buffer().selection_bounds().unwrap();
    assert_eq!((start.offset(), end.offset()), (0, 5));

    let weak = view.downgrade();
    window.destroy();
    window.set_child(None::<&gtk::Widget>);
    drop(window);
    drop(view);
    drop(pane);
    assert!(
        weak.upgrade().is_none(),
        "zoom must not retain the TextView"
    );
    assert_eq!(zoom.scroll(ctrl, -1.0, wheel), glib::Propagation::Proceed);
}
