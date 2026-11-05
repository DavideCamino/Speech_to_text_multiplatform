//! Local model library, safe imports, and explicit checksum-pinned downloads.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub struct Model {
    pub name: &'static str,
    pub file: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub const CATALOG: &[Model] = &[
    Model {
        name: "Tiny · multilingual · 78 MB",
        file: "ggml-tiny.bin",
        bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
    },
    Model {
        name: "Base · multilingual · 148 MB",
        file: "ggml-base.bin",
        bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
    },
    Model {
        name: "Small · multilingual · 488 MB",
        file: "ggml-small.bin",
        bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    },
    Model {
        name: "Medium · multilingual · 1.5 GiB",
        file: "ggml-medium.bin",
        bytes: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
    },
    Model {
        name: "Large V3 · multilingual · 2.9 GiB",
        file: "ggml-large-v3.bin",
        bytes: 3_095_033_483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    },
];

/// List installed ggml models without creating the directory on first launch.
pub fn installed(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("Cannot list the model directory"),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
            && path.is_file()
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Copy on a worker, staging the entire file before publishing it. Never
/// overwrite an existing model, and remove temporary data on failure/cancel.
pub fn import(
    source: &Path,
    directory: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f64),
) -> Result<PathBuf> {
    ensure!(!cancel.load(Ordering::Relaxed), "Copy cancelled");
    let name = source.file_name().context("Model has no filename")?;
    ensure!(
        source
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bin")),
        "Select a ggml .bin model"
    );
    let mut input = std::fs::File::open(source).context("Cannot open selected model")?;
    let metadata = input.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() > 0,
        "Selected model must be a non-empty file"
    );
    std::fs::create_dir_all(directory)?;
    let target = directory.join(name);
    ensure!(
        !target.exists(),
        "A model named {} already exists in the model folder; use it or select the original file instead",
        name.to_string_lossy()
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(".model-import-")
        .suffix(".part")
        .tempfile_in(directory)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut copied = 0_u64;
    loop {
        ensure!(!cancel.load(Ordering::Relaxed), "Copy cancelled");
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        temporary.write_all(&buffer[..count])?;
        copied += count as u64;
        ensure!(
            copied <= metadata.len(),
            "Source model changed while copying"
        );
        progress(copied as f64 / metadata.len() as f64);
    }
    ensure!(
        copied == metadata.len(),
        "Source model changed while copying"
    );
    temporary.as_file().sync_all()?;
    ensure!(!cancel.load(Ordering::Relaxed), "Copy cancelled");
    temporary
        .persist_noclobber(&target)
        .map_err(|error| error.error)
        .context("Cannot install copied model (the destination may already exist)")?;
    Ok(target)
}

async fn cancellable<T>(
    cancel: &AtomicBool,
    operation: impl std::future::Future<Output = Result<T, reqwest::Error>>,
) -> Result<T> {
    let mut operation = std::pin::pin!(operation);
    loop {
        ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
        if let Ok(result) = tokio::time::timeout(Duration::from_millis(100), &mut operation).await {
            return Ok(result?);
        }
    }
}

/// Replacement must be explicitly authorized by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replacement {
    Never,
    Confirmed,
}

fn publish_download(
    output: tempfile::NamedTempFile,
    target: &Path,
    replacement: Replacement,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    output.as_file().sync_all()?;
    ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
    // Both operations keep an existing destination intact on failure. Staging
    // in the destination directory ensures publication stays on one filesystem.
    let result = match replacement {
        Replacement::Never => output.persist_noclobber(target),
        Replacement::Confirmed => output.persist(target),
    };
    result.map_err(|error| error.error).context(
        "Cannot install downloaded model (the destination may already exist or be in use)",
    )?;
    Ok(target.to_owned())
}

fn verify_download(model: &Model, received: u64, hash: Sha256) -> Result<()> {
    ensure!(received == model.bytes, "Incomplete model download");
    ensure!(
        format!("{:x}", hash.finalize()) == model.sha256,
        "Model checksum mismatch"
    );
    Ok(())
}

/// Run on a dedicated worker, never on GTK's main thread.
/// An existing model is never deleted before its verified replacement is ready.
pub fn download(
    model: &Model,
    directory: &Path,
    replacement: Replacement,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f64),
) -> Result<PathBuf> {
    ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
    std::fs::create_dir_all(directory)?;
    let target = directory.join(model.file);
    ensure!(
        replacement == Replacement::Confirmed || !target.try_exists()?,
        "A model already exists at {}; replacement requires confirmation",
        target.display()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(1800))
            .build()?;
        let request = client.get(format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            model.file
        ));
        let mut response = cancellable(cancel, request.send())
            .await?
            .error_for_status()?;
        // Each operation owns its staging file; Drop cleans up only that file.
        let mut output = tempfile::Builder::new()
            .prefix(".model-download-")
            .suffix(".part")
            .tempfile_in(directory)?;
        let mut hash = Sha256::new();
        let mut received = 0_u64;
        while let Some(chunk) = cancellable(cancel, response.chunk())
            .await
            .context("Model download interrupted")?
        {
            received += chunk.len() as u64;
            ensure!(received <= model.bytes, "Model exceeds its expected size");
            hash.update(&chunk);
            output.write_all(&chunk)?;
            progress(received as f64 / model.bytes as f64);
        }
        ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
        verify_download(model, received, hash)?;
        publish_download(output, &target, replacement, cancel)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged(directory: &Path) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new_in(directory).unwrap();
        file.write_all(b"new model").unwrap();
        file
    }

    #[test]
    fn confirmed_replacement_publishes_new_model_and_removes_staging_path() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("model.bin");
        std::fs::write(&target, b"old model").unwrap();
        let file = staged(directory.path());
        let staging = file.path().to_owned();
        assert_eq!(
            publish_download(
                file,
                &target,
                Replacement::Confirmed,
                &AtomicBool::new(false)
            )
            .unwrap(),
            target
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"new model");
        assert!(!staging.exists());
    }

    #[test]
    fn destination_collision_without_permission_preserves_existing_model() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("model.bin");
        let file = staged(directory.path());
        let staging = file.path().to_owned();
        // Simulate another writer creating the destination during the download.
        std::fs::write(&target, b"other model").unwrap();
        assert!(
            publish_download(file, &target, Replacement::Never, &AtomicBool::new(false)).is_err()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"other model");
        assert!(!staging.exists());
    }

    #[test]
    fn cancelled_replacement_preserves_existing_model_and_cleans_staging() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("model.bin");
        std::fs::write(&target, b"old model").unwrap();
        let file = staged(directory.path());
        let staging = file.path().to_owned();
        assert!(
            publish_download(
                file,
                &target,
                Replacement::Confirmed,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"old model");
        assert!(!staging.exists());
    }

    #[test]
    fn failed_publication_does_not_delete_destination_or_leak_staging() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("model.bin");
        std::fs::create_dir(&target).unwrap();
        let existing = target.join("keep.bin");
        std::fs::write(&existing, b"keep").unwrap();
        let file = staged(directory.path());
        let staging = file.path().to_owned();
        assert!(
            publish_download(
                file,
                &target,
                Replacement::Confirmed,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(std::fs::read(existing).unwrap(), b"keep");
        assert!(!staging.exists());
    }

    #[test]
    fn verification_failures_preserve_old_model_and_clean_staging() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("model.bin");
        std::fs::write(&target, b"old model").unwrap();
        let model = Model {
            name: "Test",
            file: "model.bin",
            bytes: 9,
            sha256: "not-the-correct-checksum",
        };
        for received in [3, 9] {
            let file = staged(directory.path());
            let staging = file.path().to_owned();
            let result = (|| {
                verify_download(&model, received, Sha256::new())?;
                publish_download(
                    file,
                    &target,
                    Replacement::Confirmed,
                    &AtomicBool::new(false),
                )
            })();
            assert!(result.is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"old model");
            assert!(!staging.exists());
        }
    }

    #[test]
    fn cancelled_download_does_not_remove_another_operations_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let partial = directory.path().join("ggml-tiny.bin.part");
        std::fs::write(&partial, b"another download").unwrap();
        assert!(
            download(
                &CATALOG[0],
                directory.path(),
                Replacement::Confirmed,
                &AtomicBool::new(true),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(std::fs::read(partial).unwrap(), b"another download");
    }

    #[test]
    fn unconfirmed_existing_model_is_rejected_before_network_access() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join(CATALOG[0].file);
        std::fs::write(&target, b"old model").unwrap();
        let error = download(
            &CATALOG[0],
            directory.path(),
            Replacement::Never,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("confirmation"));
        assert_eq!(std::fs::read(target).unwrap(), b"old model");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
