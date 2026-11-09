//! Shared progress state for background work.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct Progress {
    /// i18n key of the current stage
    pub stage: Mutex<&'static str>,
    pub current: Mutex<String>,
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub bytes: AtomicU64,
    pub bytes_total: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn stage(&self, key: &'static str, total: u64) {
        *self.stage.lock().unwrap() = key;
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }
    pub fn tick(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
    pub fn set_current(&self, s: impl Into<String>) {
        *self.current.lock().unwrap() = s.into();
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn fraction(&self) -> Option<f32> {
        let bt = self.bytes_total.load(Ordering::Relaxed);
        if bt > 0 {
            return Some((self.bytes.load(Ordering::Relaxed) as f64 / bt as f64).min(1.0) as f32);
        }
        let t = self.total.load(Ordering::Relaxed);
        (t > 0).then(|| (self.done.load(Ordering::Relaxed) as f64 / t as f64).min(1.0) as f32)
    }
}

/// Result of a finished operation, shown in a dialog.
#[derive(Default, Debug, Clone)]
pub struct Report {
    pub ok_files: u64,
    pub ok_bytes: u64,
    pub errors: Vec<(String, String)>,
    pub cancelled: bool,
    pub open_path: Option<std::path::PathBuf>,
    /// indices of items that were handled successfully
    pub done_idx: Vec<usize>,
}
