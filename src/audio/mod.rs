//! Audio sources emit mono PCM. No GTK or speech-engine dependencies.
mod capture;
mod decode;
mod resample;
pub use capture::{Capture, input_devices};
pub use decode::AudioFile;
pub use resample::Resampler;
pub const SAMPLE_RATE: usize = 16_000;
