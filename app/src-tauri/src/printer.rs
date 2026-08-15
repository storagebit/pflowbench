// printer.rs -- PrusaLink commands: info / status / storage / upload / beep,
// plus the fail-closed job/printer busy checks that keep a UI-triggered
// upload or beep from ever stepping on a running print.

use crate::keys::prusalink_client;
use crate::logging;
use crate::state::AppState;
use prusalink::Client;
use tauri::State;

#[tauri::command]
pub(crate) fn printer_info(addr: String, mode: String, user: String) -> Result<String, String> {
    logging::trace("cmd:printer_info", format!("addr={addr} mode={mode}"));
    let mut c = prusalink_client(&addr, &mode, &user, true)?;
    let r = c.info().map_err(|e| {
        logging::error("cmd:printer_info", e.to_string());
        e.to_string()
    })?;
    if r.status == 200 {
        Ok(r.body_str())
    } else {
        logging::warn("cmd:printer_info", format!("HTTP {}", r.status));
        Err(format!("HTTP {}: {}", r.status, r.body_str()))
    }
}

/// Polled every 2s by the frontend while a capture is running (a 15-minute
/// capture is ~450 polls) -- like capture_delta, per-call tracing here would
/// flood the console with near-identical "GET /api/v1/status -> 200" noise
/// for the whole run and drown out anything that actually matters. Skip
/// verbose per-request logging (prusalink_client(..., false)) and instead
/// log only when the reported printer.state actually changes -- READY ->
/// PRINTING -> FINISHED is the signal a calibration run's progress needs,
/// not the polling mechanics. Errors are still always logged.
#[tauri::command]
pub(crate) fn printer_status(state: State<AppState>, addr: String, mode: String, user: String) -> Result<String, String> {
    let mut c = prusalink_client(&addr, &mode, &user, false)?;
    let r = c.status().map_err(|e| {
        logging::warn("printer", format!("status poll failed: {e}"));
        e.to_string()
    })?;
    if r.status != 200 {
        logging::warn("printer", format!("status poll: HTTP {}", r.status));
        return Err(format!("HTTP {}", r.status));
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r.body_str()) {
        if let Some(new_state) = v.get("printer").and_then(|p| p.get("state")).and_then(|s| s.as_str()) {
            let mut last = state.last_printer_state.lock().unwrap();
            if last.as_deref() != Some(new_state) {
                logging::info("printer", format!("state: {} -> {new_state}", last.as_deref().unwrap_or("(first poll)")));
                *last = Some(new_state.to_string());
            }
        }
    }
    Ok(r.body_str())
}

#[tauri::command]
pub(crate) fn printer_storage(addr: String, mode: String, user: String) -> Result<String, String> {
    logging::trace("cmd:printer_storage", format!("addr={addr} mode={mode}"));
    let mut c = prusalink_client(&addr, &mode, &user, true)?;
    let r = c.storage().map_err(|e| {
        logging::error("cmd:printer_storage", e.to_string());
        e.to_string()
    })?;
    if r.status == 200 {
        Ok(r.body_str())
    } else {
        logging::warn("cmd:printer_storage", format!("HTTP {}", r.status));
        Err(format!("HTTP {}", r.status))
    }
}

#[tauri::command]
pub(crate) fn printer_upload(
    addr: String,
    mode: String,
    user: String,
    storage: String,
    name: String,
    gcode_path: String,
    print_after: bool,
) -> Result<String, String> {
    let raw = gcode_path.trim();
    logging::trace(
        "cmd:printer_upload",
        format!("addr={addr} mode={mode} storage={storage} name={name} path={raw} print_after={print_after}"),
    );
    if raw.is_empty() {
        logging::warn("cmd:printer_upload", "no G-code path given".to_string());
        return Err("no G-code path set -- fill in the file field".into());
    }
    // accept ~/... as typed
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        match std::env::var("HOME") {
            Ok(h) => format!("{h}/{rest}"),
            Err(_) => raw.to_string(),
        }
    } else {
        raw.to_string()
    };
    let bytes = std::fs::read(&path).map_err(|e| {
        logging::error("cmd:printer_upload", format!("read {path}: {e}"));
        format!("{path}: {e}")
    })?;
    if bytes.is_empty() {
        logging::warn("cmd:printer_upload", format!("{path} is empty"));
        return Err(format!("{path} is empty"));
    }
    logging::trace("cmd:printer_upload", format!("read {} bytes from {path}", bytes.len()));
    let storage = storage.trim().trim_matches('/');
    let name = name.trim();
    let mut c = prusalink_client(addr.trim(), &mode, &user, true)?;
    let r = c
        .upload(storage, name, &bytes, print_after, true)
        .map_err(|e| {
            logging::error("cmd:printer_upload", e.to_string());
            e.to_string()
        })?;
    match r.status {
        200 | 201 | 204 => {
            logging::info(
                "cmd:printer_upload",
                format!("{} bytes -> /{storage}/{name} (HTTP {})", bytes.len(), r.status),
            );
            Ok(format!("{} bytes -> /{}/{} (HTTP {})", bytes.len(), storage, name, r.status))
        }
        s => {
            logging::warn("cmd:printer_upload", format!("HTTP {s}"));
            Err(format!("HTTP {s}: {}", r.body_str()))
        }
    }
}

/// Make the printer beep, but only if it isn't in the middle of a job --
/// there's no raw-G-code endpoint in PrusaLink's API, so this rides on the
/// same upload+print-after mechanism as a real job (see prusalink::Client::beep),
/// and a beep-print is exactly the kind of thing that should never step on
/// an actual print in progress.
#[tauri::command]
pub(crate) fn printer_beep(addr: String, mode: String, user: String) -> Result<String, String> {
    logging::trace("cmd:printer_beep", format!("addr={addr} mode={mode}"));
    let mut c = prusalink_client(&addr, &mode, &user, true)?;

    if let Some(reason) = job_or_printer_busy(&mut c)? {
        logging::info("cmd:printer_beep", format!("skipped -- {reason}"));
        return Ok(format!("skipped beep ({reason})"));
    }

    let storage = writable_storage(&mut c).map_err(|e| {
        logging::warn("cmd:printer_beep", format!("skipped -- {e}"));
        e
    })?;
    logging::trace("cmd:printer_beep", format!("using storage={storage}"));
    let r = c.beep(&storage).map_err(|e| {
        logging::error("cmd:printer_beep", e.to_string());
        e.to_string()
    })?;
    match r.status {
        200 | 201 | 204 => {
            logging::info("cmd:printer_beep", "beep sent".to_string());
            Ok("beep sent".into())
        }
        s => {
            logging::warn("cmd:printer_beep", format!("HTTP {s}"));
            Err(format!("HTTP {s}: {}", r.body_str()))
        }
    }
}

/// Two independent, fail-closed checks against the real PrusaLink v1 spec
/// (prusa3d/Prusa-Link-Web spec/openapi.yaml) -- "fail-closed" meaning any
/// ambiguity (bad status, unparseable body, unrecognized state) skips the
/// beep rather than risking it. Returns Some(reason) if busy, None if free.
///
/// - GET /api/v1/job: per spec, 204 No Content is the unambiguous "no job
///   is active" response (not 404, not an empty JSON object). A 200 carries
///   a job object whose own `state` enum is PRINTING/PAUSED/FINISHED/
///   STOPPED/ERROR -- narrower than printer.state, and the most direct
///   signal for "is something actually printing right now."
/// - GET /api/v1/status `printer.state`: the fuller enum is IDLE/BUSY/
///   PRINTING/PAUSED/FINISHED/STOPPED/ERROR/ATTENTION/READY. This catches
///   printer-level conditions (BUSY, ATTENTION) that aren't tied to a job
///   at all, so wouldn't show up in the /job check above. Allowlisted, not
///   blocklisted: only IDLE/READY/FINISHED/STOPPED are treated as free.
fn job_or_printer_busy(c: &mut Client) -> Result<Option<String>, String> {
    let jr = c.job().map_err(|e| e.to_string())?;
    match jr.status {
        204 => {} // spec-guaranteed "no job active" -- proceed to the status check
        200 => {
            let v: serde_json::Value =
                serde_json::from_str(&jr.body_str()).map_err(|e| format!("job status: {e}"))?;
            let state = v.get("state").and_then(|s| s.as_str()).unwrap_or("").to_uppercase();
            if matches!(state.as_str(), "PRINTING" | "PAUSED") {
                return Ok(Some(format!("a job is active (state={state})")));
            }
        }
        s => return Ok(Some(format!("could not confirm job status (HTTP {s})"))),
    }

    let sr = c.status().map_err(|e| e.to_string())?;
    if sr.status != 200 {
        return Ok(Some(format!("could not confirm printer status (HTTP {})", sr.status)));
    }
    let v: serde_json::Value =
        serde_json::from_str(&sr.body_str()).map_err(|e| format!("printer status: {e}"))?;
    let state = v
        .get("printer")
        .and_then(|p| p.get("state"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_uppercase();
    let free = matches!(state.as_str(), "IDLE" | "READY" | "FINISHED" | "STOPPED");
    if !free {
        return Ok(Some(format!("printer state is {state}")));
    }
    Ok(None)
}

/// Storage to upload the beep to. Prefers "usb" outright: per the actual
/// Buddy firmware source (lib/WUI/link_content/prusa_api_helpers.cpp,
/// parse_file_url) every method on /api/v1/files/{storage}/... is gated by
/// a hardcoded `strncmp(storage, "usb", ...)` check -- ANY other storage
/// segment ("local" included) gets an unconditional 403, on every firmware
/// version that matches public source. That 403 isn't a per-storage
/// `read_only` flag at all; it's a flat allowlist of exactly one name. Public
/// source's GET /api/v1/storage also only ever emits that one hardcoded
/// "usb" entry -- this printer's actual response includes more than that
/// (confirmed empirically), so its firmware has diverged from what's on
/// GitHub; the `read_only`/`available` fields it reports for those extra
/// entries can't be trusted to predict what upload will accept. So: try
/// "usb" first if the discovered list has it, and only fall back to
/// scanning for another available+non-read-only entry if it doesn't --
/// matching what's actually enforced rather than trusting the survey.
fn writable_storage(c: &mut Client) -> Result<String, String> {
    let sr = c.storage().map_err(|e| e.to_string())?;
    if sr.status != 200 {
        return Err(format!("storage list: HTTP {}", sr.status));
    }
    let v: serde_json::Value = serde_json::from_str(&sr.body_str()).map_err(|e| format!("storage list: {e}"))?;
    let list = v
        .get("storage_list")
        .and_then(|s| s.as_array())
        .ok_or_else(|| "storage list: unexpected response shape".to_string())?;
    let path_of = |entry: &serde_json::Value| {
        // storage `path` comes back wrapped in slashes (e.g. "/usb/");
        // upload() puts it between two literal slashes itself, so a
        // literal copy here would double up and 403/404 the request.
        entry.get("path").and_then(|p| p.as_str()).map(|p| p.trim_matches('/').to_string())
    };
    if list.iter().any(|e| path_of(e).as_deref() == Some("usb")) {
        return Ok("usb".to_string());
    }
    for entry in list {
        let available = entry.get("available").and_then(|a| a.as_bool()).unwrap_or(false);
        let read_only = entry.get("read_only").and_then(|r| r.as_bool()).unwrap_or(true);
        if available && !read_only {
            if let Some(path) = path_of(entry) {
                return Ok(path);
            }
        }
    }
    Err("no available, writable storage found on the printer".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prusalink::PrinterAuth;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Same pattern prusalink's own tests use: a one-shot mock HTTP server
    /// that replies with a fixed, ordered list of raw responses.
    fn mock_server(responses: Vec<String>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for resp in responses {
                let (mut s, _) = l.accept().unwrap();
                let mut buf = [0u8; 8192];
                let _ = s.read(&mut buf).unwrap();
                s.write_all(resp.as_bytes()).unwrap();
            }
        });
        addr
    }

    fn status_resp(state: &str) -> String {
        let body = format!(r#"{{"printer":{{"state":"{state}"}}}}"#);
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body)
    }
    const NO_JOB: &str = "HTTP/1.1 204 No Content\r\n\r\n";
    fn job_resp(state: &str) -> String {
        let body = format!(r#"{{"id":1,"state":"{state}","progress":0,"time_printing":0}}"#);
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body)
    }

    #[test]
    fn free_when_no_job_and_printer_idle() {
        let addr = mock_server(vec![NO_JOB.to_string(), status_resp("IDLE")]);
        let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
        assert_eq!(job_or_printer_busy(&mut c).unwrap(), None);
    }

    #[test]
    fn free_when_no_job_and_printer_ready_or_finished_or_stopped() {
        for state in ["READY", "FINISHED", "STOPPED"] {
            let addr = mock_server(vec![NO_JOB.to_string(), status_resp(state)]);
            let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
            assert_eq!(job_or_printer_busy(&mut c).unwrap(), None, "state={state} should be free");
        }
    }

    #[test]
    fn busy_when_a_job_is_printing_or_paused() {
        for state in ["PRINTING", "PAUSED"] {
            let addr = mock_server(vec![job_resp(state)]);
            let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
            let reason = job_or_printer_busy(&mut c).unwrap();
            assert!(reason.is_some(), "state={state} should be busy");
            assert!(reason.unwrap().contains(state));
        }
    }

    #[test]
    fn busy_when_no_job_but_printer_state_is_not_allowlisted() {
        // BUSY/ATTENTION/ERROR aren't job states at all -- only printer.state
        // catches them, which is exactly why both checks exist.
        for state in ["BUSY", "ATTENTION", "ERROR", "PRINTING", "PAUSED"] {
            let addr = mock_server(vec![NO_JOB.to_string(), status_resp(state)]);
            let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
            let reason = job_or_printer_busy(&mut c).unwrap();
            assert!(reason.is_some(), "printer.state={state} should be busy");
        }
    }

    #[test]
    fn fails_closed_on_an_unrecognized_job_status_code() {
        // Anything other than the spec's 204 (no job) / 200 (job object) is
        // treated as "can't confirm it's safe" -- skip, don't guess.
        let addr = mock_server(vec!["HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_string()]);
        let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
        let reason = job_or_printer_busy(&mut c).unwrap();
        assert!(reason.is_some(), "an unexpected /api/v1/job status must fail closed");
    }

    #[test]
    fn fails_closed_on_an_unparseable_printer_status_body() {
        let bad_status = "HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nnot json!";
        let addr = mock_server(vec![NO_JOB.to_string(), bad_status.to_string()]);
        let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
        assert!(job_or_printer_busy(&mut c).is_err(), "unparseable status body should error, not silently proceed");
    }
}
