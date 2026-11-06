//! Preferences snapshots, ordered saves, and nonblocking shutdown.
use super::{app::App, dialogs, views::buffer_text};
use crate::transcription;
use adw::prelude::*;
use std::rc::Rc;

impl App {
    pub(super) fn persist(&self) {
        let result = self
            .settings_writer
            .borrow_mut()
            .submit(self.settings.borrow().clone());
        if let Err(error) = result {
            self.preferences_save_finished(Err(error));
        }
    }

    pub(super) fn persist_controls(&self) {
        {
            let mut settings = self.settings.borrow_mut();
            settings.language = self.preferences.language.selected_language();
            settings.use_gpu = self.preferences.gpu.is_active();
            settings.gpu_backend = self
                .preferences
                .gpu_backend
                .selected()
                .checked_sub(1)
                .and_then(|index| {
                    transcription::compiled_gpu_backends()
                        .get(index as usize)
                        .copied()
                });
            settings.device = self.preferences.selected_device();
            if let Ok(connection) = self.llm_connection() {
                settings.llm = connection;
            }
        }
        self.persist();
    }

    pub(super) fn restore_prompts(&self) {
        let settings = self.settings.borrow();
        self.main
            .context
            .buffer()
            .set_text(&settings.whisper_context);
        self.rewrite
            .prompt
            .buffer()
            .set_text(&settings.llm_system_prompt);
    }

    fn begin_close(&self) {
        self.closing.set(true);
        self.window.set_sensitive(false);
        self.main
            .status
            .set_text("Saving preferences before closing…");
        self.job.borrow_mut().take();
        self.recording.set(false);
        self.main.meter.set_value(0.0);
        self.download.borrow_mut().take();
        self.download_check.borrow_mut().take();
        self.rewrite_state.borrow_mut().job.take();
        self.rewrite_state.borrow_mut().test.take();
        // Capture the exact text, including deliberately empty values. Prompt
        // edits are not written by intermediate settings saves during the session.
        {
            let mut settings = self.settings.borrow_mut();
            settings.whisper_context = buffer_text(&self.main.context);
            settings.llm_system_prompt = buffer_text(&self.rewrite.prompt);
        }
        // The window stays alive until the latest save completes. Never join
        // the writer or block GTK, and never lose the final save at process exit.
        self.persist_controls();
    }

    fn preferences_save_finished(&self, result: Result<(), String>) {
        match result {
            Ok(()) if self.closing.get() => {
                self.allow_close.set(true);
                self.window.close();
            }
            Ok(()) => {}
            Err(error) => {
                self.main
                    .status
                    .set_text(&format!("Could not save preferences: {error}"));
                if self.closing.replace(false) {
                    self.window.set_sensitive(true);
                    self.refresh();
                    dialogs::message(&self.window, "Could not save preferences", &error);
                }
            }
        }
    }

    pub(super) fn poll_preferences_save(&self) {
        let result = self.settings_writer.borrow_mut().poll();
        if let Some(result) = result {
            self.preferences_save_finished(result);
        }
    }

    pub(super) fn connect_close_request(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(app) = weak.upgrade() else {
                return gtk::glib::Propagation::Proceed;
            };
            if app.allow_close.get() {
                return gtk::glib::Propagation::Proceed;
            }
            if app.closing.get() {
                return gtk::glib::Propagation::Stop;
            }
            if app.busy() || app.dirty.get() || app.rewrite_dirty.get() {
                let weak = Rc::downgrade(&app);
                dialogs::confirm(
                    &app.window,
                    "Close Speech2Text?",
                    "Active work will be cancelled and any unexported text will be discarded. Audio and transcripts are not saved automatically.",
                    &[("close", "Discard and Close")],
                    move |response| {
                        if response == "close" && let Some(app) = weak.upgrade() {
                            app.begin_close();
                        }
                    },
                );
                gtk::glib::Propagation::Stop
            } else {
                app.begin_close();
                gtk::glib::Propagation::Stop
            }
        });
    }
}
