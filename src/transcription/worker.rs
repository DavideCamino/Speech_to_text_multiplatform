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
            let stopped_before_capture =
                || matches!(source, Source::Microphone) && stop.load(Ordering::Relaxed);
            if cancel.load(Ordering::Relaxed) || stopped_before_capture() {
                return Ok(());
            }
            send(
                &tx,
                Event::Status(
                    "Loading model… cancellation waits for model loading to finish".into(),
                ),
            );
            let engine = Engine::new(&settings, cancel.clone());
            if cancel.load(Ordering::Relaxed) || stopped_before_capture() {
                return Ok(());
            }
            let mut engine = engine?;
            match source {
                Source::File(path) => file(&mut engine, path, offset_ms, &tx, &cancel),
                Source::Microphone => {
                    microphone(&mut engine, &settings, offset_ms, &tx, &cancel, &stop)
                }
            }
        }));
        let result = match outcome {
            Ok(_) if cancel.load(Ordering::Relaxed) => Ok(()),
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
    cancel: &Arc<AtomicBool>,
    stop: &Arc<AtomicBool>,
) -> Result<()> {
    let capture = match Capture::start(settings.device.as_deref(), stop.clone(), cancel.clone()) {
        Ok(capture) => capture,
        Err(_) if cancel.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) => return Ok(()),
        Err(error) => return Err(error),
    };
    microphone_capture(
        capture,
        offset,
        tx,
        cancel,
        stop,
        Duration::from_secs(3),
        |samples, at| engine.transcribe(samples, at),
    )
}

fn microphone_capture(
    capture: Capture,
    offset: u64,
    tx: &SyncSender<Event>,
    cancel: &AtomicBool,
    stop: &AtomicBool,
    inference_interval: Duration,
    mut transcribe: impl FnMut(&[f32], u64) -> Result<Vec<Segment>>,
) -> Result<()> {
    let mut resampler = Resampler::new(capture.rate);
    let mut pending = Vec::new();
    let mut consumed = 0_usize;
    let mut last_inference = Instant::now();
    let mut captured_samples = 0_usize;
    let mut audio_flushed = false;
    let mut capture_done = false;
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
        match capture.receiver.recv_timeout(Duration::from_millis(80)) {
            Ok(samples) => {
                captured_samples += samples.len();
                pending.extend(resampler.push(&samples));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => capture_done = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // A temporary empty queue is NOT Stop acknowledgement. Wait for sender
        // disconnection so callbacks in flight cannot arrive after finalization.
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            match capture.receiver.try_recv() {
                Ok(samples) => {
                    captured_samples += samples.len();
                    pending.extend(resampler.push(&samples));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    capture_done = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
            if pending.len() > SAMPLE_RATE * 120 {
                bail!(
                    "Transcription fell more than two minutes behind; completed text has been kept"
                );
            }
        }
        if pending.len() > SAMPLE_RATE * 120 {
            bail!("Transcription fell more than two minutes behind; completed text has been kept");
        }
        if capture_done && !stop.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
            bail!("Microphone stopped unexpectedly; completed text has been kept");
        }
        let stopping = capture_done;
        if stopping && !audio_flushed {
            pending.extend(resampler.finish());
            audio_flushed = true;
        }
        send(
            tx,
            Event::Meter {
                seconds: captured_samples as f64 / capture.rate as f64,
                level: f32::from_bits(capture.level.load(Ordering::Relaxed)) as f64,
            },
        );
        if (last_inference.elapsed() >= inference_interval && pending.len() >= SAMPLE_RATE)
            || (stopping && !pending.is_empty())
        {
            let count = pending.len().min(SAMPLE_RATE * 28);
            let at = offset + consumed as u64 * 1000 / SAMPLE_RATE as u64;
            let words = transcribe(&pending[..count], at)?;
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

    fn hypothesis(samples: &[f32], at: u64) -> Vec<Segment> {
        let end = at + samples.len() as u64 * 1000 / SAMPLE_RATE as u64;
        (at.div_ceil(1000) * 1000..end)
            .step_by(1000)
            .map(|ms| Segment {
                start_ms: ms,
                end_ms: (ms + 400).min(end),
                text: format!("word{}", ms / 1000),
            })
            .collect()
    }

    fn final_text(events: Receiver<Event>) -> String {
        let mut transcript = Transcript::default();
        for event in events.try_iter() {
            if let Event::Final(segments) = event {
                transcript.append(segments);
            }
        }
        transcript.text()
    }

    #[test]
    fn stop_closes_capture_during_inference_then_drains_teardown_tail() {
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let (closed_tx, closed_rx) = mpsc::sync_channel(1);
        let capture = Capture::simulated(
            vec![vec![0.1; SAMPLE_RATE * 8]],
            vec![0.1; SAMPLE_RATE],
            stop.clone(),
            cancel.clone(),
            closed_tx,
        );
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = mpsc::sync_channel(1);
        let (tx, events) = mpsc::sync_channel(128);
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut first = true;
            microphone_capture(
                capture,
                0,
                &tx,
                &cancel,
                &worker_stop,
                Duration::ZERO,
                |samples, at| {
                    if first {
                        first = false;
                        entered_tx.send(()).unwrap();
                        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                    Ok(hypothesis(samples, at))
                },
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        stop.store(true, Ordering::Relaxed);
        // This must complete BEFORE inference is allowed to resume.
        closed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        resume_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert_eq!(
            final_text(events),
            (0..9)
                .map(|n| format!("word{n}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    #[test]
    fn cancel_during_inference_discards_partial_pass_but_keeps_finalized_text() {
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let (closed_tx, closed_rx) = mpsc::sync_channel(1);
        let capture = Capture::simulated(
            vec![vec![0.1; SAMPLE_RATE * 8]],
            vec![],
            stop.clone(),
            cancel.clone(),
            closed_tx,
        );
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (tx, events) = mpsc::sync_channel(128);
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            let mut passes = 0;
            microphone_capture(
                capture,
                0,
                &tx,
                &worker_cancel,
                &stop,
                Duration::ZERO,
                |samples, at| {
                    passes += 1;
                    if passes == 2 {
                        entered_tx.send(()).unwrap();
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !worker_cancel.load(Ordering::Relaxed) && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        assert!(worker_cancel.load(Ordering::Relaxed));
                    }
                    Ok(hypothesis(samples, at))
                },
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        closed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap().unwrap();
        assert_eq!(
            final_text(events),
            (0..6)
                .map(|n| format!("word{n}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    #[test]
    fn dropping_capture_stops_owner_even_without_stop_or_cancel() {
        let (closed_tx, closed_rx) = mpsc::sync_channel(1);
        let capture = Capture::simulated(
            vec![],
            vec![],
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            closed_tx,
        );
        drop(capture);
        closed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }

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
