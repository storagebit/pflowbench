// The held-open camera session: shared frame state, live counters, and the
// public Camera handle -- start/stop, preview, snapshots, recording control.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::decode::encode_jpeg;
use crate::rtsp::Rtsp;
use crate::stream::run;
use crate::timelapse::{write_timelapse_mp4, TimelapseInfo};
use crate::{noop_logger, LogFn};

/// Most recent decoded keyframe. `rgb` is kept undecoded-to-JPEG so a
/// full-resolution still can be produced on demand without re-decoding.
pub(crate) struct Latest {
    pub(crate) rgb: Vec<u8>,
    /// Packed luma plane (width*height), stride removed. Lossless -- kept
    /// because JPEG discards precisely the high-frequency luma detail that a
    /// later surface-roughness metric would depend on.
    pub(crate) y: Vec<u8>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Full-resolution JPEG for the live GUI view, re-encoded per frame.
    pub(crate) preview_jpeg: Vec<u8>,
    /// Increments per decoded keyframe, so the UI can skip redundant repaints.
    pub(crate) seq: u64,
}

#[derive(Default)]
pub(crate) struct Shared {
    pub(crate) latest: Mutex<Option<Latest>>,
    /// Set once the stream is confirmed running, for status display.
    pub(crate) connected: AtomicBool,
    /// While set, every keyframe is retained for the timelapse.
    pub(crate) recording: AtomicBool,
    /// While set, decode EVERY frame for true full-rate video instead of only
    /// self-contained keyframes. Costs real CPU, so it is opt-in.
    pub(crate) live: AtomicBool,
    /// Retained Annex-B keyframe access units, in arrival order.
    pub(crate) reel: Mutex<Vec<Vec<u8>>>,
    /// Live counters for the stats readout.
    pub(crate) counters: Mutex<Counters>,
}

#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) frames: u64,
    pub(crate) bytes: u64,
    pub(crate) last_frame_bytes: usize,
    pub(crate) decode_failures: u32,
    pub(crate) started: Option<Instant>,
    pub(crate) first_frame: Option<Instant>,
    pub(crate) last_frame: Option<Instant>,
}

/// Live counters from the streaming thread, for a stats readout.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraStats {
    pub connected: bool,
    pub recording: bool,
    pub frames: u64,
    pub recorded_frames: usize,
    pub bytes: u64,
    pub last_frame_bytes: usize,
    /// Mean gap between keyframes, once at least two have arrived.
    pub keyframe_interval_s: Option<f64>,
    /// Mean bitrate of the retained keyframe stream.
    pub kbps: Option<f64>,
    pub decode_failures: u32,
    pub uptime_s: f64,
}

pub struct Camera {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

/// A snapshot handed back to the caller.
pub struct Snapshot {
    pub jpeg: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub seq: u64,
}

impl Camera {
    /// Connect and start decoding in the background. Returns as soon as the
    /// worker thread is spawned -- the first frame typically lands ~3s later,
    /// bounded by the camera's keyframe interval. Poll `is_connected()` or
    /// `preview()` to know when imagery is actually available.
    pub fn start(host: &str, logger: Option<LogFn>) -> io::Result<Camera> {
        Self::start_with(host, logger, Rtsp::default())
    }

    /// As `start`, with an explicit RTSP transport.
    pub fn start_with(host: &str, logger: Option<LogFn>, transport: Rtsp) -> io::Result<Camera> {
        let logger = logger.unwrap_or_else(noop_logger);
        let url = format!("rtsp://{host}/live");
        let shared = Arc::new(Shared::default());
        shared.counters.lock().unwrap().started = Some(Instant::now());
        let stop = Arc::new(AtomicBool::new(false));
        let (sh, sp, lg, u) = (shared.clone(), stop.clone(), logger.clone(), url.clone());

        logger("info", format!("camera: connecting to {url}"));
        let handle = std::thread::Builder::new()
            .name("flowcam".into())
            .spawn(move || {
                // A current-thread runtime keeps this crate's public API sync,
                // matching flowcore::Capture; retina is async-only.
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        lg("error", format!("camera: tokio runtime: {e}"));
                        return;
                    }
                };
                // Reconnect loop: a 15-minute print must survive a transient
                // stream fault rather than silently losing the camera. The
                // reel and counters live in `shared`, so a reconnect resumes
                // the same timelapse instead of starting over.
                let mut attempt = 0u32;
                while !sp.load(Ordering::Relaxed) {
                    match rt.block_on(run(&u, sh.clone(), sp.clone(), lg.clone(), transport)) {
                        Ok(()) => break, // clean stop
                        Err(e) => {
                            sh.connected.store(false, Ordering::Relaxed);
                            if sp.load(Ordering::Relaxed) {
                                break;
                            }
                            attempt += 1;
                            let backoff = Duration::from_millis(500 * (1 << attempt.min(4)));
                            lg("warn", format!(
                                "camera: {e}; reconnecting in {:.1}s (attempt {attempt})",
                                backoff.as_secs_f64()
                            ));
                            std::thread::sleep(backoff);
                        }
                    }
                }
                sh.connected.store(false, Ordering::Relaxed);
            })?;

        Ok(Camera { shared, stop, handle: Some(handle) })
    }

    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Relaxed)
    }

    /// Latest full-resolution JPEG for the live view, or None before the
    /// first frame arrives.
    /// `seq` lets a caller skip repainting when nothing has changed.
    pub fn preview(&self) -> Option<Snapshot> {
        let g = self.shared.latest.lock().unwrap();
        let l = g.as_ref()?;
        Some(Snapshot {
            jpeg: l.preview_jpeg.clone(),
            width: l.width,
            height: l.height,
            seq: l.seq,
        })
    }

    /// Full-resolution JPEG of the most recent keyframe, encoded on demand.
    /// No decode and no network wait -- the decoded frame is already resident,
    /// which is the whole reason the session is held open.
    pub fn full_snapshot(&self, quality: u8) -> Option<Snapshot> {
        let g = self.shared.latest.lock().unwrap();
        let l = g.as_ref()?;
        let jpeg = encode_jpeg(&l.rgb, l.width, l.height, quality).ok()?;
        Some(Snapshot { jpeg, width: l.width, height: l.height, seq: l.seq })
    }

    /// Live counters for a stats readout: frame count, throughput, measured
    /// keyframe interval, decode failures.
    pub fn stats(&self) -> CameraStats {
        let c = self.shared.counters.lock().unwrap();
        let span = match (c.first_frame, c.last_frame) {
            (Some(a), Some(b)) if c.frames > 1 => Some(b.duration_since(a).as_secs_f64()),
            _ => None,
        };
        // n frames span n-1 gaps
        let keyframe_interval_s = span.map(|s| s / (c.frames - 1) as f64);
        let kbps = span.and_then(|s| (s > 0.0).then(|| (c.bytes as f64 * 8.0 / 1000.0) / s));
        CameraStats {
            connected: self.shared.connected.load(Ordering::Relaxed),
            recording: self.shared.recording.load(Ordering::Relaxed),
            frames: c.frames,
            recorded_frames: self.shared.reel.lock().unwrap().len(),
            bytes: c.bytes,
            last_frame_bytes: c.last_frame_bytes,
            keyframe_interval_s,
            kbps,
            decode_failures: c.decode_failures,
            uptime_s: c.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0),
        }
    }

    /// Full-rate video instead of the default one-frame-per-keyframe.
    ///
    /// OFF by default -- opt in. The camera emits a keyframe only every ~3s,
    /// so the default mode updates about that often, which is enough to watch
    /// a print and costs almost nothing. Switching this on decodes every
    /// frame for real ~25fps motion, at the cost of a 1080p decode plus a
    /// full-resolution JPEG encode per frame.
    ///
    /// The timelapse reel is unaffected -- it always keeps keyframes only,
    /// since inter frames are not independently decodable and could not be
    /// remuxed on their own.
    pub fn set_live(&self, live: bool) {
        self.shared.live.store(live, Ordering::Relaxed);
    }

    pub fn is_live(&self) -> bool {
        self.shared.live.load(Ordering::Relaxed)
    }

    /// Begin retaining keyframes for a timelapse. Clears anything previously
    /// retained, so one call per run.
    pub fn start_recording(&self) {
        self.shared.reel.lock().unwrap().clear();
        self.shared.recording.store(true, Ordering::Relaxed);
    }

    pub fn stop_recording(&self) {
        self.shared.recording.store(false, Ordering::Relaxed);
    }

    pub fn recorded_frames(&self) -> usize {
        self.shared.reel.lock().unwrap().len()
    }

    /// Write everything retained so far to an MP4. Safe to call while still
    /// recording -- it snapshots the reel rather than draining it.
    pub fn write_timelapse(
        &self,
        path: &std::path::Path,
        fps: u32,
        source_span_secs: Option<f64>,
    ) -> Result<TimelapseInfo, String> {
        let frames = self.shared.reel.lock().unwrap().clone();
        let (w, h) = {
            let g = self.shared.latest.lock().unwrap();
            match g.as_ref() {
                Some(l) => (l.width as u16, l.height as u16),
                None => return Err("no frame decoded yet -- nothing to write".into()),
            }
        };
        write_timelapse_mp4(path, &frames, w, h, fps, source_span_secs)
    }

    /// Lossless luma plane of the current frame as a binary PGM (P5).
    /// Full resolution, no compression artefacts -- the archival companion to
    /// the JPEG for any later image analysis.
    /// JPEG and PGM of the SAME frame, from one lock acquisition. The two
    /// single-format calls can straddle a decode and return different
    /// frames -- fatal for vision analysis, which reads the PGM while the
    /// freshness gate reads the JPEG's seq.
    pub fn full_snapshot_with_luma(&self, quality: u8) -> Option<(Snapshot, Vec<u8>)> {
        let g = self.shared.latest.lock().unwrap();
        let l = g.as_ref()?;
        let jpeg = encode_jpeg(&l.rgb, l.width, l.height, quality).ok()?;
        let mut pgm = format!("P5\n{} {}\n255\n", l.width, l.height).into_bytes();
        pgm.extend_from_slice(&l.y);
        Some((Snapshot { jpeg, width: l.width, height: l.height, seq: l.seq }, pgm))
    }

    pub fn full_luma_pgm(&self) -> Option<Vec<u8>> {
        let g = self.shared.latest.lock().unwrap();
        let l = g.as_ref()?;
        let mut out = format!("P5\n{} {}\n255\n", l.width, l.height).into_bytes();
        out.extend_from_slice(&l.y);
        Some(out)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe;

    #[test]
    #[ignore]
    fn live_camera_yields_a_decodable_frame() {
        let host = std::env::var("FLOWCAM_HOST").unwrap_or_else(|_| "192.0.2.20".into());
        probe(&host, Duration::from_secs(3)).expect("RTSP probe");

        let logger: LogFn = Arc::new(|lvl, msg| println!("[{lvl}] {msg}"));
        let cam = Camera::start(&host, Some(logger)).expect("start");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while cam.preview().is_none() {
            assert!(std::time::Instant::now() < deadline, "no frame within 20s");
            std::thread::sleep(Duration::from_millis(200));
        }

        let p = cam.preview().expect("preview");
        assert_eq!((p.width, p.height), (1920, 1080), "preview is full resolution");
        assert_eq!(&p.jpeg[..2], &[0xFF, 0xD8]);

        let full = cam.full_snapshot(95).expect("full snapshot");
        assert_eq!((full.width, full.height), (1920, 1080));
        assert_eq!(&full.jpeg[..2], &[0xFF, 0xD8]);

        // lossless luma companion: P5 header then exactly w*h bytes
        let pgm = cam.full_luma_pgm().expect("luma pgm");
        assert!(pgm.starts_with(b"P5\n1920 1080\n255\n"), "bad PGM header");
        let hdr = b"P5\n1920 1080\n255\n".len();
        assert_eq!(pgm.len() - hdr, 1920 * 1080, "luma plane must be full size");
        println!("luma pgm {} bytes", pgm.len());
        println!(
            "preview {} bytes ({}x{}), full {} bytes ({}x{})",
            p.jpeg.len(), p.width, p.height,
            full.jpeg.len(), full.width, full.height
        );
    }
}
