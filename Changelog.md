# Changelog

## Unreleased

### Added

- **LLM model selection:** choose a server model from a dropdown in Settings → LLM instead of automatically using the first advertised model.
- **Refresh Models:** discover available models, including unloaded models on llama.cpp router servers started with `--no-models-autoload`.
- **Reasoning level selection:** choose Server default, Off, Low, Medium, or High. Support depends on the model and server; Server default sends no override.
- **Saved LLM selections:** model and reasoning preferences persist between sessions, with backward-compatible defaults for existing settings.
- **LLM Apply button:** save selections, close Settings, and load the selected model in the background without starting generation.
- **Speech to Text Apply button:** save recognition settings and close Settings.
- Regression tests for model discovery, loading readiness, reasoning parameters, saved selections, and Apply controls.

### Changed

- Rewrite no longer initiates model loading. Unready models require applying the selection in Settings first.
- Rewrite and conflicting controls remain disabled while a model loads. Loading completion or errors appear in the main and rewrite views.
- Model loading waits for readiness and handles a model already being loaded, avoiding duplicate loading requests.
- Both settings pages now have standalone, bottom-right-aligned Apply buttons.
- The LLM model-selection row explains the Apply/loading workflow.
- Settings window default height increased from 580 to 900 pixels, reducing scrolling on larger screens while retaining scrolling on smaller displays.
- README updated with multi-model server setup, model and reasoning selection, and the Apply workflow.

### Compatibility

- Single-model servers remain supported through a fallback to `/v1/models` when `/models` is unavailable.
- The application still connects to an externally launched llama server; it does not start or stop the server, unload other models, or change GPU-layer settings.
