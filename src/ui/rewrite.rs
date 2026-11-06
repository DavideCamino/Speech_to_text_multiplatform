//! Rewrite lifecycle and batched native TextBuffer Markdown rendering.
use super::{app::App, dialogs, views::buffer_text};
use crate::llm;
use adw::prelude::*;
use std::{
    rc::Rc,
    sync::{atomic::Ordering, mpsc},
};

#[derive(Default)]
pub(super) enum RewriteOperation {
    #[default]
    Idle,
    TestingConnection(llm::Job),
    Generating(llm::Job),
}

impl RewriteOperation {
    fn connection_job(&self) -> Option<&llm::Job> {
        match self {
            Self::TestingConnection(job) => Some(job),
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

    pub(super) fn is_testing(&self) -> bool {
        self.connection_job().is_some()
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
    markdown: String,
    unexported: bool,
}

impl State {
    pub(super) fn is_dirty(&self) -> bool {
        self.unexported
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

pub(super) fn plain_text(markdown: &str) -> String {
    use pulldown_cmark::{Event, Parser, TagEnd};
    let mut text = String::new();
    for event in Parser::new(markdown) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            Event::SoftBreak | Event::HardBreak | Event::Rule => text.push('\n'),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::BlockQuote(_),
            ) if !text.ends_with('\n') => text.push('\n'),
            _ => {}
        }
    }
    text.trim_end().to_owned()
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

    pub(super) fn refresh_rewrite_controls(&self) {
        let busy = self.busy();
        let (generating, testing, can_stop, has_result) = {
            let state = self.rewrite_state.borrow();
            (
                state.operation.is_generating(),
                state.operation.is_testing(),
                state.operation.can_stop(),
                !state.markdown.trim().is_empty(),
            )
        };
        let connection_editable = !generating && !testing;
        self.preferences.llm_host.set_sensitive(connection_editable);
        self.preferences.llm_port.set_sensitive(connection_editable);
        self.preferences
            .llm_https
            .set_sensitive(connection_editable);
        self.preferences
            .llm_token
            .set_sensitive(connection_editable);
        self.preferences.llm_test.set_sensitive(connection_editable);
        self.rewrite.run.set_sensitive(!busy && !testing);
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
    pub(super) fn test_llm(&self) {
        if !self.rewrite_state.borrow().operation.is_idle() {
            return;
        }
        match self.llm_connection() {
            Ok(connection) => {
                self.settings.borrow_mut().llm = connection.clone();
                self.persist();
                self.preferences.llm_status.set_subtitle("Connecting…");
                self.rewrite_state.borrow_mut().operation =
                    RewriteOperation::TestingConnection(llm::start(
                        connection,
                        self.preferences.llm_token.text().to_string(),
                        None,
                    ));
                self.refresh();
            }
            Err(error) => self.preferences.llm_status.set_subtitle(&error.to_string()),
        }
    }
    pub(super) fn start_rewrite(&self) {
        if self.busy() || self.rewrite_state.borrow().operation.is_testing() {
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
        self.settings.borrow_mut().llm = connection.clone();
        self.persist();
        {
            let mut state = self.rewrite_state.borrow_mut();
            state.reset_result();
            state.operation = RewriteOperation::Generating(llm::start(
                connection,
                self.preferences.llm_token.text().to_string(),
                Some((prompt, text)),
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
        let test_events = self
            .rewrite_state
            .borrow()
            .operation
            .connection_job()
            .map(drain)
            .unwrap_or_default();
        let mut controls_changed = !test_events.is_empty();
        for event in test_events {
            if let llm::Event::Finished(result) = event {
                self.rewrite_state.borrow_mut().operation.finish();
                self.preferences
                    .llm_status
                    .set_subtitle(&result.unwrap_or_else(|e| format!("Connection failed: {e}")));
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
    pub(super) fn rewrite_markdown(&self) -> String {
        self.rewrite_state.borrow().markdown.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{RewriteOperation, State, plain_text};
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
                RewriteOperation::TestingConnection(job)
            } else {
                RewriteOperation::Generating(job)
            };
            assert_eq!(operation.is_testing(), testing);
            assert_eq!(operation.is_generating(), !testing);
            assert!(!operation.finish(), "normal completion is not cancellation");
            assert!(cancel.load(Ordering::Relaxed));
        }
        let (_, events) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        drop(RewriteOperation::TestingConnection(llm::Job {
            events,
            cancel: cancel.clone(),
        }));
        assert!(cancel.load(Ordering::Relaxed));
    }
    #[test]
    fn plain_text_export() {
        assert_eq!(
            plain_text("# Heading\n\n**bold** and `code`"),
            "Heading\nbold and code"
        );
        assert_eq!(
            plain_text("![alt](file:///secret) [label](https://example.com)"),
            "alt label"
        );
    }
}
