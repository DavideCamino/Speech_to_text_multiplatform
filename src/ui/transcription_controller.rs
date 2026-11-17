//! Transcription controls, session lifecycle, and batched transcript updates.
use super::{app::App, dialogs, lifecycle::WindowLifecycle, views::buffer_text};
use crate::transcription::{self, Event, Job, Source};
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
    pub(super) fn queue_audio(&self, path: std::path::PathBuf) {
        // File chooser callbacks can arrive after another operation has started.
        if self.busy() {
            return;
        }
        *self.selected_audio.borrow_mut() = Some(path);
        self.main.status.set_text(
            "Audio selected. Click Start to transcribe, or Cancel to return to recording.",
        );
        self.refresh();
    }

    pub(super) fn start_selected_source(self: &Rc<Self>) {
        let source = self
            .selected_audio
            .borrow()
            .clone()
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
        let cleared_audio = self.selected_audio.borrow_mut().take().is_some();
        let transcribing = self.session.borrow().request_cancel();
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
        let selection = self.selected_audio.borrow().clone();
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
                    if *app.selected_audio.borrow() != selection {
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
        let kind = match &source {
            Source::File(_) => SessionSource::File,
            Source::Microphone => SessionSource::Microphone,
        };
        *self.selected_audio.borrow_mut() = match &source {
            Source::File(path) => Some(path.clone()),
            Source::Microphone => None,
        };
        self.main.progress.set_fraction(0.0);
        let job = transcription::start(
            source,
            self.settings.borrow().clone(),
            offset,
            self.recognition_context(),
        );
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
                Event::Progress(fraction) => self.main.progress.set_fraction(fraction),
                Event::Final(segments) => {
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
                Event::Finished(result) => {
                    let file = matches!(
                        *self.session.borrow(),
                        TranscriptionSession::Running {
                            source: SessionSource::File,
                            ..
                        }
                    );
                    let cancelled = self.session.borrow_mut().finish();
                    if file {
                        self.selected_audio.borrow_mut().take();
                    }
                    self.check_model();
                    self.main.meter.set_value(0.0);
                    match result {
                        Ok(()) => self.main.status.set_text(if cancelled {
                            "Cancelled. Completed text has been kept."
                        } else {
                            "Transcription complete."
                        }),
                        Err(error) => {
                            self.main
                                .status
                                .set_text("Transcription failed. Completed text has been kept.");
                            dialogs::message(&self.window, "Transcription failed", &error);
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
        let selected_audio = self.selected_audio.borrow().clone();
        let has_audio = selected_audio.is_some();
        self.main
            .record
            .set_label(if has_audio { "Start" } else { "Record" });
        self.main.record.set_tooltip_text(Some(if has_audio {
            "Transcribe the selected audio file"
        } else {
            "Record from the microphone"
        }));
        self.main.selected_audio.set_visible(has_audio);
        if let Some(path) = selected_audio {
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            self.main.selected_audio.set_text(&format!("Audio: {name}"));
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
