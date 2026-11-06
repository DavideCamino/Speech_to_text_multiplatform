//! Explicit download helper for testing the same downloader the UI uses.
//! cargo run --example download_model -- /path/to/models [tiny|base|small|medium|large-v3]
use anyhow::{Context, Result};
use speech2text::model::{self, CATALOG};
use std::sync::atomic::AtomicBool;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let choices = CATALOG
        .iter()
        .map(|model| {
            model
                .file
                .strip_prefix("ggml-")
                .and_then(|name| name.strip_suffix(".bin"))
                .expect("catalog filenames use ggml-NAME.bin")
        })
        .collect::<Vec<_>>()
        .join("|");
    let directory = args
        .next()
        .with_context(|| format!("Usage: download_model DIRECTORY [{choices}]"))?;
    let name = args.next().unwrap_or_else(|| "base".into());
    let file = format!("ggml-{name}.bin");
    let model = CATALOG
        .iter()
        .find(|m| m.file == file)
        .with_context(|| format!("Choose a model: {choices}"))?;
    let mut previous = -1;
    let path = model::download(
        model,
        std::path::Path::new(&directory),
        model::Replacement::Never,
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
