//! Ordered file jobs, duration-based remaining time, and queue controls.
use crate::transcription::Completion;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::Instant,
};

const METADATA_BUFFER: usize = 64;
const METADATA_UPDATES_PER_POLL: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FileId(usize);

#[derive(Debug)]
pub(super) struct AudioFile {
    id: FileId,
    pub path: PathBuf,
    pub duration: Option<f64>,
}

pub(super) struct Batch {
    pub index: usize,
    pub offset_ms: u64,
    started: Instant,
    completed_fractions: Vec<f64>,
    pub fraction: f64,
    pub failures: Vec<String>,
    pub incomplete: Vec<PathBuf>,
}

type DurationUpdate = (FileId, Option<f64>);

struct DurationProbe {
    results: Receiver<DurationUpdate>,
    cancel: Arc<AtomicBool>,
}

impl DurationProbe {
    fn start(
        files: Vec<(FileId, PathBuf)>,
        mut probe: impl FnMut(&Path) -> Option<f64> + Send + 'static,
    ) -> Self {
        let (tx, results) = mpsc::sync_channel(METADATA_BUFFER);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        std::thread::spawn(move || {
            for (id, path) in files {
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
                let duration = probe(&path).filter(|d| d.is_finite() && *d > 0.0);
                if cancelled.load(Ordering::Relaxed) || tx.send((id, duration)).is_err() {
                    break;
                }
            }
        });
        Self { results, cancel }
    }
}

impl Drop for DurationProbe {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // Dropping the receiver also wakes a sender waiting on a full channel.
    }
}

#[derive(Default)]
pub(super) struct AudioQueue {
    pub files: Vec<AudioFile>,
    pub revision: u64,
    pub batch: Option<Batch>,
    rows: HashMap<FileId, usize>,
    metadata: Option<DurationProbe>,
    #[cfg(test)]
    pub(super) eta_calculations: std::cell::Cell<usize>,
}

impl AudioQueue {
    pub fn replace(&mut self, paths: Vec<PathBuf>) {
        self.replace_with_probe(paths, |path| {
            crate::audio::AudioFile::open(path)
                .ok()
                .and_then(|audio| audio.duration_seconds)
        });
    }

    fn replace_with_probe(
        &mut self,
        paths: Vec<PathBuf>,
        probe: impl FnMut(&Path) -> Option<f64> + Send + 'static,
    ) {
        self.clear();
        // IDs are stable for this selection. Replacements get a new channel,
        // so a late result cannot match an ID in the replacement queue.
        self.files = paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| AudioFile {
                id: FileId(index),
                path,
                duration: None,
            })
            .collect();
        self.reindex();
        if !self.files.is_empty() {
            let files = self
                .files
                .iter()
                .map(|file| (file.id, file.path.clone()))
                .collect();
            self.metadata = Some(DurationProbe::start(files, probe));
        }
    }

    fn reindex(&mut self) {
        self.rows.clear();
        self.rows.extend(
            self.files
                .iter()
                .enumerate()
                .map(|(row, file)| (file.id, row)),
        );
    }

    pub fn clear(&mut self) {
        self.metadata = None;
        self.files.clear();
        self.rows.clear();
        self.batch = None;
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn selected(&self) -> Option<&PathBuf> {
        self.files
            .get(self.batch.as_ref().map_or(0, |b| b.index))
            .map(|f| &f.path)
    }

    pub fn move_file(&mut self, from: usize, to: usize) -> bool {
        if self.batch.is_some() || from == to || from >= self.files.len() || to >= self.files.len()
        {
            return false;
        }
        let file = self.files.remove(from);
        self.files.insert(to, file);
        self.reindex();
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub fn remove(&mut self, index: usize) -> bool {
        if self.batch.is_some() || index >= self.files.len() {
            return false;
        }
        self.files.remove(index);
        self.reindex();
        if self.files.is_empty() {
            self.metadata = None;
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub fn begin(&mut self, offset_ms: u64) {
        self.batch = Some(Batch {
            index: 0,
            offset_ms,
            started: Instant::now(),
            completed_fractions: vec![],
            fraction: 0.0,
            failures: vec![],
            incomplete: vec![],
        });
    }

    /// Record the current file and return whether another file follows.
    pub fn advance(&mut self, result: &Result<Completion, String>, next_offset_ms: u64) -> bool {
        let Some(batch) = self.batch.as_mut() else {
            return false;
        };
        let Some(file) = self.files.get(batch.index) else {
            return false;
        };
        match result {
            Ok(Completion::Complete) => batch.fraction = 1.0,
            Ok(Completion::IncompleteFile) => batch.incomplete.push(file.path.clone()),
            Err(error) => batch
                .failures
                .push(format!("{}: {error}", file.path.display())),
        }
        batch.completed_fractions.push(batch.fraction);
        batch.index += 1;
        batch.offset_ms = next_offset_ms;
        batch.fraction = 0.0;
        batch.index < self.files.len()
    }

    #[cfg(test)]
    pub(super) fn set_test_durations(&mut self, seconds: f64) {
        // A pending metadata update must not overwrite the fixture when App::poll runs.
        self.metadata = None;
        for file in &mut self.files {
            file.duration = Some(seconds);
        }
    }

    pub fn remaining_seconds(&self) -> Option<f64> {
        #[cfg(test)]
        self.eta_calculations.set(self.eta_calculations.get() + 1);
        let batch = self.batch.as_ref()?;
        let current = self.files.get(batch.index)?.duration?;
        let processed = self
            .files
            .iter()
            .zip(&batch.completed_fractions)
            .map(|(file, fraction)| file.duration.unwrap_or(0.0) * fraction)
            .sum::<f64>()
            + current * batch.fraction;
        let elapsed = batch.started.elapsed().as_secs_f64();
        if processed <= 0.0 || elapsed <= 0.0 {
            return None;
        }
        let mut remaining = current * (1.0 - batch.fraction);
        for file in &self.files[batch.index + 1..] {
            remaining += file.duration?;
        }
        let seconds = remaining * elapsed / processed;
        seconds.is_finite().then_some(seconds.max(0.0))
    }

    pub fn poll_metadata(&mut self) {
        let Some(probe) = &self.metadata else {
            return;
        };
        // A ready backlog must not keep GTK from handling input or job events.
        for _ in 0..METADATA_UPDATES_PER_POLL {
            match probe.results.try_recv() {
                Ok((id, duration)) => {
                    if let Some(&row) = self.rows.get(&id) {
                        self.files[row].duration = duration;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.metadata = None;
                    break;
                }
            }
        }
    }
}

impl super::app::App {
    pub(super) fn clear_audio_queue(&self) {
        self.audio_queue.borrow_mut().clear();
        self.main.audio_list.remove_all();
        self.hide_file_progress();
    }

    pub(super) fn rebuild_audio_queue(self: &std::rc::Rc<Self>) {
        use adw::prelude::*;
        use std::rc::Rc;
        self.main.audio_list.remove_all();
        let queue = self.audio_queue.borrow();
        let count = queue.files.len();
        for (index, file) in queue.files.iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let handle = gtk::Image::from_icon_name("list-drag-handle-symbolic");
            handle.set_tooltip_text(Some("Drag to reorder"));
            content.append(&handle);
            let name = file
                .path
                .file_name()
                .unwrap_or(file.path.as_os_str())
                .to_string_lossy();
            let label = gtk::Label::builder()
                .label(format!("{}. {name}", index + 1))
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .tooltip_text(file.path.to_string_lossy())
                .build();
            content.append(&label);
            for (icon, tooltip, target) in [
                ("go-up-symbolic", "Move up", index.checked_sub(1)),
                (
                    "go-down-symbolic",
                    "Move down",
                    (index + 1 < count).then_some(index + 1),
                ),
            ] {
                let button = gtk::Button::builder()
                    .icon_name(icon)
                    .tooltip_text(tooltip)
                    .sensitive(target.is_some())
                    .build();
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let (Some(app), Some(target)) = (weak.upgrade(), target) {
                        app.move_queued_audio(index, target);
                    }
                });
                content.append(&button);
            }
            let remove = gtk::Button::builder()
                .icon_name("list-remove-symbolic")
                .tooltip_text("Remove audio file")
                .build();
            let weak = Rc::downgrade(self);
            remove.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade()
                    && !app.busy()
                {
                    app.audio_queue.borrow_mut().remove(index);
                    app.rebuild_audio_queue();
                    app.refresh();
                }
            });
            content.append(&remove);
            row.set_child(Some(&content));
            let drag = gtk::DragSource::builder()
                .actions(gtk::gdk::DragAction::MOVE)
                .build();
            let revision = queue.revision;
            drag.connect_prepare(move |_, _, _| {
                Some(gtk::gdk::ContentProvider::for_value(
                    &format!("speech2text-audio:{revision}:{index}").to_value(),
                ))
            });
            row.add_controller(drag);
            let drop = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
            let weak = Rc::downgrade(self);
            drop.connect_drop(move |_, value, _, _| {
                if let (Some(app), Ok(payload)) = (weak.upgrade(), value.get::<String>()) {
                    let current_revision = app.audio_queue.borrow().revision;
                    let mut parts = payload.split(':');
                    if parts.next() == Some("speech2text-audio")
                        && parts.next().and_then(|s| s.parse::<u64>().ok())
                            == Some(current_revision)
                        && let Some(from) = parts.next().and_then(|s| s.parse::<usize>().ok())
                        && parts.next().is_none()
                    {
                        return app.move_queued_audio(from, index);
                    }
                }
                false
            });
            row.add_controller(drop);
            self.main.audio_list.append(&row);
        }
    }

    pub(super) fn move_queued_audio(self: &std::rc::Rc<Self>, from: usize, to: usize) -> bool {
        if self.busy() {
            return false;
        }
        let changed = self.audio_queue.borrow_mut().move_file(from, to);
        if changed {
            self.rebuild_audio_queue();
            self.refresh();
        }
        changed
    }

    pub(super) fn refresh_file_progress(&self) {
        use adw::prelude::*;
        let cancelling = self.session.borrow().cancelling();
        let queue = self.audio_queue.borrow();
        if !cancelling
            && let Some(batch) = &queue.batch
            && batch.index < queue.files.len()
        {
            let remaining = queue
                .remaining_seconds()
                .map(time_remaining)
                .unwrap_or_else(|| "Calculating time remaining…".to_owned());
            self.main.file_progress.set_text(&format!(
                "File {} of {} · {remaining}",
                batch.index + 1,
                queue.files.len()
            ));
            self.main.file_progress.set_visible(true);
        } else {
            self.hide_file_progress();
        }
    }

    pub(super) fn hide_file_progress(&self) {
        use adw::prelude::*;
        self.main.file_progress.set_visible(false);
        self.main.file_progress.set_text("");
    }
}

pub(super) fn time_remaining(seconds: f64) -> String {
    if seconds < 60.0 {
        return format!("{} s remaining", seconds.ceil() as u64);
    }
    let minutes = (seconds / 60.0).ceil() as u64;
    if minutes < 60 {
        format!("{minutes} min remaining")
    } else {
        format!("{} h {} min remaining", minutes / 60, minutes % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    fn queue() -> AudioQueue {
        let mut queue = AudioQueue {
            files: [("a.wav", 60.0), ("b.wav", 120.0), ("c.wav", 180.0)]
                .into_iter()
                .enumerate()
                .map(|(index, (name, duration))| AudioFile {
                    id: FileId(index),
                    path: name.into(),
                    duration: Some(duration),
                })
                .collect(),
            ..AudioQueue::default()
        };
        queue.reindex();
        queue
    }

    fn wait_for_metadata(queue: &mut AudioQueue) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while queue.metadata.is_some() {
            queue.poll_metadata();
            assert!(Instant::now() < deadline, "duration probe did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn ordering_removal_and_running_lock() {
        let mut q = queue();
        assert!(q.move_file(0, 2));
        assert_eq!(
            q.files
                .iter()
                .map(|f| f.path.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["b.wav", "c.wav", "a.wav"]
        );
        assert!(!q.move_file(0, 3));
        assert!(!q.move_file(1, 1));
        assert!(q.remove(1));
        assert_eq!(q.files.len(), 2);
        q.begin(0);
        assert!(!q.move_file(0, 1));
        assert!(!q.remove(0));
        q.clear();
        assert!(q.selected().is_none() && q.batch.is_none());
    }

    #[test]
    fn metadata_follows_ids_after_reorder_and_removal_even_for_duplicate_paths() {
        let mut q = queue();
        let [first, second, removed] = [q.files[0].id, q.files[1].id, q.files[2].id];
        q.files[1].path = q.files[0].path.clone();
        let (tx, results) = mpsc::channel();
        q.metadata = Some(DurationProbe {
            results,
            cancel: Arc::default(),
        });
        assert!(q.move_file(0, 2)); // second, removed, first
        assert!(q.remove(1));
        tx.send((first, Some(10.0))).unwrap();
        tx.send((second, Some(20.0))).unwrap();
        tx.send((removed, Some(30.0))).unwrap();
        q.poll_metadata();
        assert_eq!(q.files[0].path, q.files[1].path);
        assert_eq!(q.files[0].id, second);
        assert_eq!(q.files[0].duration, Some(20.0));
        assert_eq!(q.files[1].id, first);
        assert_eq!(q.files[1].duration, Some(10.0));
    }

    #[test]
    fn replacing_selection_stops_old_probe_before_the_next_file() {
        let mut q = AudioQueue::default();
        let (started, attempts) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let timeout = Duration::from_secs(5);
        q.replace_with_probe(
            vec!["a.wav".into(), "b.wav".into(), "c.wav".into()],
            move |path| {
                started.send(path.to_owned()).unwrap();
                if path == Path::new("a.wav") {
                    resume.recv_timeout(timeout).unwrap();
                }
                Some(11.0)
            },
        );
        assert_eq!(attempts.recv_timeout(timeout).unwrap(), Path::new("a.wav"));
        let old_cancel = q.metadata.as_ref().unwrap().cancel.clone();
        q.replace_with_probe(vec!["new.wav".into()], |_| Some(22.0));
        assert!(old_cancel.load(Ordering::Relaxed));
        release.send(()).unwrap();
        // The probe owns the sender. Disconnection proves it exited without
        // opening b.wav or c.wav, rather than merely being slow to start them.
        assert_eq!(
            attempts.recv_timeout(timeout),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        wait_for_metadata(&mut q);
        assert_eq!(q.files[0].path, Path::new("new.wav"));
        assert_eq!(q.files[0].duration, Some(22.0));
    }

    #[test]
    fn duration_updates_arrive_before_the_rest_of_the_scan_finishes() {
        let mut q = AudioQueue::default();
        let (entered, wait_for_second) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let timeout = Duration::from_secs(5);
        q.replace_with_probe(vec!["a.wav".into(), "b.wav".into()], move |path| {
            if path == Path::new("b.wav") {
                entered.send(()).unwrap();
                resume.recv_timeout(timeout).unwrap();
                Some(20.0)
            } else {
                Some(10.0)
            }
        });
        wait_for_second.recv_timeout(timeout).unwrap();
        q.poll_metadata();
        assert_eq!(q.files[0].duration, Some(10.0));
        assert_eq!(q.files[1].duration, None);
        q.begin(0);
        q.batch.as_mut().unwrap().fraction = 0.5;
        assert!(q.remaining_seconds().is_none(), "no partial total");
        release.send(()).unwrap();
        wait_for_metadata(&mut q);
        assert_eq!(q.files[1].duration, Some(20.0));
        assert!(q.remaining_seconds().is_some());
    }

    #[test]
    fn duration_poll_limits_work_when_many_results_are_ready() {
        let mut q = AudioQueue::default();
        let (tx, results) = mpsc::channel();
        q.metadata = Some(DurationProbe {
            results,
            cancel: Arc::default(),
        });
        for index in 0..=METADATA_UPDATES_PER_POLL {
            let id = FileId(index);
            q.files.push(AudioFile {
                id,
                path: "fixture.wav".into(),
                duration: None,
            });
            tx.send((id, Some(1.0))).unwrap();
        }
        q.reindex();
        drop(tx);
        q.poll_metadata();
        assert_eq!(
            q.files
                .iter()
                .filter(|file| file.duration.is_some())
                .count(),
            METADATA_UPDATES_PER_POLL
        );
        assert!(q.metadata.is_some());
        q.poll_metadata();
        assert!(q.files.iter().all(|file| file.duration == Some(1.0)));
        assert!(q.metadata.is_none());
    }

    #[test]
    fn remaining_time_includes_current_and_all_pending_files() {
        let mut q = queue();
        q.begin(0);
        let b = q.batch.as_mut().unwrap();
        b.started = Instant::now() - Duration::from_secs(10);
        b.fraction = 0.5; // 30 audio seconds processed in 10 wall seconds
        assert!((q.remaining_seconds().unwrap() - 110.0).abs() < 0.1);
        q.files.pop();
        assert!((q.remaining_seconds().unwrap() - 50.0).abs() < 0.1);
        q.files.pop();
        assert!((q.remaining_seconds().unwrap() - 10.0).abs() < 0.1);
        q.batch.as_mut().unwrap().fraction = 0.0;
        assert!(q.remaining_seconds().is_none());
    }

    #[test]
    fn unknown_duration_waits_and_late_metadata_counts_completed_work() {
        let mut q = queue();
        q.files[0].duration = None;
        q.begin(0);
        q.batch.as_mut().unwrap().fraction = 1.0;
        assert!(q.remaining_seconds().is_none());
        assert!(q.advance(&Ok(Completion::Complete), 0));
        q.files[0].duration = Some(60.0);
        q.batch.as_mut().unwrap().started = Instant::now() - Duration::from_secs(10);
        assert!((q.remaining_seconds().unwrap() - 50.0).abs() < 0.1);
        q.files[2].duration = None;
        assert!(
            q.remaining_seconds().is_none(),
            "do not show a partial total"
        );
    }

    #[test]
    fn failures_advance_once_and_preserve_completed_fraction() {
        let mut q = queue();
        q.begin(1000);
        q.batch.as_mut().unwrap().fraction = 0.5;
        assert!(q.advance(&Err("decode failed".into()), 1500));
        let b = q.batch.as_ref().unwrap();
        assert_eq!(b.index, 1);
        assert_eq!(b.offset_ms, 1500);
        assert_eq!(b.completed_fractions, [0.5]);
        assert_eq!(b.fraction, 0.0);
        assert!(b.failures[0].contains("a.wav: decode failed"));
        assert!(q.advance(&Ok(Completion::IncompleteFile), 2000));
        assert_eq!(
            q.batch.as_ref().unwrap().incomplete,
            [PathBuf::from("b.wav")]
        );
        assert!(!q.advance(&Ok(Completion::Complete), 2500));
        assert!(!q.advance(&Ok(Completion::Complete), 2500));
    }

    #[test]
    fn background_duration_probe_reads_wav_and_tolerates_invalid_audio() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("one-second.wav");
        let invalid = root.path().join("invalid.wav");
        let mut wav = b"RIFF".to_vec();
        wav.extend(32036_u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16_u32.to_le_bytes());
        wav.extend(1_u16.to_le_bytes()); // PCM
        wav.extend(1_u16.to_le_bytes()); // mono
        wav.extend(16000_u32.to_le_bytes());
        wav.extend(32000_u32.to_le_bytes());
        wav.extend(2_u16.to_le_bytes());
        wav.extend(16_u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(32000_u32.to_le_bytes());
        wav.resize(32044, 0);
        std::fs::write(&path, &wav).unwrap();
        std::fs::write(&invalid, "invalid audio").unwrap();
        let mut q = AudioQueue::default();
        q.replace(vec![path.clone(), invalid.clone()]);
        q.move_file(0, 1);
        wait_for_metadata(&mut q);
        assert_eq!(q.files[0].path, invalid);
        assert_eq!(q.files[0].duration, None);
        assert_eq!(q.files[1].path, path);
        assert_eq!(q.files[1].duration, Some(1.0));
        assert_eq!(std::fs::read(path).unwrap(), wav);
    }

    #[test]
    fn readable_time_has_no_estimate_prefix() {
        assert_eq!(time_remaining(12.2), "13 s remaining");
        assert_eq!(time_remaining(720.0), "12 min remaining");
        assert_eq!(time_remaining(5400.0), "1 h 30 min remaining");
    }
}
