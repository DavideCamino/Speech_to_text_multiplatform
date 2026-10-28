use crate::{settings::Settings, transcript::Segment};
use anyhow::{Context, Result};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

pub struct Engine {
    state: WhisperState,
    language: Option<String>,
}

impl Engine {
    pub fn new(settings: &Settings) -> Result<Self> {
        // Native debug logs can include recognized words. Do not send private
        // transcript fragments to stderr (no logging backend is enabled).
        whisper_rs::install_logging_hooks();
        let path = settings
            .model
            .as_ref()
            .context("Select or download a Whisper model first")?;
        let mut options = WhisperContextParameters::default();
        options.use_gpu(settings.use_gpu && super::gpu_available_in_build());
        let context = WhisperContext::new_with_params(path, options)
            .context("Cannot load model. Check its format, available memory, and GPU drivers; try CPU mode if necessary")?;
        Ok(Self {
            state: context.create_state()?,
            language: settings.language.clone(),
        })
    }

    pub fn transcribe(&mut self, samples: &[f32], offset_ms: u64) -> Result<Vec<Segment>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let duration_ms = samples.len() as u64 * 1000 / crate::audio::SAMPLE_RATE as u64;
        // Silence gating avoids Whisper's common hallucinations on empty input.
        let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
        if rms < 0.0001 {
            return Ok(Vec::new());
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(
            std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32,
        );
        params.set_language(self.language.as_deref());
        params.set_translate(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        // Pad short tails for Whisper. Clamp returned times to real audio duration.
        let mut padded;
        let input = if samples.len() < crate::audio::SAMPLE_RATE {
            padded = samples.to_vec();
            padded.resize(crate::audio::SAMPLE_RATE, 0.0);
            padded.as_slice()
        } else {
            samples
        };
        self.state
            .full(params, input)
            .context("Whisper inference failed")?;
        let mut result = Vec::new();
        for segment in self.state.as_iter() {
            let start = (segment.start_timestamp().max(0) as u64 * 10).min(duration_ms);
            let end = (segment.end_timestamp().max(0) as u64 * 10).min(duration_ms);
            let text = segment.to_str_lossy()?.trim().to_owned();
            if end > start && !text.is_empty() {
                result.push(Segment {
                    start_ms: offset_ms + start,
                    end_ms: offset_ms + end,
                    text,
                });
            }
        }
        Ok(result)
    }
}
