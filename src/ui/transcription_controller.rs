//! Transcription controls, session lifecycle, and batched transcript updates.
use super::{app::App, dialogs, lifecycle::WindowLifecycle, views::buffer_text};
use crate::transcription::{self, Completion, Event, Job, Source};
use adw::prelude::*;
use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionSource {
    File,
    Microphone,
}

/// The source classification cannot outlive the job it describes. Atomic job
/// flags remain authoritative for Stop/Cancel, including cancellation on drop.
#[derive(Default)]
pub(super) enum TranscriptionSession {
    #[default]
    Idle,
    Running {
        source: SessionSource,
        job: Job,
    },
}

impl TranscriptionSession {
    pub(super) fn job(&self) -> Option<&Job> {
        match self {
            Self::Idle => None,
            Self::Running { job, .. } => Some(job),
        }
    }

    pub(super) fn is_running(&self) -> bool {
        self.job().is_some()
    }

    fn is_recording(&self) -> bool {
        matches!(
            self,
            Self::Running {
                source: SessionSource::Microphone,
                ..
            }
        )
    }

    pub(super) fn cancelling(&self) -> bool {
        self.job()
            .is_some_and(|job| job.cancel.load(Ordering::Relaxed))
    }

    fn stopping(&self) -> bool {
        self.job().is_some_and(|job| {
            job.stop.load(Ordering::Relaxed) || job.cancel.load(Ordering::Relaxed)
        })
    }

    fn request_stop(&self) -> bool {
        if !self.is_recording() || self.stopping() {
            return false;
        }
        if let Some(job) = self.job() {
            job.stop.store(true, Ordering::Relaxed);
        }
        true
    }

    fn request_cancel(&self) -> bool {
        if let Some(job) = self.job() {
            job.cancel.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Capture cancellation before dropping the job (Drop also sets its flag).
    pub(super) fn finish(&mut self) -> bool {
        let cancelled = self.cancelling();
        *self = Self::Idle;
        cancelled
    }
}

impl App {
    #[cfg(test)]
    pub(super) fn queue_audio(self: &Rc<Self>, path: std::path::PathBuf) {
        self.queue_audios(vec![path]);
    }

    pub(super) fn queue_audios(self: &Rc<Self>, paths: Vec<std::path::PathBuf>) {
        // File chooser callbacks can arrive after another operation has started.
        if self.busy() || paths.is_empty() {
            return;
        }
        self.audio_queue.borrow_mut().replace(paths);
        self.rebuild_audio_queue();
        self.main.status.set_text(
            "Audio selected. Reorder files if needed, then click Start. Cancel returns to recording.",
        );
        self.refresh();
    }

    pub(super) fn start_selected_source(self: &Rc<Self>) {
        let source = self
            .audio_queue
            .borrow()
            .selected()
            .cloned()
            .map(Source::File)
            .unwrap_or(Source::Microphone);
        self.request_start(source);
    }

    pub(super) fn request_stop(&self) {
        if !self.session.borrow().request_stop() {
            return;
        }
        self.main.status.set_text(
            "Stopping recording… finishing buffered audio. Any model load must finish first.",
        );
        self.refresh();
    }

    pub(super) fn request_cancel(&self) {
        let transcribing = self.session.borrow().request_cancel();
        let mut queue = self.audio_queue.borrow_mut();
        let cleared_audio = !queue.files.is_empty();
        // Keep offsets and boundaries until the terminal event, so any final
        // cues already queued by the worker can still be applied correctly.
        if !transcribing || queue.batch.is_none() {
            queue.clear();
        }
        drop(queue);
        let downloading = self.model_state.borrow().operation.request_cancel();
        if !transcribing && !downloading {
            if cleared_audio {
                self.main
                    .status
                    .set_text("Audio selection cleared. Ready to record.");
                self.refresh();
            }
            return;
        }
        self.main.status.set_text(if downloading {
            "Cancelling model operation…"
        } else {
            "Cancelling… inference will abort; any model load must finish first."
        });
        self.refresh();
    }

    pub(super) fn render_transcript(&self) {
        self.document.render();
        self.refresh();
    }

    pub(super) fn clear_transcript(&self) {
        self.document.reset();
    }

    pub(super) fn has_transcript(&self) -> bool {
        self.document.has_segments() || !buffer_text(&self.main.transcript).is_empty()
    }

    pub(super) fn request_start(self: &Rc<Self>, source: Source) {
        if self.busy() {
            return;
        }
        self.persist_controls();
        if let Some(language) = &self.settings.borrow().language
            && whisper_rs::get_lang_id(language).is_none()
        {
            dialogs::message(
                &self.window,
                "Unknown language",
                "Use a Whisper language code such as en, it, fr, de, or auto.",
            );
            return;
        }
        if !self.has_transcript() {
            self.start(source, false);
            return;
        }
        let weak = Rc::downgrade(self);
        let selection = self.audio_queue.borrow().revision;
        // Dialog responses may occur only once, but use a cell to move Source from an Fn callback.
        let source = RefCell::new(Some(source));
        dialogs::confirm(
            &self.window,
            "Start a new transcription?",
            "Replace the current transcript, or append the new audio after its last subtitle timestamp? Export any text you want to keep first.",
            &[("replace", "Replace"), ("append", "Append")],
            move |response| {
                if (response == "replace" || response == "append")
                    && let (Some(app), Some(source)) = (weak.upgrade(), source.borrow_mut().take())
                {
                    if app.audio_queue.borrow().revision != selection {
                        return;
                    }
                    app.start(source, response == "append");
                }
            },
        );
    }

    pub(super) fn recognition_context(&self) -> String {
        buffer_text(&self.main.context).trim().to_owned()
    }

    pub(super) fn start(&self, source: Source, append: bool) {
        if self.busy() {
            return;
        }
        if !append {
            self.clear_transcript();
        } else {
            self.document.prepare_append();
        }
        self.document.clear_provisional();
        self.render_transcript();
        let offset = self.document.end_ms();
        let settings = self.settings.borrow().clone();
        let context = self.recognition_context();
        let (kind, job) = match source {
            Source::File(path) => {
                let paths = {
                    let mut queue = self.audio_queue.borrow_mut();
                    if queue.selected() != Some(&path) {
                        queue.replace(vec![path]);
                    }
                    queue.begin(offset);
                    queue.files.iter().map(|file| file.path.clone()).collect()
                };
                (
                    SessionSource::File,
                    transcription::start_files(paths, settings, context),
                )
            }
            Source::Microphone => {
                self.audio_queue.borrow_mut().clear();
                (
                    SessionSource::Microphone,
                    transcription::start(Source::Microphone, settings, offset, context),
                )
            }
        };
        self.main.progress.set_fraction(0.0);
        *self.session.borrow_mut() = TranscriptionSession::Running { source: kind, job };
        self.refresh();
    }

    pub(super) fn poll_transcription(&self) {
        let events: Vec<_> = self
            .session
            .borrow()
            .job()
            .map(|job| job.events.try_iter().collect())
            .unwrap_or_default();
        let mut transcript_changed = false;
        for event in events {
            match event {
                Event::Status(text) => {
                    if !self.session.borrow().stopping() {
                        self.main.status.set_text(&text);
                    }
                }
                Event::Progress(fraction) => {
                    let fraction = if fraction.is_finite() {
                        fraction.clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    self.main.progress.set_fraction(fraction);
                    if let Some(batch) = &mut self.audio_queue.borrow_mut().batch {
                        batch.fraction = batch.fraction.max(fraction);
                    }
                    self.refresh_file_progress();
                }
                Event::Final(mut segments) => {
                    if let Some(batch) = &self.audio_queue.borrow().batch {
                        for segment in &mut segments {
                            segment.start_ms = segment.start_ms.saturating_add(batch.offset_ms);
                            segment.end_ms = segment.end_ms.saturating_add(batch.offset_ms);
                        }
                    }
                    transcript_changed |= self.document.accept_final(segments);
                }
                Event::Provisional(text) => {
                    transcript_changed |= self.document.set_provisional(text);
                }
                Event::Meter { seconds, level } => {
                    if self.session.borrow().stopping() {
                        self.main.meter.set_value(0.0);
                        continue;
                    }
                    self.main.meter.set_value(level.clamp(0.0, 1.0));
                    self.main.status.set_text(&format!(
                        "Recording {:02}:{:02} · grey text is provisional",
                        seconds as u64 / 60,
                        seconds as u64 % 60
                    ));
                }
                Event::FileFinished(result) => {
                    // Render this file before preparing the next boundary. The
                    // same poll can contain events from more than one file.
                    self.document.clear_provisional();
                    self.document.render();
                    transcript_changed = false;
                    let has_next = self
                        .audio_queue
                        .borrow_mut()
                        .advance(&result, self.document.end_ms());
                    if has_next {
                        self.document.prepare_file_append();
                        self.main.progress.set_fraction(0.0);
                    }
                    self.refresh();
                }
                Event::Finished(result) => {
                    let file = matches!(
                        *self.session.borrow(),
                        TranscriptionSession::Running {
                            source: SessionSource::File,
                            ..
                        }
                    );
                    let cancelled = self.session.borrow_mut().finish();
                    self.document.clear_provisional();
                    self.document.render();
                    transcript_changed = false;
                    let batch = if file {
                        let mut queue = self.audio_queue.borrow_mut();
                        let batch = queue.batch.take();
                        queue.clear();
                        batch
                    } else {
                        None
                    };
                    self.check_model();
                    self.main.meter.set_value(0.0);
                    if cancelled {
                        self.main
                            .status
                            .set_text("Cancelled. Completed text has been kept.");
                    } else if let Some(batch) = batch
                        .filter(|batch| !batch.incomplete.is_empty() || !batch.failures.is_empty())
                    {
                        let (heading, status, mut body) = if batch.incomplete.is_empty() {
                            (
                                "Some audio files failed",
                                "Batch finished with errors. Completed text has been kept.",
                                batch.failures.join("\n\n"),
                            )
                        } else {
                            let paths = batch
                                .incomplete
                                .iter()
                                .map(|path| format!("• {}", path.display()))
                                .collect::<Vec<_>>()
                                .join("\n");
                            let mut body = format!(
                                "These audio files ended before their declared length. Text from the readable portions was kept.\n\n{paths}"
                            );
                            if !batch.failures.is_empty() {
                                body.push_str("\n\nOther files failed:\n");
                                body.push_str(&batch.failures.join("\n\n"));
                            }
                            (
                                "Incomplete audio files",
                                "Batch finished with incomplete audio. Recognized text has been kept.",
                                body,
                            )
                        };
                        if let Err(error) = &result {
                            self.main.status.set_text(
                                "Batch stopped before completion. Recognized text has been kept.",
                            );
                            body.push_str(&format!("\n\nBatch stopped: {error}"));
                        } else {
                            self.main.status.set_text(status);
                        }
                        dialogs::message(&self.window, heading, &body);
                    } else {
                        match result {
                            Ok(Completion::Complete) => {
                                self.main.status.set_text("Transcription complete.")
                            }
                            Ok(Completion::IncompleteFile) => self
                                .main
                                .status
                                .set_text("Audio ended early. Recognized text has been kept."),
                            Err(error) => {
                                self.main.status.set_text(
                                    "Transcription failed. Completed text has been kept.",
                                );
                                dialogs::message(&self.window, "Transcription failed", &error);
                            }
                        }
                    }
                    self.refresh();
                }
            }
        }
        // Final/provisional events often arrive together. Render the accepted
        // suffix and latest provisional hypothesis only once per polling tick.
        if transcript_changed {
            self.render_transcript();
        }
    }

    pub(super) fn refresh_transcript_editability(&self) {
        let idle =
            !self.session.borrow().is_running() && self.lifecycle.get() == WindowLifecycle::Open;
        self.main.transcript.set_editable(idle);
        self.main.transcript.set_cursor_visible(idle);
        self.main.context.set_editable(idle);
        self.main.context.set_cursor_visible(idle);
    }

    pub(super) fn refresh_recognition_controls(&self) {
        let busy = self.busy();
        self.preferences.stt_apply.set_sensitive(!busy);
        self.preferences
            .gpu
            .set_sensitive(!busy && transcription::gpu_available_in_build());
        self.preferences.language.set_sensitive(!busy);
        self.preferences.refresh_device_controls(busy);
    }

    pub(super) fn refresh_transcription_controls(&self) {
        let busy = self.busy();
        let has_model = self.model_state.borrow().availability.is_available();
        let has_text = !buffer_text(&self.main.transcript).trim().is_empty();
        let downloading = self.model_state.borrow().operation.is_running();
        // Release all state borrows before setting GTK properties, which can
        // synchronously emit notifications and invoke other UI callbacks.
        let (transcribing, can_stop, session_cancelling) = {
            let session = self.session.borrow();
            (
                session.is_running(),
                session.is_recording() && !session.stopping(),
                session.cancelling(),
            )
        };
        let cancelling = session_cancelling || self.model_state.borrow().operation.cancelling();
        let has_segments = self.document.has_segments();
        let selected_audio = if session_cancelling {
            None
        } else {
            self.audio_queue.borrow().selected().cloned()
        };
        let has_audio = selected_audio.is_some();
        self.main
            .record
            .set_label(if has_audio { "Start" } else { "Record" });
        self.main.record.set_tooltip_text(Some(if has_audio {
            "Transcribe the selected audio files in order"
        } else {
            "Record from the microphone"
        }));
        self.main.selected_audio.set_visible(has_audio);
        self.main.audio_expander.set_visible(has_audio);
        self.main.audio_list.set_sensitive(!busy);
        self.refresh_file_progress();
        if let Some(path) = selected_audio {
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            let count = self.audio_queue.borrow().files.len();
            self.main.selected_audio.set_text(&if count > 1 {
                format!("{count} audio files · {name}")
            } else {
                format!("Audio: {name}")
            });
            self.main
                .selected_audio
                .set_tooltip_text(Some(&path.to_string_lossy()));
        } else {
            self.main.selected_audio.set_text("");
            self.main.selected_audio.set_tooltip_text(None);
        }
        self.main.open.set_sensitive(!busy && has_model);
        self.main.record.set_sensitive(!busy && has_model);
        self.main.stop.set_sensitive(can_stop);
        self.main
            .cancel
            .set_sensitive((transcribing || downloading || (has_audio && !busy)) && !cancelling);
        self.main.srt.set_sensitive(has_segments && !busy);
        for button in [&self.main.copy, &self.main.txt] {
            button.set_sensitive(has_text && !busy);
        }
        self.main.rewrite.set_sensitive(has_text && !busy);
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionSource, TranscriptionSession};
    use crate::transcription::Job;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    fn session(source: SessionSource) -> (TranscriptionSession, Arc<AtomicBool>, Arc<AtomicBool>) {
        let (_, events) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        (
            TranscriptionSession::Running {
                source,
                job: Job {
                    events,
                    stop: stop.clone(),
                    cancel: cancel.clone(),
                },
            },
            stop,
            cancel,
        )
    }

    #[test]
    fn stop_then_cancel_retains_session_until_completion() {
        let (mut session, stop, cancel) = session(SessionSource::Microphone);
        assert!(session.request_stop());
        assert!(stop.load(Ordering::Relaxed));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(!session.request_stop());
        assert!(session.is_running() && session.stopping());
        assert!(session.request_cancel());
        assert!(session.cancelling());
        assert!(session.is_running());
        assert!(session.finish());
        assert!(!session.is_running());
    }

    #[test]
    fn files_cannot_stop_and_finishing_captures_flags_before_drop() {
        let (mut session, stop, cancel) = session(SessionSource::File);
        assert!(!session.request_stop());
        assert!(!stop.load(Ordering::Relaxed));
        assert!(!session.finish(), "normal completion is not cancellation");
        assert!(
            cancel.load(Ordering::Relaxed),
            "finishing still drops the job"
        );
        assert!(!session.request_cancel());
    }

    #[test]
    fn dropping_a_session_cancels_its_job() {
        let (session, _, cancel) = session(SessionSource::File);
        drop(session);
        assert!(cancel.load(Ordering::Relaxed));
    }
}
