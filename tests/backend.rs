use speech2text::{
    audio::AudioFile,
    settings::Settings,
    transcription::{self, Event, Source},
};
use std::{io::Write, time::Duration};

fn wav(rate: u32, channels: u16, frames: usize) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
    let bytes = (frames * channels as usize * 2) as u32;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&rate.to_le_bytes()).unwrap();
    file.write_all(&(rate * channels as u32 * 2).to_le_bytes())
        .unwrap();
    file.write_all(&(channels * 2).to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&bytes.to_le_bytes()).unwrap();
    for i in 0..frames {
        let sample =
            ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 16000.0) as i16;
        for _ in 0..channels {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }
    file.flush().unwrap();
    file
}

#[test]
fn decodes_stereo_wav_incrementally_into_16khz_mono() {
    let file = wav(48000, 2, 48000);
    let mut audio = AudioFile::open(file.path()).unwrap();
    let mut samples = Vec::new();
    let mut packets = 0;
    while let Some(packet) = audio.next_samples().unwrap() {
        samples.extend(packet);
        packets += 1;
    }
    assert!(packets > 1);
    assert_eq!(samples.len(), 16000);
    assert!(samples.iter().all(|s| s.is_finite() && s.abs() < 1.0));
    assert!(samples.iter().any(|s| s.abs() > 0.4));
    assert!(audio.next_samples().unwrap().is_none());
}

#[test]
fn rejects_non_audio_without_panicking() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"This is not an audio stream").unwrap();
    assert!(AudioFile::open(file.path()).is_err());
}

#[test]
fn worker_reports_model_errors_and_always_finishes() {
    let job = transcription::start(Source::File("missing.wav".into()), Settings::default(), 0);
    loop {
        if let Event::Finished(result) = job.events.recv_timeout(Duration::from_secs(10)).unwrap() {
            assert!(result.unwrap_err().contains("model"));
            break;
        }
    }
}

#[test]
fn model_library_lists_only_bin_files_and_handles_a_missing_folder() {
    use speech2text::model;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("models");
    assert!(model::installed(&directory).unwrap().is_empty());
    assert!(!directory.exists());
    std::fs::create_dir(&directory).unwrap();
    for name in ["z.bin", "A.BIN", "notes.txt", ".import.part"] {
        std::fs::write(directory.join(name), b"test").unwrap();
    }
    std::fs::create_dir(directory.join("folder.bin")).unwrap();
    assert_eq!(
        model::installed(&directory).unwrap(),
        vec![directory.join("A.BIN"), directory.join("z.bin")]
    );
}

#[test]
fn import_copies_a_model_without_moving_the_original() {
    use speech2text::model;
    use std::sync::atomic::AtomicBool;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("ggml-☕.bin");
    let directory = root.path().join("models");
    let bytes = vec![42_u8; 150_000];
    std::fs::write(&source, &bytes).unwrap();
    let mut progress = Vec::new();
    let target = model::import(&source, &directory, &AtomicBool::new(false), |fraction| {
        progress.push(fraction)
    })
    .unwrap();
    assert_eq!(target, directory.join(source.file_name().unwrap()));
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    assert_eq!(model::installed(&directory).unwrap(), vec![target]);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    assert_eq!(progress.last(), Some(&1.0));
    assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn import_never_overwrites_an_existing_model() {
    use speech2text::model;
    use std::sync::atomic::AtomicBool;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("model.bin");
    let directory = root.path().join("models");
    std::fs::create_dir(&directory).unwrap();
    let target = directory.join("model.bin");
    std::fs::write(&source, b"replacement").unwrap();
    std::fs::write(&target, b"existing").unwrap();
    assert!(model::import(&source, &directory, &AtomicBool::new(false), |_| {}).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"existing");
    assert_eq!(std::fs::read(&source).unwrap(), b"replacement");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
}

#[test]
fn cancelled_import_cleans_up_partial_files() {
    use speech2text::model;
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("model.bin");
    let directory = root.path().join("models");
    let bytes = vec![7_u8; 150_000];
    std::fs::write(&source, &bytes).unwrap();
    let cancel = AtomicBool::new(true);
    assert!(model::import(&source, &directory, &cancel, |_| {}).is_err());
    assert!(!directory.exists());
    cancel.store(false, Ordering::Relaxed);
    let result = model::import(&source, &directory, &cancel, |_| {
        cancel.store(true, Ordering::Relaxed)
    });
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
}

#[test]
fn import_collision_during_copy_does_not_overwrite_the_destination() {
    use speech2text::model;
    use std::sync::atomic::AtomicBool;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("model.bin");
    let directory = root.path().join("models");
    std::fs::write(&source, b"source").unwrap();
    let target = directory.join("model.bin");
    let result = model::import(&source, &directory, &AtomicBool::new(false), |_| {
        std::fs::write(&target, b"other writer").unwrap();
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"other writer");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
}

#[test]
fn already_cancelled_download_does_not_publish_a_model() {
    use std::sync::atomic::AtomicBool;
    let dir = tempfile::tempdir().unwrap();
    let result = speech2text::model::download(
        &speech2text::model::CATALOG[0],
        dir.path(),
        speech2text::model::Replacement::Never,
        &AtomicBool::new(true),
        |_| {},
    );
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
