//! Persistent preferences widgets, independent of the transcription view.
use crate::settings::Settings;
use adw::prelude::*;
use gtk::Align;

pub struct Preferences {
    pub window: adw::PreferencesWindow,
    pub model_label: gtk::Label,
    pub select_model: gtk::Button,
    pub catalog: gtk::DropDown,
    pub download: gtk::Button,
    pub progress: gtk::ProgressBar,
    pub gpu: gtk::Switch,
    pub language: gtk::Entry,
    pub device: gtk::DropDown,
    pub devices: Vec<String>,
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
        let model_label = gtk::Label::builder()
            .label("No model selected")
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(24)
            .build();
        let select_model = gtk::Button::with_label("Select Model…");
        select_model.set_valign(Align::Center);
        let model_row = adw::ActionRow::builder()
            .title("Local Whisper model")
            .build();
        model_row.add_suffix(&model_label);
        model_row.add_suffix(&select_model);
        config.add(&model_row);
        let catalog = gtk::DropDown::from_strings(
            &crate::model::CATALOG
                .iter()
                .map(|m| m.name)
                .collect::<Vec<_>>(),
        );
        catalog.set_selected(1);
        catalog.set_valign(Align::Center);
        let download = gtk::Button::with_label("Download");
        download.set_valign(Align::Center);
        download.set_tooltip_text(Some(
            "Downloads from Hugging Face after confirmation; verifies SHA-256",
        ));
        let download_row = adw::ActionRow::builder().title("Download a model").build();
        download_row.add_suffix(&catalog);
        download_row.add_suffix(&download);
        config.add(&download_row);
        let progress = gtk::ProgressBar::builder()
            .visible(false)
            .show_text(true)
            .build();
        config.add(&progress);
        let gpu = gtk::Switch::builder()
            .active(settings.use_gpu && crate::transcription::gpu_available_in_build())
            .sensitive(crate::transcription::gpu_available_in_build())
            .valign(Align::Center)
            .build();
        let gpu_row = adw::ActionRow::builder()
            .title("Hardware acceleration")
            .subtitle(if crate::transcription::gpu_available_in_build() {
                "Use the GPU backend enabled in this build"
            } else {
                "CPU build — rebuild with vulkan, cuda, or metal to enable"
            })
            .build();
        gpu_row.add_suffix(&gpu);
        config.add(&gpu_row);
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
        let devices = crate::audio::input_devices().unwrap_or_default();
        let mut names = vec!["Default microphone"];
        names.extend(devices.iter().map(String::as_str));
        let device = gtk::DropDown::from_strings(&names);
        device.set_valign(Align::Center);
        if let Some(index) = devices
            .iter()
            .position(|d| Some(d) == settings.device.as_ref())
        {
            device.set_selected(index as u32 + 1);
        }
        let device_row = adw::ActionRow::builder().title("Audio input").build();
        device_row.add_suffix(&device);
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
            catalog,
            download,
            progress,
            gpu,
            language,
            device,
            devices,
        }
    }
}
