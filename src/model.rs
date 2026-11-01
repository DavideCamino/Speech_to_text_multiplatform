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

/// Run on a dedicated worker, never on GTK's main thread.
pub fn download(
    model: &Model,
    directory: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f64),
) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let target = directory.join(model.file);
    let temporary = directory.join(format!("{}.part", model.file));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
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
        let mut output = std::fs::File::create(&temporary)?;
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
        ensure!(received == model.bytes, "Incomplete model download");
        ensure!(
            format!("{:x}", hash.finalize()) == model.sha256,
            "Model checksum mismatch"
        );
        output.sync_all()?;
        drop(output);
        // A verified existing model need not be overwritten (not atomic on Windows).
        if target.exists() {
            std::fs::remove_file(&target)?;
        }
        std::fs::rename(&temporary, &target)?;
        Ok(target)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
