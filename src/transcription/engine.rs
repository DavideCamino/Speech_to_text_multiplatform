use crate::{settings::Settings, transcript::Segment};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

pub struct Engine {
    state: WhisperState,
    language: Option<String>,
    prompt_tokens: Vec<whisper_rs::WhisperTokenId>,
    // Outlives state/backend teardown, including any retained native callback.
    cancel: Arc<AtomicBool>,
}

unsafe extern "C" fn abort_inference(data: *mut c_void) -> bool {
    // SAFETY: Engine owns the Arc until after WhisperState is dropped. Native
    // inference only reads this atomic; no Rust references or state are mutated.
    unsafe { &*data.cast::<AtomicBool>() }.load(Ordering::Relaxed)
}

fn context_prompt(context: &str) -> Result<&str> {
    let prompt = context.trim();
    ensure!(
        !prompt.contains('\0'),
        "Recognition context cannot contain NUL characters"
    );
    ensure!(
        prompt.len() <= i32::MAX as usize,
        "Recognition context is too long"
    );
    Ok(prompt)
}

impl Engine {
    pub fn new(
        settings: &Settings,
        recognition_context: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        let prompt = context_prompt(recognition_context)?;
        // Native debug logs can include recognized words. Do not send private
        // transcript fragments to stderr (no logging backend is enabled).
        whisper_rs::install_logging_hooks();
        let path = settings
            .model
            .as_ref()
            .context("Select or download a Whisper model first")?;
        let mut options = WhisperContextParameters::default();
        options.use_gpu(settings.use_gpu && super::gpu_available_in_build());
        let backend = if settings.use_gpu {
            settings.gpu_backend
        } else {
            None
        };
        options.gpu_device(super::gpu::gpu_device_index(backend)?);
        let context = WhisperContext::new_with_params(path, options)
            .context("Cannot load model. Check its format, available memory, and GPU drivers; try CPU mode if necessary")?;
        // This is the same initial-prompt tokenization performed by whisper.cpp.
        // Tokenize once and borrow the owned tokens on every pass. In whisper-rs
        // 0.16 set_initial_prompt allocates an unfreed CString per call; using
        // set_tokens avoids leaking that allocation for each streaming window.
        // BPE emits at most one token per input byte, so this capacity also
        // avoids the wrapper's insufficient-token-buffer error path.
        let prompt_tokens = if prompt.is_empty() {
            Vec::new()
        } else {
            context
                .tokenize(prompt, prompt.len())
                .context("Cannot tokenize recognition context")?
        };
        Ok(Self {
            state: context.create_state()?,
            language: settings.language.clone(),
            prompt_tokens,
            cancel,
        })
    }

    pub fn transcribe(&mut self, samples: &[f32], offset_ms: u64) -> Result<Vec<Segment>> {
        if self.cancel.load(Ordering::Relaxed) || samples.is_empty() {
            return Ok(Vec::new());
        }
        let duration_ms = samples.len() as u64 * 1000 / crate::audio::SAMPLE_RATE as u64;
        // Silence gating avoids Whisper's common hallucinations on empty input.
        let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
        if rms < 0.0001 {
            return Ok(Vec::new());
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        // Use a borrowed atomic, not a heap-allocated closure per pass. The
        // native call is synchronous and Engine keeps the allocation alive.
        unsafe {
            params.set_abort_callback(Some(abort_inference));
            params.set_abort_callback_user_data(Arc::as_ptr(&self.cancel).cast_mut().cast());
        }
        params.set_n_threads(
            std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32,
        );
        params.set_language(self.language.as_deref());
        params.set_translate(false);
        // Each pass re-decodes overlapping audio. Do not feed the previous
        // pass's hypothesis back as a prompt (especially provisional speech).
        params.set_no_context(true);
        if !self.prompt_tokens.is_empty() {
            params.set_tokens(&self.prompt_tokens);
        }
        // Whisper's word-aware wrapping uses token timestamps, so the worker
        // can reconcile words instead of accepting/rejecting entire sentences.
        params.set_token_timestamps(true);
        params.set_split_on_word(true);
        params.set_max_len(1);
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
        let inference = self.state.full(params, input);
        if self.cancel.load(Ordering::Relaxed) {
            // Aborted native calls may return an error or partial segments.
            // Neither is publishable; cancellation is not an inference failure.
            return Ok(Vec::new());
        }
        inference.context("Whisper inference failed")?;
        let mut result: Vec<Segment> = Vec::new();
        let mut leading_whitespace = String::new();
        for segment in self.state.as_iter() {
            let start = (segment.start_timestamp().max(0) as u64 * 10).min(duration_ms);
            let end = (segment.end_timestamp().max(0) as u64 * 10).min(duration_ms);
            // Leading spaces distinguish words from punctuation/subword
            // continuations. Preserve them until final display/serialization.
            let text = segment.to_str_lossy()?.into_owned();
            if text.trim().is_empty() {
                // A whitespace-only fragment still carries a boundary, but
                // must not become an empty subtitle cue of its own.
                if let Some(previous) = result.last_mut() {
                    previous.text.push_str(&text);
                } else {
                    leading_whitespace.push_str(&text);
                }
                continue;
            }
            let mut text_with_boundary = std::mem::take(&mut leading_whitespace);
            text_with_boundary.push_str(&text);
            if duration_ms > 0 {
                // Several short words can share a token timestamp. Preserve
                // their text instead of dropping zero-duration word cues.
                let start = start.min(duration_ms - 1);
                result.push(Segment {
                    start_ms: offset_ms + start,
                    end_ms: offset_ms + end.max(start + 1),
                    text: text_with_boundary,
                });
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognition_context_is_optional_trimmed_unicode_and_validated() {
        assert_eq!(context_prompt("").unwrap(), "");
        assert_eq!(context_prompt(" \n\t ").unwrap(), "");
        assert_eq!(
            context_prompt("  Giulia, GTK4\n日本語  ").unwrap(),
            "Giulia, GTK4\n日本語"
        );
        assert!(context_prompt("invalid\0context").is_err());
    }

    #[test]
    fn abort_callback_reads_shared_cancel_flag() {
        let cancel = Arc::new(AtomicBool::new(false));
        let data = Arc::as_ptr(&cancel).cast_mut().cast();
        // SAFETY: the Arc remains alive for both synchronous callback calls.
        assert!(!unsafe { abort_inference(data) });
        cancel.store(true, Ordering::Relaxed);
        assert!(unsafe { abort_inference(data) });
    }

    #[test]
    #[ignore = "requires SPEECH2TEXT_TEST_MODEL pointing to a local Whisper model"]
    fn native_inference_can_be_cancelled() {
        let model = std::env::var_os("SPEECH2TEXT_TEST_MODEL")
            .expect("set SPEECH2TEXT_TEST_MODEL to a local model");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut engine = Engine::new(
            &Settings {
                model: Some(model.into()),
                use_gpu: false,
                language: Some("en".into()),
                ..Settings::default()
            },
            "Giulia, GTK4, Rust",
            cancel.clone(),
        )
        .unwrap();
        assert!(!engine.prompt_tokens.is_empty());
        let prompt = engine.prompt_tokens.clone();
        let samples: Vec<_> = (0..crate::audio::SAMPLE_RATE * 28)
            .map(|i| (i as f32 * 0.1).sin() * 0.2)
            .collect();
        let signal = cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            signal.store(true, Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let result = engine.transcribe(&samples, 0).unwrap();
        canceller.join().unwrap();
        assert_eq!(
            engine.prompt_tokens, prompt,
            "recognition must not mutate user context"
        );
        assert!(cancel.load(Ordering::Relaxed));
        assert!(
            result.is_empty(),
            "an aborted pass must not publish partial words"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }
}
