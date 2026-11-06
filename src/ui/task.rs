//! One-shot operations on background threads; no GTK objects cross this boundary.
use std::sync::mpsc::{self, Receiver};

pub(super) fn background<T: Send + 'static>(
    operation: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, events) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = tx.send(operation());
    });
    events
}
