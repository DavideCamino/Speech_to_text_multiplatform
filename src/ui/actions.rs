//! Widget signals and window/menu actions, with weak controller references.
use super::{app::App, dialogs, export::ExportKind, menu, rewrite, views::buffer_text};
use crate::transcription::Source;
use adw::prelude::*;
use std::rc::Rc;

fn on_click(button: &gtk::Button, app: &Rc<App>, action: impl Fn(Rc<App>) + 'static) {
    let weak = Rc::downgrade(app);
    button.connect_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            action(app);
        }
    });
}

impl App {
    pub(super) fn connect(self: &Rc<Self>) {
        self.connect_transcript_editing();
        self.connect_menu();
        self.connect_window_actions();
        self.connect_preferences_actions();
        self.connect_model_actions();
        self.connect_transcription_actions();
        self.connect_rewrite_actions();
        self.connect_close_request();
    }

    pub(super) fn connect_transcript_editing(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.main.transcript.buffer().connect_changed(move |_| {
            if let Some(app) = weak.upgrade()
                && app.document.record_user_edit()
            {
                app.refresh();
            }
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

    fn connect_window_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |window| {
            if window.is_active()
                && let Some(app) = weak.upgrade()
                && !app.busy()
            {
                app.check_model();
            }
        });
    }

    fn connect_preferences_actions(self: &Rc<Self>) {
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
    }

    fn connect_model_actions(self: &Rc<Self>) {
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
            let downloading = app.model_state.borrow().operation.is_running();
            if downloading {
                app.request_model_cancel();
            } else {
                app.ask_download();
            }
        });
    }

    fn connect_transcription_actions(self: &Rc<Self>) {
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
        on_click(&self.main.stop, self, |app| app.request_stop());
        on_click(&self.main.cancel, self, |app| app.request_cancel());
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
            let content = app.document.srt();
            app.export("transcript.srt", content, ExportKind::Subtitles);
        });
        on_click(&self.main.rewrite, self, |app| app.enter_rewrite());
    }

    fn connect_rewrite_actions(self: &Rc<Self>) {
        on_click(&self.preferences.llm_test, self, |app| app.test_llm());
        on_click(&self.rewrite.stop, self, |app| app.request_rewrite_stop());
        on_click(&self.rewrite.back, self, |app| {
            app.stack.set_visible_child_name("main")
        });
        on_click(&self.rewrite.run, self, |app| {
            if app.busy() {
                return;
            }
            let run = |app: &App| app.start_rewrite();
            if app.rewrite_state.borrow().is_dirty() {
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
    }
}
