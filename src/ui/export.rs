//! Export snapshots and tracking of saved versus newer edits.
use super::{app::App, dialogs, views::buffer_text};
use std::rc::Rc;

#[derive(Clone, Copy)]
pub(super) enum ExportKind {
    TranscriptText,
    Subtitles,
    Rewrite,
}

impl App {
    pub(super) fn export(self: &Rc<Self>, name: &str, content: String, kind: ExportKind) {
        let weak = Rc::downgrade(self);
        // TXT rewrite exports contain stripped text, not the authoritative Markdown.
        let snapshot = match kind {
            ExportKind::Rewrite => self.rewrite_markdown(),
            _ => content.clone(),
        };
        dialogs::save(&self.window, name, content, move || {
            if let Some(app) = weak.upgrade() {
                app.mark_export_saved(kind, &snapshot);
                app.main.status.set_text("Export saved.");
            }
        });
    }

    pub(super) fn mark_export_saved(&self, kind: ExportKind, snapshot: &str) {
        match kind {
            ExportKind::Rewrite => {
                self.rewrite_state.borrow_mut().mark_export_saved(snapshot);
            }
            ExportKind::TranscriptText => {
                self.document
                    .mark_text_saved(&buffer_text(&self.main.transcript), snapshot);
            }
            ExportKind::Subtitles => self.document.mark_subtitles_saved(snapshot),
        }
    }
}
