//! Preferences snapshots, ordered saves, and nonblocking shutdown.
use super::{
    app::App, dialogs, model_controller::ModelOperation, rewrite::RewriteOperation,
    views::buffer_text,
};
use crate::transcription;
use adw::prelude::*;
use std::rc::Rc;

/// Closing stays asynchronous: release the window after the final save or an explicit bypass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum WindowLifecycle {
    #[default]
    Open,
    SavingRecovery,
    RestoringRecovery,
    SavingBeforeClose,
    ConfirmingCloseAfterSaveFailure,
    ReadyToClose,
}

impl App {
    fn submit_preferences(&self) -> Result<(), String> {
        self.settings_writer
            .borrow_mut()
            .submit(self.settings.borrow().clone())
    }

    pub(super) fn persist(&self) {
        if self.lifecycle.get() == WindowLifecycle::Open
            && let Err(error) = self.submit_preferences()
        {
            self.show_preferences_error(&error);
        }
    }

    pub(super) fn persist_controls(&self) {
        if self.lifecycle.get() == WindowLifecycle::Open {
            self.capture_controls();
            self.persist();
        }
    }

    fn capture_controls(&self) {
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
        settings.llm_options = self.llm_options();
        if let Ok(connection) = self.llm_connection() {
            settings.llm = connection;
        }
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
        self.clear_audio_queue();
        self.main.meter.set_value(0.0);
        self.model_state.borrow_mut().operation = ModelOperation::Idle;
        self.rewrite_state.borrow_mut().operation = RewriteOperation::Idle;
    }

    pub(super) fn begin_close(self: &Rc<Self>) {
        if self.lifecycle.get() != WindowLifecycle::Open {
            return;
        }
        self.lifecycle.set(WindowLifecycle::SavingBeforeClose);
        self.discard_active_work();
        // Capture the exact text, including deliberately empty values. Prompt
        // edits are not written by intermediate settings saves during the session.
        {
            let mut settings = self.settings.borrow_mut();
            settings.whisper_context = buffer_text(&self.main.context);
            settings.llm_system_prompt = buffer_text(&self.rewrite.prompt);
        }
        self.capture_controls();
        self.save_preferences_before_close();
    }

    fn save_preferences_before_close(self: &Rc<Self>) {
        self.lifecycle.set(WindowLifecycle::SavingBeforeClose);
        self.main
            .status
            .set_text("Saving preferences before closing…");
        self.refresh();
        if let Err(error) = self.submit_preferences() {
            self.preferences_save_finished(Err(error));
        }
    }

    fn show_preferences_error(&self, error: &str) {
        self.main
            .status
            .set_text(&format!("Could not save preferences: {error}"));
    }

    fn finish_close(&self) {
        self.lifecycle.set(WindowLifecycle::ReadyToClose);
        self.window.close();
    }

    fn preferences_save_finished(self: &Rc<Self>, result: Result<(), String>) {
        let closing = self.lifecycle.get() == WindowLifecycle::SavingBeforeClose;
        match result {
            Ok(()) if closing => self.finish_close(),
            Ok(()) => {}
            Err(error) => {
                self.show_preferences_error(&error);
                if closing {
                    self.confirm_close_after_save_failure(&error);
                }
            }
        }
    }

    fn confirm_close_after_save_failure(self: &Rc<Self>, error: &str) {
        self.lifecycle
            .set(WindowLifecycle::ConfirmingCloseAfterSaveFailure);
        self.refresh();
        let weak = Rc::downgrade(self);
        dialogs::confirm(
            &self.window,
            "Could not save preferences",
            &format!(
                "{error}\n\nRetry saving, exit without updating preferences, or return to the editor."
            ),
            &[
                ("retry", "Retry"),
                ("exit", "Exit without updating preferences"),
            ],
            move |response| {
                if let Some(app) = weak.upgrade()
                    && app.lifecycle.get() == WindowLifecycle::ConfirmingCloseAfterSaveFailure
                {
                    match response.as_str() {
                        "retry" => app.save_preferences_before_close(),
                        // Do not re-run shutdown or consume a recovery snapshot.
                        "exit" => app.finish_close(),
                        _ => {
                            app.lifecycle.set(WindowLifecycle::Open);
                            app.refresh();
                        }
                    }
                }
            },
        );
    }

    pub(super) fn poll_preferences_save(self: &Rc<Self>) {
        let result = self.settings_writer.borrow_mut().poll();
        if let Some(result) = result {
            self.preferences_save_finished(result);
        }
    }

    fn show_close_confirmation(self: &Rc<Self>, offered_autosave: bool) {
        self.close_confirmation.set(true);
        let weak = Rc::downgrade(self);
        dialogs::confirm_close(&self.window, offered_autosave, move |response| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            // A worker may have produced its first output while the original
            // dialog was open. Keep the guard set and offer autosave before
            // treating this response as permission to discard that output.
            if response == "close" && !offered_autosave && app.has_unsaved_content() {
                app.show_close_confirmation(true);
                return;
            }
            app.close_confirmation.set(false);
            match response.as_str() {
                "close" => app.close_without_saving(),
                "autosave" => app.autosave_and_close(),
                _ => {}
            }
        });
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
            if app.lifecycle.get() != WindowLifecycle::Open || app.close_confirmation.get() {
                return gtk::glib::Propagation::Stop;
            }
            let has_unsaved_content = app.has_unsaved_content();
            if app.busy() || has_unsaved_content {
                app.show_close_confirmation(has_unsaved_content);
            } else {
                app.close_without_saving();
            }
            gtk::glib::Propagation::Stop
        });
    }
}
