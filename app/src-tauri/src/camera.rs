// camera.rs -- Buddy3D camera commands: connect / stop, the RTSP
// connectivity test, live-preview polling, stream stats, keyframe recording
// and MP4 timelapse export. Decoding itself lives in crates/flowcam.

use crate::logging;
use crate::snapshots;
use crate::state::AppState;
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

#[derive(Serialize)]
pub(crate) struct Preview {
    /// data: URL, or None when the frame is unchanged since `since_seq`.
    image: Option<String>,
    seq: u64,
    connected: bool,
}

/// Connect to the Buddy3D and start decoding. Probes :554 first so a disabled
/// stream produces an actionable message instead of a bare connect error --
/// the camera only listens while local streaming is enabled in the Prusa App.
#[tauri::command]
pub(crate) fn camera_start(state: State<AppState>, host: String) -> Result<(), String> {
    let host = host.trim().to_string();
    logging::trace("cmd:camera_start", format!("host={host}"));
    let mut slot = state.camera.lock().unwrap();
    if slot.is_some() {
        logging::warn("cmd:camera_start", "camera already running".to_string());
        return Err("camera already running".into());
    }
    flowcam::probe(&host, std::time::Duration::from_secs(3)).map_err(|e| {
        logging::warn("cmd:camera_start", e.clone());
        e
    })?;
    let cam = flowcam::Camera::start(&host, Some(logging::sink("flowcam"))).map_err(|e| {
        logging::error("cmd:camera_start", e.to_string());
        e.to_string()
    })?;
    *slot = Some(cam);
    Ok(())
}

/// Connectivity check for the Camera card's Test button. Does RTSP
/// OPTIONS + DESCRIBE only -- never starts a stream, so it is safe to run at
/// any time, including mid-print.
#[tauri::command]
pub(crate) fn camera_test(host: String) -> Result<serde_json::Value, String> {
    let host = host.trim().to_string();
    logging::trace("cmd:camera_test", format!("host={host}"));
    let info = flowcam::test(&host, std::time::Duration::from_secs(5)).map_err(|e| {
        logging::warn("cmd:camera_test", e.clone());
        e
    })?;
    logging::info(
        "cmd:camera_test",
        format!(
            "{} {}x{} {} {} via {}",
            info.encoding,
            info.width.unwrap_or(0),
            info.height.unwrap_or(0),
            info.profile.clone().unwrap_or_default(),
            info.level.map(|l| format!("L{l:.1}")).unwrap_or_default(),
            info.server.clone().unwrap_or_else(|| "?".into()),
        ),
    );
    Ok(serde_json::json!({
        "url": info.url,
        "encoding": info.encoding,
        "width": info.width,
        "height": info.height,
        "profile": info.profile,
        "level": info.level,
        "clockRate": info.clock_rate,
        "payloadType": info.rtp_payload_type,
        "server": info.server,
        "methods": info.methods,
        "transport": info.transport,
    }))
}

/// Live counters for the stream-details readout.
#[tauri::command]
pub(crate) fn camera_stats(state: State<AppState>) -> serde_json::Value {
    // Unlogged: polled by the UI.
    let slot = state.camera.lock().unwrap();
    let Some(cam) = slot.as_ref() else {
        return serde_json::json!({ "connected": false });
    };
    let s = cam.stats();
    serde_json::json!({
        "connected": s.connected,
        "live": cam.is_live(),
        "recording": s.recording,
        "frames": s.frames,
        "recordedFrames": s.recorded_frames,
        "bytes": s.bytes,
        "lastFrameBytes": s.last_frame_bytes,
        "keyframeIntervalS": s.keyframe_interval_s,
        "kbps": s.kbps,
        "decodeFailures": s.decode_failures,
        "uptimeS": s.uptime_s,
    })
}

/// Start or stop retaining keyframes, independent of a capture run. Starting
/// discards anything previously held, so each recording is self-contained.
#[tauri::command]
pub(crate) fn camera_record(state: State<AppState>, on: bool) -> Result<usize, String> {
    logging::trace("cmd:camera_record", format!("on={on}"));
    let slot = state.camera.lock().unwrap();
    let cam = slot.as_ref().ok_or("camera not connected")?;
    if on {
        cam.start_recording();
        logging::info("cmd:camera_record", "recording started".to_string());
    } else {
        cam.stop_recording();
        logging::info(
            "cmd:camera_record",
            format!("recording stopped, {} frames held", cam.recorded_frames()),
        );
    }
    Ok(cam.recorded_frames())
}

/// Toggle full-rate decoding. Off by default -- see flowcam::Camera::set_live.
#[tauri::command]
pub(crate) fn camera_set_live(state: State<AppState>, live: bool) -> Result<(), String> {
    logging::trace("cmd:camera_set_live", format!("live={live}"));
    let slot = state.camera.lock().unwrap();
    let cam = slot.as_ref().ok_or("camera not connected")?;
    cam.set_live(live);
    logging::info(
        "cmd:camera_set_live",
        if live { "full-rate decoding ON (higher CPU)" } else { "back to keyframe-only decoding" }.to_string(),
    );
    Ok(())
}

/// Remux the recorded keyframes into a shareable MP4 timelapse.
#[tauri::command]
pub(crate) fn camera_write_timelapse(state: State<AppState>, fps: u32) -> Result<String, String> {
    logging::trace("cmd:camera_write_timelapse", format!("fps={fps}"));
    let slot = state.camera.lock().unwrap();
    let cam = slot.as_ref().ok_or("camera not connected")?;
    let dir = state
        .run_dir
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| PathBuf::from("runs"));
    let _ = std::fs::create_dir_all(&dir);
    // Timestamped: exporting twice must not silently overwrite the first.
    let path = dir.join(format!("timelapse-{}.mp4", snapshots::run_dir_name()));
    let span = Some(cam.stats().uptime_s);
    let info = cam.write_timelapse(&path, fps, span).map_err(|e| {
        logging::warn("cmd:camera_write_timelapse", e.clone());
        e
    })?;
    let msg = format!(
        "{} frames, {}x{} @ {} fps, {:.1} MB{} -> {}",
        info.frames,
        info.width,
        info.height,
        info.fps,
        info.bytes as f64 / 1_048_576.0,
        info.speedup.map(|s| format!(", {s:.0}x speedup")).unwrap_or_default(),
        path.display()
    );
    logging::info("cmd:camera_write_timelapse", msg.clone());
    Ok(msg)
}

#[tauri::command]
pub(crate) fn camera_stop(state: State<AppState>) {
    logging::trace("cmd:camera_stop", "called".to_string());
    if let Some(mut c) = state.camera.lock().unwrap().take() {
        c.stop();
    }
}

/// Latest live frame for the GUI. Returns `image: None` when nothing has
/// changed since `since_seq`, so the ~3s keyframe cadence doesn't force a
/// repaint (or a ~100KB IPC transfer) on every poll.
#[tauri::command]
pub(crate) fn camera_preview(state: State<AppState>, since_seq: u64) -> Preview {
    // Deliberately unlogged: polled continuously by the UI, same rationale as
    // capture_delta. flowcam logs its own connect/decode milestones.
    let slot = state.camera.lock().unwrap();
    let cam = match slot.as_ref() {
        Some(c) => c,
        None => return Preview { image: None, seq: 0, connected: false },
    };
    match cam.preview() {
        Some(p) if p.seq != since_seq => Preview {
            image: Some(snapshots::jpeg_data_url(&p.jpeg)),
            seq: p.seq,
            connected: true,
        },
        Some(p) => Preview { image: None, seq: p.seq, connected: true },
        None => Preview { image: None, seq: 0, connected: cam.is_connected() },
    }
}
