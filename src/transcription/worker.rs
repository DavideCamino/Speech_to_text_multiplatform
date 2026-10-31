use super::{
    engine::Engine,
    overlap::{OVERLAP_MS, Reconciler},
};
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
    file_windows(path, offset, tx, cancel, |samples, at| {
        engine.transcribe(samples, at)
    })
}

fn file_windows(
    path: PathBuf,
    offset: u64,
    tx: &SyncSender<Event>,
    cancel: &AtomicBool,
    mut transcribe: impl FnMut(&[f32], u64) -> Result<Vec<Segment>>,
) -> Result<()> {
    let mut audio = AudioFile::open(&path)?;
    let mut pending = Vec::new();
    let mut processed = 0_usize;
    let window = SAMPLE_RATE * 28;
    let overlap = SAMPLE_RATE * OVERLAP_MS as usize / 1000;
    let mut reconciler = Reconciler::default();
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
        // One-packet lookahead distinguishes an exact-window EOF from a
        // non-final window, avoiding a second recognition of only its overlap.
        while pending.len() > window || (end && !pending.is_empty()) {
            let count = pending.len().min(window);
            let at = offset + processed as u64 * 1000 / SAMPLE_RATE as u64;
            let last_window = end && pending.len() <= window;
            let words = transcribe(&pending[..count], at)?;
            let until = at + count as u64 * 1000 / SAMPLE_RATE as u64;
            let (segments, _) = reconciler.update(words, at, until, last_window);
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            send(tx, Event::Final(segments));
            let advance = if last_window { count } else { count - overlap };
            pending.drain(..advance);
            processed += advance;
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
    let mut reconciler = Reconciler::default();
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
            let count = pending.len().min(SAMPLE_RATE * 28);
            let at = offset + consumed as u64 * 1000 / SAMPLE_RATE as u64;
            let words = engine.transcribe(&pending[..count], at)?;
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let until = at + count as u64 * 1000 / SAMPLE_RATE as u64;
            // Stop may have more than one window queued. Only the actual last
            // window is final; all earlier windows retain overlapping context.
            let last = stopping && count == pending.len();
            let (stable, provisional) = reconciler.update(words, at, until, last);
            let committed = if last {
                count
            } else {
                count.saturating_sub(SAMPLE_RATE * OVERLAP_MS as usize / 1000)
            };
            send(tx, Event::Final(stable));
            send(tx, Event::Provisional(provisional));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Transcript;
    use std::io::Write;

    fn silence(milliseconds: u64) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let data = vec![0_u8; (milliseconds * SAMPLE_RATE as u64 / 1000 * 2) as usize];
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data.len() as u32).to_le_bytes())
            .unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&(SAMPLE_RATE as u32).to_le_bytes()).unwrap();
        file.write_all(&(SAMPLE_RATE as u32 * 2).to_le_bytes())
            .unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        file.write_all(&data).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn file_lookahead_and_overlap_preserve_all_words_including_exact_eof() {
        for duration in [500, 28_000, 28_500, 52_000, 56_000] {
            let audio = silence(duration);
            let offset = 7_000;
            let (tx, rx) = mpsc::sync_channel(128);
            let mut calls = Vec::new();
            file_windows(
                audio.path().into(),
                offset,
                &tx,
                &AtomicBool::new(false),
                |samples, at| {
                    calls.push(at);
                    let end = at + samples.len() as u64 * 1000 / SAMPLE_RATE as u64;
                    Ok((0..duration)
                        .step_by(1000)
                        .filter_map(|ms| {
                            let start = offset + ms;
                            (start >= at && start + 400 <= end).then(|| Segment {
                                start_ms: start,
                                end_ms: start + 400,
                                text: format!("word{ms}"),
                            })
                        })
                        .collect())
                },
            )
            .unwrap();
            let mut transcript = Transcript::default();
            for event in rx.try_iter() {
                if let Event::Final(segments) = event {
                    transcript.append(segments);
                }
            }
            let expected = (0..duration)
                .step_by(1000)
                .map(|ms| format!("word{ms}"))
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(transcript.text(), expected, "duration {duration}");
            assert_eq!(
                calls.len(),
                if duration <= 28_000 {
                    1
                } else if duration <= 52_000 {
                    2
                } else {
                    3
                }
            );
            assert!(
                transcript
                    .segments
                    .windows(2)
                    .all(|pair| pair[0].end_ms <= pair[1].start_ms)
            );
        }
    }
}
