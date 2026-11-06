//! Preferences snapshots, ordered saves, and nonblocking shutdown.
use super::{
    app::App, dialogs, model_controller::ModelOperation, rewrite::RewriteOperation,
    views::buffer_text,
};
use crate::transcription;
use adw::prelude::*;
use std::rc::Rc;

/// Closing stays asynchronous: only the latest completed save may release the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum WindowLifecycle {
    #[default]
    Open,
    SavingBeforeClose,
    ReadyToClose,
}

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

    /// Release job ownership; each job's Drop implementation requests cancellation.
    pub(super) fn discard_active_work(&self) {
        self.session.borrow_mut().finish();
        self.main.meter.set_value(0.0);
        self.model_state.borrow_mut().operation = ModelOperation::Idle;
        self.rewrite_state.borrow_mut().operation = RewriteOperation::Idle;
    }

    fn begin_close(&self) {
        if self.lifecycle.get() != WindowLifecycle::Open {
            return;
        }
        self.lifecycle.set(WindowLifecycle::SavingBeforeClose);
        self.main
            .status
            .set_text("Saving preferences before closing…");
        self.discard_active_work();
        // Capture the exact text, including deliberately empty values. Prompt
        // edits are not written by intermediate settings saves during the session.
        {
            let mut settings = self.settings.borrow_mut();
            settings.whisper_context = buffer_text(&self.main.context);
            settings.llm_system_prompt = buffer_text(&self.rewrite.prompt);
        }
        self.refresh();
        // The window stays alive until the latest save completes. Never join
        // the writer or block GTK, and never lose the final save at process exit.
        self.persist_controls();
    }

    fn preferences_save_finished(&self, result: Result<(), String>) {
        match result {
            Ok(()) if self.lifecycle.get() == WindowLifecycle::SavingBeforeClose => {
                self.lifecycle.set(WindowLifecycle::ReadyToClose);
                self.window.close();
            }
            Ok(()) => {}
            Err(error) => {
                self.main
                    .status
                    .set_text(&format!("Could not save preferences: {error}"));
                if self.lifecycle.get() == WindowLifecycle::SavingBeforeClose {
                    self.lifecycle.set(WindowLifecycle::Open);
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
            if app.lifecycle.get() == WindowLifecycle::ReadyToClose {
                return gtk::glib::Propagation::Proceed;
            }
            if app.lifecycle.get() == WindowLifecycle::SavingBeforeClose {
                return gtk::glib::Propagation::Stop;
            }
            if app.busy() || app.document.is_dirty() || app.rewrite_state.borrow().is_dirty() {
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
