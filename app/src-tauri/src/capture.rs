// capture.rs -- the capture lifecycle: start / stop / delta polling plus CSV
// export. Owns the single flowcore::Capture in AppState; the per-band still
// saver it hooks up lives in stills.rs.

use crate::logging;
use crate::snapshots;
use crate::state::{expand, AppState};
use crate::stills;
use flowcore::{BandMap, Capture, CaptureHooks};
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

#[derive(Serialize)]
pub(crate) struct Delta {
    seq: u64,
    force: Vec<(f64, f64)>,
    z: Vec<(f64, f64)>,
    /// Head speed (mm/s), differentiated from pos_x/pos_y/pos_z.
    speed: Vec<(f64, f64)>,
    /// Temperatures on the same clock as the force samples.
    temp_noz: Vec<(f64, f64)>,
    temp_bed: Vec<(f64, f64)>,
    temp_chamber: Vec<(f64, f64)>,
    bands: Vec<BandRow>,
    cyl: usize,
    z_now: f64,
    /// `None` until the metric actually arrives -- serialized as JSON null so
    /// the UI can tell "no reading yet" from a genuine 0.0.
    now_noz: Option<f64>,
    now_bed: Option<f64>,
    now_chamber: Option<f64>,
}

#[derive(Serialize)]
struct BandRow {
    cylinder: usize,
    band: usize,
    /// Commanded flow for this band.
    flow: f64,
    n: u64,
    mean: f64,
    sd: f64,
    /// How many speed samples backed this band.
    speed_n: u64,
    /// Measured head speed, mm/s. `None` when the band got no speed samples,
    /// so the UI can print "-" instead of a confident 0.0.
    speed: Option<f64>,
    /// speed x bead cross-section -- what was actually delivered. A gap from
    /// `flow` means the printer never reached the commanded feedrate.
    actual_flow: Option<f64>,
}

/// Persist the capture to disk: per-band statistics plus the raw sample
/// series. Without this a run exists only in memory and closing the window
/// destroys it -- which is exactly what nearly happened to the first real
/// calibration run.
/// Write every CSV for one capture. Split out of `capture_export` so that
/// stopping a run can persist it automatically -- see `capture_stop`.
fn write_export(dir: &std::path::Path, d: &flowcore::Snapshot) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (force, z, speed, bands) = (&d.force, &d.z, &d.speed, &d.bands);

    // per-band summary: the actual result table
    let mut b = String::from(
        "cylinder,band,commanded_flow_mm3s,actual_flow_mm3s,speed_mm_s,speed_n,n,mean_g,sd_g,settled_n,settled_mean_g,settled_sd_g\n",
    );
    for r in bands {
        b.push_str(&format!(
            "{},{},{:.1},{},{},{},{},{:.4},{:.4},{},{},{}\n",
            r.cylinder,
            r.band,
            r.flow,
            // Empty cell, never a number, when the quantity was not measured:
            // a 0.00 here would read as a real observation of zero flow.
            r.actual_flow.map(|f| format!("{f:.2}")).unwrap_or_default(),
            r.speed_mean.map(|v| format!("{v:.2}")).unwrap_or_default(),
            r.speed_n,
            r.n,
            r.mean,
            r.sd,
            r.settled_n,
            r.settled_mean.map(|v| format!("{v:.4}")).unwrap_or_default(),
            r.settled_sd.map(|v| format!("{v:.4}")).unwrap_or_default(),
        ));
    }
    let bpath = dir.join("bands.csv");
    std::fs::write(&bpath, &b).map_err(|e| format!("{}: {e}", bpath.display()))?;

    // raw series, so the binning can be redone offline if the band mapping
    // ever turns out to be wrong
    let mut f = String::from("t_s,force_g\n");
    for (t, v) in force {
        f.push_str(&format!("{t:.4},{v:.4}\n"));
    }
    let fpath = dir.join("force.csv");
    std::fs::write(&fpath, &f).map_err(|e| format!("{}: {e}", fpath.display()))?;

    let mut zz = String::from("t_s,z_mm\n");
    for (t, v) in z {
        zz.push_str(&format!("{t:.4},{v:.4}\n"));
    }
    let zpath = dir.join("z.csv");
    std::fs::write(&zpath, &zz).map_err(|e| format!("{}: {e}", zpath.display()))?;

    let mut sp = String::from("t_s,speed_mm_s\n");
    for (t, v) in speed {
        sp.push_str(&format!("{t:.4},{v:.3}\n"));
    }
    let spath = dir.join("speed.csv");
    std::fs::write(&spath, &sp).map_err(|e| format!("{}: {e}", spath.display()))?;

    let mut tp = String::from("t_s,nozzle_c,bed_c,chamber_c\n");
    // the three series are sampled independently; emit them as separate rows
    // rather than inventing an interpolation that isn't in the data
    for (t, v) in &d.temp_noz { tp.push_str(&format!("{t:.3},{v:.2},,\n")); }
    for (t, v) in &d.temp_bed { tp.push_str(&format!("{t:.3},,{v:.2},\n")); }
    for (t, v) in &d.temp_chamber { tp.push_str(&format!("{t:.3},,,{v:.2}\n")); }
    let tpath = dir.join("temps.csv");
    std::fs::write(&tpath, &tp).map_err(|e| format!("{}: {e}", tpath.display()))?;

    // Loadcell zeros and band time windows -- the verdict engine's inputs;
    // exporting them keeps a run auditable offline.
    if !d.tares.is_empty() || !d.band_windows.is_empty() {
        let mut x = String::from("kind,cylinder,band,a,b\n");
        for (c, m, n) in &d.tares {
            x.push_str(&format!("tare,{c},,{m:.4},{n}\n"));
        }
        for (c, b, t0, t1) in &d.band_windows {
            x.push_str(&format!("window,{c},{b},{t0:.3},{t1:.3}\n"));
        }
        let xpath = dir.join("segments.csv");
        std::fs::write(&xpath, &x).map_err(|e| format!("{}: {e}", xpath.display()))?;
    }

    Ok(format!(
        "{} bands, {} force samples, {} z samples -> {}",
        bands.len(), force.len(), z.len(), dir.display()
    ))
}

/// Export the current run, or the one that just finished.
///
/// Falls back to the snapshot `capture_stop` keeps. Stop used to drop the
/// whole `Capture`, and export required a live one -- so pressing Stop then
/// Export, which is the obvious order, destroyed the run and then reported
/// "capture not running". That cost a complete PC Blend CF run: 342,634 force
/// samples and 123,727 Z samples.
#[tauri::command]
pub(crate) fn capture_export(state: State<AppState>) -> Result<String, String> {
    logging::trace("cmd:capture_export", "called".to_string());
    let dir = state.run_dir.lock().unwrap().clone().unwrap_or_else(|| PathBuf::from("runs"));

    // snapshot_all, not delta(0): delta returns empty series when `since`
    // equals the current seq, which would export headers and nothing else.
    let snap = match state.capture.lock().unwrap().as_ref() {
        Some(cap) => cap.snapshot_all(),
        None => match state.last_run.lock().unwrap().as_ref() {
            Some(s) => {
                logging::info("cmd:capture_export", "exporting the last stopped run".to_string());
                s.clone()
            }
            None => {
                let msg = "no capture running and no finished run held -- nothing to export";
                logging::warn("cmd:capture_export", msg.to_string());
                return Err(msg.to_string());
            }
        },
    };

    let msg = write_export(&dir, &snap).inspect_err(|e| {
        logging::error("cmd:capture_export", e.clone());
    })?;
    logging::info("cmd:capture_export", msg.clone());
    Ok(msg)
}

#[tauri::command]
pub(crate) fn capture_start(
    state: State<AppState>,
    port: u16,
    flows: Vec<f64>,
    revs: usize,
    layer_h: f64,
    first_layer_h: f64,
    width: f64,
    // Path of the generated G-code; its `<path>.bands.txt` manifest gives the
    // capture deterministic sdpos band addressing. Empty = legacy Z heuristic.
    gcode_path: String,
) -> Result<(), String> {
    logging::trace(
        "cmd:capture_start",
        format!("port={port} flows={flows:?} revs={revs} layer_h={layer_h} first_layer_h={first_layer_h} gcode={gcode_path:?}"),
    );
    let sd_map = if gcode_path.trim().is_empty() {
        None
    } else {
        let mpath = format!("{}.bands.txt", expand(gcode_path.trim()));
        match std::fs::read_to_string(&mpath) {
            Ok(text) => match flowcore::SdMap::parse(&text) {
                Ok(m) => {
                    logging::info("cmd:capture_start", format!("band manifest loaded: {mpath}"));
                    Some(m)
                }
                Err(e) => {
                    logging::warn("cmd:capture_start", format!("{mpath} unusable ({e}) -- Z heuristic fallback"));
                    None
                }
            },
            Err(e) => {
                logging::warn("cmd:capture_start", format!(
                    "no band manifest at {mpath} ({e}) -- regenerate the G-code to get \
                     deterministic band addressing; falling back to the Z heuristic"
                ));
                None
            }
        }
    };
    let mut slot = state.capture.lock().unwrap();
    if slot.is_some() {
        logging::warn("cmd:capture_start", "capture already running".to_string());
        return Err("capture already running".into());
    }

    // One directory per run, so a session's stills stay together and sort
    // chronologically alongside earlier runs.
    let dir = PathBuf::from("runs").join(snapshots::run_dir_name());
    if let Err(e) = std::fs::create_dir_all(&dir) {
        logging::warn("cmd:capture_start", format!("could not create {}: {e}", dir.display()));
    } else {
        logging::info("cmd:capture_start", format!("run directory: {}", dir.display()));
    }
    *state.run_dir.lock().unwrap() = Some(dir.clone());

    // Persist the generated layout beside the stills: vision_analyze on this
    // directory must work after an app restart, and against the layout that
    // MADE this run, not whatever was generated since.
    if let Some((dia, positions)) = state.last_layout.lock().unwrap().clone() {
        let mut text = format!("diameter {dia}\n");
        for (temp, x, y) in &positions {
            text.push_str(&format!("{temp} {x} {y}\n"));
        }
        if let Err(e) = std::fs::write(dir.join("vision.layout"), text) {
            logging::warn("cmd:capture_start", format!("vision.layout: {e}"));
        }
    }
    if let Ok(calib) = std::fs::read_to_string("runs/vision.calib") {
        let _ = std::fs::write(dir.join("vision.calib"), calib);
    }

    // Roll a fresh timelapse for this run. Without this the reel stays empty
    // and there is nothing to export -- recording is tied to the run, not to
    // the camera connection, so the video covers exactly the print.
    if let Some(cam) = state.camera.lock().unwrap().as_ref() {
        cam.start_recording();
        logging::info("cmd:capture_start", "camera recording started for this run".to_string());
    }

    // Per-band still saver -- the worker thread and its freshness gate live
    // in stills.rs.
    let save_still = stills::save_still(state.camera.clone(), state.run_dir.clone());

    // With photo windows in the manifest the still is taken while the head is
    // parked out of frame; band-change stills would catch the head mid-wall,
    // so they stay enabled only for legacy G-code without photo segments.
    let has_photo = sd_map.as_ref().map(|m| m.has_photo()).unwrap_or(false);
    if has_photo {
        logging::info("cmd:capture_start",
            "photo windows in manifest: stills trigger while the head is parked".to_string());
    }
    let on_band_change: Option<flowcore::BandChangeFn> =
        if has_photo { None } else { Some(save_still.clone()) };
    let on_photo_window: Option<flowcore::BandChangeFn> =
        if has_photo { Some(save_still) } else { None };

    // Bead geometry lets flowcore turn measured speed into delivered flow.
    let map = BandMap {
        flows,
        per_cylinder_flows: Vec::new(),
        revs,
        layer_h,
        first_layer_h,
        bead_xsec: Some(flowgen::extrusion_xsec(layer_h, width)),
    };
    let hooks = CaptureHooks {
        logger: Some(logging::sink("flowcore::capture")),
        on_band_change,
        on_photo_window,
    };
    let cap = Capture::start("0.0.0.0", port, map, sd_map, hooks).map_err(|e| {
        logging::error("cmd:capture_start", format!("bind :{port}: {e}"));
        format!("bind :{port}: {e}")
    })?;
    *slot = Some(cap);
    Ok(())
}

#[tauri::command]
pub(crate) fn capture_stop(state: State<AppState>) {
    logging::trace("cmd:capture_stop", "called".to_string());
    let mut slot = state.capture.lock().unwrap();
    if let Some(cam) = state.camera.lock().unwrap().as_ref() {
        cam.stop_recording();
        let frames = cam.recorded_frames();
        logging::info(
            "cmd:capture_stop",
            format!("camera recording stopped, {frames} frames held"),
        );
        // Write the timelapse NOW rather than waiting for an Export click:
        // the frames live only in RAM, and one app restart after a finished
        // run nearly cost 786 of them. Export MP4 stays available for
        // re-encoding at a different fps; this is the safety copy.
        if frames > 0 {
            let dir = state
                .run_dir
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| PathBuf::from("runs"));
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(format!("timelapse-{}.mp4", snapshots::run_dir_name()));
            let span = Some(cam.stats().uptime_s);
            match cam.write_timelapse(&path, 30, span) {
                Ok(info) => logging::info(
                    "cmd:capture_stop",
                    format!(
                        "timelapse auto-saved: {} frames, {:.1} MB -> {}",
                        info.frames,
                        info.bytes as f64 / 1_048_576.0,
                        path.display()
                    ),
                ),
                Err(e) => logging::warn(
                    "cmd:capture_stop",
                    format!("timelapse auto-save failed (frames still held for Export MP4): {e}"),
                ),
            }
        }
    }
    if let Some(mut c) = slot.take() {
        // Grab the data BEFORE the Capture is dropped. Taking it out of the
        // slot destroys every sample, and export used to require a live
        // capture -- so Stop-then-Export, the natural order, silently threw
        // the whole run away.
        let snap = c.snapshot_all();
        c.stop(); // flowcore itself logs a summary here, via the sink wired in capture_start

        // Persist immediately rather than relying on anyone pressing Export.
        // A run costs an hour of printing; a CSV write costs milliseconds.
        let dir = state.run_dir.lock().unwrap().clone().unwrap_or_else(|| PathBuf::from("runs"));
        match write_export(&dir, &snap) {
            Ok(msg) => logging::info("cmd:capture_stop", format!("auto-exported: {msg}")),
            Err(e) => logging::error("cmd:capture_stop", format!("AUTO-EXPORT FAILED: {e}")),
        }
        *state.last_run.lock().unwrap() = Some(snap);
    } else {
        logging::trace("cmd:capture_stop", "no capture was running".to_string());
    }
}

#[tauri::command]
pub(crate) fn capture_delta(state: State<AppState>, since: u64) -> Result<Delta, String> {
    // No logging here on purpose -- see the header comment at the top of
    // main.rs. The capture thread (flowcore::capture) logs its own
    // milestones at a bounded rate regardless of how often this is polled.
    let slot = state.capture.lock().unwrap();
    let cap = slot.as_ref().ok_or("capture not running")?;
    let d = cap.delta(since);
    let (seq, force, z, speed, bands, cyl, z_now) =
        (d.seq, d.force, d.z, d.speed, d.bands, d.cyl, d.z_now);
    Ok(Delta {
        seq,
        force,
        z,
        speed,
        temp_noz: d.temp_noz,
        temp_bed: d.temp_bed,
        temp_chamber: d.temp_chamber,
        now_noz: d.now_noz,
        now_bed: d.now_bed,
        now_chamber: d.now_chamber,
        bands: bands
            .into_iter()
            .map(|b| BandRow {
                cylinder: b.cylinder,
                band: b.band,
                flow: b.flow,
                n: b.n,
                mean: b.mean,
                sd: b.sd,
                speed_n: b.speed_n,
                speed: b.speed_mean,
                actual_flow: b.actual_flow,
            })
            .collect(),
        cyl,
        z_now,
    })
}
