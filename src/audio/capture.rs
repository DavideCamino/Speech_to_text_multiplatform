use anyhow::{Context, Result, bail};
use cpal::{
    Sample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub fn input_devices() -> Result<Vec<String>> {
    Ok(cpal::default_host()
        .input_devices()?
        .filter_map(|d| d.name().ok())
        .collect())
}

#[derive(Clone)]
struct Signals {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
}

impl Signals {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.cancel.load(Ordering::Relaxed)
    }
}

/// The bounded callback queue never waits for inference. A separate owner drops
/// the stream on Stop/Cancel. Receiver disconnection acknowledges teardown; the
/// inference worker must drain it before flushing the resampler on Stop.
pub struct Capture {
    shutdown: Arc<AtomicBool>,
    owner: Option<JoinHandle<()>>,
    pub receiver: Receiver<Vec<f32>>,
    pub rate: u32,
    pub level: Arc<AtomicU32>,
    pub failed: Arc<AtomicBool>,
}

impl Capture {
    pub fn start(
        name: Option<&str>,
        stop: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        let name = name.map(str::to_owned);
        let (tx, receiver) = mpsc::sync_channel(512);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let level = Arc::new(AtomicU32::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let signals = Signals {
            stop,
            cancel,
            failed: failed.clone(),
            level: level.clone(),
        };
        let owner_shutdown = shutdown.clone();
        let owner = thread::spawn(move || match open_stream(name.as_deref(), tx, &signals) {
            Ok((stream, rate)) => {
                if ready_tx.send(Ok(rate)).is_ok() {
                    own_stream(stream, &signals, &owner_shutdown);
                }
            }
            Err(error) => {
                let _ = ready_tx.send(Err(format!("{error:#}")));
            }
        });
        let mut capture = Self {
            shutdown,
            owner: Some(owner),
            receiver,
            rate: 0,
            level,
            failed,
        };
        capture.rate = ready_rx
            .recv()
            .context("Microphone owner stopped during startup")?
            .map_err(anyhow::Error::msg)?;
        Ok(capture)
    }
}

#[cfg(test)]
impl Capture {
    /// Simulate a backend whose final in-flight callback completes at teardown.
    pub(crate) fn simulated(
        initial: Vec<Vec<f32>>,
        tail: Vec<f32>,
        stop: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
        closed: SyncSender<()>,
    ) -> Self {
        struct Stream {
            tx: SyncSender<Vec<f32>>,
            tail: Vec<f32>,
            closed: SyncSender<()>,
        }
        impl Drop for Stream {
            fn drop(&mut self) {
                let _ = self.tx.send(std::mem::take(&mut self.tail));
                let _ = self.closed.send(());
            }
        }
        let (tx, receiver) = mpsc::sync_channel(512);
        let shutdown = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let signals = Signals {
            stop,
            cancel,
            failed: failed.clone(),
            level: level.clone(),
        };
        let owner_shutdown = shutdown.clone();
        let owner = thread::spawn(move || {
            for packet in initial {
                tx.send(packet).unwrap();
            }
            own_stream(Stream { tx, tail, closed }, &signals, &owner_shutdown);
        });
        Self {
            shutdown,
            owner: Some(owner),
            receiver,
            rate: super::SAMPLE_RATE as u32,
            level,
            failed,
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
    }
}

fn own_stream<S>(stream: S, signals: &Signals, shutdown: &AtomicBool) {
    while !signals.stopping()
        && !shutdown.load(Ordering::Relaxed)
        && !signals.failed.load(Ordering::Relaxed)
    {
        thread::sleep(Duration::from_millis(5));
    }
    // Drop, rather than pause: some backends process pause asynchronously.
    // Callback senders disappear only after stream teardown completes.
    drop(stream);
}

fn open_stream(
    name: Option<&str>,
    tx: SyncSender<Vec<f32>>,
    signals: &Signals,
) -> Result<(cpal::Stream, u32)> {
    if signals.stopping() {
        bail!("Recording stopped before microphone startup");
    }
    let host = cpal::default_host();
    let device = if let Some(name) = name {
        host.input_devices()?
            .find(|d| d.name().ok().as_deref() == Some(name))
    } else {
        host.default_input_device()
    }
    .context("Microphone is unavailable")?;
    let config = device.default_input_config()?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            build::<f32>(&device, &config.into(), channels, tx, signals.clone())?
        }
        cpal::SampleFormat::I16 => {
            build::<i16>(&device, &config.into(), channels, tx, signals.clone())?
        }
        cpal::SampleFormat::U16 => {
            build::<u16>(&device, &config.into(), channels, tx, signals.clone())?
        }
        other => bail!("Unsupported microphone sample format: {other:?}"),
    };
    if !signals.stopping() {
        stream.play()?;
    }
    Ok((stream, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: SyncSender<Vec<f32>>,
    signals: Signals,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let error_flag = signals.failed.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            if signals.stopping() {
                return;
            }
            let mono: Vec<f32> = data
                .chunks_exact(channels)
                .map(|frame| {
                    frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32
                })
                .collect();
            let peak = mono.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
            signals.level.store(peak.to_bits(), Ordering::Relaxed);
            // Do not enqueue another packet if Stop arrived during conversion.
            if !signals.stopping()
                && let Err(TrySendError::Full(_)) = tx.try_send(mono)
            {
                signals.failed.store(true, Ordering::Relaxed);
            }
        },
        move |_| {
            error_flag.store(true, Ordering::Relaxed);
        },
        None,
    )?)
}
