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
fn already_cancelled_download_does_not_publish_a_model() {
    use std::sync::atomic::AtomicBool;
    let dir = tempfile::tempdir().unwrap();
    let result = speech2text::model::download(
        &speech2text::model::CATALOG[0],
        dir.path(),
        &AtomicBool::new(true),
        |_| {},
    );
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
