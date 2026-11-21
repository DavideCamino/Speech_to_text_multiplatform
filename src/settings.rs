use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub llm: crate::llm::Connection,
    pub llm_options: crate::llm::Options,
    /// Saved locally as plain text on normal application close.
    pub whisper_context: String,
    /// Saved locally as plain text; empty still requires input before rewriting.
    pub llm_system_prompt: String,
    pub model: Option<PathBuf>,
    /// None means automatic language detection; otherwise a Whisper language code.
    pub language: Option<String>,
    pub device: Option<String>,
    pub use_gpu: bool,
    /// None preserves Whisper's automatic choice. Stable across device reordering.
    pub gpu_backend: Option<crate::transcription::GpuBackend>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            llm: Default::default(),
            llm_options: Default::default(),
            whisper_context: String::new(),
            llm_system_prompt: String::new(),
            model: None,
            language: None,
            device: None,
            use_gpu: true,
            gpu_backend: None,
        }
    }
}

fn dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("io.github", "", "Speech2Text")
        .context("Cannot locate the user's application directories")
}

pub fn data_dir() -> Result<PathBuf> {
    Ok(dirs()?.data_dir().to_owned())
}

pub fn model_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("models"))
}

pub(crate) const FILE_NAME: &str = "settings.json";

// A parse failure means we read the file and can try to move it aside. An I/O
// failure can be temporary; leave the original where it is in that case.
enum ReadSettings {
    Loaded(Settings),
    Missing,
    Invalid,
    ReadError(io::Error),
}

fn read_settings(path: &Path) -> ReadSettings {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(settings) => ReadSettings::Loaded(settings),
            // Parser details can include private prompt text or other values.
            Err(_) => ReadSettings::Invalid,
        },
        Err(error) if error.kind() == ErrorKind::NotFound => ReadSettings::Missing,
        Err(error) => ReadSettings::ReadError(error),
    }
}

impl Settings {
    pub fn load() -> Result<Self> {
        match read_settings(&dirs()?.config_dir().join(FILE_NAME)) {
            ReadSettings::Loaded(settings) => Ok(settings),
            ReadSettings::Missing => Ok(Self::default()),
            ReadSettings::Invalid => anyhow::bail!("Invalid preferences JSON"),
            ReadSettings::ReadError(error) => Err(error).context("Cannot read preferences"),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_in(dirs()?.config_dir())
    }

    fn save_in(&self, directory: &Path) -> Result<()> {
        self.write_in(directory, true)
    }

    fn write_in(&self, directory: &Path, may_replace: bool) -> Result<()> {
        // Serialize before touching the filesystem. User prompts are stored as
        // plain text; transcripts/results, audio, and auth tokens are not saved
        // in preferences. Explicit session recovery uses a separate file.
        let contents = serde_json::to_vec_pretty(self)?;
        std::fs::create_dir_all(directory).context("Cannot create preferences directory")?;
        // Stage on the same filesystem so persist can atomically replace the
        // old configuration without a delete-then-rename gap. On errors, Drop
        // cleans up only this operation's temporary file.
        let mut temporary = tempfile::Builder::new()
            .prefix(".settings-")
            .suffix(".tmp")
            .tempfile_in(directory)
            .context("Cannot create temporary preferences file")?;
        temporary
            .write_all(&contents)
            .context("Cannot write preferences")?;
        temporary
            .as_file()
            .sync_all()
            .context("Cannot flush preferences")?;
        let path = directory.join(FILE_NAME);
        if may_replace {
            temporary
                .persist(&path)
                .map_err(|error| error.error)
                .context("Cannot replace preferences")?;
        } else {
            temporary.persist_noclobber(&path).map_err(|error| {
                if error.error.kind() == ErrorKind::AlreadyExists {
                    anyhow::anyhow!(
                        "Preferences at {} were not overwritten. Move the file aside before retrying.",
                        path.display()
                    )
                } else {
                    anyhow::Error::new(error.error).context("Cannot create preferences without overwriting an existing file")
                }
            })?;
        }
        Ok(())
    }
}

/// Startup settings and the write policy derived from inspecting the same path.
pub(crate) struct StartupSettings {
    settings: Settings,
    warning: Option<String>,
    directory: Option<PathBuf>,
    may_replace: bool,
}

impl StartupSettings {
    pub(crate) fn load() -> Self {
        match dirs() {
            Ok(dirs) => Self::load_in(dirs.config_dir()),
            Err(error) => Self::unavailable(format!("{error:#}")),
        }
    }

    pub(crate) fn unavailable(error: String) -> Self {
        Self {
            settings: Settings::default(),
            warning: Some(error),
            directory: None,
            may_replace: false,
        }
    }

    pub(crate) fn load_in(directory: &Path) -> Self {
        Self::load_with_quarantine(directory, |path| {
            crate::storage::quarantine(path, "settings-unreadable-")
        })
    }

    fn load_with_quarantine(
        directory: &Path,
        quarantine: impl FnOnce(&Path) -> Result<PathBuf>,
    ) -> Self {
        let path = directory.join(FILE_NAME);
        let mut startup = Self {
            settings: Settings::default(),
            warning: None,
            directory: Some(directory.to_owned()),
            may_replace: false,
        };
        match read_settings(&path) {
            ReadSettings::Loaded(settings) => {
                startup.settings = settings;
                startup.may_replace = true;
            }
            ReadSettings::Missing => {}
            ReadSettings::Invalid => {
                startup.warning = Some(match quarantine(&path) {
                    Ok(backup) => format!(
                        "Preferences were not valid and were moved to {}. The app is using defaults.",
                        backup.display()
                    ),
                    Err(error) => format!(
                        "Preferences at {} were not valid and could not be moved aside: {error:#}. The app is using defaults and will not overwrite the original file.",
                        path.display()
                    ),
                });
            }
            ReadSettings::ReadError(error) => {
                startup.warning = Some(format!(
                    "Could not read preferences at {}: {error}. The app is using defaults and will not overwrite the original file.",
                    path.display()
                ));
            }
        }
        startup
    }

    pub(crate) fn into_parts(self) -> (Settings, SaveWorker, Option<String>) {
        let directory = self.directory;
        let mut may_replace = self.may_replace;
        let writer = SaveWorker::with_save(move |settings| {
            let directory = directory.as_deref().context(
                "Preferences location was not verified at startup; restart before saving",
            )?;
            settings.write_in(directory, may_replace)?;
            // Only a successful publication permits subsequent replacement.
            may_replace = true;
            Ok(())
        });
        (self.settings, writer, self.warning)
    }
}

/// A single writer serializes atomic saves. Queued snapshots may be coalesced,
/// but an older save can never finish after a newer one and overwrite it.
pub(crate) struct SaveWorker {
    requests: Sender<(u64, Settings)>,
    results: Receiver<(u64, Result<(), String>)>,
    latest: u64,
    pending: bool,
}

impl SaveWorker {
    pub(crate) fn with_save(mut save: impl FnMut(Settings) -> Result<()> + Send + 'static) -> Self {
        let (requests, incoming) = mpsc::channel::<(u64, Settings)>();
        let (finished, results) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(mut request) = incoming.recv() {
                // Only the newest queued snapshot needs to reach disk.
                for newer in incoming.try_iter() {
                    request = newer;
                }
                let (revision, settings) = request;
                let result = save(settings).map_err(|error| format!("{error:#}"));
                if finished.send((revision, result)).is_err() {
                    break;
                }
            }
        });
        Self {
            requests,
            results,
            latest: 0,
            pending: false,
        }
    }

    pub fn submit(&mut self, settings: Settings) -> Result<(), String> {
        self.latest += 1;
        self.requests
            .send((self.latest, settings))
            .map_err(|_| "Preferences worker stopped unexpectedly".to_owned())?;
        self.pending = true;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn submitted_count(&self) -> u64 {
        self.latest
    }

    /// Report only the latest requested save, never a stale success/error.
    pub fn poll(&mut self) -> Option<Result<(), String>> {
        loop {
            match self.results.try_recv() {
                Ok((revision, result)) if revision == self.latest => {
                    self.pending = false;
                    return Some(result);
                }
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return if std::mem::take(&mut self.pending) {
                        Some(Err("Preferences worker stopped unexpectedly".into()))
                    } else {
                        None
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::GpuBackend;

    #[test]
    fn llm_selection_defaults_and_round_trips() {
        let old: Settings =
            serde_json::from_str(r#"{"llm":{"host":"localhost","port":8080}}"#).unwrap();
        assert!(old.llm_options.model.is_none());
        assert_eq!(old.llm_options.reasoning, crate::llm::Reasoning::Default);
        let settings = Settings {
            llm_options: crate::llm::Options {
                model: Some("chosen-model".into()),
                reasoning: crate::llm::Reasoning::High,
            },
            ..Default::default()
        };
        let decoded: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(decoded.llm_options.model.as_deref(), Some("chosen-model"));
        assert_eq!(decoded.llm_options.reasoning, crate::llm::Reasoning::High);
    }

    #[test]
    fn background_saves_are_ordered_and_ignore_stale_results() {
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().to_owned();
        let ui_thread = std::thread::current().id();
        let (entered, started) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let mut worker = SaveWorker::with_save(move |settings| {
            assert_ne!(std::thread::current().id(), ui_thread);
            entered.send(settings.language.clone()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
            settings.save_in(&directory)
        });
        let snapshot = |language: &str| Settings {
            language: Some(language.into()),
            ..Settings::default()
        };
        worker.submit(snapshot("en")).unwrap();
        assert_eq!(
            started
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .as_deref(),
            Some("en")
        );
        // Submitting and polling stay nonblocking while the disk writer waits.
        worker.submit(snapshot("de")).unwrap();
        worker.submit(snapshot("fr")).unwrap();
        assert!(worker.poll().is_none());
        release.send(()).unwrap();
        assert_eq!(
            started
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .as_deref(),
            Some("fr")
        );
        // The older save completed, but the latest is still pending.
        assert!(worker.poll().is_none());
        assert!(worker.pending);
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = worker.poll() {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!worker.pending);
        let saved: Settings =
            serde_json::from_slice(&std::fs::read(root.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(saved.language.as_deref(), Some("fr"));
    }

    #[test]
    fn background_save_errors_and_worker_disconnect_are_reported() {
        use std::time::{Duration, Instant};
        let mut worker = SaveWorker::with_save(|_| anyhow::bail!("simulated disk failure"));
        worker.submit(Settings::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = worker.poll() {
                assert!(result.unwrap_err().contains("simulated disk failure"));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let (requests, incoming) = mpsc::channel();
        let (finished, results) = mpsc::channel();
        let mut disconnected = SaveWorker {
            requests,
            results,
            latest: 0,
            pending: false,
        };
        disconnected.submit(Settings::default()).unwrap();
        drop(incoming);
        drop(finished);
        assert!(
            disconnected
                .poll()
                .unwrap()
                .unwrap_err()
                .contains("stopped unexpectedly")
        );
    }

    fn submit_and_wait(worker: &mut SaveWorker, settings: Settings) -> Result<(), String> {
        use std::time::{Duration, Instant};
        worker.submit(settings)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = worker.poll() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for preferences"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn failed_backup_keeps_original_and_requires_a_successful_first_publication() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE_NAME);
        let original = b"{\"whisper_context\": \"private unfinished prompt\"";
        std::fs::write(&path, original).unwrap();
        let (mut defaults, mut writer, warning) =
            StartupSettings::load_with_quarantine(root.path(), |path| {
                assert_eq!(std::fs::read(path).unwrap(), original);
                anyhow::bail!("simulated backup failure");
            })
            .into_parts();
        let warning = warning.unwrap();
        assert!(warning.contains("simulated backup failure"));
        assert!(!warning.contains("private unfinished prompt"));
        for _ in 0..2 {
            let error = submit_and_wait(&mut writer, defaults.clone()).unwrap_err();
            assert!(error.contains("not overwritten"));
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }

        let manual_backup = root.path().join("manual-backup.json");
        std::fs::rename(&path, &manual_backup).unwrap();
        submit_and_wait(&mut writer, defaults.clone()).unwrap();
        defaults.whisper_context = "New prompt".into();
        submit_and_wait(&mut writer, defaults).unwrap();
        let ReadSettings::Loaded(saved) = read_settings(&path) else {
            panic!("expected new settings");
        };
        assert_eq!(saved.whisper_context, "New prompt");
        assert_eq!(std::fs::read(&manual_backup).unwrap(), original);
    }

    #[test]
    fn unreadable_path_is_not_moved_on_io_error() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE_NAME);
        std::fs::create_dir(&path).unwrap();
        let (_, mut writer, warning) = StartupSettings::load_with_quarantine(root.path(), |_| {
            panic!("must not quarantine on a read error");
        })
        .into_parts();
        assert!(warning.unwrap().contains("Could not read preferences"));
        assert!(submit_and_wait(&mut writer, Settings::default()).is_err());
        assert!(path.is_dir());
    }

    #[test]
    fn missing_preferences_do_not_authorize_replacing_a_new_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE_NAME);
        let (defaults, mut writer, warning) = StartupSettings::load_in(root.path()).into_parts();
        assert!(warning.is_none());
        let existing = br#"{"language":"en"}"#;
        std::fs::write(&path, existing).unwrap();
        assert!(submit_and_wait(&mut writer, defaults).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), existing);
    }

    #[test]
    fn loaded_preferences_can_be_replaced_on_the_first_save() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE_NAME);
        std::fs::write(&path, br#"{"language":"en"}"#).unwrap();
        let (mut loaded, mut writer, warning) = StartupSettings::load_in(root.path()).into_parts();
        assert!(warning.is_none());
        assert_eq!(loaded.language.as_deref(), Some("en"));
        loaded.language = Some("it".into());
        submit_and_wait(&mut writer, loaded).unwrap();
        let ReadSettings::Loaded(saved) = read_settings(&path) else {
            panic!("expected updated preferences");
        };
        assert_eq!(saved.language.as_deref(), Some("it"));
    }

    #[test]
    fn prompts_round_trip_and_can_be_cleared_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let mut settings = Settings {
            whisper_context: " Giulia, GTK4\n日本語 ".into(),
            llm_system_prompt: "Preserve facts.\nDo not invent information.".into(),
            ..Settings::default()
        };
        settings.save_in(root.path()).unwrap();
        let load = || -> Settings {
            serde_json::from_slice(&std::fs::read(root.path().join("settings.json")).unwrap())
                .unwrap()
        };
        let restored = load();
        assert_eq!(restored.whisper_context, settings.whisper_context);
        assert_eq!(restored.llm_system_prompt, settings.llm_system_prompt);
        settings.whisper_context.clear();
        settings.llm_system_prompt.clear();
        settings.save_in(root.path()).unwrap();
        let restored = load();
        assert!(restored.whisper_context.is_empty());
        assert!(restored.llm_system_prompt.is_empty());
    }

    #[test]
    fn save_creates_directory_and_writes_valid_preferences() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("config");
        let settings = Settings {
            language: Some("it".into()),
            ..Settings::default()
        };
        settings.save_in(&directory).unwrap();
        let saved = std::fs::read(directory.join("settings.json")).unwrap();
        let restored: Settings = serde_json::from_slice(&saved).unwrap();
        assert_eq!(restored.language.as_deref(), Some("it"));
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }

    #[test]
    fn save_replaces_existing_preferences_without_leaking_temporary_files() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("settings.json");
        let mut settings = Settings::default();
        settings.save_in(root.path()).unwrap();
        settings.language = Some("fr".into());
        settings.use_gpu = false;
        settings.save_in(root.path()).unwrap();
        let restored: Settings = serde_json::from_slice(&std::fs::read(target).unwrap()).unwrap();
        assert_eq!(restored.language.as_deref(), Some("fr"));
        assert!(!restored.use_gpu);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_replacement_preserves_destination_and_cleans_up_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        // A nonempty directory cannot be replaced by the staged regular file.
        let target = root.path().join("settings.json");
        std::fs::create_dir(&target).unwrap();
        let existing = target.join("keep.json");
        std::fs::write(&existing, b"existing data").unwrap();
        let error = Settings::default().save_in(root.path()).unwrap_err();
        assert!(error.to_string().contains("Cannot replace preferences"));
        assert_eq!(std::fs::read(existing).unwrap(), b"existing data");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn older_preferences_default_to_automatic_backend() {
        let settings: Settings = serde_json::from_str(r#"{"use_gpu":true}"#).unwrap();
        assert_eq!(settings.gpu_backend, None);
        assert!(settings.use_gpu);
        assert_eq!(settings.llm.host, "127.0.0.1");
        assert_eq!(settings.llm.port, 8080);
        assert!(!settings.llm.https);
        assert!(settings.whisper_context.is_empty());
        assert!(settings.llm_system_prompt.is_empty());
    }

    #[test]
    fn llm_connection_round_trips_without_credentials() {
        let settings = Settings {
            llm: crate::llm::Connection {
                host: "192.168.1.8".into(),
                port: 8443,
                https: true,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("token"));
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.llm.host, "192.168.1.8");
        assert_eq!(restored.llm.port, 8443);
        assert!(restored.llm.https);
    }

    #[test]
    fn backend_choice_round_trips_even_with_acceleration_off() {
        for backend in [
            GpuBackend::Vulkan,
            GpuBackend::Cuda,
            GpuBackend::Metal,
            GpuBackend::Rocm,
        ] {
            let settings = Settings {
                use_gpu: false,
                gpu_backend: Some(backend),
                ..Settings::default()
            };
            let json = serde_json::to_string(&settings).unwrap();
            let restored: Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(restored.gpu_backend, Some(backend));
            assert!(!restored.use_gpu);
        }
    }
}
