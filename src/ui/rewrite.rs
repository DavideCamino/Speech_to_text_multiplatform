//! Rewrite lifecycle and batched native TextBuffer Markdown rendering.
use super::{app::App, dialogs, views::buffer_text};
use crate::llm;
use adw::prelude::*;
use std::{
    rc::Rc,
    sync::{atomic::Ordering, mpsc},
};

#[derive(Default)]
pub(super) struct State {
    pub job: Option<llm::Job>,
    pub test: Option<llm::Job>,
    pub markdown: String,
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
            if self.rewrite_dirty.get() {
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
        self.rewrite_state.borrow_mut().markdown.clear();
        self.rewrite.renderer.borrow_mut().reset();
        self.rewrite.status.set_text("Ready");
        self.rewrite_dirty.set(false);
        self.stack.set_visible_child_name("rewrite");
        self.refresh();
    }

    pub(super) fn refresh_rewrite_controls(&self) {
        let busy = self.busy();
        let state = self.rewrite_state.borrow();
        let generating = state.job.is_some();
        let connection_editable = !generating && state.test.is_none();
        self.preferences.llm_host.set_sensitive(connection_editable);
        self.preferences.llm_port.set_sensitive(connection_editable);
        self.preferences
            .llm_https
            .set_sensitive(connection_editable);
        self.preferences
            .llm_token
            .set_sensitive(connection_editable);
        self.preferences.llm_test.set_sensitive(connection_editable);
        self.rewrite
            .run
            .set_sensitive(!busy && state.test.is_none());
        self.rewrite.prompt.set_editable(!generating);
        self.rewrite.back.set_sensitive(!generating);
        self.rewrite.stop.set_sensitive(
            state
                .job
                .as_ref()
                .is_some_and(|job| !job.cancel.load(Ordering::Relaxed)),
        );
        let has_result = !state.markdown.trim().is_empty();
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
        if self.rewrite_state.borrow().test.is_some() || self.rewrite_state.borrow().job.is_some() {
            return;
        }
        match self.llm_connection() {
            Ok(connection) => {
                self.settings.borrow_mut().llm = connection.clone();
                self.persist();
                self.preferences.llm_status.set_subtitle("Connecting…");
                self.preferences.llm_test.set_sensitive(false);
                self.rewrite_state.borrow_mut().test = Some(llm::start(
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
        if self.busy() || self.rewrite_state.borrow().test.is_some() {
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
            state.markdown.clear();
            state.job = Some(llm::start(
                connection,
                self.preferences.llm_token.text().to_string(),
                Some((prompt, text)),
            ));
        }
        self.rewrite.renderer.borrow_mut().reset();
        self.rewrite_dirty.set(false);
        self.rewrite.status.set_text("Connecting and generating…");
        self.refresh();
    }
    pub(super) fn poll_rewrite(&self) {
        let test_events = self
            .rewrite_state
            .borrow()
            .test
            .as_ref()
            .map(drain)
            .unwrap_or_default();
        let mut controls_changed = !test_events.is_empty();
        for event in test_events {
            if let llm::Event::Finished(result) = event {
                self.rewrite_state.borrow_mut().test.take();
                self.preferences.llm_test.set_sensitive(true);
                self.preferences
                    .llm_status
                    .set_subtitle(&result.unwrap_or_else(|e| format!("Connection failed: {e}")));
            }
        }
        let events = self
            .rewrite_state
            .borrow()
            .job
            .as_ref()
            .map(drain)
            .unwrap_or_default();
        controls_changed |= !events.is_empty();
        let mut changed = false;
        for event in events {
            match event {
                llm::Event::Text(text) => {
                    self.rewrite_state.borrow_mut().markdown.push_str(&text);
                    self.rewrite_dirty.set(true);
                    changed = true;
                }
                llm::Event::Finished(result) => {
                    let job = self.rewrite_state.borrow_mut().job.take();
                    let cancelled = job
                        .as_ref()
                        .is_some_and(|job| job.cancel.load(Ordering::Relaxed));
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
    use super::plain_text;
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
