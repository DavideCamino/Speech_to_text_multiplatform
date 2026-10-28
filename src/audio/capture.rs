use anyhow::{Context, Result, bail};
use cpal::{
    Sample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
};

pub fn input_devices() -> Result<Vec<String>> {
    Ok(cpal::default_host()
        .input_devices()?
        .filter_map(|d| d.name().ok())
        .collect())
}

/// The bounded callback queue never waits for inference. Overflow is an explicit
/// failure, not silent audio loss. The stream lives and drops on its worker thread.
pub struct Capture {
    _stream: cpal::Stream,
    pub receiver: Receiver<Vec<f32>>,
    pub rate: u32,
    pub level: Arc<AtomicU32>,
    pub failed: Arc<AtomicBool>,
}

impl Capture {
    pub fn start(name: Option<&str>) -> Result<Self> {
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
        let (tx, receiver) = mpsc::sync_channel(512);
        let level = Arc::new(AtomicU32::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(
                &device,
                &config.into(),
                channels,
                tx,
                level.clone(),
                failed.clone(),
            )?,
            cpal::SampleFormat::I16 => build::<i16>(
                &device,
                &config.into(),
                channels,
                tx,
                level.clone(),
                failed.clone(),
            )?,
            cpal::SampleFormat::U16 => build::<u16>(
                &device,
                &config.into(),
                channels,
                tx,
                level.clone(),
                failed.clone(),
            )?,
            other => bail!("Unsupported microphone sample format: {other:?}"),
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            receiver,
            rate,
            level,
            failed,
        })
    }

    pub fn pause(&self) -> Result<()> {
        self._stream.pause()?;
        Ok(())
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: SyncSender<Vec<f32>>,
    level: Arc<AtomicU32>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let error_flag = failed.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mono: Vec<f32> = data
                .chunks_exact(channels)
                .map(|frame| {
                    frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32
                })
                .collect();
            let peak = mono.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
            level.store(peak.to_bits(), Ordering::Relaxed);
            if let Err(TrySendError::Full(_)) = tx.try_send(mono) {
                failed.store(true, Ordering::Relaxed);
            }
        },
        move |_| {
            error_flag.store(true, Ordering::Relaxed);
        },
        None,
    )?)
}
