//! Backend smoke-test tool. No GTK application/window is created.
//! cargo run --example transcribe -- /path/to/ggml-base.bin /path/to/audio.m4a
use anyhow::{Context, Result, bail};
use speech2text::{
    settings::Settings,
    transcript::Transcript,
    transcription::{self, Event, GpuBackend, Source},
};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let model = args
        .next()
        .context("Usage: transcribe MODEL AUDIO [--gpu]")?;
    let audio = args
        .next()
        .context("Usage: transcribe MODEL AUDIO [--gpu]")?;
    let mut settings = Settings {
        model: Some(model.into()),
        use_gpu: false,
        ..Settings::default()
    };
    while let Some(option) = args.next() {
        match option.to_str() {
            Some("--gpu") => settings.use_gpu = true,
            Some("--backend") => {
                let name = args
                    .next()
                    .context("--backend requires vulkan, cuda, metal, or rocm")?;
                settings.gpu_backend = Some(match name.to_str() {
                    Some("vulkan") => GpuBackend::Vulkan,
                    Some("cuda") => GpuBackend::Cuda,
                    Some("metal") => GpuBackend::Metal,
                    Some("rocm") => GpuBackend::Rocm,
                    _ => bail!("Choose vulkan, cuda, metal, or rocm"),
                });
                settings.use_gpu = true;
            }
            _ => bail!("Usage: transcribe MODEL AUDIO [--gpu] [--backend vulkan|cuda|metal|rocm]"),
        }
    }
    let job = transcription::start(Source::File(audio.into()), settings, 0);
    let mut transcript = Transcript::default();
    loop {
        match job.events.recv()? {
            Event::Status(text) => eprintln!("{text}"),
            Event::Final(segments) => transcript.append(segments),
            Event::Finished(Ok(())) => break,
            Event::Finished(Err(error)) => bail!(error),
            _ => {}
        }
    }
    println!("{}", transcript.text());
    eprintln!("\n{}", transcript.srt());
    Ok(())
}
