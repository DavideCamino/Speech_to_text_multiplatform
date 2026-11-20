//! Explicit save-on-close recovery. No audio, credentials, or running jobs.
use crate::transcript::Transcript;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
};

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const FILE_NAME: &str = "recovery.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranscriptSnapshot {
    pub text: String,
    pub original: Transcript,
    pub dirty: bool,
    pub text_edited: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RewriteSnapshot {
    pub source: String,
    pub prompt: String,
    pub markdown: String,
    pub dirty: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub transcript: TranscriptSnapshot,
    pub rewrite: RewriteSnapshot,
    pub recognition_context: String,
    pub showing_rewrite: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: VERSION,
            transcript: Default::default(),
            rewrite: Default::default(),
            recognition_context: String::new(),
            showing_rewrite: false,
        }
    }
}
impl Snapshot {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == VERSION, "Unsupported recovery file version");
        let mut end = 0;
        for segment in &self.transcript.original.segments {
            ensure!(
                segment.start_ms >= end && segment.end_ms > segment.start_ms,
                "Invalid recovered subtitle timings"
            );
            end = segment.end_ms;
        }
        Ok(())
    }
}

pub fn save(path: &Path, snapshot: &Snapshot) -> Result<()> {
    write_snapshot(path, snapshot, true)
}

/// A new session must not replace a recovery file it did not restore or save.
pub(crate) fn save_new(path: &Path, snapshot: &Snapshot) -> Result<()> {
    write_snapshot(path, snapshot, false)
}

fn write_snapshot(path: &Path, snapshot: &Snapshot, replace: bool) -> Result<()> {
    snapshot.validate()?;
    let bytes = serde_json::to_vec(snapshot).context("Cannot encode recovery data")?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Recovery data exceeds the 64 MiB limit"
    );
    let directory = path
        .parent()
        .context("Recovery file has no parent directory")?;
    std::fs::create_dir_all(directory).context("Cannot create recovery directory")?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".recovery-")
        .suffix(".tmp")
        .tempfile_in(directory)?;
    temporary
        .write_all(&bytes)
        .context("Cannot write recovery file")?;
    temporary
        .as_file()
        .sync_all()
        .context("Cannot flush recovery file")?;
    if replace {
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .context("Cannot replace recovery file")?;
    } else {
        temporary.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::anyhow!(
                    "A recovery file already exists at {} and was not overwritten. Move it aside before retrying.",
                    path.display()
                )
            } else {
                anyhow::Error::new(error.error).context("Cannot publish recovery file")
            }
        })?;
    }
    Ok(())
}

/// Preserve unreadable recovery files before offering the primary path to a new session.
pub(crate) fn load_at_startup(path: &Path) -> Result<Option<Snapshot>> {
    match load(path) {
        Ok(snapshot) => Ok(snapshot),
        Err(error) => match quarantine(path) {
            Ok(backup) => Err(anyhow::anyhow!(
                "{error:#}\n\nThe recovery file was moved to {}. It will not be overwritten by a new autosave.",
                backup.display()
            )),
            Err(move_error) => Err(anyhow::anyhow!(
                "{error:#}\n\nCould not move the recovery file aside: {move_error:#}. The app will not overwrite it."
            )),
        },
    }
}

fn quarantine(path: &Path) -> Result<std::path::PathBuf> {
    ensure!(
        std::fs::symlink_metadata(path)?.is_file(),
        "Recovery path is not a regular file"
    );
    let directory = path
        .parent()
        .context("Recovery file has no parent directory")?;
    // A unique directory in the same parent gives the rename a fresh destination
    // on the same filesystem without replacing any previous backup.
    let backup_dir = tempfile::Builder::new()
        .prefix("recovery-unreadable-")
        .tempdir_in(directory)?;
    let backup = backup_dir.path().join(FILE_NAME);
    std::fs::rename(path, &backup)?;
    Ok(backup_dir.keep().join(FILE_NAME))
}

/// Reading never consumes the file. Delete only after the UI has restored it.
pub fn load(path: &Path) -> Result<Option<Snapshot>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot open recovery file"),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Cannot read recovery file")?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Recovery file exceeds the 64 MiB limit"
    );
    // Do not expose parser diagnostics: they may contain private text.
    let snapshot: Snapshot = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid recovery file; the file has been preserved"))?;
    snapshot.validate()?;
    Ok(Some(snapshot))
}

pub fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Cannot delete restored recovery file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Segment;

    #[test]
    fn round_trip_replacement_and_explicit_consumption() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("data/recovery.json");
        assert!(load(&path).unwrap().is_none());
        let mut snapshot = Snapshot::default();
        snapshot.transcript.text = "Edited transcript 世界".into();
        snapshot.transcript.dirty = true;
        snapshot.transcript.text_edited = true;
        snapshot.transcript.original.segments.push(Segment {
            start_ms: 0,
            end_ms: 1000,
            text: " Original".into(),
        });
        snapshot.rewrite.markdown = "# Partial rewrite".into();
        snapshot.rewrite.dirty = true;
        save(&path, &snapshot).unwrap();
        let restored = load(&path).unwrap().unwrap();
        assert_eq!(restored.transcript.text, snapshot.transcript.text);
        assert_eq!(
            restored.transcript.original.srt(),
            snapshot.transcript.original.srt()
        );
        assert!(restored.transcript.dirty && restored.rewrite.dirty);
        assert!(path.exists());
        snapshot.transcript.text.clear();
        save(&path, &snapshot).unwrap();
        assert!(load(&path).unwrap().unwrap().transcript.text.is_empty());
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        remove(&path).unwrap();
        remove(&path).unwrap();
        assert!(load(&path).unwrap().is_none());
    }

    #[test]
    fn unreadable_recovery_survives_a_new_session_autosave() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE_NAME);
        let future = Snapshot {
            version: VERSION + 1,
            ..Snapshot::default()
        };
        let previous = serde_json::to_vec(&future).unwrap();
        std::fs::write(&path, &previous).unwrap();

        let message = load_at_startup(&path).unwrap_err().to_string();
        assert!(message.contains("Unsupported recovery file version"));
        let backup = std::fs::read_dir(root.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join(FILE_NAME);
        assert!(message.contains(&backup.display().to_string()));
        assert!(!path.exists());

        save_new(&path, &Snapshot::default()).unwrap();
        assert!(load(&path).unwrap().is_some());
        assert_eq!(std::fs::read(&backup).unwrap(), previous);
        remove(&path).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), previous);
    }

    #[test]
    fn unowned_recovery_path_is_never_replaced() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE_NAME);
        std::fs::write(&path, b"unowned data").unwrap();
        let error = save_new(&path, &Snapshot::default()).unwrap_err();
        assert!(format!("{error:#}").contains("not overwritten"));
        assert_eq!(std::fs::read(&path).unwrap(), b"unowned data");

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(load_at_startup(&path).is_err());
        assert!(save_new(&path, &Snapshot::default()).is_err());
        assert!(path.is_dir());
    }

    #[test]
    fn invalid_files_and_failed_saves_preserve_existing_data() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE_NAME);
        for bytes in [b"not json".as_slice(), b"{\"version\":999}".as_slice()] {
            std::fs::write(&path, bytes).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let snapshot = Snapshot::default();
        save(&path, &snapshot).unwrap();
        let before = std::fs::read(&path).unwrap();
        let invalid = Snapshot {
            version: 999,
            ..snapshot
        };
        assert!(save(&path, &invalid).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let blocked = root.path().join("blocked");
        std::fs::write(&blocked, "file").unwrap();
        assert!(save(&blocked.join(FILE_NAME), &Snapshot::default()).is_err());
    }
}
