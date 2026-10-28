use super::engine::Engine;
use crate::{
    audio::{AudioFile, Capture, Resampler, SAMPLE_RATE},
    settings::Settings,
    transcript::Segment,
};
use anyhow::{Result, bail};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

pub enum Source {
    File(PathBuf),
    Microphone,
}

#[derive(Debug)]
pub enum Event {
    Status(String),
    Progress(f64),
    Final(Vec<Segment>),
    Provisional(String),
    Meter { seconds: f64, level: f64 },
    Finished(Result<(), String>),
}

pub struct Job {
    pub events: Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn start(source: Source, settings: Settings, offset_ms: u64) -> Job {
    let (tx, events) = mpsc::sync_channel(128);
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let job = Job {
        events,
        cancel: cancel.clone(),
        stop: stop.clone(),
    };
    std::thread::spawn(move || {
        // A panic must still unlock the interface, not leave it in a busy state.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            send(&tx, Event::Status("Loading model…".into()));
            let mut engine = Engine::new(&settings)?;
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            match source {
                Source::File(path) => file(&mut engine, path, offset_ms, &tx, &cancel),
                Source::Microphone => {
                    microphone(&mut engine, &settings, offset_ms, &tx, &cancel, &stop)
                }
            }
        }));
        let result = match outcome {
            Ok(result) => result.map_err(|e| format!("{e:#}")),
            Err(_) => Err("The transcription worker stopped unexpectedly".into()),
        };
        send(&tx, Event::Provisional(String::new()));
        send(&tx, Event::Finished(result));
    });
    job
}

fn send(tx: &SyncSender<Event>, event: Event) {
    let _ = tx.send(event);
}

fn file(
    engine: &mut Engine,
    path: PathBuf,
    offset: u64,
    tx: &SyncSender<Event>,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut audio = AudioFile::open(&path)?;
    let mut pending = Vec::new();
    let mut processed = 0_usize;
    let window = SAMPLE_RATE * 28;
    send(tx, Event::Status("Transcribing audio file…".into()));
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let packet = audio.next_samples()?;
        let end = packet.is_none();
        if let Some(samples) = packet {
            pending.extend(samples);
        }
        while pending.len() >= window || (end && !pending.is_empty()) {
            let count = pending.len().min(window);
            let segments = engine.transcribe(
                &pending[..count],
                offset + processed as u64 * 1000 / SAMPLE_RATE as u64,
            )?;
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            send(tx, Event::Final(segments));
            pending.drain(..count);
            processed += count;
            if let Some(duration) = audio.duration_seconds.filter(|d| *d > 0.0) {
                send(
                    tx,
                    Event::Progress((processed as f64 / SAMPLE_RATE as f64 / duration).min(1.0)),
                );
            }
        }
        if end {
            break;
        }
    }
    send(tx, Event::Progress(1.0));
    Ok(())
}

fn microphone(
    engine: &mut Engine,
    settings: &Settings,
    offset: u64,
    tx: &SyncSender<Event>,
    cancel: &AtomicBool,
    stop: &AtomicBool,
) -> Result<()> {
    let capture = Capture::start(settings.device.as_deref())?;
    let mut resampler = Resampler::new(capture.rate);
    let mut pending = Vec::new();
    let mut consumed = 0_usize;
    let mut last_inference = Instant::now();
    let started = Instant::now();
    let mut audio_flushed = false;
    send(
        tx,
        Event::Status("Listening… provisional text may change".into()),
    );
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if capture.failed.load(Ordering::Relaxed) {
            bail!(
                "Microphone disconnected or inference could not keep up. Try a smaller model or GPU acceleration. Completed text has been kept."
            );
        }
        let stopping = stop.load(Ordering::Relaxed);
        if stopping && !audio_flushed {
            capture.pause()?;
        }
        if let Ok(samples) = capture.receiver.recv_timeout(Duration::from_millis(80)) {
            pending.extend(resampler.push(&samples));
        }
        // Drain all currently queued audio before Stop finalization.
        while let Ok(samples) = capture.receiver.try_recv() {
            pending.extend(resampler.push(&samples));
            if pending.len() > SAMPLE_RATE * 120 {
                bail!(
                    "Transcription fell more than two minutes behind; completed text has been kept"
                );
            }
        }
        if stopping && !audio_flushed {
            pending.extend(resampler.finish());
            audio_flushed = true;
        }
        send(
            tx,
            Event::Meter {
                seconds: started.elapsed().as_secs_f64(),
                level: f32::from_bits(capture.level.load(Ordering::Relaxed)) as f64,
            },
        );
        if (last_inference.elapsed() >= Duration::from_secs(3) && pending.len() >= SAMPLE_RATE)
            || (stopping && !pending.is_empty())
        {
            // Keep a revisable tail. Only discard audio through a finalized segment boundary.
            let count = pending.len().min(SAMPLE_RATE * 28);
            let at = offset + consumed as u64 * 1000 / SAMPLE_RATE as u64;
            let segments = engine.transcribe(&pending[..count], at)?;
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let duration_ms = count as u64 * 1000 / SAMPLE_RATE as u64;
            let finalize_all = stopping || count >= SAMPLE_RATE * 28;
            let cutoff = at + duration_ms.saturating_sub(2000);
            let (stable, provisional): (Vec<_>, Vec<_>) = segments
                .into_iter()
                .partition(|s| finalize_all || (count >= SAMPLE_RATE * 8 && s.end_ms <= cutoff));
            let committed = if finalize_all {
                count
            } else {
                stable
                    .last()
                    .map_or(0, |s| {
                        ((s.end_ms - at) * SAMPLE_RATE as u64 / 1000) as usize
                    })
                    .min(count)
            };
            send(tx, Event::Final(stable));
            send(
                tx,
                Event::Provisional(
                    provisional
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
            );
            pending.drain(..committed);
            consumed += committed;
            last_inference = Instant::now();
        }
        if stopping && pending.is_empty() {
            break;
        }
    }
    Ok(())
}
