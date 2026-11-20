//! Shared on-disk backup operations.
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

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
