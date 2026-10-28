//! All decoding, model loading, and inference run off the GTK main thread.
mod engine;
mod worker;
pub use worker::{Event, Job, Source, start};

/// Compiled backends, not a runtime probe of installed GPUs or drivers.
pub fn gpu_backends_in_build() -> &'static [&'static str] {
    &[
        #[cfg(feature = "vulkan")]
        "Vulkan",
        #[cfg(feature = "cuda")]
        "CUDA",
        #[cfg(feature = "metal")]
        "Metal",
        #[cfg(feature = "rocm")]
        "AMD ROCm (HIP/hipBLAS)",
    ]
}

pub fn gpu_available_in_build() -> bool {
    !gpu_backends_in_build().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_backends_match_feature_flags() {
        let backends = gpu_backends_in_build();
        assert_eq!(backends.contains(&"Vulkan"), cfg!(feature = "vulkan"));
        assert_eq!(backends.contains(&"CUDA"), cfg!(feature = "cuda"));
        assert_eq!(backends.contains(&"Metal"), cfg!(feature = "metal"));
        assert_eq!(
            backends.contains(&"AMD ROCm (HIP/hipBLAS)"),
            cfg!(feature = "rocm")
        );
        assert_eq!(gpu_available_in_build(), !backends.is_empty());
    }
}
