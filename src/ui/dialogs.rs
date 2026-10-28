use adw::prelude::*;
use std::path::PathBuf;

pub fn message(window: &adw::ApplicationWindow, heading: &str, body: &str) {
    let dialog = adw::MessageDialog::new(Some(window), Some(heading), Some(body));
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    dialog.present();
}

pub fn confirm(
    window: &adw::ApplicationWindow,
    heading: &str,
    body: &str,
    responses: &[(&str, &str)],
    action: impl Fn(String) + 'static,
) {
    let dialog = adw::MessageDialog::new(Some(window), Some(heading), Some(body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_responses(responses);
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("cancel"));
    dialog.connect_response(None, move |_, response| action(response.to_owned()));
    dialog.present();
}

pub fn open(window: &adw::ApplicationWindow, model: bool, action: impl Fn(PathBuf) + 'static) {
    let filter = gtk::FileFilter::new();
    if model {
        filter.set_name(Some("Whisper ggml models"));
        filter.add_pattern("*.bin");
    } else {
        filter.set_name(Some("Audio files"));
        for extension in ["wav", "mp3", "flac", "ogg", "m4a"] {
            filter.add_pattern(&format!("*.{extension}"));
            filter.add_pattern(&format!("*.{}", extension.to_uppercase()));
        }
    }
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let all = gtk::FileFilter::new();
    all.set_name(Some("All files"));
    all.add_pattern("*");
    filters.append(&all);
    let dialog = gtk::FileDialog::builder()
        .title(if model {
            "Select Whisper Model"
        } else {
            "Open Audio"
        })
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let parent = window.clone();
    dialog.open(
        Some(window),
        None::<&gtk::gio::Cancellable>,
        move |result| match result {
            Ok(file) => {
                if let Some(path) = file.path() {
                    action(path);
                } else {
                    message(
                        &parent,
                        "Local file required",
                        "Select a file on this computer.",
                    );
                }
            }
            Err(error)
                if error.matches(gtk::DialogError::Dismissed)
                    || error.matches(gtk::DialogError::Cancelled) => {}
            Err(error) => message(&parent, "Cannot open file chooser", &error.to_string()),
        },
    );
}

pub fn save(
    window: &adw::ApplicationWindow,
    name: &str,
    content: String,
    done: impl Fn() + 'static,
) {
    let dialog = gtk::FileDialog::builder()
        .title("Export Text")
        .initial_name(name)
        .build();
    let parent = window.clone();
    dialog.save(
        Some(window),
        None::<&gtk::gio::Cancellable>,
        move |result| match result {
            Ok(file) => {
                let content = content.clone();
                let parent = parent.clone();
                // GIO asynchronous writes support the desktop file chooser's locations
                // and keep slow disks/network mounts off the main loop.
                let done = std::rc::Rc::new(done);
                file.replace_contents_async(
                    content.into_bytes(),
                    None,
                    false,
                    gtk::gio::FileCreateFlags::REPLACE_DESTINATION,
                    None::<&gtk::gio::Cancellable>,
                    move |result| match result {
                        Ok(_) => done(),
                        Err((_, error)) => message(&parent, "Export failed", &error.to_string()),
                    },
                );
            }
            Err(error)
                if error.matches(gtk::DialogError::Dismissed)
                    || error.matches(gtk::DialogError::Cancelled) => {}
            Err(error) => message(&parent, "Cannot save file", &error.to_string()),
        },
    );
}
