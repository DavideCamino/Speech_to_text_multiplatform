use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub model: Option<PathBuf>,
    /// None means automatic language detection; otherwise a Whisper language code.
    pub language: Option<String>,
    pub device: Option<String>,
    pub use_gpu: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: None,
            language: None,
            device: None,
            use_gpu: true,
        }
    }
}

fn dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("io.github", "YOUR_NAME", "Speech2Text")
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
