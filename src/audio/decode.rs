use super::Resampler;
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io::{ErrorKind, Read, Seek},
    path::Path,
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{
        CODEC_TYPE_PCM_F32LE, CODEC_TYPE_PCM_F64LE, CODEC_TYPE_PCM_S16LE, CODEC_TYPE_PCM_S24LE,
        CODEC_TYPE_PCM_S32LE, CODEC_TYPE_PCM_U8, Decoder, DecoderOptions,
    },
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
    expected_pcm_frames: Option<u64>,
    decoded_pcm_frames: u64,
    pub duration_seconds: Option<f64>,
}

impl AudioFile {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut header = [0; 12];
        let pcm_wav_candidate = match file.read_exact(&mut header) {
            Ok(()) => &header[..4] == b"RIFF" && &header[8..] == b"WAVE",
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => false,
            Err(error) => return Err(error.into()),
        };
        file.rewind()?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
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
        // For PCM WAV, Symphonia derives n_frames from the data chunk size,
        // not from a duration estimate. Other formats may report approximate lengths.
        let expected_pcm_frames = if pcm_wav_candidate
            && matches!(
                track.codec_params.codec,
                CODEC_TYPE_PCM_U8
                    | CODEC_TYPE_PCM_S16LE
                    | CODEC_TYPE_PCM_S24LE
                    | CODEC_TYPE_PCM_S32LE
                    | CODEC_TYPE_PCM_F32LE
                    | CODEC_TYPE_PCM_F64LE
            ) {
            track.codec_params.n_frames
        } else {
            None
        };
        let track = track.id;
        Ok(Self {
            format,
            decoder,
            track,
            resampler: None,
            rate: None,
            finished: false,
            expected_pcm_frames,
            decoded_pcm_frames: 0,
            duration_seconds,
        })
    }

    /// True only after EOF if a PCM WAV delivered fewer frames than declared.
    pub fn ended_early(&self) -> bool {
        self.finished
            && self
                .expected_pcm_frames
                .is_some_and(|expected| self.decoded_pcm_frames < expected)
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
            if self.expected_pcm_frames.is_some() {
                self.decoded_pcm_frames += decoded.frames() as u64;
            }
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
