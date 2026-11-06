//! Model library, availability checks, imports, and downloads.
use super::{app::App, dialogs, task::background};
use crate::{model, settings};
use adw::prelude::*;
use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

enum DownloadEvent {
    Progress(f64),
    Finished(Result<PathBuf, String>),
}
enum ModelTask {
    Download(usize, model::Replacement),
    Copy(PathBuf),
}
pub(super) struct ModelJob {
    events: Receiver<DownloadEvent>,
    cancel: Arc<AtomicBool>,
    copying: bool,
}
impl Drop for ModelJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

type ModelScan = Receiver<Result<Vec<PathBuf>, String>>;
type DownloadCheck = Receiver<Result<(usize, PathBuf, bool), String>>;

#[derive(Default)]
pub(super) enum ModelAvailability {
    #[default]
    Unavailable,
    Checking(Receiver<bool>),
    Available,
}

impl ModelAvailability {
    pub(super) fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub(super) fn is_checking(&self) -> bool {
        matches!(self, Self::Checking(_))
    }
}

/// A destination check and its subsequent transfer cannot run simultaneously.
/// Confirmation dialogs leave this Idle, as they did before the state split.
#[derive(Default)]
pub(super) enum ModelOperation {
    #[default]
    Idle,
    CheckingDestination(DownloadCheck),
    Running(ModelJob),
}

impl ModelOperation {
    fn job(&self) -> Option<&ModelJob> {
        match self {
            Self::Running(job) => Some(job),
            _ => None,
        }
    }

    pub(super) fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub(super) fn is_running(&self) -> bool {
        self.job().is_some()
    }

    pub(super) fn cancelling(&self) -> bool {
        self.job()
            .is_some_and(|job| job.cancel.load(Ordering::Relaxed))
    }

    pub(super) fn request_cancel(&self) -> bool {
        if let Some(job) = self.job() {
            job.cancel.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Inspect cancellation before dropping a transfer, whose Drop also cancels.
    fn finish(&mut self) -> bool {
        let cancelled = self.cancelling();
        *self = Self::Idle;
        cancelled
    }
}

#[derive(Default)]
pub(super) struct ModelState {
    pub(super) availability: ModelAvailability,
    pub(super) scan: Option<ModelScan>,
    pub(super) operation: ModelOperation,
}

impl App {
    pub(super) fn check_model(&self) {
        if self.model_state.borrow().availability.is_checking() {
            return;
        }
        let path = self.settings.borrow().model.clone();
        self.model_state.borrow_mut().availability = path
            .map_or(ModelAvailability::Unavailable, |path| {
                ModelAvailability::Checking(background(move || path.is_file()))
            });
        self.refresh();
    }

    pub(super) fn poll_model_check(&self) {
        let result = match &self.model_state.borrow().availability {
            ModelAvailability::Checking(events) => match events.try_recv() {
                Ok(available) => Some(available),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(false),
            },
            _ => None,
        };
        if let Some(available) = result {
            self.model_state.borrow_mut().availability = if available {
                ModelAvailability::Available
            } else {
                ModelAvailability::Unavailable
            };
            self.refresh();
        }
    }

    pub(super) fn update_model_label(&self) {
        let settings = self.settings.borrow();
        self.preferences.set_model(settings.model.as_deref());
        drop(settings);
        // Replacing the receiver discards late results for an older selection.
        self.model_state.borrow_mut().availability = ModelAvailability::Unavailable;
        self.check_model();
    }

    pub(super) fn refresh_models(self: &Rc<Self>) {
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
        // Replace any older scan: late results cannot overwrite a newer list.
        self.model_state.borrow_mut().scan = Some(background(move || {
            model::installed(&directory).map_err(|error| format!("{error:#}"))
        }));
        self.refresh();
    }

    pub(super) fn select_model(&self, path: PathBuf) {
        if self.busy() {
            return;
        }
        self.accept_model(path);
        self.models.window.close();
    }

    pub(super) fn accept_model(&self, path: PathBuf) {
        self.settings.borrow_mut().model = Some(path);
        self.persist();
        self.update_model_label();
    }

    pub(super) fn ask_import(self: &Rc<Self>, path: PathBuf) {
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

    pub(super) fn ask_download(self: &Rc<Self>) {
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
        self.model_state.borrow_mut().operation =
            ModelOperation::CheckingDestination(background(move || {
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

    pub(super) fn poll_download_check(self: &Rc<Self>) {
        let result = match &self.model_state.borrow().operation {
            ModelOperation::CheckingDestination(events) => match events.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Model destination worker stopped unexpectedly".into()))
                }
            },
            _ => None,
        };
        if let Some(result) = result {
            self.model_state.borrow_mut().operation.finish();
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

    pub(super) fn ask_replace_download(self: &Rc<Self>, index: usize, target: PathBuf) {
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
        self.model_state.borrow_mut().operation = ModelOperation::Running(ModelJob {
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

    pub(super) fn poll_model_scan(self: &Rc<Self>) {
        let models =
            self.model_state
                .borrow()
                .scan
                .as_ref()
                .and_then(|events| match events.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Model folder scan stopped unexpectedly".into()))
                    }
                });
        if let Some(result) = models {
            self.model_state.borrow_mut().scan.take();
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
    }

    pub(super) fn poll_model_download(self: &Rc<Self>) {
        let events: Vec<_> = self
            .model_state
            .borrow()
            .operation
            .job()
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
                    let cancelled = self.model_state.borrow_mut().operation.finish();
                    match result {
                        Ok(path) => {
                            self.accept_model(path);
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

    pub(super) fn request_model_cancel(&self) {
        if !self.model_state.borrow().operation.request_cancel() {
            return;
        }
        self.main.status.set_text("Cancelling model operation…");
        self.models.status.set_text("Cancelling model operation…");
        self.refresh();
    }

    pub(super) fn refresh_model_controls(&self) {
        let busy = self.busy();
        let (downloading, cancelling, copying, scanning) = {
            let state = self.model_state.borrow();
            (
                state.operation.is_running(),
                state.operation.cancelling(),
                state.operation.job().is_some_and(|job| job.copying),
                state.scan.is_some(),
            )
        };
        let transcribing = self.session.borrow().is_running();
        self.preferences.select_model.set_sensitive(!transcribing);
        self.models
            .download
            .set_sensitive((!busy || downloading) && !cancelling);
        self.models.download.set_label(if downloading {
            if copying {
                "Cancel Copy"
            } else {
                "Cancel Download"
            }
        } else {
            "Download…"
        });
        self.models.progress.set_visible(downloading);
        self.models.browse.set_sensitive(!busy);
        self.models.list.set_sensitive(!busy && !scanning);
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::{DownloadEvent, ModelAvailability, ModelJob, ModelOperation};
    use crate::{
        settings::{self, Settings},
        ui::app::App,
    };
    use adw::prelude::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    #[test]
    fn model_completion_captures_cancellation_before_drop() {
        for cancelled in [false, true] {
            let (_, events) = mpsc::sync_channel(1);
            let cancel = Arc::new(AtomicBool::new(cancelled));
            let mut operation = ModelOperation::Running(ModelJob {
                events,
                cancel: cancel.clone(),
                copying: false,
            });
            assert_eq!(operation.finish(), cancelled);
            assert!(!operation.is_busy());
            assert!(cancel.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn dropping_model_operation_cancels_transfer() {
        let (_, events) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        drop(ModelOperation::Running(ModelJob {
            events,
            cancel: cancel.clone(),
            copying: true,
        }));
        assert!(cancel.load(Ordering::Relaxed));
        let (_, events) = mpsc::channel();
        let mut check = ModelOperation::CheckingDestination(events);
        assert!(check.is_busy());
        assert!(!check.request_cancel());
        assert!(!check.finish());
        assert!(!check.is_busy());
    }

    // Called only by the shared GTK test entry point, never as a parallel test.
    pub(in crate::ui) fn operation_controls() {
        for from_model_window in [true, false] {
            let app = App::new(
                None,
                Settings::default(),
                settings::SaveWorker::with_save(|_| Ok(())),
            );
            app.connect();
            let (tx, events) = mpsc::sync_channel(8);
            let cancel = Arc::new(AtomicBool::new(false));
            app.model_state.borrow_mut().operation = ModelOperation::Running(ModelJob {
                events,
                cancel: cancel.clone(),
                copying: true,
            });
            let (scanned, scan) = mpsc::channel();
            app.model_state.borrow_mut().scan = Some(scan);
            app.refresh();
            assert!(app.busy());
            assert_eq!(app.models.download.label().as_deref(), Some("Cancel Copy"));
            assert!(app.models.download.is_sensitive() && app.main.cancel.is_sensitive());
            assert!(!app.models.browse.is_sensitive());
            assert!(app.preferences.select_model.is_sensitive());
            if from_model_window {
                app.models.download.emit_clicked();
            } else {
                app.main.cancel.emit_clicked();
            }
            assert!(cancel.load(Ordering::Relaxed));
            for _ in 0..3 {
                app.refresh();
                assert!(!app.models.download.is_sensitive());
                assert!(!app.main.cancel.is_sensitive());
                assert!(app.busy());
            }
            tx.send(DownloadEvent::Finished(Err("Cancelled".into())))
                .unwrap();
            app.poll_model_download();
            assert!(!app.busy());
            assert_eq!(app.main.status.text(), "Model operation cancelled.");
            assert!(app.models.download.is_sensitive() && app.models.browse.is_sensitive());
            assert!(
                !app.models.list.is_sensitive(),
                "the independent scan is still pending"
            );
            scanned.send(Ok(Vec::new())).unwrap();
            app.poll_model_scan();
            assert!(app.models.list.is_sensitive());
            app.window.destroy();
        }

        let app = App::new(
            None,
            Settings::default(),
            settings::SaveWorker::with_save(|_| Ok(())),
        );
        let (_, events) = mpsc::channel();
        app.model_state.borrow_mut().availability = ModelAvailability::Checking(events);
        app.poll_model_check();
        assert!(!app.model_state.borrow().availability.is_checking());
        assert!(!app.model_state.borrow().availability.is_available());
        assert!(!app.main.open.is_sensitive());
        let (_, events) = mpsc::channel();
        app.model_state.borrow_mut().scan = Some(events);
        app.poll_model_scan();
        assert!(app.model_state.borrow().scan.is_none());
        assert!(app.models.status.text().contains("stopped unexpectedly"));
        let (_, events) = mpsc::channel();
        app.model_state.borrow_mut().operation = ModelOperation::CheckingDestination(events);
        app.poll_download_check();
        assert!(!app.busy());
        assert!(app.models.download.is_sensitive());
        let dialog = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|window| window.downcast::<adw::MessageDialog>().ok())
            .find(|dialog| dialog.heading().as_deref() == Some("Cannot check model destination"))
            .expect("destination worker failure is surfaced");
        dialog.close();
        app.window.destroy();
    }
}
