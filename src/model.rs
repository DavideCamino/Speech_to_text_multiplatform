//! Explicit, cancellable downloads from a small, checksum-pinned model catalog.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
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
