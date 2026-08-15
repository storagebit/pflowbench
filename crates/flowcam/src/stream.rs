// The background streaming task: DESCRIBE/SETUP/PLAY against the camera,
// then decode keyframes (every frame in live mode) and publish the latest
// into the shared state, retaining keyframes for the timelapse reel while
// recording.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use openh264::formats::YUVSource;

use crate::camera::{Latest, Shared};
use crate::decode::encode_jpeg;
use crate::rtsp::Rtsp;
use crate::LogFn;

/// Keyframes are ~170 KB each and arrive every ~3s, so a 15-minute run holds
/// ~300 (~50 MB). The cap stops an all-day session from consuming the machine;
/// hitting it logs a warning rather than silently dropping the tail.
const MAX_REEL_FRAMES: usize = 2000;

pub(crate) async fn run(
    url: &str,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    logger: LogFn,
    transport: Rtsp,
) -> Result<(), String> {
    use retina::client::{PlayOptions, SessionOptions, SetupOptions, TcpTransportOptions, Transport};
    use retina::codec::CodecItem;

    let parsed: url::Url = url.parse().map_err(|e| format!("bad url {url}: {e}"))?;
    let mut session = retina::client::Session::describe(
        parsed,
        SessionOptions::default().user_agent("pflowbench".to_owned()),
    )
    .await
    .map_err(|e| format!("DESCRIBE failed: {e}"))?;

    let idx = session
        .streams()
        .iter()
        .position(|s| s.media() == "video" && s.encoding_name() == "h264")
        .ok_or_else(|| "no H.264 video stream advertised in the SDP".to_string())?;

    session
        .setup(
            idx,
            SetupOptions::default()
                // TCP interleaved: survives any NAT/firewall between here and
                // the camera, no UDP ports to open.
                .transport(match transport {
                    Rtsp::Tcp => Transport::Tcp(TcpTransportOptions::default()),
                    Rtsp::Udp => Transport::Udp(retina::client::UdpTransportOptions::default()),
                })
                // openh264 requires Annex-B start codes with SPS/PPS inline on
                // each keyframe. retina's default is 4-byte-length-prefixed
                // (MP4 style) which the decoder rejects with a bare "native
                // error 4" -- verified against this camera.
                .frame_format(retina::codec::FrameFormat::SIMPLE),
        )
        .await
        .map_err(|e| format!("SETUP failed: {e}"))?;

    let playing = session
        .play(PlayOptions::default())
        .await
        .map_err(|e| format!("PLAY failed: {e}"))?;
    let mut demuxed = Box::pin(playing.demuxed().map_err(|e| format!("demux: {e}"))?);

    let mut decoder = openh264::decoder::Decoder::new()
        .map_err(|e| format!("openh264 init: {e}"))?;

    logger("info", "camera: streaming, waiting for first keyframe".to_string());
    let mut seq = 0u64;
    let mut decode_failures = 0u32;
    // The RGB repack + full-resolution JPEG encode is by far the most
    // expensive step. Doing it on every frame in live mode starves this same
    // task's socket reads, which desynchronises RTSP interleaved framing and
    // kills the stream ("Invalid RTSP message: ... request-line"). Decoding
    // still happens for EVERY frame -- inter frames need the reference chain,
    // so they cannot be skipped -- but the preview is produced at a bounded
    // rate, which is all a display can use anyway.
    let mut last_encode: Option<Instant> = None;
    let mut have_key = false;
    const MIN_ENCODE_GAP: Duration = Duration::from_millis(80); // ~12 fps ceiling

    while !stop.load(Ordering::Relaxed) {
        // Bounded wait so the stop flag is still honoured on a silent stream.
        let next = tokio::time::timeout(Duration::from_millis(500), demuxed.next()).await;
        let item = match next {
            Err(_) => continue, // timeout tick
            Ok(None) => return Err("stream ended".into()),
            Ok(Some(Err(e))) => return Err(format!("stream error: {e}")),
            Ok(Some(Ok(item))) => item,
        };

        let frame = match item {
            CodecItem::VideoFrame(f) => f,
            _ => continue,
        };
        // Keyframes are self-contained; inter frames need the reference chain,
        // which the decoder has because it runs continuously from the start.
        // In the default (non-live) mode we skip them: one frame per ~3s is
        // plenty for monitoring and costs almost nothing.
        let is_key = frame.is_random_access_point();
        // A freshly built decoder has no reference frames. Feeding it inter
        // frames before the first IDR produces a burst of Native:16 errors,
        // which is exactly what a reconnect used to log. Wait for the IDR.
        if !have_key {
            if !is_key {
                continue;
            }
            have_key = true;
        }
        if !is_key && !shared.live.load(Ordering::Relaxed) {
            continue;
        }

        {
            let mut c = shared.counters.lock().unwrap();
            let now = Instant::now();
            c.frames += 1;
            c.bytes += frame.data().len() as u64;
            c.last_frame_bytes = frame.data().len();
            c.first_frame.get_or_insert(now);
            c.last_frame = Some(now);
        }

        // Retain the raw access unit before decoding: the timelapse remuxes
        // these verbatim, so it stays lossless and costs no extra encode.
        if is_key && shared.recording.load(Ordering::Relaxed) {
            let mut reel = shared.reel.lock().unwrap();
            if reel.len() < MAX_REEL_FRAMES {
                reel.push(frame.data().to_vec());
                if reel.len() == MAX_REEL_FRAMES {
                    logger("warn", format!(
                        "camera: timelapse reel hit its {MAX_REEL_FRAMES}-frame cap; \
                         later frames will not be recorded"
                    ));
                }
            }
        }

        let decoded = match decoder.decode(frame.data()) {
            Ok(Some(d)) => d,
            // Ok(None) just means "need more data" -- not an error.
            Ok(None) => continue,
            Err(e) => {
                decode_failures += 1;
                shared.counters.lock().unwrap().decode_failures = decode_failures;
                if decode_failures <= 3 {
                    logger("warn", format!("camera: decode failed ({decode_failures}): {e}"));
                } else if decode_failures == 4 {
                    logger("warn", "camera: further decode failures suppressed".to_string());
                }
                continue;
            }
        };

        // Decode always (reference chain); publish a frame only on cadence.
        let due = match last_encode {
            None => true,
            Some(t) => !shared.live.load(Ordering::Relaxed) || t.elapsed() >= MIN_ENCODE_GAP,
        };
        if !due {
            continue;
        }
        last_encode = Some(Instant::now());

        let (w, h) = decoded.dimensions();
        let mut rgb = vec![0u8; w * h * 3];
        decoded.write_rgb8(&mut rgb);

        // Repack the luma plane, dropping the decoder's row stride.
        let (ys, _, _) = decoded.strides();
        let src_y = decoded.y();
        let mut y = vec![0u8; w * h];
        for row in 0..h {
            let from = row * ys;
            if from + w <= src_y.len() {
                y[row * w..(row + 1) * w].copy_from_slice(&src_y[from..from + w]);
            }
        }

        // Full-resolution preview: this machine has the CPU and bandwidth,
        // and a 1080p live view is the point. downscale2x_rgb stays available
        // (and tested) for anywhere a cheap thumbnail is wanted later.
        let preview_jpeg = match encode_jpeg(&rgb, w, h, 92) {
            Ok(j) => j,
            Err(e) => {
                logger("warn", format!("camera: preview encode: {e}"));
                continue;
            }
        };

        seq += 1;
        if seq == 1 {
            shared.connected.store(true, Ordering::Relaxed);
            logger("info", format!("camera: first frame decoded, {w}x{h}"));
        }
        *shared.latest.lock().unwrap() = Some(Latest {
            rgb,
            y,
            width: w,
            height: h,
            preview_jpeg,
            seq,
        });
    }

    logger("info", format!("camera: stopped after {seq} frame(s)"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Camera, LogFn};
    use std::sync::Arc;
    use std::time::Duration;

    /// Regression test for the framing desync: full-rate live mode used to
    /// starve the socket reader and kill the stream after ~12s with
    /// "Invalid RTSP message: ... request-line".
    #[test]
    #[ignore]
    fn live_mode_survives_sustained_streaming() {
        let host = std::env::var("FLOWCAM_HOST").unwrap_or_else(|_| "192.0.2.20".into());
        let logger: LogFn = Arc::new(|lvl, msg| println!("[{lvl}] {msg}"));
        let cam = Camera::start(&host, Some(logger)).expect("start");
        cam.set_live(true);

        // wait for the stream to establish
        let t0 = std::time::Instant::now();
        while cam.preview().is_none() {
            assert!(t0.elapsed() < Duration::from_secs(20), "no first frame");
            std::thread::sleep(Duration::from_millis(200));
        }
        let after_start = cam.stats().frames;

        // run well past the ~12s point where it used to die
        std::thread::sleep(Duration::from_secs(30));
        let s = cam.stats();
        println!("{s:?}");
        assert!(s.connected, "stream dropped during sustained live mode");
        assert!(
            s.frames > after_start + 100,
            "expected full-rate decoding, only {} frames total", s.frames
        );
        // decoding every frame means well above the ~0.33fps keyframe rate
        let fps = 1.0 / s.keyframe_interval_s.unwrap_or(1.0);
        println!("effective {fps:.1} fps, {} decode failures", s.decode_failures);
        assert!(fps > 10.0, "expected >10fps in live mode, got {fps:.1}");
    }
}
