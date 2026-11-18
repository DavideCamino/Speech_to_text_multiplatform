# Changelog

## Unreleased

### Added

- **Batch audio transcription:** select multiple files, expand the queue, reorder with drag-and-drop or up/down buttons, and remove entries before Start. Files run sequentially with one Replace/Append confirmation for the whole batch.
- **File progress and time remaining:** display the current file's progress and position alongside total time remaining, including for a single file. Duration inspection runs in the background; unavailable timing information shows Calculating time remaining.
- **Batch boundaries and failure handling:** separate files' transcriptions with a blank line, preserve continuous subtitle timestamps, continue past failed files with an end-of-batch summary, and cancel the entire batch while retaining finalized text and all source audio.
- Regression coverage for queue ordering and removal, drag-and-drop/buttons, stale confirmations/drags, remaining-time calculations, sequential processing, empty files, transcript/subtitle boundaries, cancellation, failures, and closing during a batch.

- **Unsaved-changes indicator:** a theme-aware dot beside Speech2Text in the header bar shows when the transcript or rewrite has unexported changes. It updates after edits, recognition, generation, exports, and session recovery; autosaving does not clear it. The indicator includes an accessible label and an Unsaved changes tooltip.
- **Save-on-close recovery:** closing with unsaved content or active work offers Cancel, Close Without Saving, and Autosave & Close.
- **Temporary recovery snapshots:** atomically save edited transcripts, original subtitle timings, rewrite source/instructions/output, recognition context, and unsaved flags to `recovery.json` in the app's data folder. Audio and authentication tokens are excluded.
- **Next-launch restoration:** restore saved content without restarting recording or generation, retain unsaved flags, and delete the recovery file only after restoration succeeds.
- **Recovery error handling:** failed autosaves keep the app open with retry/discard options; unreadable recovery files are preserved. Files are versioned and limited to 64 MiB.
- Tests for recovery serialization, atomic replacement, close choices, edited-text/subtitle restoration, partial rewrites, file deletion, and failure handling.
- **LLM model selection:** choose a server model from a dropdown in Settings → LLM instead of automatically using the first advertised model.
- **Refresh Models:** discover available models, including unloaded models on llama.cpp router servers started with `--no-models-autoload`.
- **Reasoning level selection:** choose Server default, Off, Low, Medium, or High. Support depends on the model and server; Server default sends no override.
- **Saved LLM selections:** model and reasoning preferences persist between sessions, with backward-compatible defaults for existing settings.
- **LLM Apply button:** save selections, close Settings, and load the selected model in the background without starting generation.
- **Speech to Text Apply button:** save recognition settings and close Settings.
- Regression tests for model discovery, loading readiness, reasoning parameters, saved selections, and Apply controls.

### Changed

- Opening audio queues one or more files without starting transcription. Record becomes Start; Replace/Append confirmation appears when Start is clicked. Opening audio again replaces the pending queue. Cancel clears the selection without deleting source audio or existing finalized text. Completion or cancellation returns the controls to microphone recording mode. Queued files are not included in recovery snapshots.
- The unsaved-changes dot and autosave close option now require non-whitespace, unexported transcript or rewrite text. Erasing all text no longer prompts to save an empty recovery file; active work still requires cancellation confirmation without an autosave option when no text needs saving.
- Rewrite no longer initiates model loading. Unready models require applying the selection in Settings first.
- Rewrite and conflicting controls remain disabled while a model loads. Loading completion or errors appear in the main and rewrite views.
- Model loading waits for readiness and handles a model already being loaded, avoiding duplicate loading requests.
- Both settings pages now have standalone, bottom-right-aligned Apply buttons.
- The LLM model-selection row explains the Apply/loading workflow.
- Settings window default height increased from 580 to 900 pixels, reducing scrolling on larger screens while retaining scrolling on smaller displays.
- README updated with multi-model server setup, model and reasoning selection, and the Apply workflow.

### Fixed

- Isolated zoom widget-lifetime tests from desktop input-method services by using GTK's simple input method in test fixtures. Added an unshown-widget ownership check and bounded GTK cleanup waits while retaining widget-release assertions. Application input-method behavior is unchanged.

### Compatibility

- Single-model servers remain supported through a fallback to `/v1/models` when `/models` is unavailable.
- The application still connects to an externally launched llama server; it does not start or stop the server, unload other models, or change GPU-layer settings.
