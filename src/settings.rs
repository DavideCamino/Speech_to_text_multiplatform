use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
        let dir = dirs()?.config_dir().to_owned();
        std::fs::create_dir_all(&dir)?;
        // No transcript, prompt, or audio is ever persisted here.
        std::fs::write(dir.join("settings.json"), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::GpuBackend;

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
