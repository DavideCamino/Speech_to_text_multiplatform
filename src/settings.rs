use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub llm: crate::llm::Connection,
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
            model: None,
            language: None,
            device: None,
            use_gpu: true,
            gpu_backend: None,
        }
    }
}

fn dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("io.github", "Davide Camino", "Speech2Text")
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
        // Serialize before touching the filesystem. No transcript, prompt, or
        // audio is ever persisted here.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::GpuBackend;

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
