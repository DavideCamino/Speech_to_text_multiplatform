//! GTK stays on the main thread. Workers communicate through typed messages;
//! no widget is ever sent to an audio, network, or inference thread.
mod dialogs;
mod views;

use crate::{
    model,
    settings::{self, Settings},
    transcript::Transcript,
    transcription::{self, Event, Job, Source},
};
use adw::prelude::*;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::Duration,
};
use views::{MainView, RewriteView, buffer_text};

enum DownloadEvent {
    Progress(f64),
    Finished(Result<PathBuf, String>),
}
struct DownloadJob {
    events: Receiver<DownloadEvent>,
    cancel: Arc<AtomicBool>,
}
impl Drop for DownloadJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct App {
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    main: MainView,
    rewrite: RewriteView,
    settings: RefCell<Settings>,
    transcript: RefCell<Transcript>,
    provisional: RefCell<String>,
    job: RefCell<Option<Job>>,
    download: RefCell<Option<DownloadJob>>,
    recording: Cell<bool>,
    dirty: Cell<bool>,
    rewrite_dirty: Cell<bool>,
    allow_close: Cell<bool>,
}

pub fn build(application: &adw::Application) {
    if let Some(window) = application.active_window() {
        window.present();
        return;
    }
    let (settings, load_error) = match Settings::load() {
        Ok(settings) => (settings, None),
        Err(error) => (Settings::default(), Some(format!("{error:#}"))),
    };
    let main = MainView::new(&settings);
    let rewrite = RewriteView::new();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::SlideLeftRight)
        .build();
    stack.add_named(&main.root, Some("main"));
    stack.add_named(&rewrite.root, Some("rewrite"));
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        "Speech2Text",
        "Private, on-device transcription",
    )));
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Speech2Text")
        .default_width(980)
        .default_height(820)
        .width_request(600)
        .height_request(600)
        .content(&toolbar)
        .build();
    let app = Rc::new(App {
        window,
        stack,
        main,
        rewrite,
        settings: RefCell::new(settings),
        transcript: RefCell::default(),
        provisional: RefCell::default(),
        job: RefCell::default(),
        download: RefCell::default(),
        recording: Cell::new(false),
        dirty: Cell::new(false),
        rewrite_dirty: Cell::new(false),
        allow_close: Cell::new(false),
    });
    app.connect();
    app.update_model_label();
    app.refresh();
    app.window.present();
    if let Some(error) = load_error {
        dialogs::message(&app.window, "Preferences could not be loaded", &error);
    }
    // This main-loop source owns the controller until the window closes. All
    // widget callbacks hold weak references, avoiding widget/controller cycles.
    gtk::glib::timeout_add_local(Duration::from_millis(80), move || {
        if !app.window.is_visible() {
            app.job.borrow_mut().take();
            app.download.borrow_mut().take();
            return gtk::glib::ControlFlow::Break;
        }
        app.poll();
        let orientation = if app.window.width() < 800 {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        };
        app.rewrite.panes.set_orientation(orientation);
        gtk::glib::ControlFlow::Continue
    });
}

fn on_click(button: &gtk::Button, app: &Rc<App>, action: impl Fn(Rc<App>) + 'static) {
    let weak = Rc::downgrade(app);
    button.connect_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            action(app);
        }
    });
}

impl App {
    fn connect(self: &Rc<Self>) {
        on_click(&self.main.select_model, self, |app| {
            let weak = Rc::downgrade(&app);
            dialogs::open(&app.window, true, move |path| {
                if let Some(app) = weak.upgrade() {
                    app.settings.borrow_mut().model = Some(path);
                    app.persist();
                    app.update_model_label();
                    app.refresh();
                }
            });
        });
        on_click(&self.main.download, self, |app| app.ask_download());
        on_click(&self.main.open, self, |app| {
            let weak = Rc::downgrade(&app);
            dialogs::open(&app.window, false, move |path| {
                if let Some(app) = weak.upgrade() {
                    app.request_start(Source::File(path));
                }
            });
        });
        on_click(&self.main.record, self, |app| {
            app.request_start(Source::Microphone)
        });
        on_click(&self.main.stop, self, |app| {
            if let Some(job) = app.job.borrow().as_ref() {
                job.stop.store(true, Ordering::Relaxed);
            }
            app.main.stop.set_sensitive(false);
            app.main.status.set_text("Finishing buffered audio…");
        });
        on_click(&self.main.cancel, self, |app| {
            if let Some(job) = app.job.borrow().as_ref() {
                job.cancel.store(true, Ordering::Relaxed);
            }
            if let Some(job) = app.download.borrow().as_ref() {
                job.cancel.store(true, Ordering::Relaxed);
            }
            app.main
                .status
                .set_text("Cancelling… waiting for the current operation to finish");
            app.main.cancel.set_sensitive(false);
        });
        on_click(&self.main.copy, self, |app| {
            app.window
                .clipboard()
                .set_text(&app.transcript.borrow().text())
        });
        on_click(&self.main.txt, self, |app| {
            let content = app.transcript.borrow().text();
            app.export("transcript.txt", content, false);
        });
        on_click(&self.main.srt, self, |app| {
            let content = app.transcript.borrow().srt();
            app.export("transcript.srt", content, false);
        });
        on_click(&self.main.rewrite, self, |app| app.enter_rewrite());
        on_click(&self.rewrite.back, self, |app| {
            app.stack.set_visible_child_name("main")
        });
        on_click(&self.rewrite.run, self, |app| {
            let run = move |app: &App| {
                app.rewrite
                    .result
                    .buffer()
                    .set_text(&buffer_text(&app.rewrite.source));
                app.rewrite_dirty.set(true);
                app.refresh();
            };
            if app.rewrite_dirty.get() {
                let weak = Rc::downgrade(&app);
                dialogs::confirm(
                    &app.window,
                    "Replace rewrite result?",
                    "The current result has not been exported.",
                    &[("replace", "Replace")],
                    move |response| {
                        if response == "replace"
                            && let Some(app) = weak.upgrade()
                        {
                            run(&app);
                        }
                    },
                );
            } else {
                run(&app);
            }
        });
        on_click(&self.rewrite.copy, self, |app| {
            app.window
                .clipboard()
                .set_text(&buffer_text(&app.rewrite.result))
        });
        on_click(&self.rewrite.txt, self, |app| {
            app.export("rewritten.txt", buffer_text(&app.rewrite.result), true)
        });
        on_click(&self.rewrite.markdown, self, |app| {
            app.export("rewritten.md", buffer_text(&app.rewrite.result), true)
        });
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(app) = weak.upgrade() else { return gtk::glib::Propagation::Proceed; };
            if app.allow_close.get() { return gtk::glib::Propagation::Proceed; }
            if app.busy() || app.dirty.get() || app.rewrite_dirty.get() {
                let weak = Rc::downgrade(&app);
                dialogs::confirm(&app.window, "Close Speech2Text?",
                    "Active work will be cancelled and any unexported text will be discarded. Audio and transcripts are not saved automatically.",
                    &[("close", "Discard and Close")], move |response| {
                        if response == "close" && let Some(app) = weak.upgrade() {
                            app.persist_controls(); app.allow_close.set(true); app.window.close();
                        }
                    });
                gtk::glib::Propagation::Stop
            } else { app.persist_controls(); gtk::glib::Propagation::Proceed }
        });
    }

    fn busy(&self) -> bool {
        self.job.borrow().is_some() || self.download.borrow().is_some()
    }

    fn persist(&self) {
        if let Err(error) = self.settings.borrow().save() {
            self.main
                .status
                .set_text(&format!("Could not save preferences: {error:#}"));
        }
    }

    fn persist_controls(&self) {
        {
            let mut settings = self.settings.borrow_mut();
            let language = self.main.language.text().trim().to_lowercase();
            settings.language = if language.is_empty() || language == "auto" {
                None
            } else {
                Some(language)
            };
            settings.use_gpu = self.main.gpu.is_active();
            settings.device = self
                .main
                .device
                .selected()
                .checked_sub(1)
                .and_then(|index| self.main.devices.get(index as usize).cloned());
        }
        self.persist();
    }

    fn update_model_label(&self) {
        let settings = self.settings.borrow();
        let text = settings
            .model
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No model selected".into());
        self.main.model_label.set_text(&text);
        self.main.model_label.set_tooltip_text(
            settings
                .model
                .as_ref()
                .map(|p| p.to_string_lossy())
                .as_deref(),
        );
    }

    fn refresh(&self) {
        let busy = self.busy();
        let has_model = self
            .settings
            .borrow()
            .model
            .as_ref()
            .is_some_and(|p| p.is_file());
        let has_text = !self.transcript.borrow().segments.is_empty();
        for widget in [&self.main.select_model, &self.main.download] {
            widget.set_sensitive(!busy);
        }
        self.main.catalog.set_sensitive(!busy);
        self.main
            .gpu
            .set_sensitive(!busy && transcription::gpu_available_in_build());
        self.main.language.set_sensitive(!busy);
        self.main.device.set_sensitive(!busy);
        self.main.open.set_sensitive(!busy && has_model);
        self.main.record.set_sensitive(!busy && has_model);
        self.main.stop.set_sensitive(self.recording.get());
        self.main.cancel.set_sensitive(busy);
        for button in [&self.main.copy, &self.main.txt, &self.main.srt] {
            button.set_sensitive(has_text && !busy);
        }
        self.main.rewrite.set_sensitive(has_text && !busy);
        let has_result = !buffer_text(&self.rewrite.result).trim().is_empty();
        for button in [
            &self.rewrite.copy,
            &self.rewrite.txt,
            &self.rewrite.markdown,
        ] {
            button.set_sensitive(has_result);
        }
    }

    fn render_transcript(&self) {
        let buffer = self.main.transcript.buffer();
        buffer.set_text(&self.transcript.borrow().text());
        let provisional = self.provisional.borrow();
        if !provisional.is_empty() {
            let mut end = buffer.end_iter();
            buffer.insert(&mut end, " ");
            buffer.insert_with_tags_by_name(&mut end, &provisional, &["provisional"]);
        }
    }

    fn request_start(self: &Rc<Self>, source: Source) {
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
        if self.transcript.borrow().segments.is_empty() {
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

    fn start(&self, source: Source, append: bool) {
        if self.busy() {
            return;
        }
        if !append {
            *self.transcript.borrow_mut() = Transcript::default();
            self.dirty.set(false);
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
        ));
        self.refresh();
    }

    fn export(self: &Rc<Self>, name: &str, content: String, rewrite: bool) {
        let weak = Rc::downgrade(self);
        // Compare snapshots so an asynchronous save cannot mark newer text saved.
        let snapshot = content.clone();
        dialogs::save(&self.window, name, content, move || {
            if let Some(app) = weak.upgrade() {
                if rewrite {
                    if buffer_text(&app.rewrite.result) == snapshot {
                        app.rewrite_dirty.set(false);
                    }
                } else {
                    let transcript = app.transcript.borrow();
                    if transcript.text() == snapshot || transcript.srt() == snapshot {
                        app.dirty.set(false);
                    }
                }
                app.main.status.set_text("Export saved.");
            }
        });
    }

    fn enter_rewrite(self: &Rc<Self>) {
        let source = self.transcript.borrow().text();
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

    fn reset_rewrite(&self) {
        self.rewrite
            .source
            .buffer()
            .set_text(&self.transcript.borrow().text());
        self.rewrite.result.buffer().set_text("");
        self.rewrite_dirty.set(false);
        self.stack.set_visible_child_name("rewrite");
        self.refresh();
    }

    fn ask_download(self: &Rc<Self>) {
        let index = self.main.catalog.selected() as usize;
        let Some(model) = model::CATALOG.get(index) else {
            return;
        };
        let weak = Rc::downgrade(self);
        dialogs::confirm(
            &self.window,
            "Download Whisper model?",
            &format!(
                "{} will be downloaded from huggingface.co to the application's data directory. No audio or transcript is uploaded. The file's SHA-256 checksum will be verified.",
                model.name
            ),
            &[("download", "Download")],
            move |response| {
                if response == "download"
                    && let Some(app) = weak.upgrade()
                {
                    app.start_download(index);
                }
            },
        );
    }

    fn start_download(&self, index: usize) {
        if self.busy() {
            return;
        }
        let directory = match settings::model_dir() {
            Ok(path) => path,
            Err(error) => {
                dialogs::message(&self.window, "Cannot download model", &error.to_string());
                return;
            }
        };
        let (tx, events) = mpsc::sync_channel(64);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.download.borrow_mut() = Some(DownloadJob {
            events,
            cancel: cancel.clone(),
        });
        self.main.progress.set_fraction(0.0);
        self.main.status.set_text("Downloading model…");
        self.refresh();
        std::thread::spawn(move || {
            let mut last = -1_i32;
            let result = model::download(&model::CATALOG[index], &directory, &cancel, |fraction| {
                let percent = (fraction * 100.0) as i32;
                if percent != last {
                    let _ = tx.send(DownloadEvent::Progress(fraction));
                    last = percent;
                }
            })
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(DownloadEvent::Finished(result));
        });
    }

    fn poll(&self) {
        let events: Vec<_> = self
            .job
            .borrow()
            .as_ref()
            .map(|job| job.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Status(text) => self.main.status.set_text(&text),
                Event::Progress(fraction) => self.main.progress.set_fraction(fraction),
                Event::Final(segments) => {
                    if !segments.is_empty() {
                        self.dirty.set(true);
                    }
                    self.transcript.borrow_mut().append(segments);
                    self.render_transcript();
                }
                Event::Provisional(text) => {
                    *self.provisional.borrow_mut() = text;
                    self.render_transcript();
                }
                Event::Meter { seconds, level } => {
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
        let events: Vec<_> = self
            .download
            .borrow()
            .as_ref()
            .map(|job| job.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                DownloadEvent::Progress(fraction) => self.main.progress.set_fraction(fraction),
                DownloadEvent::Finished(result) => {
                    let cancelled = self
                        .download
                        .borrow()
                        .as_ref()
                        .is_some_and(|j| j.cancel.load(Ordering::Relaxed));
                    self.download.borrow_mut().take();
                    match result {
                        Ok(path) => {
                            self.settings.borrow_mut().model = Some(path);
                            self.persist();
                            self.update_model_label();
                            self.main.status.set_text("Model verified and ready.");
                        }
                        Err(_) if cancelled => {
                            self.main.status.set_text("Model download cancelled.")
                        }
                        Err(error) => {
                            self.main.status.set_text("Model download failed.");
                            dialogs::message(&self.window, "Download failed", &error);
                        }
                    }
                    self.refresh();
                }
            }
        }
    }
}
