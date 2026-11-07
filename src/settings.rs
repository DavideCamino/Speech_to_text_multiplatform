use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub llm: crate::llm::Connection,
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

pub fn model_dir() -> Result<PathBuf> {
    Ok(dirs()?.data_dir().join("models"))
}

impl Settings {
    pub fn load() -> Result<Self> {
        let path = dirs()?.config_dir().join("settings.json");
        if !path.exists() {
            return Ok(Self::default());
        }
        serde_json::from_slice(&std::fs::read(path)?).context("Cannot read preferences")
    }

    pub fn save(&self) -> Result<()> {
        self.save_in(dirs()?.config_dir())
    }

    fn save_in(&self, directory: &Path) -> Result<()> {
        // Serialize before touching the filesystem. User prompts are stored as
        // plain text; transcript/results, audio, and auth tokens are not saved.
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
        temporary
            .persist(directory.join("settings.json"))
            .map_err(|error| error.error)
            .context("Cannot replace preferences")?;
        Ok(())
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
    pub fn new() -> Self {
        Self::with_save(|settings| settings.save())
    }

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
