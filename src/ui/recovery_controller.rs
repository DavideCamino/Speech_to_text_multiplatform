//! UI snapshots and nonblocking recovery-file operations.
use super::{app::App, dialogs, lifecycle::WindowLifecycle, task::background, views::buffer_text};
use crate::recovery;
use adw::prelude::*;
use std::{rc::Rc, sync::mpsc};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action { Save, Consume, Discard }

pub(super) struct RecoveryTask {
    action: Action,
    events: mpsc::Receiver<Result<(), String>>,
}

impl App {
    pub(super) fn recovery_snapshot(&self) -> recovery::Snapshot {
        recovery::Snapshot {
            transcript: self.document.snapshot(buffer_text(&self.main.transcript)),
            rewrite: self.rewrite_state.borrow().snapshot(
                buffer_text(&self.rewrite.source),
                buffer_text(&self.rewrite.prompt),
            ),
            recognition_context: buffer_text(&self.main.context),
            showing_rewrite: self.stack.visible_child_name().as_deref() == Some("rewrite"),
            ..Default::default()
        }
    }

    pub(super) fn restore_recovery(&self, snapshot: &recovery::Snapshot) {
        self.recovery_owned.set(true);
        self.document.restore(&snapshot.transcript);
        self.main
            .context
            .buffer()
            .set_text(&snapshot.recognition_context);
        self.rewrite
            .source
            .buffer()
            .set_text(&snapshot.rewrite.source);
        self.rewrite
            .prompt
            .buffer()
            .set_text(&snapshot.rewrite.prompt);
        self.rewrite_state.borrow_mut().restore(&snapshot.rewrite);
        self.rewrite
            .renderer
            .borrow_mut()
            .update(&self.rewrite.result, &snapshot.rewrite.markdown);
        self.stack
            .set_visible_child_name(if snapshot.showing_rewrite {
                "rewrite"
            } else {
                "main"
            });
        self.main
            .status
            .set_text("Unsaved content restored from the previous session.");
        self.rewrite
            .status
            .set_text("Previous session restored. Content still needs to be exported.");
        self.refresh();
    }

    /// Called only after restoration into widgets succeeds.
    pub(super) fn consume_recovery(&self) {
        self.delete_recovery(Action::Consume);
    }

    pub(super) fn close_without_saving(&self) {
        if self.lifecycle.get() != WindowLifecycle::Open { return; }
        if self.recovery_owned.get() {
            self.discard_active_work();
            self.delete_recovery(Action::Discard);
        } else {
            self.begin_close();
        }
    }

    fn delete_recovery(&self, action: Action) {
        self.lifecycle.set(WindowLifecycle::RestoringRecovery);
        let path = self.recovery_path.clone();
        let events = background(move || {
            let path = path.ok_or_else(|| "Cannot locate the recovery directory".to_owned())?;
            recovery::remove(&path).map_err(|error| format!("{error:#}"))
        });
        *self.recovery_task.borrow_mut() = Some(RecoveryTask {
            action,
            events,
        });
        self.refresh();
    }

    pub(super) fn autosave_and_close(self: &Rc<Self>) {
        if self.lifecycle.get() != WindowLifecycle::Open {
            return;
        }
        // Snapshot the displayed text, including partial output, before cancelling jobs.
        let snapshot = self.recovery_snapshot();
        self.lifecycle.set(WindowLifecycle::SavingRecovery);
        self.discard_active_work();
        self.main
            .status
            .set_text("Autosaving unsaved content before closing…");
        let path = self.recovery_path.clone();
        let events = background(move || {
            let path = path.ok_or_else(|| "Cannot locate the recovery directory".to_owned())?;
            recovery::save(&path, &snapshot).map_err(|error| format!("{error:#}"))
        });
        *self.recovery_task.borrow_mut() = Some(RecoveryTask {
            action: Action::Save,
            events,
        });
        self.refresh();
    }

    pub(super) fn poll_recovery(self: &Rc<Self>) {
        let result = {
            let task = self.recovery_task.borrow();
            let Some(task) = task.as_ref() else {
                return;
            };
            match task.events.try_recv() {
                Ok(result) => Some((task.action, result)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some((
                    task.action,
                    Err("Recovery worker stopped unexpectedly".into()),
                )),
            }
        };
        let Some((action, result)) = result else {
            return;
        };
        self.recovery_task.borrow_mut().take();
        self.lifecycle.set(WindowLifecycle::Open);
        match result {
            Ok(()) if action == Action::Save => {
                self.recovery_owned.set(true);
                self.begin_close();
            }
            Ok(()) => {
                self.recovery_owned.set(false);
                if action == Action::Discard { self.begin_close(); } else { self.refresh(); }
            }
            Err(error) if action == Action::Save => {
                self.main
                    .status
                    .set_text(&format!("Autosave failed: {error}"));
                self.refresh();
                let weak = Rc::downgrade(self);
                dialogs::confirm(
                    &self.window,
                    "Could not autosave",
                    &format!(
                        "{error}\n\nYour content is still in the app. Retry autosaving or close without saving."
                    ),
                    &[
                        ("close", "Close Without Saving"),
                        ("retry", "Retry Autosave"),
                    ],
                    move |response| {
                        if let Some(app) = weak.upgrade() {
                            match response.as_str() {
                                "retry" => app.autosave_and_close(),
                                "close" => app.close_without_saving(),
                                _ => {}
                            }
                        }
                    },
                );
            }
            Err(error) => {
                self.refresh();
                dialogs::message(
                    &self.window,
                    "Recovery file could not be deleted",
                    &format!(
                        "{error}\n\nThe file has been preserved and may be restored again next time."
                    ),
                );
            }
        }
    }
}
