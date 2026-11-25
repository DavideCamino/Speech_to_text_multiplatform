//! Shared on-disk publication and backup operations.
use anyhow::{Context, Result, ensure};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
pub(crate) enum PublishMode {
    Replace,
    CreateOnly,
}

#[derive(Debug)]
pub(crate) enum PublishError {
    MissingParent,
    CreateDirectory(io::Error),
    CreateTemporary(io::Error),
    Write(io::Error),
    Flush(io::Error),
    Publish(io::Error),
}

/// Stage and flush bytes beside their destination before publishing them.
/// In create-only mode the filesystem operation, not a prior check, detects collisions.
pub(crate) fn publish_bytes(
    path: &Path,
    bytes: &[u8],
    prefix: &str,
    mode: PublishMode,
) -> std::result::Result<(), PublishError> {
    let directory = path.parent().ok_or(PublishError::MissingParent)?;
    std::fs::create_dir_all(directory).map_err(PublishError::CreateDirectory)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".tmp")
        .tempfile_in(directory)
        .map_err(PublishError::CreateTemporary)?;
    temporary.write_all(bytes).map_err(PublishError::Write)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(PublishError::Flush)?;
    // On failure the persist error still owns the temporary file. Dropping it
    // cleans up this operation's staging file without touching the destination.
    match mode {
        PublishMode::Replace => temporary.persist(path),
        PublishMode::CreateOnly => temporary.persist_noclobber(path),
    }
    .map(|_| ())
    .map_err(|error| PublishError::Publish(error.error))
}

/// Move a regular file to a fresh destination on the same filesystem.
pub(crate) fn quarantine(path: &Path, prefix: &str) -> Result<PathBuf> {
    ensure!(
        std::fs::symlink_metadata(path)?.is_file(),
        "Not a regular file: {}",
        path.display()
    );
    let directory = path.parent().context("File has no parent directory")?;
    let name = path.file_name().context("File has no name")?;
    // A unique sibling directory keeps the rename on the same filesystem and
    // avoids replacing any previous backup. Drop removes it if the move fails.
    let backup_dir = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(directory)?;
    std::fs::rename(path, backup_dir.path().join(name))?;
    Ok(backup_dir.keep().join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_only_collision_keeps_original_and_cleans_up_staging_file() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("new-directory");
        let path = directory.join("data.json");
        publish_bytes(&path, b"original", ".test-", PublishMode::CreateOnly).unwrap();
        let error =
            publish_bytes(&path, b"different", ".test-", PublishMode::CreateOnly).unwrap_err();
        assert!(
            matches!(error, PublishError::Publish(error) if error.kind() == io::ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }

    #[test]
    fn replace_publishes_new_bytes_without_leaving_a_staging_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("data.json");
        std::fs::write(&path, b"longer original").unwrap();
        publish_bytes(&path, b"new", ".test-", PublishMode::Replace).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
