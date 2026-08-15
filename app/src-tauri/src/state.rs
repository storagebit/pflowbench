// state.rs -- shared app state and the small helpers every command module
// leans on. AppState is Tauri-managed and reaches commands as State<AppState>;
// nothing in this file talks to the printer or the camera itself.

use flowcore::Capture;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

pub(crate) struct AppState {
    pub(crate) capture: Mutex<Option<Capture>>,
    /// Last printer.state seen by printer_status, so it can log only on
    /// transition instead of on every 2s poll -- see printer_status itself.
    pub(crate) last_printer_state: Mutex<Option<String>>,
    /// Shared with the flowcore capture thread's band-change hook, which
    /// grabs a still at each flow-band boundary.
    pub(crate) camera: Arc<Mutex<Option<flowcam::Camera>>>,
    /// Where this run's per-band stills are written; set by capture_start.
    pub(crate) run_dir: Arc<Mutex<Option<PathBuf>>>,
    /// The last stopped run, kept so Export still works after Stop.
    pub(crate) last_run: Mutex<Option<flowcore::Snapshot>>,
    /// TestObject layout of the last generated G-code: (cylinder diameter,
    /// per-cylinder (temp, x, y) in print order). Vision calibration pairs
    /// the user's image clicks with these bed positions.
    pub(crate) last_layout: Mutex<Option<(f64, Vec<(i64, f64, f64)>)>>,
}

/// Clears an AtomicBool on drop -- panic-safe busy flag for worker threads.
pub(crate) struct DropFlag(pub(crate) Arc<std::sync::atomic::AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn expand(p: &str) -> String {
    let p = p.trim();
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(h) => format!("{h}/{rest}"),
            Err(_) => p.to_string(),
        },
        None => p.to_string(),
    }
}
