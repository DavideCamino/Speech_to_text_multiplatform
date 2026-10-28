# Speech2Text

A local-first Rust desktop demo using **GTK 4**, **libadwaita**, and **whisper.cpp** via `whisper-rs`.

## Current features

- Transcribe a selected **WAV, MP3, FLAC, OGG/Vorbis, or M4A (AAC/ALAC)** file.
- Microphone transcription with an input selector, level indicator, duration, Stop, and Cancel.
- Finalized text uses the theme's normal foreground; revisable live text is grey.
- Read-only transcript, clipboard copy, UTF-8 TXT, and segment-timed SRT export.
- Select a local whisper.cpp **ggml `.bin` model**, or explicitly download multilingual Tiny, Base, or Small models. Downloads show progress, support cancellation, and verify pinned SHA-256 checksums before selecting the model.
- Automatic language detection or a manual Whisper language code (`en`, `it`, `fr`, etc.). Transcription preserves the spoken language; translation is disabled.
- GPU-enabled builds for Vulkan, CUDA, Metal, or **AMD ROCm (HIP/hipBLAS)**, with a runtime switch to CPU mode and a saved backend selector.
- A three-dot header menu with **Settings**, **Keyboard Shortcuts**, and **About Speech2Text**. Settings has a **Speech to Text** page for model/audio options and an empty **LLM** page reserved for future integration.
- A second view with an editable example prompt, read-only original/result panes, and a **demo Rewrite action that copies the original unchanged**. Results can be copied or exported as TXT/Markdown. No LLM is contacted.
- Replace/append confirmation, cancellation that keeps completed segments, unexported-text warnings, and saved non-sensitive preferences.

The application ID is the requested placeholder: `io.github.YOUR_NAME.Speech2Text`.

## Build and run on Linux

Requirements:

- Rust **1.92+** (edition 2024; required by the current GTK Rust bindings).
- GTK **4.12+**, libadwaita **1.5+**, their development packages, and `pkg-config`.
- A C/C++ compiler, CMake, and libclang (for whisper.cpp and generated bindings).
- ALSA development headers for CPAL microphone support; an operational desktop audio stack.
- Internet access for Cargo's initial dependency fetch and any explicitly requested model download.

For example, on Ubuntu 24.04:

```sh
sudo apt install build-essential cmake clang libclang-dev pkg-config \
  libgtk-4-dev libadwaita-1-dev libasound2-dev
cargo run
```

On Arch Linux the corresponding packages include `base-devel`, `cmake`, `clang`, `pkgconf`, `gtk4`, `libadwaita`, and `alsa-lib`.

Open **⋮ → Settings → Speech to Text** and **select/download a model first**, then open an audio file or start recording. Settings are saved when its window closes. Base is a useful starting point; Tiny is faster for live transcription. The Hugging Face download confirmation states the approximate size and destination. No model is bundled with the source.

Use a release build for meaningful performance measurements:

```sh
cargo run --release
```

### Hardware acceleration

GPU support is implemented as explicit Cargo features, because each backend needs different native tooling. A default build is CPU-only and labels the acceleration switch accordingly. Compatible backends can be compiled together (all corresponding toolkits are required); for example:

```sh
cargo run --release --features rocm,vulkan
```

In **Settings → Speech to Text**, enable **Hardware acceleration**, then select **GPU backend**. The dropdown lists the backends compiled into that build, plus **Automatic (Whisper default)**. It is disabled when acceleration is off or work is running. The choice is saved independently of the on/off switch and applies to the next transcription job.

| Backend | Build/run command | Additional requirements |
| --- | --- | --- |
| Vulkan (Linux/Windows) | `cargo run --release --features vulkan` | Vulkan headers, loader, shader compiler (`glslc`), compatible GPU drivers; Windows generally needs the Vulkan SDK |
| CUDA (NVIDIA) | `cargo run --release --features cuda` | Compatible NVIDIA GPU/driver and CUDA toolkit including `nvcc` |
| Metal (macOS) | `cargo run --release --features metal` | Xcode command-line tools and supported Apple hardware |
| ROCm (AMD, Linux) | `cargo run --release --features rocm` | ROCm-compatible AMD GPU/driver, HIP SDK (`hipcc`), hipBLAS and rocBLAS development libraries |

For example, Ubuntu Vulkan development packages include `libvulkan-dev` and `glslc`; Arch uses `vulkan-headers`, `vulkan-icd-loader`, and `shaderc`, plus the appropriate vendor driver.

#### AMD ROCm setup

The `rocm` feature enables `whisper-rs/hipblas`, which compiles whisper.cpp's HIP backend. It is a separate build option from Vulkan, not an additional driver needed by Vulkan. Settings identifies it as **AMD ROCm (HIP/hipBLAS)**.

Install the ROCm HIP SDK, hipBLAS, and rocBLAS for your distribution and GPU. If installed in `/opt/rocm`, make the toolchain discoverable with:

```sh
export HIP_PATH=/opt/rocm
export PATH="$HIP_PATH/bin:$PATH"
export CMAKE_PREFIX_PATH="$HIP_PATH${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH}"
cargo run --release --features rocm
```

Use `rocminfo` to check whether the runtime can see your GPU and identify its `gfx...` architecture. If automatic architecture detection fails, set `AMDGPU_TARGETS` to your GPU's actual target. For example, **only for a `gfx1103` GPU**:

```sh
AMDGPU_TARGETS=gfx1103 cargo run --release --features rocm
```

The selected architecture must also be supported by the installed ROCm/rocBLAS libraries; merely being listed by `rocminfo` does not guarantee compatibility. Do not use an arbitrary architecture override to bypass that requirement. First builds compile native GPU kernels and can take several minutes. For a custom ROCm installation, the runtime loader may also need its `lib` directory in `LD_LIBRARY_PATH`.

**ROCm support is Linux-only with the current bindings.** The pinned `whisper-rs-sys` version explicitly rejects HIP builds on Windows; use Vulkan there. ROCm is not available on macOS.

In a GPU-enabled build the app requests GPU inference by default. Disable **Hardware acceleration** to force CPU mode. **Automatic** preserves Whisper's default device selection. An explicit backend selects its first detected GPU using Whisper's GPU-device ordinal; it does not silently select a different GPU backend. If the backend is not compiled in or no compatible device is detected, transcription reports an error. Driver discovery happens on the worker, not the GTK thread. The native engine can still fall back to CPU if GPU initialization fails internally; the app does not report the actual device used. There is no per-device picker within a backend yet.

Old preferences without a backend choice default to Automatic. If a saved backend is absent from a different build, Settings explains that Automatic has been selected instead.

## Flatpak: CPU + Vulkan

Packaging files live in `build-aux/flatpak/` and `data/`; the Rust source layout is unchanged. This package enables **CPU and Vulkan only** (not CUDA, ROCm, or Metal). The GitHub workflow produces an **x86_64** sideload bundle. Vulkan can serve compatible AMD, Intel, and NVIDIA GPUs through the host drivers exposed by Flatpak.

### 1. Install build tools and SDKs

On Ubuntu 24.04 or newer:

```sh
sudo apt install flatpak flatpak-builder python3
```

On Arch Linux:

```sh
sudo pacman -S flatpak flatpak-builder python
```

Python **3.11+** is needed for the packaging scripts. Add Flathub and install the matching SDK extensions (GNOME 50 is based on Freedesktop 25.08):

```sh
flatpak remote-add --user --if-not-exists flathub \
  https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub \
  org.gnome.Platform//50 org.gnome.Sdk//50 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08 \
  org.freedesktop.Sdk.Extension.llvm21//25.08
```

The SDK supplies GTK/libadwaita, CMake, Vulkan headers, and `glslc`; its extensions supply Rust and libclang. You do not need the host's Rust/LLVM/ROCm/CUDA installations for this build. Allow several GB for SDKs, downloaded sources, and build output; a first release build can take a while. The manifest sets Cargo/CMake concurrency to 16 jobs, limit the concurrency to reduce peak memory use.

### 2. Check or refresh the locked Cargo sources

From the repository root:

```sh
python3 build-aux/flatpak/generate-cargo-sources.py --check
python3 -m unittest discover -s build-aux/flatpak -p 'test_*.py'
python3 build-aux/flatpak/check-packaging.py \
  build-aux/flatpak/io.github.YOUR_NAME.Speech2Text.json
```

Whenever `Cargo.lock` changes, regenerate and **commit** the source list:

```sh
python3 build-aux/flatpak/generate-cargo-sources.py
```

The generator uses only Python's standard library, makes no network requests, and records every crates.io dependency's archive URL and SHA-256 from `Cargo.lock`. Flatpak downloads/verifies those archives **before** building. Cargo then uses the vendored directory with `--frozen --offline`; there is no network dependency resolution inside the build sandbox. Git/private-registry dependencies are deliberately rejected until explicit sources are added for them.

### 3. Compile and export

Compile with:
```sh
flatpak-builder --user --force-clean --disable-rofiles-fuse \
  --install-deps-from=flathub --repo=flatpak-repo \
  flatpak-build build-aux/flatpak/io.github.YOUR_NAME.Speech2Text.json
```
if it is needed a clean compilation:
```sh
flatpak-builder --user --force-clean --disable-cache \
  --disable-rofiles-fuse --install-deps-from=flathub \
  --repo=flatpak-repo flatpak-build \
  build-aux/flatpak/io.github.YOUR_NAME.Speech2Text.json
```

Then:
```sh
mkdir -p dist
flatpak build-bundle --arch=x86_64 \
  --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
  flatpak-repo dist/Speech2Text-x86_64.flatpak \
  io.github.YOUR_NAME.Speech2Text stable
(cd dist && sha256sum Speech2Text-x86_64.flatpak > SHA256SUMS)
```

Use an x86_64 host for the commands above. The manifest uses the local working tree, including uncommitted source changes; build from a clean checkout/tag when preparing a release. Build output, `.git`, `.private`, and local model directories are excluded from the source copy. Native code does not use `-march=native` or require AVX/AVX2, so the resulting CPU build is portable across baseline x86_64 machines; it can be slower than a locally optimized native build. Installed files include the executable, desktop launcher, icon, AppStream metadata, and project/dependency license notices.

### 4. Install and test the bundle

```sh
(cd dist && sha256sum --check SHA256SUMS)
flatpak install --user ./dist/Speech2Text-x86_64.flatpak
flatpak run io.github.YOUR_NAME.Speech2Text
```

Test file selection/export through the desktop portals, local model selection/download, microphone recording, CPU mode, and Vulkan mode on an actual GPU. A successful CI build does not prove all GPU/driver combinations work.

Sandbox permissions are deliberately limited:

- Wayland and fallback X11/IPC for the interface.
- `dri` for GPU access, and PulseAudio for microphone/audio access through the desktop audio server.
- Network access for explicitly requested model downloads.
- **No blanket home/host filesystem permission**: file dialogs use portals, and private data/preferences/models live under Flatpak's per-app XDG directories.

Models are not bundled. Compatible Vulkan drivers must be available through Flatpak's graphics-driver extensions; keep the host driver and Flatpak runtimes updated. NVIDIA uses Vulkan here, not CUDA.

### Publish on GitHub

`.github/workflows/flatpak.yml` builds on manual dispatch and `v*` tags:

- **Manual run:** Actions → Flatpak bundle → Run workflow. Download the `Speech2Text-flatpak-x86_64` artifact to test the bundle/checksum.
- **Tagged release:** after replacing the placeholders below, update `Cargo.toml`'s version and the first AppStream `<release>` version/date. The tag must exactly match `v<Cargo.toml version>`.

```sh
python3 build-aux/flatpak/check-packaging.py \
  build-aux/flatpak/io.github.YOUR_NAME.Speech2Text.json --release
git tag v0.1.0
git push origin v0.1.0
```

The build runs with read-only repository permissions. A separate release job attaches `Speech2Text-x86_64.flatpak` and `SHA256SUMS` to the GitHub Release. Tag builds **reject identity placeholders** and inconsistent versions instead of publishing a misleading package. Keep Actions enabled and allow the release job to use its declared `contents: write` permission.

These are **unsigned sideload bundles**. GitHub Releases do not provide automatic Flatpak app updates; users download/install each new bundle. The checksum detects download corruption, not publisher identity independently of GitHub. A signed Flatpak repository or Flathub publication is a separate next step; runtime updates still come from Flathub normally.

### Replace identity placeholders before the first public release

Use a real ID such as `io.github.YOUR_GITHUB_USERNAME.Speech2Text`. Update the following files **together**, including filenames where indicated:

| File | Required edits |
| --- | --- |
| `src/main.rs` | Replace the `.application_id(...)` value. |
| `src/settings.rs` | Replace the `YOUR_NAME` organization in `ProjectDirs::from(...)`. |
| `build-aux/flatpak/io.github.YOUR_NAME.Speech2Text.json` | Rename to `<YOUR_APP_ID>.json`; replace `app-id` and all identity-dependent install/validation paths. |
| `data/io.github.YOUR_NAME.Speech2Text.desktop` | Rename to `<YOUR_APP_ID>.desktop`; replace `Icon`. |
| `data/io.github.YOUR_NAME.Speech2Text.metainfo.xml` | Rename to `<YOUR_APP_ID>.metainfo.xml`; replace `<id>`, `<launchable>`, developer name/ID, and all `YOUR_NAME/YOUR_REPOSITORY` GitHub URLs. The developer ID placeholder is `io.github.yourname`: use a lowercase reverse-DNS developer identity. |
| `data/icons/io.github.YOUR_NAME.Speech2Text.svg` | Rename to `<YOUR_APP_ID>.svg`. The SVG content has no owner placeholder. |
| `.github/workflows/flatpak.yml` | Update `MANIFEST` to the renamed manifest's path. |
| `README.md` | Update identity-dependent example commands/paths and repository references for your project. |

`Cargo.toml` and the AppStream release entries also need version updates for each release, but contain no owner placeholder. `LICENSE` uses “Speech2Text contributors”; adjust the attribution if desired. The generated `cargo-sources.json`, generator/config/helper scripts, and Rust-check workflow contain no application-identity placeholder requiring replacement.

Changing the application ID after publication creates a different Flatpak identity/data directory. Choose the permanent ID before asking users to install a public release. Run the packaging check again after renaming; also validate the desktop file and AppStream metadata with `desktop-file-validate` and `appstreamcli validate --no-net` (the manifest runs these during the build).

## Windows and macOS

The backend has no Linux-only file-transcription code: audio decoding is in-process through Symphonia, inference uses whisper.cpp, preferences use platform-specific directories, and file dialogs/clipboard are GTK/GIO. FFmpeg is **not** required by the application.

However, **only Linux has been built and smoke-tested so far**. Windows is the next validation target, with selected-file transcription taking priority over live recording; macOS follows. GTK/libadwaita runtime bundling, Windows toolchain selection, microphone permissions (including a macOS app bundle's usage description), code signing, and Windows/macOS installers are not implemented yet. Linux Flatpak packaging is provided above. Cross-platform source is not a promise of tested installers.

## Data and privacy

- Audio is decoded/captured in memory, not saved by the app. Long files are processed incrementally.
- Transcript text and rewrite prompts/results are not persisted automatically.
- Model paths, language, microphone selection, and the acceleration preference are saved as JSON in the OS-specific application configuration directory.
- Downloaded models live in the OS-specific application data directory under `models/`.
- Only explicit model downloads make application network requests, to Hugging Face and its download/CDN redirects. No audio/text is uploaded. Native Whisper logging hooks are silenced to avoid logging recognized words.
- TXT/Markdown/SRT export writes only to the location selected in the file chooser. Markdown export preserves the result as-is; it does not invent Markdown formatting.

## Architecture

```text
src/
  transcript.rs          Timed segments, normalization, plain text and SRT
  settings.rs            Platform directories and JSON preferences
  model.rs               Model catalog, cancellable HTTPS download, SHA-256
  audio/
    decode.rs            Incremental Symphonia file decoding and mono mixing
    capture.rs           CPAL input stream, bounded callback queue, level meter
    resample.rs          Stateful windowed-sinc conversion to 16 kHz
  transcription/
    engine.rs            Whisper context/state and per-window recognition
    gpu.rs               Stable backend IDs, GGML registry/device-index mapping
    worker.rs            Job lifecycle, file chunks, live provisional/final events
  ui/
    views.rs             Transcription and rewrite view widgets
    preferences.rs       Speech-to-text and empty LLM settings pages
    menu.rs              Header menu, shared shortcuts, application information
    dialogs.rs           Confirmation, file selection, asynchronous export
    mod.rs               Controller: state, event polling, navigation, actions
```

Workers receive settings snapshots and emit typed events. GTK objects never cross thread boundaries. The UI polls bounded event queues through GLib; file decoding, inference, audio resampling, and downloads do not run on the UI thread. Audio queue overflow or device errors stop the job with an explicit error instead of silently dropping speech. Original timed segments stay independent of the rewrite result, which is why rewritten subtitles are not offered.

## Tests and backend tools

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings

# Explicit download to a directory you choose:
cargo run --example download_model -- ./models tiny

# Exercise the same file-transcription worker without creating a GTK window:
cargo run --release --example transcribe -- ./models/ggml-tiny.bin ./audio.wav

# GPU-enabled equivalent:
cargo run --release --features vulkan --example transcribe -- \
  ./models/ggml-tiny.bin ./audio.m4a --gpu

# Test explicit backend selection in a combined build:
cargo run --release --features rocm,vulkan --example transcribe -- \
  ./models/ggml-tiny.bin ./audio.wav --backend rocm
# Replace --backend rocm with --backend vulkan to exercise the other backend.
```

The transcription example prints plain text to stdout and SRT to stderr. The normal app does not print transcripts.

Automated tests cover SRT formatting/Unicode, segment normalization, resampling across packet boundaries, incremental stereo WAV decoding, invalid input, worker model errors, pre-cancelled downloads, compiled GPU feature detection, backend device-index mapping, preference migration/round-tripping, and GTK settings/menu/selector behavior. They do not require a model, network access, display, or microphone, but native build dependencies are still required.

### Validation performed in this environment

- `cargo test --locked`: **13 tests passed**, including compiled GPU feature detection and a GTK settings/menu surface test (the GTK test is skipped when no display is available).
- `AMDGPU_TARGETS=gfx1103 cargo check --locked --features rocm`: passed with the installed ROCm toolkit.
- `AMDGPU_TARGETS=gfx1103 cargo test --locked --features rocm,vulkan`: **13 tests passed** in a combined-backend build.
- Transcription smoke tests with Tiny, generated speech, and explicit `--backend rocm` / `--backend vulkan` both succeeded in that same combined build and produced text/SRT. The app does not report the actual device used internally.
- `cargo clippy --all-targets -- -D warnings`: passed.
- GTK desktop launch smoke test: no GTK errors; ALSA device enumeration emitted host-specific warnings.
- Tiny model downloaded and verified through the actual application downloader.
- Actual CPU transcription and SRT generation succeeded on generated speech in **all five requested audio formats**. FFmpeg/espeak were used only to generate test inputs, not by the application.
- Vulkan and ROCm selection were exercised on this Linux host; CUDA and Metal runtime selection have not been tested here.
- Flatpak packaging: 6 Python tests, desktop/AppStream validation, and Flatpak Builder manifest expansion passed. All 284 generated crate archives were verified from the local Cargo cache and successfully resolved using the offline vendor configuration; dependency notice installation was also exercised.
- The complete sandboxed Flatpak build/install and the GitHub release workflow have **not** been executed here. Run the documented local build or manual Actions workflow before publishing.
- Live microphone correctness/latency, exhaustive UI interaction, and Windows/macOS builds are not yet verified.

## Demo limitations / follow-up work

- Whisper is chunk-based, not a native streaming recognizer. Live inference is scheduled roughly every three seconds after the previous pass; actual latency depends on hardware/model size. It retains a revisable tail and commits sufficiently old segment boundaries, with a 28-second forced window limit. Provisional words can change.
- Files are processed in bounded 28-second windows. Hard boundaries can split words or lose context; timestamps are approximate. Better VAD/overlap handling and alignment are follow-up work, not professional subtitle guarantees.
- Silence gating is deliberately conservative; noisy silence can still cause Whisper hallucinations.
- Cancel takes effect between decoding/inference passes; it does **not** interrupt an in-flight native inference call or model load. Stop finishes buffered audio. Downloads are cancellation-polled even while waiting for network data.
- Real-time capture requires inference to keep up. Its bounded callback queue may overflow on slow hardware; a clear error preserves completed text. Select Tiny or use a supported GPU build.
- Device names are enumerated at startup; there is no hot-plug refresh UI yet. Some unusual input sample formats are not supported.
- File progress is updated at inference-window boundaries; it can remain unchanged while a model loads or the first window runs.
- No model download resume, transcript history, automatic audio saving, translation, speaker diarization, LLM, Flathub publication/automatic app updates, or Windows/macOS installer integration yet.

The agreed requirements remain in [`question.md`](question.md).
