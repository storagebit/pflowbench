// verdict.rs -- the Phase D command: joins the capture's band statistics
// with vision votes, runs flowcore's rules, and delivers the sentence plus
// the slicer snippet. Output lands in the run directory as verdict.json and
// snippet.txt so a run's result survives the session.

use crate::logging;
use crate::state::AppState;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tauri::State;

fn vote_of(v: flowvision::Vote) -> flowcore::VisionVote {
    match v {
        flowvision::Vote::Grow => flowcore::VisionVote::Grow,
        flowvision::Vote::Marginal => flowcore::VisionVote::Marginal,
        flowvision::Vote::Stall => flowcore::VisionVote::Stall,
        flowvision::Vote::NoVote => flowcore::VisionVote::NoVote,
    }
}

/// Compute the run verdict from the live capture (or the last stopped run).
/// `revs` mirrors the test-parameters card; `margin_rungs` is the safety
/// margin below the measured ceiling.
#[tauri::command]
pub(crate) fn verdict_compute(
    state: State<AppState>,
    revs: usize,
    margin_rungs: usize,
) -> Result<serde_json::Value, String> {
    logging::trace("cmd:verdict_compute", format!("revs={revs} margin={margin_rungs}"));

    let snap = {
        let slot = state.capture.lock().unwrap();
        match slot.as_ref() {
            Some(cap) => cap.snapshot_all(),
            None => state
                .last_run
                .lock()
                .unwrap()
                .clone()
                .ok_or("no capture data -- run a capture first")?,
        }
    };
    if snap.bands.is_empty() {
        return Err("capture has no per-band statistics yet".into());
    }
    // Always the current run's directory: the band statistics come from the
    // capture, and stills from any other directory would silently mix runs.
    let dir = state.run_dir.lock().unwrap().clone().unwrap_or_else(|| PathBuf::from("runs"));

    // vision votes are optional: without calibration the verdict runs on
    // the force family alone and says so
    let mut vision_flag: Option<String> = None;
    let vision: BTreeMap<usize, Vec<(usize, flowcore::VisionVote)>> =
        match crate::vision::analyze_dir(&state, &dir) {
            Ok(bands) => {
                let mut by_cyl: BTreeMap<usize, Vec<(usize, flowcore::VisionVote)>> =
                    BTreeMap::new();
                for b in bands {
                    by_cyl.entry(b.cylinder).or_default().push((b.band + 1, vote_of(b.vote)));
                }
                by_cyl
            }
            Err(e) => {
                logging::warn("cmd:verdict_compute", format!("no vision family: {e}"));
                vision_flag = Some(format!("vision family absent: {e}"));
                BTreeMap::new()
            }
        };

    let mut by_cyl: BTreeMap<usize, Vec<flowcore::BandStat>> = BTreeMap::new();
    for b in &snap.bands {
        by_cyl.entry(b.cylinder).or_default().push(b.clone());
    }
    let cylinders: Vec<flowcore::CylinderInput> = by_cyl
        .into_iter()
        .map(|(cyl, bands)| flowcore::CylinderInput {
            cylinder: cyl,
            temp: snap.cylinder_temps.iter().find(|(c, _)| *c == cyl).map(|(_, t)| *t),
            tare: snap
                .tares
                .iter()
                .find(|(c, _, n)| *c == cyl && *n > 0)
                .map(|(_, m, n)| (*m, *n)),
            vision: vision.get(&cyl).cloned().unwrap_or_default(),
            bands,
        })
        .collect();

    let mut verdict = flowcore::judge(&flowcore::VerdictInput {
        cylinders,
        revs: revs.max(1),
        primary_temp: None,
        margin_rungs,
    });

    if let Some(f) = vision_flag {
        verdict.run_flags.push(f);
    }
    let bands: Vec<serde_json::Value> = verdict
        .bands
        .iter()
        .map(|b| {
            serde_json::json!({
                "cylinder": b.cylinder, "band": b.band, "flow": b.flow,
                "class": format!("{:?}", b.class), "fired": b.fired,
                "confidence": b.confidence,
            })
        })
        .collect();
    let temps: Vec<serde_json::Value> = verdict
        .temps
        .iter()
        .map(|t| {
            serde_json::json!({
                "cylinder": t.cylinder, "temp": t.temp, "ceiling": t.ceiling,
                "sentence": t.sentence,
            })
        })
        .collect();
    let out = serde_json::json!({
        "bands": bands,
        "temps": temps,
        "recommendation": verdict.recommendation,
        "runFlags": verdict.run_flags,
    });

    // persist: the verdict is the run's product
    let _ = std::fs::create_dir_all(&dir);
    if let Err(e) = std::fs::write(dir.join("verdict.json"), out.to_string()) {
        logging::warn("cmd:verdict_compute", format!("verdict.json: {e}"));
    }
    if let Some(rec) = verdict.recommendation {
        let snippet = format!("filament_max_volumetric_speed = {rec}\n");
        if let Err(e) = std::fs::write(dir.join("snippet.txt"), &snippet) {
            logging::warn("cmd:verdict_compute", format!("snippet.txt: {e}"));
        }
        logging::info(
            "cmd:verdict_compute",
            format!("recommended filament_max_volumetric_speed = {rec}"),
        );
    }
    for t in &verdict.temps {
        logging::info(
            "cmd:verdict_compute",
            format!(
                "cylinder {}{}: {}",
                t.cylinder,
                t.temp.map(|x| format!(" ({x} C)")).unwrap_or_default(),
                t.sentence
            ),
        );
    }
    for f in &verdict.run_flags {
        logging::warn("cmd:verdict_compute", f.clone());
    }
    Ok(out)
}
