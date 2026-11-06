//! GTK-dependent checks run through one entry point on one test thread. Keep
//! feature helpers as ordinary functions; pure tests may still run in parallel.
mod construction;
mod devices;
mod lifecycle;
mod models;
mod preferences;
mod rewrite;
pub(super) mod support;
mod surfaces;
mod transcription;
mod zoom;

use super::{markdown_buffer, markdown_scroll, model_controller, text_zoom};
use adw::prelude::*;

#[test]
fn settings_pages_and_menu_surfaces_build() {
    if gtk::init().is_err() {
        eprintln!("Skipping GTK surface test: no display available");
        return;
    }
    adw::init().expect("libadwaita initialization");
    // Preserve the existing execution order while moving checks by feature.
    transcription::incremental_transcript_updates();
    markdown_buffer::test_markdown_buffer();
    markdown_buffer::test_markdown_scrolling();
    markdown_scroll::test_scroll_lifecycle();
    text_zoom::test_markdown_zoom();
    text_zoom::test_font_zoom();
    text_zoom::test_shared_recognized_zoom();
    zoom::font_zoom_panes();
    let parent = adw::ApplicationWindow::builder().build();
    devices::device_discovery_states(&parent);
    preferences::language_selection(&parent);
    construction::shared_app_construction();
    preferences::recognition_context_controls();
    preferences::prompt_placeholders_and_restore();
    models::model_library_window(&parent);
    models::replacement_confirmation();
    rewrite::rewrite_generation_states();
    transcription::editable_transcript();
    transcription::transcript_document_states();
    transcription::transcription_session_controls();
    lifecycle::lifecycle_job_ownership();
    model_controller::tests::operation_controls();
    models::model_availability_states();
    lifecycle::final_save_ordering();
    lifecycle::failed_save_retry();
    // Additional deterministic regression cases; no native audio/network work.
    models::replaced_scan_discards_stale_results();
    devices::discovery_disconnect_and_retry();
    transcription::failed_session_restores_controls();
    rewrite::failed_generation_restores_controls();
    model_controller::tests::operation_failure_controls();
    surfaces::settings_pages_and_menu_surfaces(&parent);
    parent.destroy();
    support::destroy_windows();
}
