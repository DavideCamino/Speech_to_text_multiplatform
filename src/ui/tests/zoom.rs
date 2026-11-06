//! Cross-pane font zoom wiring (implementation-private checks remain local).
use crate::ui::{
    text_zoom,
    views::{MainView, RewriteView},
};
use adw::prelude::*;
use std::rc::Rc;

pub(super) fn font_zoom_panes() {
    fn zoom_controller(view: &impl IsA<gtk::Widget>) -> Option<gtk::EventControllerScroll> {
        let pane = view.as_ref().parent().unwrap().parent().unwrap();
        let controllers = pane.observe_controllers();
        (0..controllers.n_items())
            .filter_map(|index| {
                controllers
                    .item(index)?
                    .downcast::<gtk::EventControllerScroll>()
                    .ok()
            })
            .find(|controller| controller.name().as_deref() == Some("text-font-zoom"))
    }
    let recognized_zoom = Rc::new(text_zoom::ZoomGroup::default());
    let main = MainView::new(&recognized_zoom);
    let rewrite = RewriteView::new(&recognized_zoom);
    for view in [
        &main.transcript,
        &rewrite.source,
        &rewrite.prompt,
        &rewrite.result,
    ] {
        let controller = zoom_controller(view).expect("plain-text pane zoom controller");
        assert_eq!(
            controller.propagation_phase(),
            gtk::PropagationPhase::Capture
        );
        assert_eq!(
            controller.flags(),
            gtk::EventControllerScrollFlags::VERTICAL
        );
        // Synthetic scroll signals without Ctrl must propagate normally.
        assert!(!controller.emit_by_name::<bool>("scroll", &[&0.0_f64, &-1.0_f64]));
    }
}
