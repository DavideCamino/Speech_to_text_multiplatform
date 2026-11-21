//! Rewrite lifecycle and batched native TextBuffer Markdown rendering.
use super::{app::App, dialogs, lifecycle::WindowLifecycle, views::buffer_text};
use crate::llm;
use adw::prelude::*;
use std::{
    rc::Rc,
    sync::{atomic::Ordering, mpsc},
};

const MODEL_LOADING: &str = "Loading selected model…";
const SELECTION_CHANGED: &str = "LLM settings changed. Click Apply to load the selected model.";
const REFRESH_REQUIRED: &str = "Connection settings changed. Click Refresh Models.";

pub(super) struct ModelRefresh {
    job: llm::Job,
    connection: llm::Connection,
    selection_revision: u64,
    previous_model: Option<String>,
    models: Option<Vec<llm::Model>>,
}

pub(super) struct LoadTarget {
    connection: llm::Connection,
    model: String,
    selection_revision: u64,
}

impl LoadTarget {
    fn ready_message(&self) -> String {
        format!("Model ready: {}", self.model)
    }
}

#[derive(Default)]
pub(super) enum RewriteOperation {
    #[default]
    Idle,
    RefreshingModels(ModelRefresh),
    LoadingModel {
        job: llm::Job,
        target: LoadTarget,
    },
    Generating(llm::Job),
}

impl RewriteOperation {
    fn connection_job(&self) -> Option<&llm::Job> {
        match self {
            Self::RefreshingModels(refresh) => Some(&refresh.job),
            Self::LoadingModel { job, .. } => Some(job),
            _ => None,
        }
    }

    fn generation_job(&self) -> Option<&llm::Job> {
        match self {
            Self::Generating(job) => Some(job),
            _ => None,
        }
    }

    pub(super) fn is_generating(&self) -> bool {
        self.generation_job().is_some()
    }

    pub(super) fn is_loading(&self) -> bool {
        matches!(self, Self::LoadingModel { .. })
    }

    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    fn can_stop(&self) -> bool {
        self.generation_job()
            .is_some_and(|job| !job.cancel.load(Ordering::Relaxed))
    }

    fn request_stop(&self) -> bool {
        if let Some(job) = self.generation_job() {
            job.cancel.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Capture cancellation before Drop marks the job as cancelled itself.
    fn finish(&mut self) -> bool {
        let cancelled = self
            .generation_job()
            .is_some_and(|job| job.cancel.load(Ordering::Relaxed));
        *self = Self::Idle;
        cancelled
    }
}

#[derive(Default)]
pub(super) struct State {
    pub(super) operation: RewriteOperation,
    // Incremented when the selection changes, including credentials. No
    // session token is duplicated in readiness state or status messages.
    selection_revision: u64,
    ready_model: Option<LoadTarget>,
    markdown: String,
    unexported: bool,
}

impl State {
    pub(super) fn is_dirty(&self) -> bool {
        self.unexported
    }

    pub(super) fn has_unsaved_text(&self) -> bool {
        self.unexported && !self.markdown.trim().is_empty()
    }

    pub(super) fn snapshot(
        &self,
        source: String,
        prompt: String,
    ) -> crate::recovery::RewriteSnapshot {
        crate::recovery::RewriteSnapshot {
            source,
            prompt,
            markdown: self.markdown.clone(),
            dirty: self.unexported,
        }
    }

    pub(super) fn restore(&mut self, snapshot: &crate::recovery::RewriteSnapshot) {
        self.operation = RewriteOperation::Idle;
        self.ready_model = None;
        self.markdown.clone_from(&snapshot.markdown);
        self.unexported = snapshot.dirty;
    }

    fn reset_result(&mut self) {
        self.markdown.clear();
        self.unexported = false;
    }

    fn append_text(&mut self, text: &str) {
        self.markdown.push_str(text);
        self.unexported = true;
    }

    pub(super) fn mark_export_saved(&mut self, snapshot: &str) {
        if self.markdown == snapshot {
            self.unexported = false;
        }
    }
}

fn drain(job: &llm::Job) -> Vec<llm::Event> {
    let mut events = Vec::new();
    for _ in 0..64 {
        match job.events.try_recv() {
            Ok(event) => {
                let finished = matches!(event, llm::Event::Finished(_));
                events.push(event);
                if finished {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                events.push(llm::Event::Finished(Err(
                    "LLM worker stopped unexpectedly".into()
                )));
                break;
            }
        }
    }
    events
}

impl App {
    pub(super) fn enter_rewrite(self: &Rc<Self>) {
        let source = buffer_text(&self.main.transcript);
        if source != buffer_text(&self.rewrite.source) {
            if self.rewrite_state.borrow().is_dirty() {
                let weak = Rc::downgrade(self);
                dialogs::confirm(
                    &self.window,
                    "Use the new transcript?",
                    "This will discard the previous unexported rewrite result.",
                    &[("replace", "Use New Transcript")],
                    move |response| {
                        if response == "replace"
                            && let Some(app) = weak.upgrade()
                        {
                            app.reset_rewrite();
                        }
                    },
                );
                return;
            }
            self.reset_rewrite();
        } else {
            self.stack.set_visible_child_name("rewrite");
        }
    }

    pub(super) fn reset_rewrite(&self) {
        self.rewrite
            .source
            .buffer()
            .set_text(&buffer_text(&self.main.transcript));
        self.rewrite_state.borrow_mut().reset_result();
        self.rewrite.renderer.borrow_mut().reset();
        self.rewrite.status.set_text("Ready");
        self.stack.set_visible_child_name("rewrite");
        self.refresh();
    }

    fn can_use_llm_settings(&self) -> bool {
        self.lifecycle.get() == WindowLifecycle::Open
            && self.rewrite_state.borrow().operation.is_idle()
    }

    pub(super) fn can_start_llm_task(&self) -> bool {
        self.can_use_llm_settings() && !self.busy()
    }

    pub(super) fn refresh_rewrite_controls(&self) {
        let (generating, can_stop, has_result) = {
            let state = self.rewrite_state.borrow();
            (
                state.operation.is_generating(),
                state.operation.can_stop(),
                !state.markdown.trim().is_empty(),
            )
        };
        let connection_editable = self.can_use_llm_settings();
        let can_start_task = self.can_start_llm_task();
        self.preferences.llm_host.set_sensitive(connection_editable);
        self.preferences.llm_port.set_sensitive(connection_editable);
        self.preferences
            .llm_https
            .set_sensitive(connection_editable);
        self.preferences
            .llm_token
            .set_sensitive(connection_editable);
        self.preferences
            .llm_refresh
            .set_sensitive(connection_editable);
        self.preferences.llm_model.set_sensitive(
            connection_editable && self.preferences.llm_model_selection.n_items() > 0,
        );
        self.preferences
            .llm_reasoning
            .set_sensitive(connection_editable);
        self.preferences
            .llm_apply
            .set_sensitive(can_start_task && self.llm_options().model.is_some());
        self.rewrite.run.set_sensitive(can_start_task);
        self.rewrite.prompt.set_editable(!generating);
        self.rewrite.back.set_sensitive(!generating);
        self.rewrite.stop.set_sensitive(can_stop);
        for button in [
            &self.rewrite.copy,
            &self.rewrite.txt,
            &self.rewrite.markdown,
        ] {
            button.set_sensitive(has_result && !generating);
        }
    }

    pub(super) fn llm_connection(&self) -> anyhow::Result<llm::Connection> {
        let connection = llm::Connection {
            host: self.preferences.llm_host.text().trim().to_owned(),
            port: self
                .preferences
                .llm_port
                .text()
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("Port must be between 1 and 65535"))?,
            https: self.preferences.llm_https.is_active(),
        };
        connection.base_url()?;
        Ok(connection)
    }
    fn llm_selected_model(&self) -> Option<String> {
        self.preferences
            .llm_model_selection
            .selected_item()
            .and_downcast::<gtk::StringObject>()
            .map(|item| item.string().to_string())
    }

    pub(super) fn llm_options(&self) -> llm::Options {
        llm::Options {
            model: self.llm_selected_model(),
            reasoning: llm::Reasoning::ALL
                .get(self.preferences.llm_reasoning.selected() as usize)
                .copied()
                .unwrap_or_default(),
        }
    }

    pub(super) fn llm_connection_changed(&self) {
        self.invalidate_llm_readiness(REFRESH_REQUIRED);
        self.set_llm_models(&[], None);
        self.preferences.llm_status.set_subtitle(REFRESH_REQUIRED);
        self.refresh_rewrite_controls();
    }

    pub(super) fn llm_model_changed(&self) {
        let model = self.llm_selected_model();
        if self.settings.borrow().llm_options.model != model {
            self.invalidate_llm_readiness(SELECTION_CHANGED);
            self.settings.borrow_mut().llm_options.model = model;
            self.persist();
            self.refresh();
        }
    }

    fn invalidate_llm_readiness(&self, message: &str) {
        let ready = {
            let mut state = self.rewrite_state.borrow_mut();
            state.selection_revision = state.selection_revision.wrapping_add(1);
            state.ready_model.take()
        };
        if let Some(ready) = ready {
            self.replace_model_status(&ready.ready_message(), message);
        }
    }

    fn set_llm_models(&self, ids: &[&str], previous_model: Option<&str>) {
        let selection = &self.preferences.llm_model_selection;
        if ids.is_empty() && selection.n_items() == 0 {
            return;
        }
        let selected = if ids.len() == 1 {
            0
        } else {
            previous_model
                .and_then(|previous| ids.iter().position(|&id| id == previous))
                .map_or(gtk::INVALID_LIST_POSITION, |index| index as u32)
        };
        // Update the list and selection together so observers only see
        // the final selected item, even if the index itself is unchanged.
        let _freeze = selection.freeze_notify();
        selection.set_model(Some(&gtk::StringList::new(ids)));
        selection.set_selected(selected);
    }

    // Only replace labels that still show the model's previous status; other
    // jobs may have taken over the shared main/rewrite labels in the meantime.
    fn replace_model_status(&self, previous: &str, next: &str) {
        if self.preferences.llm_status.subtitle().as_deref() == Some(previous) {
            self.preferences.llm_status.set_subtitle(next);
        }
        for label in [&self.main.status, &self.rewrite.status] {
            if label.text() == previous {
                label.set_text(next);
            }
        }
    }

    pub(super) fn apply_llm(&self) {
        self.apply_llm_with(llm::start_loading);
    }

    pub(super) fn apply_llm_with(
        &self,
        start: impl FnOnce(llm::Connection, String, llm::Options) -> llm::Job,
    ) {
        if !self.can_start_llm_task() {
            return;
        }
        let options = self.llm_options();
        let Some(model) = options.model.clone() else {
            self.preferences
                .llm_status
                .set_subtitle("Select a model first.");
            return;
        };
        let connection = match self.llm_connection() {
            Ok(connection) => connection,
            Err(error) => {
                self.preferences.llm_status.set_subtitle(&error.to_string());
                return;
            }
        };
        let target = LoadTarget {
            connection: connection.clone(),
            model,
            selection_revision: self.rewrite_state.borrow().selection_revision,
        };
        let job = start(
            connection,
            self.preferences.llm_token.text().to_string(),
            options,
        );
        {
            let mut state = self.rewrite_state.borrow_mut();
            state.ready_model = None;
            state.operation = RewriteOperation::LoadingModel { job, target };
        }
        self.preferences.llm_status.set_subtitle(MODEL_LOADING);
        self.rewrite.status.set_text(MODEL_LOADING);
        self.main.status.set_text(MODEL_LOADING);
        self.preferences.window.close();
        self.refresh();
    }

    pub(super) fn refresh_llm_models(&self) {
        self.refresh_llm_models_with(|connection, token| {
            llm::start_configured(connection, token, None, llm::Options::default())
        });
    }

    pub(super) fn refresh_llm_models_with(
        &self,
        start: impl FnOnce(llm::Connection, String) -> llm::Job,
    ) {
        if !self.can_use_llm_settings() {
            return;
        }
        let connection = match self.llm_connection() {
            Ok(connection) => connection,
            Err(error) => {
                self.invalidate_llm_readiness(REFRESH_REQUIRED);
                self.set_llm_models(&[], None);
                self.preferences.llm_status.set_subtitle(&error.to_string());
                self.refresh_rewrite_controls();
                return;
            }
        };
        self.settings.borrow_mut().llm = connection.clone();
        self.persist();
        let refresh = ModelRefresh {
            job: start(
                connection.clone(),
                self.preferences.llm_token.text().to_string(),
            ),
            connection,
            selection_revision: self.rewrite_state.borrow().selection_revision,
            previous_model: self.llm_selected_model(),
            models: None,
        };
        self.rewrite_state.borrow_mut().operation = RewriteOperation::RefreshingModels(refresh);
        self.preferences
            .llm_status
            .set_subtitle("Connecting and refreshing models…");
        self.refresh();
    }
    pub(super) fn start_rewrite(&self) {
        if !self.can_start_llm_task() {
            return;
        }
        let prompt = buffer_text(&self.rewrite.prompt);
        if prompt.trim().is_empty() {
            self.rewrite
                .status
                .set_text("System Prompt (required) must not be empty.");
            self.rewrite.prompt_section.set_expanded(true);
            self.rewrite.prompt.grab_focus();
            return;
        }
        let text = buffer_text(&self.rewrite.source);
        if text.trim().is_empty() {
            self.rewrite
                .status
                .set_text("Original text must not be empty.");
            return;
        }
        let connection = match self.llm_connection() {
            Ok(connection) => connection,
            Err(error) => {
                self.rewrite.status.set_text(&error.to_string());
                return;
            }
        };
        let options = self.llm_options();
        if options.model.is_none() {
            self.rewrite
                .status
                .set_text("Refresh and select an LLM model in Settings first.");
            return;
        }
        self.settings.borrow_mut().llm = connection.clone();
        self.settings.borrow_mut().llm_options = options.clone();
        self.persist();
        {
            let mut state = self.rewrite_state.borrow_mut();
            state.reset_result();
            state.operation = RewriteOperation::Generating(llm::start_configured(
                connection,
                self.preferences.llm_token.text().to_string(),
                Some((prompt, text)),
                options,
            ));
        }
        self.rewrite.renderer.borrow_mut().reset();
        self.rewrite.status.set_text("Connecting and generating…");
        self.refresh();
    }
    pub(super) fn request_rewrite_stop(&self) {
        if !self.rewrite_state.borrow().operation.request_stop() {
            return;
        }
        self.rewrite.status.set_text("Stopping…");
        self.refresh();
    }

    pub(super) fn poll_rewrite(&self) {
        let connection_events = self
            .rewrite_state
            .borrow()
            .operation
            .connection_job()
            .map(drain)
            .unwrap_or_default();
        let mut controls_changed = !connection_events.is_empty();
        for event in connection_events {
            match event {
                llm::Event::Models(models) => {
                    if let RewriteOperation::RefreshingModels(refresh) =
                        &mut self.rewrite_state.borrow_mut().operation
                    {
                        // Only replace the dropdown after a successful terminal
                        // event, so invalid or disconnected requests cannot populate it.
                        refresh.models = Some(models);
                    }
                }
                llm::Event::Finished(result) => {
                    let operation = std::mem::take(&mut self.rewrite_state.borrow_mut().operation);
                    match operation {
                        RewriteOperation::LoadingModel { target, .. } => {
                            self.complete_model_load(target, result)
                        }
                        RewriteOperation::RefreshingModels(refresh) => {
                            self.complete_model_refresh(refresh, result)
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let events = self
            .rewrite_state
            .borrow()
            .operation
            .generation_job()
            .map(drain)
            .unwrap_or_default();
        controls_changed |= !events.is_empty();
        let mut changed = false;
        for event in events {
            match event {
                llm::Event::Models(_) => {}
                llm::Event::Status(status) => self.rewrite.status.set_text(&status),
                llm::Event::Text(text) => {
                    self.rewrite_state.borrow_mut().append_text(&text);
                    changed = true;
                }
                llm::Event::Finished(result) => {
                    let cancelled = self.rewrite_state.borrow_mut().operation.finish();
                    self.rewrite.status.set_text(if cancelled {
                        "Stopped. Partial output has been kept."
                    } else {
                        match &result {
                            Ok(message) => message,
                            Err(_) => "Generation failed. Partial output has been kept.",
                        }
                    });
                    if !cancelled && let Err(error) = result {
                        self.rewrite.status.set_text(&format!(
                            "Generation failed: {error}. Partial output has been kept."
                        ));
                    }
                }
            }
        }
        if changed {
            self.rewrite
                .renderer
                .borrow_mut()
                .update(&self.rewrite.result, &self.rewrite_state.borrow().markdown);
        }
        if controls_changed {
            self.refresh();
        }
    }
    fn complete_model_refresh(&self, refresh: ModelRefresh, result: Result<String, String>) {
        let current = refresh.selection_revision == self.rewrite_state.borrow().selection_revision
            && self
                .llm_connection()
                .is_ok_and(|connection| connection == refresh.connection);
        let (ids, status) = if !current {
            (Vec::new(), REFRESH_REQUIRED.to_owned())
        } else {
            match (result, refresh.models.as_deref()) {
                (Ok(_), Some([])) => (
                    Vec::new(),
                    "Connected, but the server offers no models.".into(),
                ),
                (Ok(_), Some(models)) => (
                    models.iter().map(|model| model.id.as_str()).collect(),
                    if models.len() == 1 {
                        "Connected. One model available.".into()
                    } else {
                        format!("Connected. {} models available.", models.len())
                    },
                ),
                (Ok(_), None) => (
                    Vec::new(),
                    "Model refresh failed: no model list was returned.".into(),
                ),
                (Err(error), _) => (Vec::new(), format!("Model refresh failed: {error}")),
            }
        };
        if ids.is_empty() {
            self.invalidate_llm_readiness(
                "No LLM model selected. Click Refresh Models in Settings.",
            );
        }
        self.set_llm_models(&ids, refresh.previous_model.as_deref());
        self.preferences.llm_status.set_subtitle(&status);
    }

    fn complete_model_load(&self, target: LoadTarget, result: Result<String, String>) {
        let current = target.selection_revision == self.rewrite_state.borrow().selection_revision
            && self
                .llm_connection()
                .is_ok_and(|connection| connection == target.connection)
            && self.llm_options().model.as_deref() == Some(target.model.as_str());
        let status = if !current {
            if self.llm_selected_model().is_some() {
                SELECTION_CHANGED
            } else {
                REFRESH_REQUIRED
            }
            .to_owned()
        } else if let Err(error) = result {
            format!("Model loading failed: {error}")
        } else {
            let message = target.ready_message();
            self.rewrite_state.borrow_mut().ready_model = Some(target);
            message
        };
        self.replace_model_status(MODEL_LOADING, &status);
    }

    pub(super) fn rewrite_markdown(&self) -> String {
        self.rewrite_state.borrow().markdown.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelRefresh, RewriteOperation, State};
    use crate::llm;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    #[test]
    fn rewrite_exports_only_clear_a_matching_markdown_snapshot() {
        let mut state = State::default();
        state.append_text("**Original**");
        let old = state.markdown.clone();
        state.append_text(" and newer output");
        state.mark_export_saved(&old);
        assert!(state.is_dirty());
        state.mark_export_saved("Original and newer output");
        assert!(
            state.is_dirty(),
            "TXT output is not the authoritative snapshot"
        );
        state.mark_export_saved(&state.markdown.clone());
        assert!(!state.is_dirty());
        state.reset_result();
        assert!(state.markdown.is_empty());
    }

    #[test]
    fn rewrite_operation_keeps_cancelling_job_until_finished() {
        let (_, events) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut operation = RewriteOperation::Generating(llm::Job {
            events,
            cancel: cancel.clone(),
        });
        assert!(operation.can_stop());
        assert!(operation.request_stop());
        assert!(!operation.can_stop());
        assert!(operation.is_generating());
        assert!(operation.finish());
        assert!(operation.is_idle());
        assert!(cancel.load(Ordering::Relaxed));
    }

    fn model_refresh(job: llm::Job) -> RewriteOperation {
        RewriteOperation::RefreshingModels(ModelRefresh {
            job,
            connection: llm::Connection::default(),
            selection_revision: 0,
            previous_model: None,
            models: None,
        })
    }

    #[test]
    fn rewrite_completion_and_drop_preserve_cancellation_contract() {
        for testing in [false, true] {
            let (_, events) = mpsc::sync_channel(1);
            let cancel = Arc::new(AtomicBool::new(false));
            let job = llm::Job {
                events,
                cancel: cancel.clone(),
            };
            let mut operation = if testing {
                model_refresh(job)
            } else {
                RewriteOperation::Generating(job)
            };
            assert_eq!(operation.connection_job().is_some(), testing);
            assert_eq!(operation.is_generating(), !testing);
            assert!(!operation.finish(), "normal completion is not cancellation");
            assert!(cancel.load(Ordering::Relaxed));
        }
        let (_, events) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        drop(model_refresh(llm::Job {
            events,
            cancel: cancel.clone(),
        }));
        assert!(cancel.load(Ordering::Relaxed));
    }
}
