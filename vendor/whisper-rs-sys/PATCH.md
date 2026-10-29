# Local whisper-rs-sys patch

Source: crates.io `whisper-rs-sys` 0.15.0 (copied from Cargo's registry).
Upstream: https://codeberg.org/tazz4843/whisper-rs

The root Cargo.toml uses this copy through `[patch.crates-io]` on all platforms.

## Change

In `build.rs`, apply `/utf-8` only when `CARGO_CFG_TARGET_ENV` is `msvc`.
Previously it was applied on all Windows hosts, causing MinGW/GCC builds to
fail because GCC treats `/utf-8` as a filename. The Windows `advapi32` link
instruction remains unchanged. Linux compilation behavior is unchanged.

After CMake installation, Windows GNU targets also copy unprefixed static
archives in the install `lib/` directory (for example, `ggml.a`) to their GNU
linker names (`libggml.a`) and add that directory to Rust's link search path.
Existing prefixed archives and DLL import libraries are left alone. Originals
are retained for CMake. This step does not run on Linux or MSVC targets.

For Windows GNU Vulkan builds, explicitly set CMake's
`Vulkan_GLSLC_EXECUTABLE` to the `glslc.exe` found beside `gcc.exe` on PATH.
This uses MSYS2's shader compiler rather than the LunarG SDK compiler, which
crashed on this machine and silently produced empty shader embedding files.
The SDK still supplies Vulkan headers and libraries. Linux and MSVC compiler
selection is unchanged.

Keep this directory in version control. When upgrading whisper-rs-sys, check
whether upstream has fixed the issue before replacing or removing this patch.

## Windows build (MSYS2 UCRT64)

```sh
export LIBCLANG_PATH='C:/msys64/ucrt64/bin'
export CMAKE_GENERATOR=Ninja
cargo build --release
```

For Vulkan support, also install the shader compiler and set the SDK location
(adjust the version to match the installed SDK):

```sh
pacman -S --needed mingw-w64-ucrt-x86_64-shaderc
export VULKAN_SDK='C:/VulkanSDK/1.4.363.0'
# Keep nested shader build paths short on Windows.
export CARGO_TARGET_DIR='C:/s2t-target'
cargo build --release --features vulkan
```

If the SDK shader compiler was previously used, clean the affected native
outputs first so empty generated shader files are not reused:

```sh
cargo clean --release -p whisper-rs-sys
```

