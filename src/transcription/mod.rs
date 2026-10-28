//! All decoding, model loading, and inference run off the GTK main thread.
mod engine;
mod worker;
pub use worker::{Event, Job, Source, start};

pub fn gpu_available_in_build() -> bool {
    cfg!(any(feature = "vulkan", feature = "cuda", feature = "metal"))
}
