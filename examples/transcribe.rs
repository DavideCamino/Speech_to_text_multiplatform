//! Backend smoke-test tool. No GTK application/window is created.
//! cargo run --example transcribe -- /path/to/ggml-base.bin /path/to/audio.m4a
use anyhow::{Context, Result, bail};
use speech2text::{
    settings::Settings,
    transcript::Transcript,
    transcription::{self, Event, Source},
};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let model = args
        .next()
        .context("Usage: transcribe MODEL AUDIO [--gpu]")?;
    let audio = args
        .next()
        .context("Usage: transcribe MODEL AUDIO [--gpu]")?;
    let use_gpu = args.next().is_some_and(|s| s == "--gpu");
    let settings = Settings {
        model: Some(model.into()),
        use_gpu,
        ..Settings::default()
    };
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
