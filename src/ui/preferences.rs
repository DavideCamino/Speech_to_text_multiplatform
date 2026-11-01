//! Persistent preferences widgets, independent of the transcription view.
use crate::settings::Settings;
use adw::prelude::*;
use gtk::Align;
use std::cell::{Cell, RefCell};

pub struct Preferences {
    pub window: adw::PreferencesWindow,
    pub model_label: gtk::Label,
    pub select_model: gtk::Button,
    pub gpu: gtk::Switch,
    pub gpu_backend: gtk::DropDown,
    pub language: gtk::Entry,
    pub device: gtk::DropDown,
    pub device_refresh: gtk::Button,
    pub device_row: adw::ActionRow,
    devices: RefCell<Vec<String>>,
    device_loading: Cell<bool>,
    device_unavailable: Cell<Option<u32>>,
    device_error: RefCell<Option<String>>,
}

impl Preferences {
    pub fn new(parent: &adw::ApplicationWindow, settings: &Settings) -> Self {
        let window = adw::PreferencesWindow::builder()
            .title("Settings")
            .transient_for(parent)
            .modal(true)
            .hide_on_close(true)
            .destroy_with_parent(true)
            .default_width(760)
            .default_height(580)
            .search_enabled(false)
            .build();
        let stt = adw::PreferencesPage::builder()
            .title("Speech to Text")
            .name("stt")
            .icon_name("audio-input-microphone-symbolic")
            .build();
        let config = adw::PreferencesGroup::builder()
            .title("Recognition")
            .build();
        let model_name = settings
            .model
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No model selected".into());
        let model_label = gtk::Label::builder()
            .label(&model_name)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(24)
            .build();
        let select_model = gtk::Button::builder().child(&model_label).build();
        select_model.set_valign(Align::Center);
        let model_row = adw::ActionRow::builder()
            .title("Local Whisper model")
            .build();
        model_row.add_suffix(&select_model);
        config.add(&model_row);
        let gpu = gtk::Switch::builder()
            .active(settings.use_gpu && crate::transcription::gpu_available_in_build())
            .sensitive(crate::transcription::gpu_available_in_build())
            .valign(Align::Center)
            .build();
        let backends = crate::transcription::gpu_backends_in_build();
        let gpu_subtitle = if backends.is_empty() {
            "CPU build — rebuild with vulkan, cuda, metal, or rocm to enable".to_owned()
        } else {
            format!("Compiled GPU backends: {}", backends.join(", "))
        };
        let gpu_row = adw::ActionRow::builder()
            .title("Hardware acceleration")
            .subtitle(&gpu_subtitle)
            .build();
        gpu_row.add_suffix(&gpu);
        config.add(&gpu_row);
        let compiled = crate::transcription::compiled_gpu_backends();
        let mut backend_names = vec!["Automatic (Whisper default)"];
        backend_names.extend(compiled.iter().map(|backend| backend.label()));
        let gpu_backend = gtk::DropDown::from_strings(&backend_names);
        gpu_backend.set_valign(Align::Center);
        let selected = settings
            .gpu_backend
            .and_then(|saved| compiled.iter().position(|backend| *backend == saved));
        gpu_backend.set_selected(selected.map_or(0, |index| index as u32 + 1));
        let backend_row = adw::ActionRow::builder()
            .title("GPU backend")
            .subtitle(if settings.gpu_backend.is_some() && selected.is_none() {
                "Saved backend is not in this build; Automatic is selected"
            } else {
                "Compiled backends; device availability is checked when transcription starts"
            })
            .sensitive(gpu.is_active() && gpu.is_sensitive())
            .build();
        backend_row.add_suffix(&gpu_backend);
        config.add(&backend_row);
        let row = backend_row.downgrade();
        gpu_backend.connect_selected_notify(move |_| {
            if let Some(row) = row.upgrade() {
                row.set_subtitle(
                    "Compiled backends; device availability is checked when transcription starts",
                );
            }
        });
        // Follow both the toggle and its busy/CPU-build sensitivity. Weak widget
        // references avoid cycles; changing this control never loads a GPU driver.
        for property in ["active", "sensitive"] {
            let row = backend_row.downgrade();
            gpu.connect_notify_local(Some(property), move |gpu, _| {
                if let Some(row) = row.upgrade() {
                    row.set_sensitive(gpu.is_active() && gpu.is_sensitive());
                }
            });
        }
        let language = gtk::Entry::builder()
            .placeholder_text("auto, en, it, fr…")
            .text(settings.language.as_deref().unwrap_or("auto"))
            .valign(Align::Center)
            .width_chars(12)
            .build();
        let language_row = adw::ActionRow::builder()
            .title("Spoken language")
            .subtitle("Whisper language code, or auto")
            .build();
        language_row.add_suffix(&language);
        config.add(&language_row);
        // Preserve the saved name before discovery completes. Constructing
        // widgets must never open/enumerate native audio devices on GTK's thread.
        let devices: Vec<String> = settings.device.iter().cloned().collect();
        let mut names = vec!["Default microphone".to_owned()];
        names.extend(
            devices
                .iter()
                .map(|name| format!("{name} (not yet discovered)")),
        );
        let device =
            gtk::DropDown::from_strings(&names.iter().map(String::as_str).collect::<Vec<_>>());
        device.set_valign(Align::Center);
        device.set_selected(if devices.is_empty() { 0 } else { 1 });
        let device_refresh = gtk::Button::with_label("Refresh");
        device_refresh.set_valign(Align::Center);
        let device_row = adw::ActionRow::builder()
            .title("Audio input")
            .use_markup(false)
            .subtitle("Microphones have not been discovered yet")
            .build();
        device_row.add_suffix(&device);
        device_row.add_suffix(&device_refresh);
        config.add(&device_row);
        stt.add(&config);
        window.add(&stt);
        // Deliberately empty until LLM integration is implemented.
        let llm = adw::PreferencesPage::builder()
            .title("LLM")
            .name("llm")
            .icon_name("text-editor-symbolic")
            .build();
        window.add(&llm);
        Self {
            window,
            model_label,
            select_model,
            gpu,
            gpu_backend,
            language,
            device,
            device_refresh,
            device_row,
            devices: RefCell::new(devices),
            device_loading: Cell::new(false),
            device_unavailable: Cell::new(settings.device.as_ref().map(|_| 1)),
            device_error: RefCell::new(None),
        }
    }

    pub fn selected_device(&self) -> Option<String> {
        self.device
            .selected()
            .checked_sub(1)
            .and_then(|index| self.devices.borrow().get(index as usize).cloned())
    }

    pub fn devices_loading(&self) -> bool {
        self.device_loading.get()
    }

    pub fn refresh_device_controls(&self, busy: bool) {
        let enabled = !busy && !self.devices_loading();
        self.device.set_sensitive(enabled);
        self.device_refresh.set_sensitive(enabled);
    }

    pub fn begin_device_refresh(&self) {
        self.device_loading.set(true);
        self.refresh_device_controls(false);
        self.update_device_status();
    }

    pub fn finish_device_refresh(&self, result: Result<Vec<String>, String>) {
        let selected = self.selected_device();
        match result {
            Ok(mut devices) => {
                // Multiple devices may expose the same name; name-based capture
                // cannot distinguish them, so do not offer duplicate choices.
                let mut seen = std::collections::HashSet::new();
                devices.retain(|name| seen.insert(name.clone()));
                let missing = selected.as_ref().filter(|name| !devices.contains(name));
                let unavailable = missing.map(|name| {
                    devices.push(name.clone());
                    devices.len() as u32
                });
                let mut labels = vec!["Default microphone".to_owned()];
                labels.extend(devices.iter().enumerate().map(|(index, name)| {
                    if unavailable == Some(index as u32 + 1) {
                        format!("{name} (unavailable)")
                    } else {
                        name.clone()
                    }
                }));
                let index = selected
                    .as_ref()
                    .and_then(|name| devices.iter().position(|d| d == name))
                    .map_or(0, |index| index as u32 + 1);
                *self.devices.borrow_mut() = devices;
                self.device_unavailable.set(unavailable);
                self.device_error.borrow_mut().take();
                self.device.set_model(Some(&gtk::StringList::new(
                    &labels.iter().map(String::as_str).collect::<Vec<_>>(),
                )));
                self.device.set_selected(index);
            }
            Err(error) => {
                // Keep the existing choices and selection on failure. An error
                // is not evidence that a previously selected device vanished.
                *self.device_error.borrow_mut() = Some(error);
            }
        }
        self.device_loading.set(false);
        self.update_device_status();
    }

    pub fn update_device_status(&self) {
        if self.device_loading.get() {
            self.device_row.set_subtitle("Loading microphones…");
        } else if let Some(error) = self.device_error.borrow().as_ref() {
            self.device_row.set_subtitle(&format!(
                "Could not discover microphones: {error}. Use Refresh to retry."
            ));
        } else if self.device_unavailable.get() == Some(self.device.selected()) {
            self.device_row.set_subtitle("Selected microphone is unavailable. Reconnect it and refresh, or choose another input.");
        } else if self.devices.borrow().len()
            == usize::from(self.device_unavailable.get().is_some())
        {
            self.device_row
                .set_subtitle("No microphones found. Connect a microphone and refresh.");
        } else {
            self.device_row
                .set_subtitle("Use Refresh to update connected microphones.");
        }
    }
}
