Speech2Text is a Rust/GTK4/libadwaita desktop app for local Whisper file/microphone transcription, model downloads, TXT/SRT export, and a placeholder rewrite view; Linux Flatpak and Windows packaging are included.

## Bugs — highest importance first

1. **High — Boundary text can be duplicated/lost.** File windows independently recognize overlapping audio, then filter whole segments by midpoint; different segment boundaries can retain both copies or discard both. `Transcript::append` only clamps timestamps, not duplicated words. Live capture also forcibly cuts at 28 seconds without overlap. (`src/transcription/worker.rs`, `src/transcript.rs`)
2. **High — Stop does not immediately stop recording.** Capture continues throughout an in-flight inference; Stop is checked and capture paused only afterward. Extra speech is recorded, and queue overflow can turn Stop into failure before buffered audio is finalized. Cancel likewise cannot interrupt model loading/inference. (`src/transcription/worker.rs`, `src/transcription/engine.rs`)
3. **Medium — Model replacement can destroy the previous model.** Download completion deletes the existing target before renaming; rename failure/crash loses it. A fixed `.part` filename also makes concurrent downloads to the same directory interfere. (`src/model.rs`)
4. **Medium — Flatpak omits local dependency notices.** License installation scans only `cargo/vendor`; the patched `vendor/whisper-rs-sys`, including whisper.cpp’s MIT notice, is excluded. Windows explicitly handles both directories. (`build-aux/flatpak/io.github.davidecamino.Speech2Text.json`)
5. **Medium — Preferences can be truncated.** Settings overwrite JSON directly rather than atomically; interrupted/failed writes can leave an unreadable configuration that subsequently falls back to defaults. (`src/settings.rs`)
6. **Medium — Regression checks are not enforced on PRs.** The “Rust checks” workflow only builds Vulkan: no Rust/Python tests, formatting, or Clippy. Current tests also lack real inference, window-boundary, live Stop/Cancel/overflow, and download-integrity failure coverage. (`.github/workflows/check.yml`, `tests/`)
7. **Medium — Long transcripts incur growing UI work.** Every final/provisional update rebuilds and replaces the entire transcript, including empty final events; cumulative work grows roughly quadratically and disrupts text selection. (`src/ui/mod.rs`)
8. **Low — Device discovery blocks startup and hides failures.** Microphones are enumerated synchronously on GTK’s thread; errors become an empty list. A missing saved device silently becomes Default and is overwritten when preferences are saved. (`src/ui/preferences.rs`, `src/ui/mod.rs`)
9. **Low — Recording status becomes misleading.** Meter events overwrite “Finishing…”/“Cancelling…” with “Recording”; elapsed time continues increasing while stopped audio is finalized. (`src/ui/mod.rs`, `src/transcription/worker.rs`)
10. **Low — Repository/documentation drift.** `cargo fmt --check` fails in `src/transcription/worker.rs`; README omits Medium/Large downloads, incorrectly claims normal CI runs packaging tests, and links to nonexistent root `question.md`.

Validation: 13 Rust tests and 13 Python packaging tests passed; Flatpak source/identity checks passed. Findings otherwise follow source inspection; actual microphone/GPU inference and Windows/Flatpak installation were not exercised in this review. Vendored native dependencies were not exhaustively audited.

## Missing features

These are absent capabilities, not necessarily promised requirements:

- Real LLM rewriting, usable LLM settings, and prompt execution (currently an explicitly labelled copy-only demo).
- Transcript/result editing, session reopening/history, opt-in autosave/recovery, and optional audio retention.
- VAD, robust cross-window alignment/deduplication, word-level timestamps, speaker diarization, and translation.
- Microphone hot-plug refresh, broader sample-format support, pause/resume, and system-audio capture.
- Model download resume/cache reuse, installed-model management/deletion, and storage-space preflight.
- Per-GPU selection and reporting the actual inference device/fallback.
- Batch processing; localization; macOS app packaging/permissions/signing; signed Windows releases and automatic application updates/Flathub distribution.
