# Build and package Speech2Text from source

[Application overview](README.md) · [Install an existing package](INSTALL.md)

This guide covers native compilation and **local distributable packaging**: Flatpak on Linux and a portable runtime plus Inno Setup installer on Windows. Run commands from the repository root unless stated otherwise. Windows packaging must run on Windows; Linux packaging fixtures do not create or validate a real Windows runtime.

## 1. Obtain the source

Install Git, then clone the repository:

```sh
git clone https://github.com/DavideCamino/Speech_to_text_multiplatform.git
cd Speech_to_text_multiplatform
```

For a release build, check out the intended release tag or commit. Keep `Cargo.lock` and the committed `vendor/whisper-rs-sys` tree: Cargo patches that dependency with local native-build fixes. Do not replace it with the unpatched registry copy.

Requirements for native builds:

- Rust **1.92 or newer**, edition 2024; use a current stable toolchain compatible with the lockfile.
- GTK **4.12+**, libadwaita **1.5+**, and their development files.
- A C/C++ compiler, CMake, `pkg-config`, and libclang for native code and bindings.
- Python **3.11+** for packaging scripts.
- Network access for initial tools/dependencies and explicitly requested models; adequate disk space for native build output and SDKs.

Models are separate from compilation and packaging. A normal build does not download one.

## 2. Linux: native executable

### Dependencies

On Ubuntu 24.04 or newer:

```sh
sudo apt update
sudo apt install build-essential git cmake clang libclang-dev pkg-config \
  libgtk-4-dev libadwaita-1-dev libasound2-dev python3
```

On Arch Linux:

```sh
sudo pacman -S --needed base-devel git cmake clang pkgconf gtk4 libadwaita alsa-lib python
```

Install a current stable Rust toolchain using [rustup](https://rustup.rs/) if the distribution's Rust is too old. For an existing rustup installation:

```sh
rustup update stable
rustup default stable
rustup component add rustfmt clippy
rustc --version
```

### Build and run

```sh
# CPU-only release executable
cargo build --release --locked --bin speech2text
./target/release/speech2text

# Alternatively build and run together
cargo run --release --locked --bin speech2text
```

The executable is dynamically dependent on the host's native libraries; copying it alone is **not** a self-contained Linux package. Use the Flatpak instructions below for distribution. Build outputs move if you set `CARGO_TARGET_DIR`.

### Optional GPU backends

| Backend | Build command | Additional requirements |
| --- | --- | --- |
| Vulkan | `cargo build --release --locked --features vulkan --bin speech2text` | Vulkan headers/loader, `glslc`, compatible driver |
| CUDA | `cargo build --release --locked --features cuda --bin speech2text` | NVIDIA CUDA toolkit including `nvcc`, compatible GPU/driver |
| ROCm | `cargo build --release --locked --features rocm --bin speech2text` | Linux HIP SDK, hipBLAS, rocBLAS, supported AMD GPU/driver |
| Metal | `cargo build --release --locked --features metal --bin speech2text` | macOS and Xcode command-line tools; see platform caveat below |

For Linux Vulkan development:

```sh
# Ubuntu
sudo apt install libvulkan-dev glslc

# Arch Linux
sudo pacman -S --needed vulkan-headers vulkan-icd-loader shaderc
```

Install the appropriate runtime GPU driver separately. Compatible backends can be combined, for example `--features rocm,vulkan`, provided all their native toolkits are installed.

For ROCm installed under `/opt/rocm`:

```sh
export HIP_PATH=/opt/rocm
export PATH="$HIP_PATH/bin:$PATH"
export CMAKE_PREFIX_PATH="$HIP_PATH${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH}"
cargo build --release --locked --features rocm --bin speech2text
```

Use `rocminfo` to identify the GPU. If native build architecture detection fails, set `AMDGPU_TARGETS` to your actual supported target, for example `AMDGPU_TARGETS=gfx1103 cargo build --release --locked --features rocm --bin speech2text` **only for that GPU target**. A custom installation may also need its library directory in `LD_LIBRARY_PATH`. Do not use architecture overrides to disguise unsupported hardware. The pinned bindings reject ROCm/HIP on Windows.

Enable **Hardware acceleration** in Settings and choose a compiled backend. Explicit selection uses that backend's first detected GPU; there is no per-device picker. Automatic preserves Whisper's default selection, and the native engine may fall back to CPU internally. Compiling a backend does not establish runtime compatibility.

## 3. Linux: build a Flatpak locally

The committed manifest packages **CPU + Vulkan, x86_64**. It does not package CUDA, ROCm, or Metal. Use an x86_64 Linux host for these commands.

### Install Flatpak tools and SDKs

```sh
# Ubuntu
sudo apt install flatpak flatpak-builder python3

# Arch Linux
sudo pacman -S --needed flatpak flatpak-builder python
```

```sh
flatpak remote-add --user --if-not-exists flathub \
  https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub \
  org.gnome.Platform//50 org.gnome.Sdk//50 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08 \
  org.freedesktop.Sdk.Extension.llvm21//25.08
```

The GNOME SDK and extensions supply the Rust/native build toolchain, GTK, libadwaita, and Vulkan tools inside the sandbox; a host Rust or Vulkan SDK installation is not required for this packaging route. Allow several GB for downloads and build output. The manifest sets Cargo and CMake concurrency to **16**; reduce both `CARGO_BUILD_JOBS` and `CMAKE_BUILD_PARALLEL_LEVEL` in its `build-options.env` on memory-constrained machines.

### Validate locked sources

```sh
python3 -m unittest discover -s build-aux/packaging -p 'test_*.py' -v
python3 -m unittest discover -s build-aux/flatpak -p 'test_*.py' -v
python3 build-aux/flatpak/generate-cargo-sources.py --check
python3 build-aux/flatpak/check-packaging.py \
  build-aux/flatpak/io.github.davidecamino.Speech2Text.json
```

If you intentionally changed `Cargo.lock`, regenerate the source list and include it in the change:

```sh
python3 build-aux/flatpak/generate-cargo-sources.py
```

Flatpak downloads and verifies registry archives before building. Cargo runs with `--frozen --offline` against those vendored sources inside the sandbox. Initial source downloads still need network access. Git/private-registry dependencies are rejected by the generator rather than silently omitted.

### Build and export the package

```sh
flatpak-builder --user --force-clean --disable-rofiles-fuse \
  --install-deps-from=flathub --repo=flatpak-repo \
  flatpak-build build-aux/flatpak/io.github.davidecamino.Speech2Text.json

mkdir -p dist
flatpak build-bundle --arch=x86_64 \
  --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
  flatpak-repo dist/Speech2Text-x86_64.flatpak \
  io.github.davidecamino.Speech2Text stable
(cd dist && sha256sum Speech2Text-x86_64.flatpak > SHA256SUMS)
```

`--force-clean` replaces the `flatpak-build` output directory; do not use it for personal files. Add `--disable-cache` to the builder command when you need to rebuild without reusing its cached stages. The manifest copies the current working tree, including uncommitted changes, with explicit exclusions for Git metadata, build output, private files, and models. Use a clean reviewed checkout for releases.

The result is `dist/Speech2Text-x86_64.flatpak`, with its checksum in `dist/SHA256SUMS`. The package includes desktop integration and project/dependency notices. It uses baseline CPU build flags rather than host-specific AVX optimization.

### Install and test your local package

```sh
(cd dist && sha256sum --check SHA256SUMS)
flatpak install --user ./dist/Speech2Text-x86_64.flatpak
flatpak run io.github.davidecamino.Speech2Text
```

Verify file portals, model selection/download, microphone capture, TXT/SRT export, and CPU/Vulkan recognition on actual hardware. A successful build is not a GPU or microphone compatibility test. See [installation](INSTALL.md#linux-flatpak) for updates and removal.

## 4. Windows: build the executable and package locally

Use **Windows 10/11 x86_64** with one coherent **MSYS2 UCRT64** toolchain, matching the workflow. Do not mix MSVC, MINGW64, CLANG64, or unrelated GTK DLLs into this package. Use a short source/build path to reduce native-tool path-length issues.

### Install MSYS2 and dependencies

Install [MSYS2](https://www.msys2.org/) and open its **UCRT64** terminal. Update it:

```sh
pacman -Syu
```

If asked to close the terminal, reopen UCRT64 and repeat the update before proceeding. Install the build/runtime packages:

```sh
pacman -S --needed git \
  mingw-w64-ucrt-x86_64-rust \
  mingw-w64-ucrt-x86_64-gcc \
  mingw-w64-ucrt-x86_64-binutils \
  mingw-w64-ucrt-x86_64-cmake \
  mingw-w64-ucrt-x86_64-ninja \
  mingw-w64-ucrt-x86_64-clang \
  mingw-w64-ucrt-x86_64-pkgconf \
  mingw-w64-ucrt-x86_64-python \
  mingw-w64-ucrt-x86_64-gtk4 \
  mingw-w64-ucrt-x86_64-libadwaita \
  mingw-w64-ucrt-x86_64-gdk-pixbuf2 \
  mingw-w64-ucrt-x86_64-librsvg \
  mingw-w64-ucrt-x86_64-adwaita-icon-theme \
  mingw-w64-ucrt-x86_64-hicolor-icon-theme \
  mingw-w64-ucrt-x86_64-vulkan-headers \
  mingw-w64-ucrt-x86_64-vulkan-loader \
  mingw-w64-ucrt-x86_64-shaderc
```

Clone the source as in section 1, then enter the repository in the UCRT64 terminal. Confirm `echo "$MSYSTEM"` prints `UCRT64`, and `which cargo python gcc` points to the UCRT64 tools. In particular, use native UCRT64 Python, not MSYS `/usr/bin/python`. The MSYS2 prefix supplies Vulkan headers, loader, and `glslc`; a separate LunarG SDK is not needed for this route.

### Configure and build CPU + Vulkan

Run in UCRT64 Bash; these environment settings apply to the current terminal:

```sh
set -euo pipefail
export CARGO_TARGET_DIR=C:/s2t-target
export CARGO_BUILD_JOBS=2
export CMAKE_BUILD_PARALLEL_LEVEL=2
export CMAKE_GENERATOR=Ninja
export CC=gcc
export CXX=g++
export LIBCLANG_PATH="$(cygpath -m "$MINGW_PREFIX/bin")"
export VULKAN_SDK="$(cygpath -m "$MINGW_PREFIX")"
export GGML_NATIVE=OFF
export GGML_SSE42=OFF
export GGML_AVX=OFF
export GGML_AVX2=OFF
export GGML_BMI2=OFF
export GGML_FMA=OFF
export GGML_F16C=OFF

mkdir -p target
pacman -Q > target/windows-msys2-packages.txt
rustc -vV > target/windows-build-info.txt
glslc --version >> target/windows-build-info.txt
cargo vendor --locked --versioned-dirs target/windows-vendor \
  > target/windows-vendor-config.toml
cargo build --release --locked --features vulkan --bin speech2text
cargo test --release --locked --features vulkan --lib --tests -- \
  --skip ui::tests::settings_pages_and_menu_surfaces_build
```

Choose another short writable `CARGO_TARGET_DIR` if necessary and use it consistently below. The executable is at `C:/s2t-target/release/speech2text.exe`; it is **not yet a portable app**. The vendored registry sources are for license collection. Do not install the generated vendor configuration over the project's configuration or replace the committed patched dependency.

For a CPU-only development executable, omit `--features vulkan`. The current Windows packager labels its artifacts **CPU + Vulkan**, so use the Vulkan build above for this packaging procedure rather than distributing a mislabeled CPU-only build.

### Assemble the portable runtime

Still in the same UCRT64 terminal:

```sh
python -m unittest discover -s build-aux/packaging -p 'test_*.py' -v
python -m unittest discover -s build-aux/windows -p 'test_*.py' -v
python build-aux/windows/package.py \
  --prefix "$(cygpath -m "$MINGW_PREFIX")" \
  --binary "$CARGO_TARGET_DIR/release/speech2text.exe"
```

Outputs:

- `dist/Speech2Text/`: complete runtime tree, launched with `Start-Speech2Text.cmd`.
- `dist/Speech2Text-windows-x86_64.zip`: portable archive.
- `dist/Speech2Text-windows-x86_64.sha256`: archive checksum.

Assembly recreates only the validated `dist/Speech2Text` destination. It resolves non-system DLLs from the selected prefix, never from arbitrary `PATH` directories, and rejects missing or MSYS dependencies. Runtime-loaded plugins, schemas, icons, fonts/configuration, native/Rust notices, package versions, and patched Whisper sources are included. Build tools and models are not shipped.

### Create the installer

Install [Inno Setup 6](https://jrsoftware.org/isinfo.php). With its default installation directory, run this from UCRT64 Bash in the repository root; adjust the compiler path if needed:

```sh
VERSION="$(python -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')"
MSYS2_ARG_CONV_EXCL='*' '/c/Program Files (x86)/Inno Setup 6/ISCC.exe' \
  "/DAppVersion=$VERSION" build-aux/windows/installer.iss
(cd dist && sha256sum "Speech2Text-${VERSION}-windows-x86_64-setup.exe" \
  > Speech2Text-installer.sha256)
```

The result is `dist/Speech2Text-<version>-windows-x86_64-setup.exe`. The installer consumes the previously assembled runtime tree, installs per user, and offers Start Menu/desktop shortcuts. It is unsigned; code signing is not configured.

### Test relocation, installation, and removal

Close other Speech2Text sessions. In **PowerShell 7 (`pwsh`)**, change to the repository root and run each script in a fresh process:

```powershell
pwsh -NoProfile -File ./build-aux/windows/smoke-test.ps1
pwsh -NoProfile -File ./build-aux/windows/test-installer.ps1
```

Separate processes keep the smoke test's deliberately restricted `PATH` from affecting your interactive shell. The first script extracts the ZIP to a temporary path with spaces, launches without MSYS2 on `PATH`, and checks window/SVG startup. The second silently installs into a temporary location, checks startup, and uninstalls. Keep exactly one `*-setup.exe` in `dist` when running the installer test; move older installers elsewhere.

Logs are under `dist/windows-smoke/` and `dist/windows-installer-smoke/`. These tests require an interactive Windows desktop and do not prove GPU inference or microphone compatibility. Also test file transcription, Stop/Cancel, export, microphone permissions, and CPU/Vulkan operation on a clean target machine.

## 5. Validation and backend tools

On a Linux development host with native dependencies:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features vulkan -- -D warnings
cargo test --locked --all-targets
cargo test --locked --all-targets --features vulkan
python3 -m unittest discover -s build-aux/packaging -p 'test_*.py' -v
python3 -m unittest discover -s build-aux/windows -p 'test_*.py' -v
python3 -m unittest discover -s build-aux/flatpak -p 'test_*.py' -v
```

The Vulkan checks require its native development tools. GTK checks use one test entry point, `ui::tests::settings_pages_and_menu_surfaces_build`; it skips if display initialization fails. To run it in a virtual X11 display/session bus, install `xvfb`, `xauth`, and `dbus-x11` on Ubuntu, then:

```sh
set -o pipefail
GDK_BACKEND=x11 GTK_A11Y=none GSK_RENDERER=cairo \
  xvfb-run -a dbus-run-session -- \
  cargo test --locked --all-targets -- --nocapture 2>&1 | tee test-output.log
if grep -Fq 'Skipping GTK surface test' test-output.log; then
  echo 'GTK test did not run' >&2
  exit 1
fi
```

Repeat with `--features vulkan` before `-- --nocapture` for the Vulkan configuration. Most tests use fixtures and loopback mock servers, not real models or microphones. The ignored native-inference cancellation test requires `SPEECH2TEXT_TEST_MODEL` pointing to a local model and an explicit ignored-test run. Generated vendored bindings may emit warnings separately from first-party checks.

Optional backend tools:

```sh
# Explicit model download
cargo run --locked --example download_model -- ./models tiny

# File transcription: text on stdout, SRT on stderr
cargo run --release --locked --example transcribe -- \
  ./models/ggml-tiny.bin ./audio.wav

# Explicit GPU selection
cargo run --release --locked --features vulkan --example transcribe -- \
  ./models/ggml-tiny.bin ./audio.wav --backend vulkan
```

For release testing, additionally verify recording Stop/Cancel, append after manual editing, model replacement, export snapshots, streamed Markdown scrolling, and close/save-failure behavior. Do not treat CPU/Vulkan validation as proof of CUDA, ROCm, or Metal compatibility.

## 6. Packaging ownership and release checks

- `build-aux/flatpak/`: manifest, locked-source generator, and identity checks.
- `build-aux/windows/`: runtime assembly, launcher, Inno Setup definition, and smoke tests.
- `build-aux/packaging/install-licenses.py`: shared Rust dependency notice/metadata collector.
- `data/`: application desktop entry, icon, and AppStream metadata.

The application ID is `io.github.davidecamino.Speech2Text`. Before a release, update `Cargo.toml` and the first AppStream release entry's version/date consistently, then run:

```sh
python3 build-aux/flatpak/check-packaging.py \
  build-aux/flatpak/io.github.davidecamino.Speech2Text.json --release
```

Review [LICENSES.txt](LICENSES.txt) and the exact bundled components' notices and source obligations. Collecting license texts does **not** certify distribution compliance. Preserve the patched native dependency sources and fulfill applicable MPL/LGPL source-availability and replacement/relinking obligations.

Local builds do not publish anything. If you use GitHub Actions instead:

- **Flatpak bundle** builds the Linux package on manual dispatch or `v*` tags.
- **Windows installer (Vulkan)** builds and smoke-tests the portable ZIP and installer on Windows on manual dispatch or `v*` tags.
- Ordinary pushes/PRs run the separate Rust/packaging checks, not release packaging.
- A release tag must match `v<Cargo.toml version>`. Commit and push the reviewed changes before creating a tag; tagged workflows publish assets through separate release jobs.

The workflows live in `.github/workflows/`. Their successful execution does not replace clean-machine testing, signing, or release license review. Sideload Flatpaks and unsigned Windows installers have no automatic application-update mechanism.

## macOS status

Cargo exposes a Metal feature, but this repository does not provide a complete supported macOS packaging procedure. GTK runtime bundling, microphone permission integration, signing/notarization, and installer creation remain follow-up work. Do not assume a successful compilation produces a distributable macOS application.
