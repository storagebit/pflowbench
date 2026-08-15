// keys.rs -- printer-credential commands and the keychain-backed auth
// helpers. Secrets policy (see also main.rs's header): the credential lives
// ONLY in the OS keychain, is never logged -- length and auth kind only --
// and never reaches the frontend; key_status reports presence, nothing more.

use crate::logging;
use prusalink::{Client, PrinterAuth};

// Deliberately NOT renamed with the app: this is the OS-keychain storage key
// for the printer credential. Changing it would orphan every saved key and
// silently break auth until the user re-saves. Branding lives elsewhere.
const KEYRING_SERVICE: &str = "flowbench";

/// `verbose` gates the trace-level entry/exit lines only -- warn/error on an
/// actual problem always logs regardless. Callers on a tight polling loop
/// (printer_status) pass false so 2s-interval polling doesn't spam the
/// console with an identical "resolving credential" / "credential resolved"
/// pair every cycle for the whole duration of a run.
fn auth_from_keyring(mode: &str, user: &str, verbose: bool) -> Result<PrinterAuth, String> {
    if verbose {
        logging::trace(
            "auth",
            format!("resolving credential for mode={mode}{}", if mode == "digest" { format!(" user={user}") } else { String::new() }),
        );
    }
    let entry = keyring::Entry::new(KEYRING_SERVICE, "prusalink").map_err(|e| {
        logging::error("auth", format!("keyring entry: {e}"));
        e.to_string()
    })?;
    // Trim on read as well as on write: a key pasted with a trailing newline
    // produces a malformed header value and the printer answers 401.
    let secret = entry.get_password().map_err(|_| {
        logging::warn("auth", "no credential stored in keychain".to_string());
        "no credential stored -- open Settings and save the printer key first".to_string()
    })?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        logging::warn("auth", "stored credential is empty".to_string());
        return Err("stored credential is empty -- re-save the printer key".into());
    }
    if verbose {
        logging::trace("auth", format!("credential resolved ({} chars)", secret.len()));
    }
    Ok(match mode {
        "digest" => PrinterAuth::Digest { user: user.to_string(), pass: secret },
        _ => PrinterAuth::ApiKey(secret),
    })
}

/// `verbose` also gates whether the request-level prusalink logger (every
/// "-> METHOD path" / "<- status") gets wired in at all -- see
/// auth_from_keyring's doc comment for why polling needs this off.
pub(crate) fn prusalink_client(addr: &str, mode: &str, user: &str, verbose: bool) -> Result<Client, String> {
    let mut c = Client::new(addr, auth_from_keyring(mode, user, verbose)?);
    if verbose {
        c.set_logger(logging::sink("prusalink"));
    }
    Ok(c)
}

#[tauri::command]
pub(crate) async fn key_save(secret: String) -> Result<(), String> {
    let s = secret.trim();
    logging::trace("cmd:key_save", format!("called ({} chars, {} after trim)", secret.len(), s.len()));
    if s.is_empty() {
        logging::warn("cmd:key_save", "empty credential, nothing saved".to_string());
        return Err("empty credential -- nothing saved".into());
    }
    // Off the main thread: see key_status for why a blocking keychain call
    // there freezes the whole app.
    let s_owned = s.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        keyring::Entry::new(KEYRING_SERVICE, "prusalink").and_then(|e| e.set_password(&s_owned))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        logging::error("cmd:key_save", format!("keyring write failed: {e}"));
        e.to_string()
    })?;
    // Read back through a FRESH Entry. A keyring build with no platform backend
    // silently accepts writes into a per-Entry mock store; this catches that,
    // and any other case where the write did not actually persist.
    let readback = tauri::async_runtime::spawn_blocking(|| {
        keyring::Entry::new(KEYRING_SERVICE, "prusalink").and_then(|e| e.get_password())
    })
    .await
    .map_err(|e| e.to_string())?
        .map_err(|e| {
            logging::error("cmd:key_save", format!("keyring read-back failed: {e}"));
            format!("saved but could not read back: {e}")
        })?;
    if readback.trim() != s {
        logging::error("cmd:key_save", "keychain read-back did not match what was written".to_string());
        return Err("keychain returned a different value on read-back".into());
    }
    logging::info("cmd:key_save", format!("credential stored and read back OK ({} chars)", s.len()));
    Ok(())
}

/// `async` on purpose, and the keychain read is pushed onto a blocking thread.
///
/// A synchronous `#[tauri::command]` runs on the main thread, and a keychain
/// read is not a quick local lookup: macOS can put up an access-authorisation
/// dialog and block until it is answered. Doing that on the main thread wedges
/// the whole IPC bridge -- the UI stops mid-init, every later command queues
/// behind it, and because the on-screen console is built by that same script,
/// the app looks simply dead with nothing logged anywhere. This is called
/// during frontend init, so it was exactly that failure.
#[tauri::command]
pub(crate) async fn key_status() -> bool {
    logging::trace("cmd:key_status", "reading keychain (may prompt on macOS)".to_string());
    let has = tauri::async_runtime::spawn_blocking(|| {
        keyring::Entry::new(KEYRING_SERVICE, "prusalink")
            .and_then(|e| e.get_password())
            .is_ok()
    })
    .await
    .unwrap_or(false);
    logging::trace("cmd:key_status", format!("keychain entry present: {has}"));
    has
}

#[tauri::command]
pub(crate) async fn key_clear() -> Result<(), String> {
    logging::trace("cmd:key_clear", "called".to_string());
    let r = tauri::async_runtime::spawn_blocking(|| {
        keyring::Entry::new(KEYRING_SERVICE, "prusalink").and_then(|e| e.delete_credential())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string());
    match &r {
        Ok(_) => logging::info("cmd:key_clear", "credential cleared".to_string()),
        Err(e) => logging::error("cmd:key_clear", e.clone()),
    }
    r
}
