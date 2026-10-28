//! Explicit download helper for testing the same downloader the UI uses.
//! cargo run --example download_model -- /path/to/models [tiny|base|small]
use anyhow::{Context, Result};
use speech2text::model::{self, CATALOG};
use std::sync::atomic::AtomicBool;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let directory = args
        .next()
        .context("Usage: download_model DIRECTORY [tiny|base|small]")?;
    let name = args.next().unwrap_or_else(|| "base".into());
    let file = format!("ggml-{name}.bin");
    let model = CATALOG
        .iter()
        .find(|m| m.file == file)
        .context("Choose tiny, base, or small")?;
    let mut previous = -1;
    let path = model::download(
        model,
        std::path::Path::new(&directory),
        &AtomicBool::new(false),
        |fraction| {
            let percent = (fraction * 100.0) as i32;
            if percent != previous {
                eprint!("\rDownloading: {percent}%");
                previous = percent;
            }
        },
    )?;
    println!("\nVerified model: {}", path.display());
    Ok(())
}
