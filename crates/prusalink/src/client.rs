// Client -- the PrusaLink v1 endpoints over the raw HTTP layer, with
// X-Api-Key or Digest auth (on 401, answer the challenge and retry once)
// and the pluggable logging hook.

use std::time::Duration;

use crate::digest::{digest_authorization, parse_challenge};
use crate::http::{raw_request, Response};

// ---------------------------------------------------------------- logging hook

/// (level, message) sink for trace/info/warn/error events from this client --
/// level is one of "trace"/"info"/"warn"/"error", matching the on-screen
/// console in app/ui. The default (from `Client::new`) is a silent no-op, so
/// this crate's own tests stay independent of any log sink; the app layer
/// wires in a real one via `set_logger`.
///
/// NEVER pass the API key or Digest password (or anything derived from
/// checking them) into this sink -- only method/path/status/byte-counts and
/// the auth *kind*.
pub type LogFn = std::sync::Arc<dyn Fn(&'static str, String) + Send + Sync>;

fn noop_logger() -> LogFn {
    std::sync::Arc::new(|_level, _msg| {})
}

// ---------------------------------------------------------------- client

#[derive(Clone, Debug)]
pub enum PrinterAuth {
    /// PrusaLink local API key (X-Api-Key). NOT the PrusaConnect key.
    ApiKey(String),
    /// Digest username/password from the printer's PrusaLink settings screen.
    Digest { user: String, pass: String },
}

impl PrinterAuth {
    fn kind(&self) -> &'static str {
        match self {
            PrinterAuth::ApiKey(_) => "apikey",
            PrinterAuth::Digest { .. } => "digest",
        }
    }
}

pub struct Client {
    pub addr: String, // "192.0.2.2" or "192.0.2.2:80"
    pub auth: PrinterAuth,
    pub timeout: Duration,
    nc: u32,
    logger: LogFn,
}

impl Client {
    pub fn new(addr: &str, auth: PrinterAuth) -> Client {
        let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:80") };
        Client { addr, auth, timeout: Duration::from_secs(20), nc: 0, logger: noop_logger() }
    }

    /// Wire a (level, message) sink for every request this client makes.
    pub fn set_logger(&mut self, logger: LogFn) {
        self.logger = logger;
    }

    fn request(
        &mut self,
        method: &str,
        path: &str,
        extra: &[(String, String)],
        body: &[u8],
    ) -> std::io::Result<Response> {
        let mut headers: Vec<(String, String)> = extra.to_vec();
        (self.logger)("trace", format!(
            "-> {method} {path} (auth={}, {} byte body)", self.auth.kind(), body.len()
        ));
        let result = match &self.auth {
            PrinterAuth::ApiKey(k) => {
                headers.push(("X-Api-Key".into(), k.clone()));
                raw_request(&self.addr, method, path, &headers, body, self.timeout)
            }
            PrinterAuth::Digest { user, pass } => {
                // First attempt unauthenticated; on 401, answer the challenge and retry once.
                let first = match raw_request(&self.addr, method, path, &headers, body, self.timeout) {
                    Ok(r) => r,
                    Err(e) => {
                        (self.logger)("error", format!("{method} {path} failed: {e}"));
                        return Err(e);
                    }
                };
                if first.status != 401 {
                    (self.logger)("trace", format!("<- {} {method} {path}", first.status));
                    return Ok(first);
                }
                (self.logger)("info", format!("<- 401 {method} {path}, answering the Digest challenge"));
                let ch = match first.header("WWW-Authenticate").and_then(parse_challenge) {
                    Some(ch) => ch,
                    None => {
                        (self.logger)("error", "401 response had no parseable WWW-Authenticate Digest challenge".to_string());
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "401 without digest challenge",
                        ));
                    }
                };
                (self.logger)("trace", format!("digest challenge: realm=\"{}\" qop={:?}", ch.realm, ch.qop));
                self.nc += 1;
                let cnonce = format!("{:016x}", pseudo_random());
                let auth = digest_authorization(&ch, user, pass, method, path, &cnonce, self.nc);
                headers.push(("Authorization".into(), auth));
                (self.logger)("trace", format!("-> {method} {path} (Digest retry, nc={:08x})", self.nc));
                raw_request(&self.addr, method, path, &headers, body, self.timeout)
            }
        };
        match &result {
            Ok(r) => (self.logger)("trace", format!("<- {} {method} {path} ({} bytes)", r.status, r.body.len())),
            Err(e) => (self.logger)("error", format!("{method} {path} failed: {e}")),
        }
        result
    }

    pub fn info(&mut self) -> std::io::Result<Response> {
        self.request("GET", "/api/v1/info", &[], &[])
    }
    pub fn status(&mut self) -> std::io::Result<Response> {
        self.request("GET", "/api/v1/status", &[], &[])
    }
    pub fn job(&mut self) -> std::io::Result<Response> {
        self.request("GET", "/api/v1/job", &[], &[])
    }
    pub fn storage(&mut self) -> std::io::Result<Response> {
        self.request("GET", "/api/v1/storage", &[], &[])
    }
    pub fn stop_job(&mut self, id: u64) -> std::io::Result<Response> {
        self.request("DELETE", &format!("/api/v1/job/{id}"), &[], &[])
    }
    pub fn upload(
        &mut self,
        storage: &str, // "usb" or "local" -- from /api/v1/storage `path`, no leading slash here
        name: &str,
        gcode: &[u8],
        print_after: bool,
        overwrite: bool,
    ) -> std::io::Result<Response> {
        let path = format!("/api/v1/files/{storage}/{name}");
        let extra = vec![
            ("Content-Type".into(), "application/octet-stream".into()),
            ("Print-After-Upload".into(), if print_after { "?1" } else { "?0" }.into()),
            ("Overwrite".into(), if overwrite { "?1" } else { "?0" }.into()),
        ];
        self.request("PUT", &path, &extra, gcode)
    }

    /// Make the printer beep. There is no raw-G-code-command endpoint in the
    /// PrusaLink v1 API (verified against prusa3d/Prusa-Link-Web's
    /// spec/openapi.yaml -- unlike OctoPrint's /api/printer/command, it isn't
    /// there), so this uploads a tiny M300 job to `storage` and prints it
    /// immediately. `storage` should be "usb" -- confirmed directly in the
    /// Buddy firmware source (lib/WUI/link_content/prusa_api_helpers.cpp,
    /// parse_file_url): every method on /api/v1/files/{storage}/... is
    /// gated by a hardcoded `strncmp(storage, "usb", ...)` check, so ANY
    /// other storage name gets an unconditional 403 regardless of what GET
    /// /api/v1/storage reports for it -- it is NOT a per-storage read_only
    /// flag. See the caller (app/src-tauri's `writable_storage`) for how it
    /// picks "usb" when available. The caller is also responsible for
    /// checking /api/v1/status first and skipping this while a real job is
    /// running -- this method doesn't know the printer's current state.
    ///
    /// The compatibility-check block (M862.x / M115) is required. Confirmed
    /// in gcode_info.cpp/gcode_compatibility.cpp: a file with none of it
    /// still passes the (separate, content-blind) file-completeness gate,
    /// but `generate_without_toolmapping()` unconditionally flags
    /// `GeneralCheck::input_shaper` unless M862.6 P"Input shaper" is present
    /// -- default severity Warning, which is the dismissable "not printable"
    /// dialog actually observed. These exact values (0.8mm nozzle,
    /// "COREONEL" model, g-code level 2, Input shaper feature, firmware >=
    /// 6.5.7) were copied verbatim from a real PrusaSlicer export for this
    /// printer (reference/ref.gcode). M115's version check is a minimum, so
    /// it stays valid across future firmware upgrades (and, incidentally,
    /// guarantees firmware >= 6.0.4, the release that fixed a real M300
    /// crash bug -- github.com/prusa3d/Prusa-Firmware-Buddy/issues/3328).
    ///
    /// The trailing `; filament used` / `; estimated printing time`
    /// comments are NOT worth relying on for a clean progress display, and
    /// this is now a confirmed structural fact, not an assumption: Buddy's
    /// `sd_percent_done` (marlin_server.cpp) is either driven by explicit
    /// M73 (we don't emit any) or falls back to byte-position-in-file, and
    /// that value is only recomputed between fully-completed gcode
    /// commands -- which never happens while blocked inside M300's
    /// `GcodeSuite::dwell()`. Once the job reaches FINISHED (EOF-triggered),
    /// `has_job()` goes false and GET /api/v1/job returns bare 204 --
    /// `progress` stops being reported at all rather than ever reading 100.
    /// A near-instant beep job will always read "0% then Finished"; there
    /// is no fix for this short of intentionally slowing the beep down,
    /// which isn't worth it. The comments stay only because they're free
    /// and harmless, not because they solve anything.
    pub fn beep(&mut self, storage: &str) -> std::io::Result<Response> {
        const BEEP_GCODE: &[u8] = b"; pflowbench connectivity beep -- not a real print job\n\
            M862.1 P0.8 A1 F0 ; nozzle check\n\
            M862.3 P \"COREONEL\" ; printer model check\n\
            M862.5 P2 ; g-code level check\n\
            M862.6 P\"Input shaper\" ; FW feature check\n\
            M115 U6.5.7+12836\n\
            M300 S880 P120\n\
            G4 P60\n\
            M300 S1175 P150\n\
            ; filament used [g] = 0.00\n\
            ; estimated printing time (normal mode) = 1s\n";
        self.upload(storage, "pflowbench-beep.gcode", BEEP_GCODE, true, true)
    }

    /// List of configured cameras (PrusaLink v1: GET /api/v1/cameras).
    pub fn cameras(&mut self) -> std::io::Result<Response> {
        self.request("GET", "/api/v1/cameras", &[], &[])
    }

    /// Current still frame from the printer's default camera (PrusaLink v1
    /// spec: GET /api/v1/cameras/snap, image/png body). 204 means no camera
    /// is configured/available -- callers should treat that as "no image",
    /// not an error.
    pub fn camera_snapshot(&mut self) -> std::io::Result<Response> {
        self.request("GET", "/api/v1/cameras/snap", &[], &[])
    }
}

fn pseudo_random() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let mut x = t.as_nanos() as u64 ^ 0x9e3779b97f4a7c15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::{digest_authorization, DigestChallenge};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn mock_server(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for resp in responses {
                let (mut s, _) = l.accept().unwrap();
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).into_owned());
                s.write_all(resp.as_bytes()).unwrap();
            }
            seen
        });
        (addr, h)
    }

    #[test]
    fn apikey_header_and_json_roundtrip() {
        let body = r#"{"name":"CORE One L"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let (addr, h) = mock_server(vec![resp]);
        let mut c = Client::new(&addr, PrinterAuth::ApiKey("TESTKEY".into()));
        let r = c.info().unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body_str(), body);
        let seen = h.join().unwrap();
        assert!(seen[0].contains("X-Api-Key: TESTKEY"));
        assert!(seen[0].starts_with("GET /api/v1/info HTTP/1.1"));
    }

    #[test]
    fn digest_401_challenge_then_retry() {
        let challenge = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"Prusa\", nonce=\"n1\", qop=\"auth\"\r\nContent-Length: 0\r\n\r\n".to_string();
        let ok = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string();
        let (addr, h) = mock_server(vec![challenge, ok]);
        let mut c = Client::new(
            &addr,
            PrinterAuth::Digest { user: "maker".into(), pass: "pw".into() },
        );
        let r = c.status().unwrap();
        assert_eq!(r.status, 200);
        let seen = h.join().unwrap();
        assert!(!seen[0].contains("Authorization"));
        assert!(seen[1].contains("Authorization: Digest username=\"maker\""));
        assert!(seen[1].contains("uri=\"/api/v1/status\""));
        // verify the response hash matches an independent computation
        let ch = DigestChallenge { realm: "Prusa".into(), nonce: "n1".into(), qop: Some("auth".into()), opaque: None };
        let line = seen[1].lines().find(|l| l.starts_with("Authorization")).unwrap();
        let cnonce = line.split("cnonce=\"").nth(1).unwrap().split('"').next().unwrap();
        let expect = digest_authorization(&ch, "maker", "pw", "GET", "/api/v1/status", cnonce, 1);
        let want = expect.split("response=\"").nth(1).unwrap().split('"').next().unwrap();
        assert!(line.contains(want));
    }

    #[test]
    fn upload_sets_prusalink_headers() {
        let ok = "HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n".to_string();
        let (addr, h) = mock_server(vec![ok]);
        let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
        let r = c.upload("usb", "flowcliff.gcode", b"G28\n", false, true).unwrap();
        assert_eq!(r.status, 201);
        let seen = h.join().unwrap();
        assert!(seen[0].starts_with("PUT /api/v1/files/usb/flowcliff.gcode HTTP/1.1"));
        assert!(seen[0].contains("Print-After-Upload: ?0"));
        assert!(seen[0].contains("Overwrite: ?1"));
        assert!(seen[0].contains("Content-Length: 4"));
        assert!(seen[0].ends_with("G28\n"));
    }

    #[test]
    fn camera_snapshot_hits_the_default_camera_endpoint() {
        // mock_server's helper takes Vec<String>, so fake body content here --
        // real PNG magic bytes (0x89 0x50...) aren't valid UTF-8 on their own
        // and would get mangled by the String round-trip. The point of this
        // test is the request path and that Client::request's body/status
        // plumbing works, not real image parsing.
        let body = "not-a-real-png-but-thats-fine-for-this-test";
        let ok = format!("HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
        let (addr, h) = mock_server(vec![ok]);
        let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
        let r = c.camera_snapshot().unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body_str(), body);
        let seen = h.join().unwrap();
        assert!(seen[0].starts_with("GET /api/v1/cameras/snap HTTP/1.1"));
    }

    #[test]
    fn beep_uploads_a_tiny_m300_job_to_the_given_storage() {
        let ok = "HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n".to_string();
        let (addr, h) = mock_server(vec![ok]);
        let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
        let r = c.beep("usb").unwrap(); // "local" is read-only on some printers -- caller picks the storage
        assert_eq!(r.status, 201);
        let seen = h.join().unwrap();
        assert!(seen[0].starts_with("PUT /api/v1/files/usb/pflowbench-beep.gcode HTTP/1.1"));
        assert!(seen[0].contains("Print-After-Upload: ?1"), "beep must actually play, not just sit uploaded");
        assert!(seen[0].contains("M300"));
        // the compatibility block Buddy firmware requires to accept the file as printable
        assert!(seen[0].contains("M862.1 P0.8"), "missing nozzle check -- firmware will refuse the file");
        assert!(seen[0].contains("M862.3 P \"COREONEL\""), "missing printer model check -- firmware will refuse the file");
        assert!(seen[0].contains("M115 U"), "missing firmware version check -- firmware will refuse the file");
        // without these the job runs fine but the printer's progress display has
        // nothing to compute a percentage against
        assert!(seen[0].contains("; estimated printing time"), "missing time estimate -- progress display can look stuck");
        assert!(seen[0].contains("; filament used"));
    }

    #[test]
    fn logger_hook_sees_requests_and_never_the_secret() {
        let seen: std::sync::Arc<std::sync::Mutex<Vec<(&'static str, String)>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        let logger: LogFn = std::sync::Arc::new(move |level, msg| sink.lock().unwrap().push((level, msg)));

        let challenge = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"Prusa\", nonce=\"n1\", qop=\"auth\"\r\nContent-Length: 0\r\n\r\n".to_string();
        let ok = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string();
        let (addr, h) = mock_server(vec![challenge, ok]);
        let mut c = Client::new(&addr, PrinterAuth::Digest { user: "maker".into(), pass: "correct horse battery staple".into() });
        c.set_logger(logger);
        c.status().unwrap();
        h.join().unwrap();

        let logged = seen.lock().unwrap();
        assert!(logged.iter().any(|(_, m)| m.contains("-> GET /api/v1/status")));
        assert!(logged.iter().any(|(_, m)| m.contains("401")));
        assert!(logged.iter().any(|(_, m)| m.contains("<- 200")));
        for (_, m) in logged.iter() {
            assert!(!m.contains("correct horse battery staple"), "logged the password: {m}");
            assert!(!m.contains("maker"), "logged the username: {m}"); // kind() only, not user/pass
        }
    }

    #[test]
    fn chunked_bodies_are_decoded() {
        let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n".to_string();
        let (addr, h) = mock_server(vec![resp]);
        let mut c = Client::new(&addr, PrinterAuth::ApiKey("K".into()));
        let r = c.info().unwrap();
        assert_eq!(r.body_str(), "Wikipedia");
        h.join().unwrap();
    }
}
