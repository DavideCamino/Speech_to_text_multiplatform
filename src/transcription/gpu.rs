//! Stable backend preferences and a small, isolated GGML FFI boundary.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{ffi::CStr, sync::OnceLock};
use whisper_rs::whisper_rs_sys as sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuBackend {
    Vulkan,
    Cuda,
    Metal,
    Rocm,
}

impl GpuBackend {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vulkan => "Vulkan",
            Self::Cuda => "CUDA",
            Self::Metal => "Metal",
            Self::Rocm => "AMD ROCm (HIP/hipBLAS)",
        }
    }

    fn from_registry_name(name: &str) -> Option<Self> {
        match name {
            "Vulkan" => Some(Self::Vulkan),
            "CUDA" => Some(Self::Cuda),
            "Metal" => Some(Self::Metal),
            "ROCm" => Some(Self::Rocm),
            _ => None,
        }
    }
}

pub fn compiled_gpu_backends() -> &'static [GpuBackend] {
    &[
        #[cfg(feature = "vulkan")]
        GpuBackend::Vulkan,
        #[cfg(feature = "cuda")]
        GpuBackend::Cuda,
        #[cfg(feature = "metal")]
        GpuBackend::Metal,
        #[cfg(feature = "rocm")]
        GpuBackend::Rocm,
    ]
}

// Match whisper.cpp's indexing exactly: gpu_device is an ordinal among GPU and
// IGPU devices, NOT an absolute GGML index or an index local to a backend.
fn select_device(backend: GpuBackend, devices: &[Option<GpuBackend>]) -> Result<i32> {
    let index = devices.iter().position(|candidate| *candidate == Some(backend))
        .ok_or_else(|| anyhow::anyhow!("{} has no compatible GPU available. Check its drivers or choose another backend/CPU mode.", backend.label()))?;
    Ok(i32::try_from(index)?)
}

/// Called on an inference worker, never on GTK's main thread. GGML registry
/// initialization can load drivers and is not safe to race on first access.
pub(super) fn gpu_device_index(backend: Option<GpuBackend>) -> Result<i32> {
    if let Some(backend) = backend {
        ensure!(
            compiled_gpu_backends().contains(&backend),
            "{} was not compiled into this build. Select an available backend in Settings.",
            backend.label()
        );
    }
    static DEVICES: OnceLock<Vec<Option<GpuBackend>>> = OnceLock::new();
    let devices = DEVICES.get_or_init(|| {
        let mut devices = Vec::new();
        // SAFETY: GGML owns the registry and its devices for process lifetime.
        // Enumerate in bounds, use only non-null handles, copy the registry name
        // immediately, and retain no native pointers. OnceLock serializes access.
        unsafe {
            for index in 0..sys::ggml_backend_dev_count() {
                let device = sys::ggml_backend_dev_get(index);
                if device.is_null() {
                    continue;
                }
                let kind = sys::ggml_backend_dev_type(device);
                if kind != sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU
                    && kind != sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU
                {
                    continue;
                }
                let registry = sys::ggml_backend_dev_backend_reg(device);
                let backend = if registry.is_null() {
                    None
                } else {
                    let name = sys::ggml_backend_reg_name(registry);
                    if name.is_null() {
                        None
                    } else {
                        GpuBackend::from_registry_name(&CStr::from_ptr(name).to_string_lossy())
                    }
                };
                // Unknown GPU backends must still occupy an ordinal.
                devices.push(backend);
            }
        }
        devices
    });
    match backend {
        Some(backend) => select_device(backend, devices),
        None => Ok(0), // Preserve Whisper's automatic selection/CPU fallback.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_global_gpu_ordinal_not_backend_local_index() {
        let devices = [
            Some(GpuBackend::Rocm),
            Some(GpuBackend::Rocm),
            None,
            Some(GpuBackend::Vulkan),
        ];
        assert_eq!(select_device(GpuBackend::Rocm, &devices).unwrap(), 0);
        assert_eq!(select_device(GpuBackend::Vulkan, &devices).unwrap(), 3);
        assert!(select_device(GpuBackend::Cuda, &devices).is_err());
    }

    #[test]
    fn registry_names_are_backend_specific() {
        assert_eq!(
            GpuBackend::from_registry_name("ROCm"),
            Some(GpuBackend::Rocm)
        );
        assert_eq!(
            GpuBackend::from_registry_name("CUDA"),
            Some(GpuBackend::Cuda)
        );
        assert_eq!(
            GpuBackend::from_registry_name("Vulkan"),
            Some(GpuBackend::Vulkan)
        );
        assert_eq!(
            GpuBackend::from_registry_name("Metal"),
            Some(GpuBackend::Metal)
        );
        assert_eq!(GpuBackend::from_registry_name("CPU"), None);
    }
}
