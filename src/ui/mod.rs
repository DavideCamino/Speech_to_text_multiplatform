//! GTK stays on the main thread. Workers communicate through typed messages;
//! no widget is ever sent to an audio, network, or inference thread.
mod dialogs;
mod markdown_buffer;
mod menu;
mod models;
mod preferences;
mod rewrite;
mod text_zoom;
mod transcript_buffer;
mod views;

#[cfg(test)]
mod tests;

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
enum ModelTask {
    Download(usize, model::Replacement),
    Copy(PathBuf),
}
struct ModelJob {
    events: Receiver<DownloadEvent>,
    cancel: Arc<AtomicBool>,
    copying: bool,
}
impl Drop for ModelJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

type DeviceDiscovery = Receiver<Result<Vec<String>, String>>;
type ModelScan = Receiver<Result<Vec<PathBuf>, String>>;
type DownloadCheck = Receiver<Result<(usize, PathBuf, bool), String>>;

fn background<T: Send + 'static>(operation: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (tx, events) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = tx.send(operation());
    });
    events
}

fn discover_devices(
    discover: impl FnOnce() -> Result<Vec<String>, String> + Send + 'static,
) -> DeviceDiscovery {
    background(discover)
}

#[derive(Clone, Copy)]
enum ExportKind {
    TranscriptText,
    Subtitles,
    Rewrite,
}

struct App {
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    main: MainView,
    preferences: preferences::Preferences,
    models: models::Models,
    model_scan: RefCell<Option<ModelScan>>,
    model_check: RefCell<Option<Receiver<bool>>>,
    model_available: Cell<bool>,
    download_check: RefCell<Option<DownloadCheck>>,
    settings_writer: RefCell<settings::SaveWorker>,
    closing: Cell<bool>,
    rewrite: RewriteView,
    rewrite_state: RefCell<rewrite::State>,
    settings: RefCell<Settings>,
    transcript: RefCell<Transcript>,
    transcript_buffer: RefCell<transcript_buffer::TranscriptBuffer>,
    provisional: RefCell<String>,
    job: RefCell<Option<Job>>,
    download: RefCell<Option<ModelJob>>,
    device_discovery: RefCell<Option<DeviceDiscovery>>,
    recording: Cell<bool>,
    dirty: Cell<bool>,
    text_dirty: Cell<bool>,
    rendering_transcript: Cell<bool>,
    rewrite_dirty: Cell<bool>,
    allow_close: Cell<bool>,
}

pub fn build(application: &adw::Application) {
    if let Some(window) = application.active_window() {
        window.present();
        return;
    }
    let loading = adw::ApplicationWindow::builder()
        .application(application)
        .title("Speech2Text — Loading preferences…")
        .default_width(600)
        .default_height(400)
        .content(
            &gtk::Spinner::builder()
                .spinning(true)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .width_request(32)
                .height_request(32)
                .build(),
        )
        .build();
    loading.present();
    let events = background(|| Settings::load().map_err(|error| format!("{error:#}")));
    let application = application.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(30), move || {
        if !loading.is_visible() {
            return gtk::glib::ControlFlow::Break;
        }
        let result = match events.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Preferences worker stopped unexpectedly".into())
            }
        };
        let (settings, error) = match result {
            Ok(settings) => (settings, None),
            Err(error) => (Settings::default(), Some(error)),
        };
        build_loaded(&application, settings, error);
        loading.close();
        gtk::glib::ControlFlow::Break
    });
}

fn build_loaded(application: &adw::Application, settings: Settings, load_error: Option<String>) {
    let recognized_zoom = Rc::new(text_zoom::ZoomGroup::default());
    let main = MainView::new(&recognized_zoom);
    let rewrite = RewriteView::new(&recognized_zoom);
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
    header.pack_end(&menu::button());
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
    let preferences = preferences::Preferences::new(&window, &settings);
    let models = models::Models::new(&preferences.window);
    main.progress
        .bind_property("fraction", &models.progress, "fraction")
        .sync_create()
        .build();
    let transcript_buffer = RefCell::new(transcript_buffer::TranscriptBuffer::new(
        main.transcript.buffer(),
    ));
    let app = Rc::new(App {
        window,
        stack,
        main,
        preferences,
        models,
        model_scan: RefCell::default(),
        model_check: RefCell::default(),
        model_available: Cell::new(false),
        download_check: RefCell::default(),
        settings_writer: RefCell::new(settings::SaveWorker::new()),
        closing: Cell::new(false),
        rewrite,
        rewrite_state: RefCell::default(),
        settings: RefCell::new(settings),
        transcript: RefCell::default(),
        transcript_buffer,
        provisional: RefCell::default(),
        job: RefCell::default(),
        download: RefCell::default(),
        device_discovery: RefCell::default(),
        recording: Cell::new(false),
        dirty: Cell::new(false),
        text_dirty: Cell::new(false),
        rendering_transcript: Cell::new(false),
        rewrite_dirty: Cell::new(false),
        allow_close: Cell::new(false),
    });
    app.connect();
    app.refresh_devices();
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
            app.rewrite_state.borrow_mut().job.take();
            app.rewrite_state.borrow_mut().test.take();
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
    fn connect_transcript_editing(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.main.transcript.buffer().connect_changed(move |_| {
            if let Some(app) = weak.upgrade()
                && !app.rendering_transcript.get()
            {
                app.dirty.set(true);
                app.text_dirty.set(true);
                app.refresh();
            }
        });
    }

    fn connect(self: &Rc<Self>) {
        self.connect_transcript_editing();
        self.connect_menu();
        let weak = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |window| {
            if window.is_active()
                && let Some(app) = weak.upgrade()
                && !app.busy()
            {
                app.check_model();
            }
        });
        on_click(&self.preferences.device_refresh, self, |app| {
            app.refresh_devices()
        });
        let weak = Rc::downgrade(self);
        self.preferences.device.connect_selected_notify(move |_| {
            if let Some(app) = weak.upgrade() {
                app.preferences.update_device_status();
            }
        });
        let weak = Rc::downgrade(self);
        self.preferences.window.connect_close_request(move |_| {
            if let Some(app) = weak.upgrade() {
                app.persist_controls();
            }
            gtk::glib::Propagation::Proceed
        });
        on_click(&self.preferences.select_model, self, |app| {
            app.models.window.present();
            app.refresh_models();
        });
        on_click(&self.models.browse, self, |app| {
            let weak = Rc::downgrade(&app);
            dialogs::open(&app.models.window, true, move |path| {
                if let Some(app) = weak.upgrade() {
                    app.ask_import(path);
                }
            });
        });
        on_click(&self.models.download, self, |app| {
            if let Some(job) = app.download.borrow().as_ref() {
                job.cancel.store(true, Ordering::Relaxed);
                app.models.download.set_sensitive(false);
                app.main.status.set_text("Cancelling model operation…");
                app.models.status.set_text("Cancelling model operation…");
            } else {
                app.ask_download();
            }
        });
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
            app.main.status.set_text(
                "Stopping recording… finishing buffered audio. Any model load must finish first.",
            );
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
                .set_text(if app.download.borrow().is_some() {
                    "Cancelling model operation…"
                } else {
                    "Cancelling… inference will abort; any model load must finish first."
                });
            app.main.stop.set_sensitive(false);
            app.main.cancel.set_sensitive(false);
        });
        on_click(&self.main.copy, self, |app| {
            app.window
                .clipboard()
                .set_text(&buffer_text(&app.main.transcript))
        });
        on_click(&self.main.txt, self, |app| {
            let content = buffer_text(&app.main.transcript);
            app.export("transcript.txt", content, ExportKind::TranscriptText);
        });
        on_click(&self.main.srt, self, |app| {
            let content = app.transcript.borrow().srt();
            app.export("transcript.srt", content, ExportKind::Subtitles);
        });
        on_click(&self.main.rewrite, self, |app| app.enter_rewrite());
        on_click(&self.preferences.llm_test, self, |app| app.test_llm());
        on_click(&self.rewrite.stop, self, |app| {
            if let Some(job) = app.rewrite_state.borrow().job.as_ref() {
                job.cancel.store(true, Ordering::Relaxed);
            }
            app.rewrite.stop.set_sensitive(false);
            app.rewrite.status.set_text("Stopping…");
        });
        on_click(&self.rewrite.back, self, |app| {
            app.stack.set_visible_child_name("main")
        });
        on_click(&self.rewrite.run, self, |app| {
            if app.busy() {
                return;
            }
            let run = |app: &App| app.start_rewrite();
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
            app.window.clipboard().set_text(&app.rewrite_markdown())
        });
        on_click(&self.rewrite.txt, self, |app| {
            app.export(
                "rewritten.txt",
                rewrite::plain_text(&app.rewrite_markdown()),
                ExportKind::Rewrite,
            )
        });
        on_click(&self.rewrite.markdown, self, |app| {
            app.export("rewritten.md", app.rewrite_markdown(), ExportKind::Rewrite)
        });
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            let Some(app) = weak.upgrade() else { return gtk::glib::Propagation::Proceed; };
            if app.allow_close.get() { return gtk::glib::Propagation::Proceed; }
            if app.closing.get() { return gtk::glib::Propagation::Stop; }
            if app.busy() || app.dirty.get() || app.rewrite_dirty.get() {
                let weak = Rc::downgrade(&app);
                dialogs::confirm(&app.window, "Close Speech2Text?",
                    "Active work will be cancelled and any unexported text will be discarded. Audio and transcripts are not saved automatically.",
                    &[("close", "Discard and Close")], move |response| {
                        if response == "close" && let Some(app) = weak.upgrade() {
                            app.begin_close();
                        }
                    });
                gtk::glib::Propagation::Stop
            } else { app.begin_close(); gtk::glib::Propagation::Stop }
        });
    }

    fn connect_menu(self: &Rc<Self>) {
        for (name, callback) in [
            (
                "settings",
                (|app: &App| app.preferences.window.present()) as fn(&App),
            ),
            ("shortcuts", |app: &App| menu::shortcuts(&app.window)),
            ("about", |app: &App| menu::about(&app.window)),
            ("close", |app: &App| app.window.close()),
        ] {
            let action = gtk::gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(app) = weak.upgrade() {
                    callback(&app);
                }
            });
            self.window.add_action(&action);
        }
        for (name, button) in [
            ("open", &self.main.open),
            ("record", &self.main.record),
            ("stop", &self.main.stop),
            ("save", &self.main.txt),
        ] {
            let action = gtk::gio::SimpleAction::new(name, None);
            button
                .bind_property("sensitive", &action, "enabled")
                .sync_create()
                .build();
            let weak = button.downgrade();
            action.connect_activate(move |_, _| {
                if let Some(button) = weak.upgrade() {
                    button.emit_clicked();
                }
            });
            self.window.add_action(&action);
        }
        if let Some(application) = self.window.application() {
            for (action, _, accelerator) in menu::SHORTCUTS {
                application.set_accels_for_action(action, &[*accelerator]);
            }
        }
    }

    fn dialog_parent(&self) -> gtk::Window {
        if self.models.window.is_visible() {
            self.models.window.clone().upcast()
        } else if self.preferences.window.is_visible() {
            self.preferences.window.clone().upcast()
        } else {
            self.window.clone().upcast()
        }
    }

    fn busy(&self) -> bool {
        self.closing.get()
            || self.download_check.borrow().is_some()
            || self.job.borrow().is_some()
            || self.download.borrow().is_some()
            || self.rewrite_state.borrow().job.is_some()
    }

    fn job_stopping(&self) -> bool {
        self.job.borrow().as_ref().is_some_and(|job| {
            job.stop.load(Ordering::Relaxed) || job.cancel.load(Ordering::Relaxed)
        })
    }

    fn persist(&self) {
        let result = self
            .settings_writer
            .borrow_mut()
            .submit(self.settings.borrow().clone());
        if let Err(error) = result {
            self.preferences_save_finished(Err(error));
        }
    }

    fn begin_close(&self) {
        self.closing.set(true);
        self.window.set_sensitive(false);
        self.main
            .status
            .set_text("Saving preferences before closing…");
        self.job.borrow_mut().take();
        self.recording.set(false);
        self.main.meter.set_value(0.0);
        self.download.borrow_mut().take();
        self.download_check.borrow_mut().take();
        self.rewrite_state.borrow_mut().job.take();
        self.rewrite_state.borrow_mut().test.take();
        // The window stays alive until the latest save completes. Never join
        // the writer or block GTK, and never lose the final save at process exit.
        self.persist_controls();
    }

    fn preferences_save_finished(&self, result: Result<(), String>) {
        match result {
            Ok(()) if self.closing.get() => {
                self.allow_close.set(true);
                self.window.close();
            }
            Ok(()) => {}
            Err(error) => {
                self.main
                    .status
                    .set_text(&format!("Could not save preferences: {error}"));
                if self.closing.replace(false) {
                    self.window.set_sensitive(true);
                    self.refresh();
                    dialogs::message(&self.window, "Could not save preferences", &error);
                }
            }
        }
    }

    fn poll_preferences_save(&self) {
        let result = self.settings_writer.borrow_mut().poll();
        if let Some(result) = result {
            self.preferences_save_finished(result);
        }
    }

    fn check_model(&self) {
        if self.model_check.borrow().is_some() {
            return;
        }
        let path = self.settings.borrow().model.clone();
        self.model_available.set(false);
        *self.model_check.borrow_mut() = path.map(|path| background(move || path.is_file()));
        self.refresh();
    }

    fn poll_model_check(&self) {
        let result =
            self.model_check
                .borrow()
                .as_ref()
                .and_then(|events| match events.try_recv() {
                    Ok(available) => Some(available),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some(false),
                });
        if let Some(available) = result {
            self.model_check.borrow_mut().take();
            self.model_available.set(available);
            self.refresh();
        }
    }

    fn persist_controls(&self) {
        {
            let mut settings = self.settings.borrow_mut();
            let language = self.preferences.language.text().trim().to_lowercase();
            settings.language = if language.is_empty() || language == "auto" {
                None
            } else {
                Some(language)
            };
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

    fn refresh_devices(&self) {
        if self.busy() || self.device_discovery.borrow().is_some() {
            return;
        }
        self.preferences.begin_device_refresh();
        *self.device_discovery.borrow_mut() = Some(discover_devices(|| {
            crate::audio::input_devices().map_err(|error| format!("{error:#}"))
        }));
    }

    fn update_model_label(&self) {
        let settings = self.settings.borrow();
        let text = settings
            .model
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No model selected".into());
        self.preferences.model_label.set_text(&text);
        self.preferences.select_model.set_tooltip_text(
            settings
                .model
                .as_ref()
                .map(|p| p.to_string_lossy())
                .as_deref(),
        );
        drop(settings);
        // Replacing the receiver discards late results for an older selection.
        self.model_check.borrow_mut().take();
        self.check_model();
    }

    fn refresh(&self) {
        let busy = self.busy();
        let has_model = self.model_available.get();
        let has_text = !buffer_text(&self.main.transcript).trim().is_empty();
        let idle = self.job.borrow().is_none() && !self.closing.get();
        self.main.transcript.set_editable(idle);
        self.main.transcript.set_cursor_visible(idle);
        let downloading = self.download.borrow().is_some();
        self.preferences
            .select_model
            .set_sensitive(self.job.borrow().is_none());
        let cancelling = self
            .download
            .borrow()
            .as_ref()
            .is_some_and(|job| job.cancel.load(Ordering::Relaxed));
        self.models
            .download
            .set_sensitive((!busy || downloading) && !cancelling);
        self.models.download.set_label(if downloading {
            if self
                .download
                .borrow()
                .as_ref()
                .is_some_and(|job| job.copying)
            {
                "Cancel Copy"
            } else {
                "Cancel Download"
            }
        } else {
            "Download…"
        });
        self.models.progress.set_visible(downloading);
        self.models.browse.set_sensitive(!busy);
        self.models
            .list
            .set_sensitive(!busy && self.model_scan.borrow().is_none());
        self.preferences
            .gpu
            .set_sensitive(!busy && transcription::gpu_available_in_build());
        self.preferences.language.set_sensitive(!busy);
        self.preferences.refresh_device_controls(busy);
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

    fn render_transcript(&self) {
        self.rendering_transcript.set(true);
        self.transcript_buffer
            .borrow_mut()
            .update(&self.transcript.borrow(), &self.provisional.borrow());
        self.rendering_transcript.set(false);
        self.refresh();
    }

    fn clear_transcript(&self) {
        self.rendering_transcript.set(true);
        *self.transcript.borrow_mut() = Transcript::default();
        self.transcript_buffer.borrow_mut().reset();
        self.provisional.borrow_mut().clear();
        self.rendering_transcript.set(false);
        self.dirty.set(false);
        self.text_dirty.set(false);
    }

    fn has_transcript(&self) -> bool {
        !self.transcript.borrow().segments.is_empty()
            || !buffer_text(&self.main.transcript).is_empty()
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

    fn start(&self, source: Source, append: bool) {
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
        ));
        self.refresh();
    }

    fn export(self: &Rc<Self>, name: &str, content: String, kind: ExportKind) {
        let weak = Rc::downgrade(self);
        // TXT rewrite exports contain stripped text, not the authoritative Markdown.
        let snapshot = match kind {
            ExportKind::Rewrite => self.rewrite_markdown(),
            _ => content.clone(),
        };
        dialogs::save(&self.window, name, content, move || {
            if let Some(app) = weak.upgrade() {
                app.mark_export_saved(kind, &snapshot);
                app.main.status.set_text("Export saved.");
            }
        });
    }

    fn mark_export_saved(&self, kind: ExportKind, snapshot: &str) {
        match kind {
            ExportKind::Rewrite if self.rewrite_markdown() == snapshot => {
                self.rewrite_dirty.set(false);
            }
            ExportKind::TranscriptText if buffer_text(&self.main.transcript) == snapshot => {
                self.dirty.set(false);
                self.text_dirty.set(false);
            }
            ExportKind::Subtitles
                if !self.text_dirty.get() && self.transcript.borrow().srt() == snapshot =>
            {
                self.dirty.set(false);
            }
            _ => {}
        }
    }

    fn enter_rewrite(self: &Rc<Self>) {
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

    fn reset_rewrite(&self) {
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

    fn refresh_models(self: &Rc<Self>) {
        let directory = match settings::model_dir() {
            Ok(path) => path,
            Err(error) => {
                self.models
                    .status
                    .set_text(&format!("Cannot locate model folder: {error:#}"));
                return;
            }
        };
        self.models.directory.set_text(&directory.to_string_lossy());
        if !self.busy() {
            self.models.status.set_text("Loading installed models…");
        }
        let (tx, events) = mpsc::sync_channel(1);
        // Replace any older scan: late results cannot overwrite a newer list.
        *self.model_scan.borrow_mut() = Some(events);
        self.refresh();
        std::thread::spawn(move || {
            let _ = tx.send(model::installed(&directory).map_err(|e| format!("{e:#}")));
        });
    }

    fn select_model(&self, path: PathBuf) {
        if self.busy() {
            return;
        }
        self.settings.borrow_mut().model = Some(path);
        self.persist();
        self.update_model_label();
        self.refresh();
        self.models.window.close();
    }

    fn ask_import(self: &Rc<Self>, path: PathBuf) {
        if self.busy() {
            return;
        }
        let directory = match settings::model_dir() {
            Ok(directory) => directory,
            Err(error) => {
                dialogs::message(
                    &self.models.window,
                    "Cannot locate model folder",
                    &error.to_string(),
                );
                return;
            }
        };
        if path.parent() == Some(directory.as_path()) {
            self.select_model(path);
            return;
        }
        let weak = Rc::downgrade(self);
        dialogs::confirm(
            &self.models.window,
            "Copy model to the model folder?",
            &format!(
                "Selected: {}\n\nCopy it into {} so it appears in the model library, or use it from its original location? The original file will not be moved or deleted. Existing models will not be overwritten.",
                path.display(),
                directory.display()
            ),
            &[
                ("original", "Use Original"),
                ("copy", "Copy to Model Folder"),
            ],
            move |response| {
                if let Some(app) = weak.upgrade() {
                    match response.as_str() {
                        "original" => app.select_model(path.clone()),
                        "copy" => app.start_model_task(ModelTask::Copy(path.clone())),
                        _ => {}
                    }
                }
            },
        );
    }

    fn ask_download(self: &Rc<Self>) {
        if self.busy() {
            return;
        }
        let catalog = gtk::DropDown::from_strings(
            &model::CATALOG
                .iter()
                .map(|model| model.name)
                .collect::<Vec<_>>(),
        );
        catalog.set_selected(1);
        let dialog = adw::MessageDialog::new(
            Some(&self.models.window),
            Some("Download Whisper model"),
            Some(
                "Choose a model to download from huggingface.co into the model folder. SHA-256 is verified before selection. No audio or transcript is uploaded.",
            ),
        );
        dialog.set_extra_child(Some(&catalog));
        dialog.add_responses(&[("cancel", "Cancel"), ("download", "Download")]);
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("cancel"));
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if response == "download"
                && let Some(app) = weak.upgrade()
            {
                app.confirm_download(catalog.selected() as usize);
            }
        });
        dialog.present();
    }

    fn confirm_download(self: &Rc<Self>, index: usize) {
        if self.busy() {
            return;
        }
        let Some(model) = model::CATALOG.get(index) else {
            return;
        };
        *self.download_check.borrow_mut() = Some(background(move || {
            (|| -> anyhow::Result<_> {
                let target = settings::model_dir()?.join(model.file);
                let exists = target.try_exists()?;
                Ok((index, target, exists))
            })()
            .map_err(|error| format!("{error:#}"))
        }));
        self.models.status.set_text("Checking model destination…");
        self.refresh();
    }

    fn poll_download_check(self: &Rc<Self>) {
        let result =
            self.download_check
                .borrow()
                .as_ref()
                .and_then(|events| match events.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Model destination worker stopped unexpectedly".into()))
                    }
                });
        if let Some(result) = result {
            self.download_check.borrow_mut().take();
            self.refresh();
            match result {
                Ok((index, target, true)) => self.ask_replace_download(index, target),
                Ok((index, _, false)) => {
                    self.start_model_task(ModelTask::Download(index, model::Replacement::Never))
                }
                Err(error) => dialogs::message(
                    &self.dialog_parent(),
                    "Cannot check model destination",
                    &error,
                ),
            }
        }
    }

    fn ask_replace_download(self: &Rc<Self>, index: usize, target: PathBuf) {
        let weak = Rc::downgrade(self);
        dialogs::confirm(
            &self.models.window,
            "Replace existing model?",
            &format!(
                "A model already exists at {}. Download a fresh copy and replace it? The existing model will be kept until the download is complete and verified. Cancelling or a failed download will leave it unchanged.",
                target.display()
            ),
            &[("replace", "Download and Replace")],
            move |response| {
                if response == "replace"
                    && let Some(app) = weak.upgrade()
                {
                    app.start_model_task(ModelTask::Download(index, model::Replacement::Confirmed));
                }
            },
        );
    }

    fn start_model_task(&self, task: ModelTask) {
        if self.busy() {
            return;
        }
        if let ModelTask::Download(index, _) = &task
            && model::CATALOG.get(*index).is_none()
        {
            return;
        }
        let directory = match settings::model_dir() {
            Ok(path) => path,
            Err(error) => {
                dialogs::message(
                    &self.dialog_parent(),
                    "Cannot locate model folder",
                    &error.to_string(),
                );
                return;
            }
        };
        let copying = matches!(task, ModelTask::Copy(_));
        let (tx, events) = mpsc::sync_channel(64);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.download.borrow_mut() = Some(ModelJob {
            events,
            cancel: cancel.clone(),
            copying,
        });
        self.main.progress.set_fraction(0.0);
        let status = if copying {
            "Copying model…"
        } else {
            "Downloading model…"
        };
        self.main.status.set_text(status);
        self.models.status.set_text(status);
        self.refresh();
        std::thread::spawn(move || {
            let mut last = -1_i32;
            let progress = |fraction: f64| {
                let percent = (fraction * 100.0) as i32;
                if percent != last {
                    let _ = tx.send(DownloadEvent::Progress(fraction));
                    last = percent;
                }
            };
            let result = match task {
                ModelTask::Download(index, replacement) => model::download(
                    &model::CATALOG[index],
                    &directory,
                    replacement,
                    &cancel,
                    progress,
                ),
                ModelTask::Copy(path) => model::import(&path, &directory, &cancel, progress),
            }
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(DownloadEvent::Finished(result));
        });
    }

    fn poll(self: &Rc<Self>) {
        self.poll_preferences_save();
        if self.allow_close.get() {
            return;
        }
        self.poll_model_check();
        self.poll_download_check();
        self.poll_rewrite();
        let devices =
            self.device_discovery
                .borrow()
                .as_ref()
                .and_then(|events| match events.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                        "Microphone discovery worker stopped unexpectedly".into(),
                    )),
                });
        if let Some(result) = devices {
            self.device_discovery.borrow_mut().take();
            self.preferences.finish_device_refresh(result);
            self.refresh();
        }
        let models = self
            .model_scan
            .borrow()
            .as_ref()
            .and_then(|events| match events.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Model folder scan stopped unexpectedly".into()))
                }
            });
        if let Some(result) = models {
            self.model_scan.borrow_mut().take();
            match result {
                Ok(paths) => {
                    let weak = Rc::downgrade(self);
                    self.models.set_entries(
                        paths,
                        self.settings.borrow().model.as_deref(),
                        move |path| {
                            if let Some(app) = weak.upgrade() {
                                app.select_model(path);
                            }
                        },
                    );
                    if !self.busy() {
                        self.models.status.set_text("Select a model above, download one, or choose a file from another folder.");
                    }
                }
                Err(error) => {
                    self.models.set_entries(Vec::new(), None, |_| {});
                    self.models
                        .status
                        .set_text(&format!("Cannot list models: {error}"));
                }
            }
            self.refresh();
        }
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
        let events: Vec<_> = self
            .download
            .borrow()
            .as_ref()
            .map(|job| {
                let mut events = Vec::new();
                loop {
                    match job.events.try_recv() {
                        Ok(event) => events.push(event),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            if !events
                                .iter()
                                .any(|event| matches!(event, DownloadEvent::Finished(_)))
                            {
                                events.push(DownloadEvent::Finished(Err(
                                    "Model operation stopped unexpectedly".into(),
                                )));
                            }
                            break;
                        }
                    }
                }
                events
            })
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
                            self.main.status.set_text("Model ready.");
                            self.refresh_models();
                        }
                        Err(_) if cancelled => {
                            self.main.status.set_text("Model operation cancelled.");
                            self.models.status.set_text(
                                "Model operation cancelled. The previous selection has been kept.",
                            );
                        }
                        Err(error) => {
                            self.main.status.set_text("Model operation failed.");
                            self.models.status.set_text(
                                "Model operation failed. The previous selection has been kept.",
                            );
                            dialogs::message(
                                &self.dialog_parent(),
                                "Model operation failed",
                                &error,
                            );
                        }
                    }
                    self.refresh();
                }
            }
        }
    }
}
