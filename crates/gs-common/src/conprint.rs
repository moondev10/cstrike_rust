//! `Con_Printf` / `Con_DPrintf` from crates that have no console handle (movement, studio,
//! prediction): the text queues here and the host prints it, in order, at its next drain.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

static QUEUE: Mutex<Vec<(bool, String)>> = Mutex::new(Vec::new());
static DEVELOPER: AtomicBool = AtomicBool::new(false);

/// The host's `developer != 0`, which `Con_DPrintf` tests at print time.
pub fn set_developer(on: bool) {
    DEVELOPER.store(on, Ordering::Relaxed);
}

/// `Con_Printf`.
pub fn printf(text: &str) {
    QUEUE.lock().unwrap_or_else(PoisonError::into_inner).push((false, text.to_owned()));
}

/// `Con_DPrintf`: dropped unless `developer` is on.
pub fn dprintf(text: &str) {
    if DEVELOPER.load(Ordering::Relaxed) {
        QUEUE.lock().unwrap_or_else(PoisonError::into_inner).push((true, text.to_owned()));
    }
}

/// The queued prints, oldest first, as `(developer_only, text)`.
pub fn drain() -> Vec<(bool, String)> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(PoisonError::into_inner))
}
