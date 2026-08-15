// vision.rs -- vision commands: solve the bed-to-image homography from four
// clicked brim centres, and run the Tier-1 per-band still analysis over a
// run directory. The measurements themselves live in crates/flowvision.

use crate::logging;
use crate::state::{expand, AppState};
use std::path::PathBuf;
use tauri::State;

/// Solve the bed-to-image homography from four clicked brim centres, paired
/// in print order with the last generated layout. Stored at runs/vision.calib
/// (the camera is fixed, so the calibration outlives any one run) and copied
/// into the active run directory when there is one.
#[tauri::command]
pub(crate) fn vision_calibrate(state: State<AppState>, clicks: Vec<Vec<f64>>) -> Result<String, String> {
    logging::trace("cmd:vision_calibrate", format!("clicks={clicks:?}"));
    if clicks.len() != 4 || clicks.iter().any(|c| c.len() != 2) {
        return Err("need exactly 4 [x, y] image points".into());
    }
    let layout = state.last_layout.lock().unwrap().clone();
    let (_, positions) = layout.ok_or("no layout: generate G-code first, then calibrate")?;
    if positions.len() < 4 {
        return Err(format!(
            "layout has {} test objects; calibration needs 4 -- use 4 temperatures",
            positions.len()
        ));
    }
    let mut pairs = [((0.0, 0.0), (0.0, 0.0)); 4];
    for i in 0..4 {
        let (_, bx, by) = positions[i];
        pairs[i] = ((bx, by), (clicks[i][0], clicks[i][1]));
    }
    // A Row layout puts all four brim centres on one bed line, and four
    // collinear points cannot define a plane homography. Say so before the
    // solver's generic "degenerate" error confuses anyone.
    let area2 = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
        ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs()
    };
    let beds: Vec<(f64, f64)> = pairs.iter().map(|(b, _)| *b).collect();
    if area2(beds[0], beds[1], beds[2]) < 1.0 && area2(beds[0], beds[1], beds[3]) < 1.0 {
        return Err("test objects are collinear on the bed (row layout) -- vision \
                    calibration needs a stagger or grid layout"
            .into());
    }
    let h = flowvision::Homography::solve(&pairs).map_err(|e| {
        logging::warn("cmd:vision_calibrate", e.clone());
        e
    })?;
    let _ = std::fs::create_dir_all("runs");
    let path = PathBuf::from("runs").join("vision.calib");
    std::fs::write(&path, h.to_text()).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(dir) = state.run_dir.lock().unwrap().clone() {
        let _ = std::fs::write(dir.join("vision.calib"), h.to_text());
    }
    // 4 points = an exact solve; there is no residual to report. Say which
    // test objects the clicks were paired with instead -- with more than 4 on
    // the bed, that pairing is the thing to get wrong.
    let paired: Vec<String> = pairs
        .iter()
        .map(|((bx, by), _)| format!("({bx:.0},{by:.0})"))
        .collect();
    let msg = format!(
        "calibrated against the first 4 test objects in print order: {} -> {}",
        paired.join(" "),
        path.display()
    );
    logging::info("cmd:vision_calibrate", msg.clone());
    Ok(msg)
}

/// Tier 1 vision pass over a run directory's per-band stills. Growth and
/// top-edge raggedness per band, self-normalized, votes downgrade-only --
/// see crates/flowvision. `dir` empty = the active/last run directory.
#[tauri::command]
pub(crate) fn vision_analyze(state: State<AppState>, dir: String) -> Result<serde_json::Value, String> {
    logging::trace("cmd:vision_analyze", format!("dir={dir:?}"));
    let dir = if dir.trim().is_empty() {
        state.run_dir.lock().unwrap().clone().ok_or("no run directory yet -- pass one")?
    } else {
        PathBuf::from(expand(dir.trim()))
    };
    let bands = analyze_dir(&state, &dir)?;
    let mut stalls = 0usize;
    let mut rows = Vec::new();
    for b in &bands {
        if b.vote == flowvision::Vote::Stall {
            stalls += 1;
        }
        rows.push(serde_json::json!({
            "cylinder": b.cylinder,
            "band": b.band + 1,
            "flow": b.flow,
            "vote": format!("{:?}", b.vote),
            "heightPx": b.measure.height_px,
            "growthPx": b.growth_px,
            "raggedness": b.raggedness_ratio,
            "usable": b.usable,
            "note": b.note,
        }));
    }
    logging::info(
        "cmd:vision_analyze",
        format!("{} bands analyzed, {stalls} stalled -- votes only ever downgrade", rows.len()),
    );
    Ok(serde_json::json!({ "dir": dir.to_string_lossy(), "bands": rows }))
}

/// The reusable analysis core: every band still in `dir`, judged. Used by
/// the vision command above and by the verdict's vision family.
pub(crate) fn analyze_dir(
    state: &State<AppState>,
    dir: &std::path::Path,
) -> Result<Vec<flowvision::BandVision>, String> {
    let calib = [dir.join("vision.calib"), PathBuf::from("runs").join("vision.calib")]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .ok_or("no vision.calib found -- calibrate first")?;
    let h = flowvision::Homography::from_text(&calib)?;
    // Full camera model beats the plane homography when present: measured
    // vertical scale and viewing angle at every position (vision modal).
    let camera = [dir.join("vision.camera"), PathBuf::from("runs").join("vision.camera")]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| flowvision::Projection::from_text(&text).ok());
    let layout_file = std::fs::read_to_string(dir.join("vision.layout")).ok().and_then(|text| {
        let mut lines = text.lines();
        let dia: f64 = lines.next()?.strip_prefix("diameter ")?.trim().parse().ok()?;
        let mut positions = Vec::new();
        for l in lines {
            let mut it = l.split_whitespace();
            positions.push((
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
            ));
        }
        Some((dia, positions))
    });
    let layout = layout_file.or_else(|| state.last_layout.lock().unwrap().clone());
    let (dia, positions) =
        layout.ok_or("no vision.layout in the run directory and nothing generated this session")?;

    // which cylinders have stills on disk?
    let mut cyls: Vec<usize> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| flowvision::parse_name(&e.file_name().to_string_lossy()))
        .map(|(c, ..)| c)
        .collect();
    cyls.sort_unstable();
    cyls.dedup();
    if cyls.is_empty() {
        return Err(format!("no per-band .pgm stills in {}", dir.display()));
    }

    let cfg = flowvision::VisionCfg::default();
    let mut bands = Vec::new();
    for c in cyls {
        let Some((_, bx, by)) = positions.get(c) else {
            logging::warn(
                "cmd:vision_analyze",
                format!("cylinder {c} has stills but no layout position -- skipped"),
            );
            continue;
        };
        let mut roi = match &camera {
            Some(p) => flowvision::Roi::from_projection(p, *bx, *by, dia, 25.0, 1920, 1080, 30),
            None => flowvision::Roi::from_homography(&h, *bx, *by, dia, 1920, 1080, 30),
        };
        // clip against neighbouring test_objects: their crowns projecting into
        // this window fake tops (measured on the real run's wide-ROI attempt)
        let (cx_this, _) = h.map(*bx, *by);
        for (i, (_, ox, oy)) in positions.iter().enumerate() {
            if i == c {
                continue;
            }
            let (cx_o, _) = h.map(*ox, *oy);
            let mid = ((cx_this + cx_o) / 2.0) as usize;
            if cx_o > cx_this {
                roi.x1 = roi.x1.min(mid.saturating_sub(5));
            } else {
                roi.x0 = roi.x0.max(mid + 5);
            }
        }
        match flowvision::analyze_cylinder(&dir, c, &roi, &cfg) {
            Ok(mut res) => bands.append(&mut res),
            Err(e) => logging::warn("cmd:vision_analyze", format!("cylinder {c}: {e}")),
        }
    }
    Ok(bands)
}

// ---------------------------------------------------- camera calibration

/// Is the camera calibrated for vision, and with what? Backs the
/// calibration modal: file presence decides, the info sidecar carries the
/// date and residual, the pillar list drives the guided click flow.
#[tauri::command]
pub(crate) fn vision_camera_status() -> serde_json::Value {
    let calibrated = std::path::Path::new("runs/vision.camera").exists();
    let info = std::fs::read_to_string("runs/vision.camera.info").unwrap_or_default();
    let pillars: Vec<Vec<f64>> = std::fs::read_to_string("runs/vision.pillars")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            (v.len() == 3).then_some(v)
        })
        .collect();
    serde_json::json!({ "calibrated": calibrated, "info": info.trim(), "pillars": pillars })
}

/// Solve the full camera model from the calibration print: two clicks per
/// pillar (base centre, then top centre), paired with the printed geometry
/// from runs/vision.pillars. Stored persistently; every later vision pass
/// prefers it over the plane homography.
#[tauri::command]
pub(crate) fn vision_camera_calibrate(
    state: State<AppState>,
    clicks: Vec<Vec<f64>>,
) -> Result<String, String> {
    logging::trace("cmd:vision_camera_calibrate", format!("clicks={}", clicks.len()));
    let pillars: Vec<(f64, f64, f64)> = std::fs::read_to_string("runs/vision.pillars")
        .map_err(|_| "no runs/vision.pillars -- generate the calibration print first")?
        .lines()
        .filter_map(|l| {
            let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            (v.len() == 3).then(|| (v[0], v[1], v[2]))
        })
        .collect();
    if clicks.len() != pillars.len() * 2 || clicks.iter().any(|c| c.len() != 2) {
        return Err(format!(
            "need {} clicks (base+top per pillar), got {}",
            pillars.len() * 2,
            clicks.len()
        ));
    }
    let mut points = Vec::new();
    for (i, (x, y, h)) in pillars.iter().enumerate() {
        points.push(((*x, *y, 0.0), (clicks[2 * i][0], clicks[2 * i][1])));
        points.push(((*x, *y, *h), (clicks[2 * i + 1][0], clicks[2 * i + 1][1])));
    }
    let p = flowvision::Projection::solve(&points).map_err(|e| {
        logging::warn("cmd:vision_camera_calibrate", e.clone());
        e
    })?;
    let worst = p.worst_residual(&points);
    // The solve is overdetermined, so the residual is a real quality gate:
    // a mis-click shows up here instead of poisoning every later verdict.
    if worst > 5.0 {
        let msg = format!(
            "worst reprojection {worst:.1} px (> 5) -- one of the clicks is off, redo them"
        );
        logging::warn("cmd:vision_camera_calibrate", msg.clone());
        return Err(msg);
    }
    std::fs::create_dir_all("runs").ok();
    std::fs::write("runs/vision.camera", p.to_text()).map_err(|e| e.to_string())?;
    let scale_front = p.vertical_px_per_mm(pillars[0].0, pillars[0].1);
    let info = format!(
        "calibrated {} | worst residual {worst:.2} px | vertical scale at ({:.0},{:.0}): {scale_front:.1} px/mm | {} pillars",
        crate::snapshots::run_dir_name(),
        pillars[0].0,
        pillars[0].1,
        pillars.len()
    );
    std::fs::write("runs/vision.camera.info", &info).ok();
    if let Some(dir) = state.run_dir.lock().unwrap().clone() {
        let _ = std::fs::write(dir.join("vision.camera"), p.to_text());
    }
    logging::info("cmd:vision_camera_calibrate", info.clone());
    Ok(info)
}
