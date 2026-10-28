# Speech2Text — implementation questions

No application code has been implemented. Edit the **Answer:** fields below, then ask me to read this file. You can write **“use recommendation”** for any question or leave an answer as **“decide later.”** Recommendations are proposals, not decisions already made.

## 1. Your requirements as understood

- Desktop application for Linux, macOS, and Windows, with GNOME/Linux as the main target.
- GTK 4 and libadwaita interface, with a Rust backend.
- Whisper for speech recognition.
- Main view: select an audio file or record from a microphone, display recognized text, and copy or export it as plain text or timestamped subtitles.
- Microphone transcription runs while recording, rather than only after stopping.
- Rewrite view: available when transcript text exists; prompt at the top, original text below on the left, rewritten text below on the right.
- For this demo, Rewrite copies the original text into the result box. No LLM integration yet.

**Is this summary correct? Any additions or corrections?**

**Answer:** correct

## 2. Scope, platforms, and appearance

### Q1. What should the app be called?
Do you have a display name and application ID/reverse domain (for example, `io.github.YOUR_NAME.Speech2Text`)? A placeholder is fine for now.

**Answer:** io.github.YOUR_NAME.Speech2Text (A placeholder for now)

### Q2. What does cross-platform support mean for the first milestone?
GTK and libadwaita can run on macOS and Windows, but their appearance is not native there, and packaging requires extra work. Linux audio capture and packaging also differ from those platforms.

**Recommendation:** implement and test the demo on Linux first, keep the Rust design portable, then explicitly validate and package macOS/Windows builds. Keep GTK/libadwaita on all platforms.

Do you need tested builds for all three operating systems immediately, or is Linux-first acceptable? Which OS versions/CPU architectures matter (including Apple Silicon)?

**Answer:** After linux the implementation has to work on windows, the main feature is stt on selected audio, the real time recording is not a priority for now on windows

### Q3. How should the application be distributed?
**Recommendation:** runnable development build first; Flatpak for GNOME/Linux as a later packaging milestone. macOS app bundles and Windows installers are separate work.

Do you need packaging in the initial demo? Any required formats or sandbox restrictions?

**Answer:** Recommendation

### Q4. Is a fully Rust application acceptable?
**Recommendation:** Rust for both the interface and backend, using `gtk4-rs` and `libadwaita` bindings, with background workers for audio and inference so the UI stays responsive. No Python service or web frontend.

**Answer:** Recommendation

### Q5. What visual style and window sizes should we target?
**Recommendation:** standard GNOME/libadwaita styling, system light/dark preference, keyboard-accessible controls, and a resizable window. Use side-by-side rewrite panes on wide windows; stack them on narrow windows.

Do you have mockups, preferred colors/icons, a minimum window size, or a requirement to keep the rewrite panes side by side at all sizes? Is English-only UI enough for the demo?

**Answer:** Use all the GNOME/libadwaita syle and design language

## 3. Whisper and models

### Q6. Must speech recognition run entirely on the user's machine?
“Whisper” could mean a local inference engine or a remote transcription API.

**Recommendation:** local-only inference, with no audio/text sent to a server. Network access only for an explicitly requested model download.

**Answer:** Recommendation

### Q7. Which Whisper implementation should we use?
**Recommendation:** `whisper.cpp` through the Rust `whisper-rs` bindings. This keeps the application in Rust while using an established native inference engine. It introduces a C/C++ build dependency.

Do you prefer this, another Rust-compatible engine, an external executable, or an API? Any dependency/license constraints?

**Answer:** Recommendation

### Q8. How should users obtain and choose a model?
Model size affects download size, RAM use, accuracy, and live-transcription speed. Model files are generally too large to bundle casually with a demo.

**Recommendation:** allow selecting an existing compatible model file initially. Add explicit download/model management later. Start with a multilingual `base` model for development, subject to testing on your hardware.

Should the demo support manual model selection, built-in downloads, or both? Do you have a model you want to use?

**Answer:** Both select a local model and download a new model

### Q9. What hardware and acceleration must we support?
**Recommendation:** provide a CPU baseline first and keep GPU acceleration optional. Real-time performance must be measured; a large model may not keep up on a CPU.

What CPU, RAM, and GPU will you use? Is GPU support (Metal, CUDA, Vulkan, etc.) required in the first demo?

**Answer:** Yes implement from start the hw acceleration

### Q10. Which speech languages and recognition modes are needed?
**Recommendation:** support automatic language detection and manual language selection; transcribe in the spoken language. Do not enable Whisper's translation-to-English mode unless requested.

Which languages do you need? Do you also want translation to English? Note that this is separate from the later LLM rewrite feature.

**Answer:** Recommendation

## 4. Audio files and live recording

### Q11. Which input file formats must work?
Examples: WAV, MP3, FLAC, OGG, M4A, or video files containing audio.

**Recommendation:** support common audio formats through a decoding layer, then normalize to the sample format Whisper expects. Decide between a native decoder and FFmpeg after the required formats are known; FFmpeg offers broad compatibility but adds packaging/licensing considerations.

List required formats. Should users also be able to drop files onto the window? Is selecting one file at a time sufficient?

**Answer:** WAV, MP3, FLAC, OGG, M4A

### Q12. What exactly do you expect from “real-time” transcription?
Whisper is not inherently a streaming recognizer. Live transcription usually processes overlapping audio chunks, deduplicates text, and may revise a provisional ending as more context arrives.

**Recommendation:** show stable finalized segments plus a clearly marked provisional tail, aiming for updates every few seconds rather than on every word. Actual delay depends on the model and hardware.

What delay is acceptable (for example, roughly 2–5 seconds versus 10+ seconds)? May provisional text change before becoming final?

**Answer:** Recommendation Black for fixed text grey for provisional

### Q13. Which recording controls and sources are needed?
**Recommendation:** microphone capture with an input-device selector, Start/Stop buttons, recording duration, and a basic input-level indicator. No system/desktop audio capture in the first demo.

Do you need pause/resume, system audio, simultaneous microphone + system audio, or only a microphone? What should happen when a selected device disconnects?

**Answer:** Recommendation

### Q14. Should recording audio itself be retained?
**Recommendation:** keep only the working audio buffers needed for transcription and discard them afterward; do not save recordings automatically.

Do you want a “save recording” option, temporary recovery files, or a strict no-audio-on-disk requirement? Should any recording-duration limit apply?

**Answer:** Recommendation

### Q15. How should jobs, cancellation, and existing text behave?
**Recommendation:** one transcription job at a time; offer cancellation and show progress for files. Before starting a new job, ask whether to replace or append if a transcript already exists. On Stop, finish processing buffered microphone audio; provide a separate cancellation action if needed.

Should cancellation keep the text already recognized? Do you need file queues, background jobs, or recording while a file is being transcribed?

**Answer:** Recommendation

### Q16. Are there expected file lengths or performance limits?
**Recommendation:** process longer inputs incrementally instead of loading entire decoded recordings into RAM; keep completed text if processing fails partway through.

What are typical and maximum recording/file durations? Is faster-than-real-time file processing important?

**Answer:** Recommendation

## 5. Transcript editing and export

### Q17. Should the recognized text be editable?
Editing plain text can invalidate its association with timestamped segments.

**Recommendation:** store timestamped segments independently and keep the live transcript read-only for the first demo. Add transcript editing only with an explicit policy for updating or invalidating subtitle timing.

Do you need editing immediately? If yes, should subtitle export use the original recognized segments, edited timed segments, or be disabled after free-form edits?

**Answer:** No the text is only readable.

### Q18. Which subtitle formats and timing detail do you need?
**Recommendation:** SRT export initially, with segment-level timestamps rather than word-level timing. Add WebVTT if needed. Whisper timing is approximate and is not guaranteed to produce professionally aligned subtitles without additional alignment/correction.

Do you need SRT, WebVTT, or both? Are segment-level timestamps sufficient? Any subtitle rules such as maximum line length, two-line cues, speaker labels, or minimum cue duration?

**Answer:** SRT with segment-level timestamps

### Q19. What should appear in the main text area and exports?
**Recommendation:** show plain recognized text in the main area; Copy copies text without timestamps, TXT saves UTF-8 plain text, and subtitle export uses the stored timed segments. Optionally expose timestamps in a separate display mode later.

Should timestamps be visible in the main area? Do TXT/clipboard output need timestamps, speaker labels, or a specific paragraph layout? Is speaker identification required? It would need work beyond basic Whisper transcription.

**Answer:** Recommendation with no later timestamps shows

### Q20. Should transcripts and settings survive closing the app?
**Recommendation:** remember non-sensitive preferences such as model path, language, and input device, but do not automatically persist audio or transcript text. Warn before discarding an unexported transcript.

Do you want session restore, autosave, transcript history, or multiple documents/windows? Any privacy requirements beyond local processing?

**Answer:** Recommendation

## 6. Rewrite view — demo behavior

### Q21. How should users move between the two views?
**Recommendation:** use one application window with a “Rewrite” action enabled only when the transcript contains non-whitespace text, plus a Back action. Disable rewriting while transcription is running to avoid changing the source underneath it.

Would you prefer tabs, a view switcher, or a separate window? Should users be able to open Rewrite while transcription is still running?

**Answer:** Recommendation with a button that allows change view

### Q22. What should the rewrite prompt and button do in the demo?
**Recommendation:** editable multi-line prompt above the two panes, with a “Rewrite” button. Clicking it copies the left pane exactly into the right pane, ignoring the prompt, with a visible note that LLM rewriting is not implemented. No simulated network request or model download.

Should the prompt start empty or contain a default instruction? Should an empty prompt still allow the demo copy action?

**Answer:** Recommendation, contain a default instruction with an example request that specify the context of the audio for the llm

### Q23. Are the source and result panes editable, and how is the source populated?
**Recommendation:** take a snapshot of the main transcript when entering the rewrite view, make the left pane read-only and the right pane editable. Re-entering with a changed transcript should ask before discarding an existing rewrite result.

Do you want different behavior, including editing the left pane, preserving the prompt/result across navigation, or an explicit “refresh source” action?

**Answer:** all read only

### Q24. What can users do with the rewritten result?
**Recommendation:** provide Copy and Save as TXT for the result. Keep it separate from the original transcript; do not offer subtitles for rewritten text because rewriting can break timing alignment.

Should there also be “Replace original transcript”? Do you expect subtitle export from rewritten text? If so, we need a separate timing policy rather than reusing the original timestamps blindly.

**Answer:** Save as TXT or markdown

## 7. Priorities and acceptance

### Q25. What must work for you to call the first demo successful?
**Suggested milestone:** Linux application launches; loads a local model; transcribes one audio file; transcribes microphone audio incrementally without freezing the UI; copies text; exports TXT/SRT; and demonstrates the two-pane rewrite copy action.

Please mark must-haves versus features that can wait. Are there reference audio files, expected transcripts, or particular live-recording scenarios we can use to test accuracy, timing, and performance?

**Answer:** Suggested milestone

### Q26. Any additional constraints or preferences?
Examples: application license, Rust toolchain/minimum supported version, offline builds, accessibility requirements, preferred project structure, automated tests, CI, or dependencies you want to avoid.

**Answer:** no

---

## After you answer

I will read this file, summarize the agreed decisions and remaining trade-offs, and propose a concrete architecture and implementation plan. No application implementation will begin until you ask for it.
