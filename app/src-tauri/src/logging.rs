// logging.rs -- unified trace/info/warn/error log for the Tauri backend.
//
// Every call here does two things: print a line to stderr (visible under
// `cargo tauri dev`) and broadcast a `backend-log` event to the webview, so
// backend activity -- HTTP requests, the Digest challenge/retry flow, UDP
// capture milestones, command entry/exit -- lands in the SAME on-screen
// console as the frontend's own trace/info/warn/error lines in app/ui, at
// the same four levels, filterable and copyable together.
//
// NEVER pass secret material (the PrusaLink API key or Digest password) into
// these functions, not even truncated -- length and auth *kind* only. See
// crates/prusalink's own logger, which already enforces this at its layer;
// this module is the second half of that same contract for command-level logs.

use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter};

static APP: OnceLock<AppHandle> = OnceLock::new();

/// Call once, from the Tauri `.setup()` hook, before any command can fire.
pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

#[derive(serde::Serialize, Clone)]
struct LogEvent {
    level: &'static str,
    target: String,
    msg: String,
}

fn emit(level: &'static str, target: impl Into<String>, msg: impl Into<String>) {
    let (target, msg) = (target.into(), msg.into());
    eprintln!("[{:<5}] {:<22} {}", level.to_uppercase(), target, msg);
    if let Some(app) = APP.get() {
        let _ = app.emit("backend-log", LogEvent { level, target, msg });
    }
}

pub fn trace(target: impl Into<String>, msg: impl Into<String>) {
    emit("trace", target, msg)
}
pub fn info(target: impl Into<String>, msg: impl Into<String>) {
    emit("info", target, msg)
}
pub fn warn(target: impl Into<String>, msg: impl Into<String>) {
    emit("warn", target, msg)
}
pub fn error(target: impl Into<String>, msg: impl Into<String>) {
    emit("error", target, msg)
}

/// Log a line that originated in the webview.
///
/// Without this, a JavaScript exception during frontend init is invisible
/// outside the app window -- the page simply stops running and the on-screen
/// console it would have reported into never gets built. Routing frontend
/// errors back through stderr means a broken UI leaves a trace somewhere a
/// terminal can see.
pub fn from_ui(level: &str, target: impl Into<String>, msg: impl Into<String>) {
    let level: &'static str = match level {
        "error" => "error",
        "warn" => "warn",
        "info" => "info",
        _ => "trace",
    };
    emit(level, target, msg)
}

/// Frontend-originated log line. Chiefly so an uncaught exception during UI
/// init -- which kills the on-screen console before it can report anything --
/// still reaches stderr.
#[tauri::command]
pub(crate) fn ui_log(level: String, target: String, msg: String) {
    from_ui(&level, format!("ui:{target}"), msg);
}

/// A (level, message) sink bound to `target`, for flowcore/prusalink's
/// pluggable logger hooks -- this is the one place those dependency-free
/// crates' generic callback gets wired into the app's log bus.
pub fn sink(target: &'static str) -> Arc<dyn Fn(&'static str, String) + Send + Sync> {
    Arc::new(move |level, msg| emit(level, target, msg))
}

/// Push an arbitrary event to the webview. Shares the `AppHandle` captured by
/// `init`, so callers on background threads (e.g. flowcore's capture thread)
/// can reach the UI without threading a handle through every layer.
/// Silently no-ops before `init` -- the UI isn't listening yet either.
pub fn emit_event<T: serde::Serialize + Clone>(name: &str, payload: T) {
    if let Some(app) = APP.get() {
        let _ = app.emit(name, payload);
    }
}
