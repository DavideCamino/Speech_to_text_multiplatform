use super::Resampler;
use anyhow::{Context, Result, bail};
use std::{fs::File, io::ErrorKind, path::Path};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions},
    errors::Error,
    formats::{FormatOptions, FormatReader},
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};

/// Decodes one packet at a time, keeping memory independent of file duration.
pub struct AudioFile {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    resampler: Option<Resampler>,
    rate: Option<u32>,
    finished: bool,
    pub duration_seconds: Option<f64>,
}

impl AudioFile {
    pub fn open(path: &Path) -> Result<Self> {
        let stream = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            hint.with_extension(extension);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .context("Unsupported audio file")?;
        let format = probed.format;
        let track = format.default_track().context("No audio track found")?;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())?;
        let duration_seconds = track
            .codec_params
            .time_base
            .zip(track.codec_params.n_frames)
            .map(|(base, frames)| {
                let t = base.calc_time(frames);
                t.seconds as f64 + t.frac
            });
        let track = track.id;
        Ok(Self {
            format,
            decoder,
            track,
            resampler: None,
            rate: None,
            finished: false,
            duration_seconds,
        })
    }

    pub fn next_samples(&mut self) -> Result<Option<Vec<f32>>> {
        if self.finished {
            return Ok(None);
        }
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => {
                    self.finished = true;
                    return Ok(self.resampler.as_mut().map(Resampler::finish));
                }
                Err(error) => return Err(error.into()),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let decoded = self
                .decoder
                .decode(&packet)
                .context("Cannot decode audio packet")?;
            let spec = *decoded.spec();
            if let Some(rate) = self.rate {
                if rate != spec.rate {
                    bail!("Sample-rate changes within a file are not supported");
                }
            } else {
                self.rate = Some(spec.rate);
                self.resampler = Some(Resampler::new(spec.rate));
            }
            let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buffer.copy_interleaved_ref(decoded);
            let channels = spec.channels.count();
            let mono: Vec<f32> = buffer
                .samples()
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect();
            return Ok(Some(self.resampler.as_mut().unwrap().push(&mono)));
        }
    }
}
