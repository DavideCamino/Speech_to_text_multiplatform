//! Shared application construction and UI-wide coordination.
use super::{
    device_controller::DeviceDiscovery,
    lifecycle::WindowLifecycle,
    menu,
    model_controller::ModelState,
    models, preferences, rewrite, text_zoom,
    transcript_state::TranscriptState,
    transcription_controller::TranscriptionSession,
    views::{MainView, RewriteView},
};
use crate::settings::{self, Settings};
use adw::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub(super) struct App {
    pub(super) window: adw::ApplicationWindow,
    pub(super) stack: gtk::Stack,
    pub(super) main: MainView,
    pub(super) preferences: preferences::Preferences,
    pub(super) models: models::Models,
    pub(super) model_state: RefCell<ModelState>,
    pub(super) settings_writer: RefCell<settings::SaveWorker>,
    pub(super) lifecycle: Cell<WindowLifecycle>,
    pub(super) rewrite: RewriteView,
    pub(super) rewrite_state: RefCell<rewrite::State>,
    pub(super) settings: RefCell<Settings>,
    pub(super) document: TranscriptState,
    pub(super) session: RefCell<TranscriptionSession>,
    pub(super) device_discovery: RefCell<Option<DeviceDiscovery>>,
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
        let document = TranscriptState::new(main.transcript.buffer());
        Rc::new(Self {
            window,
            stack,
            main,
            preferences,
            models,
            model_state: RefCell::default(),
            settings_writer: RefCell::new(settings_writer),
            lifecycle: Cell::default(),
            rewrite,
            rewrite_state: RefCell::default(),
            settings: RefCell::new(settings),
            document,
            session: RefCell::default(),
            device_discovery: RefCell::default(),
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

    /// Only operations that block starting other work. Library/device discovery,
    /// availability checks, and LLM connection tests can run independently.
    pub(super) fn busy(&self) -> bool {
        self.lifecycle.get() != WindowLifecycle::Open
            || self.model_state.borrow().operation.is_busy()
            || self.session.borrow().is_running()
            || self.rewrite_state.borrow().operation.is_generating()
    }

    pub(super) fn poll(self: &Rc<Self>) {
        self.poll_preferences_save();
        if self.lifecycle.get() == WindowLifecycle::ReadyToClose {
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
        self.window
            .set_sensitive(self.lifecycle.get() == WindowLifecycle::Open);
        self.refresh_transcript_editability();
        self.refresh_model_controls();
        self.refresh_recognition_controls();
        self.refresh_transcription_controls();
        self.refresh_rewrite_controls();
    }
}
