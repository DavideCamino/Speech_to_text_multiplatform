use adw::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let application = adw::Application::builder()
        .application_id("io.github.YOUR_NAME.Speech2Text")
        .build();
    application.connect_activate(speech2text::ui::build);
    application.run()
}
