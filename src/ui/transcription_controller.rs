//! Transcription controls, session lifecycle, and batched transcript updates.
use super::{app::App, dialogs, views::buffer_text};
use crate::{
    transcript::Transcript,
    transcription::{self, Event, Source},
};
use adw::prelude::*;
use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering};

impl App {
    pub(super) fn job_stopping(&self) -> bool {
        self.job.borrow().as_ref().is_some_and(|job| {
            job.stop.load(Ordering::Relaxed) || job.cancel.load(Ordering::Relaxed)
        })
    }

    pub(super) fn render_transcript(&self) {
        self.rendering_transcript.set(true);
        self.transcript_buffer
            .borrow_mut()
            .update(&self.transcript.borrow(), &self.provisional.borrow());
        self.rendering_transcript.set(false);
        self.refresh();
    }

    pub(super) fn clear_transcript(&self) {
        self.rendering_transcript.set(true);
        *self.transcript.borrow_mut() = Transcript::default();
        self.transcript_buffer.borrow_mut().reset();
        self.provisional.borrow_mut().clear();
        self.rendering_transcript.set(false);
        self.dirty.set(false);
        self.text_dirty.set(false);
    }

    pub(super) fn has_transcript(&self) -> bool {
        !self.transcript.borrow().segments.is_empty()
            || !buffer_text(&self.main.transcript).is_empty()
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
            self.transcript_buffer.borrow_mut().prepare_append();
        }
        self.provisional.borrow_mut().clear();
        self.render_transcript();
        let offset = self.transcript.borrow().end_ms();
        self.recording.set(matches!(source, Source::Microphone));
        self.main.progress.set_fraction(0.0);
        *self.job.borrow_mut() = Some(transcription::start(
            source,
            self.settings.borrow().clone(),
            offset,
            self.recognition_context(),
        ));
        self.refresh();
    }

    pub(super) fn poll_transcription(&self) {
        let events: Vec<_> = self
            .job
            .borrow()
            .as_ref()
            .map(|job| job.events.try_iter().collect())
            .unwrap_or_default();
        let mut transcript_changed = false;
        for event in events {
            match event {
                Event::Status(text) => {
                    if !self.job_stopping() {
                        self.main.status.set_text(&text);
                    }
                }
                Event::Progress(fraction) => self.main.progress.set_fraction(fraction),
                Event::Final(segments) => {
                    if !segments.is_empty() {
                        let mut transcript = self.transcript.borrow_mut();
                        let previous_len = transcript.segments.len();
                        transcript.append(segments);
                        if transcript.segments.len() > previous_len {
                            self.dirty.set(true);
                            transcript_changed = true;
                        }
                    }
                }
                Event::Provisional(text) => {
                    let mut provisional = self.provisional.borrow_mut();
                    if *provisional != text {
                        *provisional = text;
                        transcript_changed = true;
                    }
                }
                Event::Meter { seconds, level } => {
                    if self.job_stopping() {
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
                    let cancelled = self
                        .job
                        .borrow()
                        .as_ref()
                        .is_some_and(|j| j.cancel.load(Ordering::Relaxed));
                    self.job.borrow_mut().take();
                    self.check_model();
                    self.recording.set(false);
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
        let idle = self.job.borrow().is_none() && !self.closing.get();
        self.main.transcript.set_editable(idle);
        self.main.transcript.set_cursor_visible(idle);
        self.main.context.set_editable(idle);
        self.main.context.set_cursor_visible(idle);
    }

    pub(super) fn refresh_recognition_controls(&self) {
        let busy = self.busy();
        self.preferences
            .gpu
            .set_sensitive(!busy && transcription::gpu_available_in_build());
        self.preferences.language.set_sensitive(!busy);
        self.preferences.refresh_device_controls(busy);
    }

    pub(super) fn refresh_transcription_controls(&self) {
        let busy = self.busy();
        let has_model = self.model_available.get();
        let has_text = !buffer_text(&self.main.transcript).trim().is_empty();
        let downloading = self.download.borrow().is_some();
        self.main.open.set_sensitive(!busy && has_model);
        self.main.record.set_sensitive(!busy && has_model);
        self.main
            .stop
            .set_sensitive(self.recording.get() && !self.job_stopping());
        let cancelling = self
            .job
            .borrow()
            .as_ref()
            .is_some_and(|job| job.cancel.load(Ordering::Relaxed))
            || self
                .download
                .borrow()
                .as_ref()
                .is_some_and(|job| job.cancel.load(Ordering::Relaxed));
        self.main
            .cancel
            .set_sensitive((self.job.borrow().is_some() || downloading) && !cancelling);
        self.main
            .srt
            .set_sensitive(!self.transcript.borrow().segments.is_empty() && !busy);
        for button in [&self.main.copy, &self.main.txt] {
            button.set_sensitive(has_text && !busy);
        }
        self.main.rewrite.set_sensitive(has_text && !busy);
    }
}
