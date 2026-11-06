//! Shared application construction and UI-wide coordination.
use super::{
    device_controller::DeviceDiscovery,
    menu,
    model_controller::{DownloadCheck, ModelJob, ModelScan},
    models, preferences, rewrite, text_zoom, transcript_buffer,
    views::{MainView, RewriteView},
};
use crate::{
    settings::{self, Settings},
    transcript::Transcript,
    transcription::Job,
};
use adw::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc::Receiver,
};

pub(super) struct App {
    pub(super) window: adw::ApplicationWindow,
    pub(super) stack: gtk::Stack,
    pub(super) main: MainView,
    pub(super) preferences: preferences::Preferences,
    pub(super) models: models::Models,
    pub(super) model_scan: RefCell<Option<ModelScan>>,
    pub(super) model_check: RefCell<Option<Receiver<bool>>>,
    pub(super) model_available: Cell<bool>,
    pub(super) download_check: RefCell<Option<DownloadCheck>>,
    pub(super) settings_writer: RefCell<settings::SaveWorker>,
    pub(super) closing: Cell<bool>,
    pub(super) rewrite: RewriteView,
    pub(super) rewrite_state: RefCell<rewrite::State>,
    pub(super) settings: RefCell<Settings>,
    pub(super) transcript: RefCell<Transcript>,
    pub(super) transcript_buffer: RefCell<transcript_buffer::TranscriptBuffer>,
    pub(super) provisional: RefCell<String>,
    pub(super) job: RefCell<Option<Job>>,
    pub(super) download: RefCell<Option<ModelJob>>,
    pub(super) device_discovery: RefCell<Option<DeviceDiscovery>>,
    pub(super) recording: Cell<bool>,
    pub(super) dirty: Cell<bool>,
    pub(super) text_dirty: Cell<bool>,
    pub(super) rendering_transcript: Cell<bool>,
    pub(super) rewrite_dirty: Cell<bool>,
    pub(super) allow_close: Cell<bool>,
}

impl App {
    /// Construct widgets and state only. Callers explicitly connect signals,
    /// restore prompts, start discovery, present the window, and install timers.
    pub(super) fn new(
        application: Option<&adw::Application>,
        settings: Settings,
        settings_writer: settings::SaveWorker,
    ) -> Rc<Self> {
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
            .title("Speech2Text")
            .default_width(980)
            .default_height(820)
            .width_request(600)
            .height_request(600)
            .content(&toolbar)
            .build();
        if let Some(application) = application {
            window.set_application(Some(application));
        }
        let preferences = preferences::Preferences::new(&window, &settings);
        let models = models::Models::new(&preferences.window);
        main.progress
            .bind_property("fraction", &models.progress, "fraction")
            .sync_create()
            .build();
        let transcript_buffer = RefCell::new(transcript_buffer::TranscriptBuffer::new(
            main.transcript.buffer(),
        ));
        Rc::new(Self {
            window,
            stack,
            main,
            preferences,
            models,
            model_scan: RefCell::default(),
            model_check: RefCell::default(),
            model_available: Cell::new(false),
            download_check: RefCell::default(),
            settings_writer: RefCell::new(settings_writer),
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
        })
    }

    pub(super) fn dialog_parent(&self) -> gtk::Window {
        if self.models.window.is_visible() {
            self.models.window.clone().upcast()
        } else if self.preferences.window.is_visible() {
            self.preferences.window.clone().upcast()
        } else {
            self.window.clone().upcast()
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.closing.get()
            || self.download_check.borrow().is_some()
            || self.job.borrow().is_some()
            || self.download.borrow().is_some()
            || self.rewrite_state.borrow().job.is_some()
    }

    pub(super) fn poll(self: &Rc<Self>) {
        self.poll_preferences_save();
        if self.allow_close.get() {
            return;
        }
        self.poll_model_check();
        self.poll_download_check();
        self.poll_rewrite();
        self.poll_devices();
        self.poll_model_scan();
        self.poll_transcription();
        self.poll_model_download();
    }

    pub(super) fn refresh(&self) {
        self.refresh_transcript_editability();
        self.refresh_model_controls();
        self.refresh_recognition_controls();
        self.refresh_transcription_controls();
        self.refresh_rewrite_controls();
    }
}
