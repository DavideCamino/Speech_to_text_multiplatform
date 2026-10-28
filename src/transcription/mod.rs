//! All decoding, model loading, and inference run off the GTK main thread.
mod engine;
mod gpu;
mod worker;
pub use gpu::{GpuBackend, compiled_gpu_backends};
pub use worker::{Event, Job, Source, start};

/// Compiled backends, not a runtime probe of installed GPUs or drivers.
pub fn gpu_backends_in_build() -> Vec<&'static str> {
    compiled_gpu_backends()
        .iter()
        .map(|backend| backend.label())
        .collect()
}

pub fn gpu_available_in_build() -> bool {
    !compiled_gpu_backends().is_empty()
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
