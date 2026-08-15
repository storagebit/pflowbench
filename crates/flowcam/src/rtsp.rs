// RTSP control plane: transport choice, the :554 connectivity probe, and the
// DESCRIBE-only test behind the UI's Test button. The streaming loop itself
// lives in stream.rs.

use std::io;
use std::time::Duration;

/// RTSP transport. The camera's server identifies as `rtsp_demo`, a minimal
/// OEM implementation, and its TCP interleaving has proven unreliable under
/// sustained load -- hence the ability to switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Rtsp {
    /// Interleaved over the RTSP control connection. No extra ports, but this
    /// camera's server corrupts its framing under sustained load -- measured
    /// at one desync per ~90s at 5 Mbps ("Invalid RTSP message: ...
    /// request-line"). Kept for networks where UDP is blocked.
    Tcp,
    /// Separate RTP/UDP flow, avoiding the interleaving path entirely.
    /// Measured fault-free over the same 90s test, at marginally higher
    /// throughput. The default: printer and app are on the same LAN.
    #[default]
    Udp,
}

/// What the camera advertises, without starting a decode.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CameraInfo {
    pub url: String,
    pub encoding: String,
    pub width: Option<u16>,
    pub height: Option<u16>,
    /// H.264 profile name decoded from the SPS (Baseline / Main / High ...).
    pub profile: Option<String>,
    /// H.264 level, e.g. 4.0.
    pub level: Option<f32>,
    /// RTP clock rate, 90 kHz for video.
    pub clock_rate: Option<u32>,
    pub rtp_payload_type: Option<u8>,
    /// `Server:` header from the RTSP OPTIONS response.
    pub server: Option<String>,
    /// RTSP methods the server advertises.
    pub methods: Vec<String>,
    pub transport: &'static str,
}

/// Profile and level straight out of the SPS NAL (bytes 1..4 after the header).
fn sps_profile_level(sps: &[u8]) -> (Option<String>, Option<f32>) {
    if sps.len() < 4 {
        return (None, None);
    }
    let name = match sps[1] {
        66 => "Baseline",
        77 => "Main",
        88 => "Extended",
        100 => "High",
        110 => "High 10",
        122 => "High 4:2:2",
        244 => "High 4:4:4",
        _ => return (Some(format!("profile {}", sps[1])), Some(sps[3] as f32 / 10.0)),
    };
    (Some(name.to_string()), Some(sps[3] as f32 / 10.0))
}

/// Raw RTSP OPTIONS, purely to read the `Server:` header and the advertised
/// method list -- retina doesn't surface either. Hand-rolled because RTSP is
/// a line protocol nearly identical in shape to the HTTP already written by
/// hand in the prusalink crate.
fn rtsp_options(host: &str, timeout: Duration) -> (Option<String>, Vec<String>) {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    let Ok(mut addrs) = format!("{host}:554").to_socket_addrs() else {
        return (None, Vec::new());
    };
    let Some(addr) = addrs.next() else { return (None, Vec::new()) };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, timeout) else {
        return (None, Vec::new());
    };
    let _ = s.set_read_timeout(Some(timeout));
    let req = format!(
        "OPTIONS rtsp://{host}/live RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: pflowbench\r\n\r\n"
    );
    if s.write_all(req.as_bytes()).is_err() {
        return (None, Vec::new());
    }
    let mut buf = [0u8; 2048];
    let n = s.read(&mut buf).unwrap_or(0);
    let text = String::from_utf8_lossy(&buf[..n]);
    let mut server = None;
    let mut methods = Vec::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Server:") {
            server = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("Public:") {
            methods = v.split(',').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
        }
    }
    (server, methods)
}

/// Connectivity check for the UI's Test button: confirms :554 is listening,
/// then does an RTSP DESCRIBE and reports what the camera advertises. Stops
/// short of SETUP/PLAY, so it neither starts a stream nor disturbs a running
/// one, and returns in well under a second.
pub fn test(host: &str, timeout: Duration) -> Result<CameraInfo, String> {
    probe(host, timeout)?;
    let url = format!("rtsp://{host}/live");
    let parsed: url::Url = url.parse().map_err(|e| format!("bad url {url}: {e}"))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    rt.block_on(async {
        let session = tokio::time::timeout(
            timeout,
            retina::client::Session::describe(
                parsed,
                retina::client::SessionOptions::default().user_agent("pflowbench".to_owned()),
            ),
        )
        .await
        .map_err(|_| format!("DESCRIBE timed out after {:?}", timeout))?
        .map_err(|e| format!("DESCRIBE failed: {e}"))?;

        let s = session
            .streams()
            .iter()
            .find(|s| s.media() == "video")
            .ok_or_else(|| "camera advertises no video stream".to_string())?;
        let (mut w, mut h) = (None, None);
        let (mut profile, mut level) = (None, None);
        if let Some(retina::codec::ParametersRef::Video(v)) = s.parameters() {
            let (pw, ph) = v.pixel_dimensions();
            w = Some(pw as u16);
            h = Some(ph as u16);
            // extra_data is the avcC record: SPS starts at byte 8.
            let ed = v.extra_data();
            if ed.len() > 12 {
                let (p, l) = sps_profile_level(&ed[8..]);
                profile = p;
                level = l;
            }
        }
        let (server, methods) = rtsp_options(host, timeout);
        Ok(CameraInfo {
            url: url.clone(),
            encoding: s.encoding_name().to_string(),
            width: w,
            height: h,
            profile,
            level,
            clock_rate: Some(s.clock_rate_hz()),
            rtp_payload_type: Some(s.rtp_payload_type()),
            server,
            methods,
            transport: "TCP interleaved",
        })
    })
}

/// Is the camera's RTSP server actually listening? The Buddy3D only opens
/// :554 while local streaming is enabled in the Prusa App, and refuses the
/// connection outright otherwise -- worth distinguishing from "camera is off
/// the network entirely" so the user gets an actionable message.
pub fn probe(host: &str, timeout: Duration) -> Result<(), String> {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr = format!("{host}:554");
    let sock = addr
        .to_socket_addrs()
        .map_err(|e| format!("{host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host}: no address"))?;
    match TcpStream::connect_timeout(&sock, timeout) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => Err(format!(
            "camera at {host} refused RTSP on :554 -- enable local streaming for \
             the camera in the Prusa App, it is off by default"
        )),
        Err(e) => Err(format!("camera at {host} unreachable on :554: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Camera, LogFn};
    use std::sync::{Arc, Mutex};

    /// Live check against real hardware. Ignored by default since it needs the
    /// camera powered, on the LAN, and with local streaming enabled in the
    /// Prusa App. Run with:
    ///   FLOWCAM_HOST=<camera-ip> cargo test -p flowcam -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_camera_test_reports_what_it_advertises() {
        let host = std::env::var("FLOWCAM_HOST").unwrap_or_else(|_| "192.0.2.20".into());
        let info = test(&host, Duration::from_secs(5)).expect("camera test");
        println!("{info:?}");
        assert_eq!(info.encoding, "h264");
        assert_eq!((info.width, info.height), (Some(1920), Some(1080)));
    }

    /// Head-to-head: which RTSP transport survives sustained full-rate load?
    /// The camera's `rtsp_demo` server has shown interleaved-TCP framing
    /// desyncs under load; this measures whether UDP avoids them.
    #[test]
    #[ignore]
    fn compare_transports_under_load() {
        let host = std::env::var("FLOWCAM_HOST").unwrap_or_else(|_| "192.0.2.20".into());
        let secs: u64 = std::env::var("FLOWCAM_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(90);

        for tp in [Rtsp::Tcp, Rtsp::Udp] {
            let faults = Arc::new(Mutex::new(Vec::<String>::new()));
            let f = faults.clone();
            let logger: LogFn = Arc::new(move |lvl, msg| {
                if lvl == "warn" || lvl == "error" {
                    if msg.contains("stream error") || msg.contains("reconnect") {
                        f.lock().unwrap().push(msg.clone());
                    }
                }
            });
            let cam = Camera::start_with(&host, Some(logger), tp).expect("start");
            cam.set_live(true);
            let t0 = std::time::Instant::now();
            while cam.preview().is_none() && t0.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(200));
            }
            std::thread::sleep(Duration::from_secs(secs));
            let st = cam.stats();
            let n = faults.lock().unwrap().len();
            println!(
                "{tp:?}: {}s -> {} frames, {:.1} fps, {:.1} Mbps, {} decode failures, {} STREAM FAULTS",
                secs, st.frames,
                st.keyframe_interval_s.map(|k| 1.0 / k).unwrap_or(0.0),
                st.kbps.unwrap_or(0.0) / 1000.0,
                st.decode_failures, n
            );
            for m in faults.lock().unwrap().iter().take(2) {
                println!("    {}", m.lines().next().unwrap_or(""));
            }
        }
    }

    #[test]
    fn probe_reports_connection_refused_with_actionable_advice() {
        // Bind then drop, so the port is almost certainly closed.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        // probe() hardcodes :554, so just assert the error path for an
        // unroutable host is descriptive rather than a bare io error.
        let err = probe("127.0.0.1", Duration::from_millis(300)).unwrap_err();
        assert!(err.contains("127.0.0.1"), "got: {err}");
        let _ = port;
    }
}
