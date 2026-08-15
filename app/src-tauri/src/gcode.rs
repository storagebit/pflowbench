// gcode.rs -- calibration G-code generation and the catalogues feeding it:
// profile listing, reference identification, and the local-IP derivation
// that arms the printer's UDP metrics streaming for the job.

use crate::logging;
use crate::state::{expand, AppState};
use std::path::PathBuf;
use tauri::State;

/// LAN address this machine would use to reach `target`. Opens a UDP socket and
/// reads back the chosen source address -- connect() on UDP sends nothing.
#[tauri::command]
pub(crate) fn local_ip(target: String) -> Result<String, String> {
    logging::trace("cmd:local_ip", format!("target={target}"));
    let s = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    let host = target.trim().split(':').next().unwrap_or("192.0.2.1").to_string();
    s.connect((host.as_str(), 80)).map_err(|e| {
        logging::warn("cmd:local_ip", format!("connect probe to {host}:80 failed: {e}"));
        e.to_string()
    })?;
    let ip = s.local_addr().map(|a| a.ip().to_string()).map_err(|e| e.to_string())?;
    logging::trace("cmd:local_ip", format!("resolved {ip}"));
    Ok(ip)
}

/// Generate the flowcliff test G-code directly via the flowgen crate (no more
/// shelling out to a separately-built bin/flowcliff -- see the README's old
/// Step 4), then widen M555 so mesh bed levelling covers the whole plate
/// instead of the dummy reference object's footprint.
#[tauri::command]
pub(crate) fn gcode_generate(
    state: State<AppState>,
    reference: String,
    out: String,
    // Printer address, used to derive the metrics host when it isn't supplied.
    addr: String,
    // Optional path to a .profile file. When set it supplies the whole
    // parameter set (temps, flow ladder, bed, geometry) and its own reference
    // export, so a material switch is one selection rather than a dozen fields.
    profile: String,
    metrics_host: String,
    metrics_port: u16,
    temps: String,
    flows: String,
    bed_w: f64,
    bed_h: f64,
) -> Result<String, String> {
    let (refp, outp) = (expand(&reference), expand(&out));
    logging::trace(
        "cmd:gcode_generate",
        format!("reference={refp} out={outp} metrics_host={metrics_host:?} metrics_port={metrics_port} temps={temps:?} flows={flows:?}"),
    );
    if !std::path::Path::new(&refp).exists() {
        logging::error("cmd:gcode_generate", format!("{refp} not found"));
        return Err(format!("{refp} not found -- export an ASCII G-code from PrusaSlicer"));
    }
    if let Some(dir) = std::path::Path::new(&outp).parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    // A profile, when given, is the source of truth for the parameter set --
    // and brings its own reference export, since the machine start block must
    // match the material (bed/chamber temps, levelling and purge sequence).
    let (mut cfg, chosen_ref) = if !profile.trim().is_empty() {
        let prof = flowgen::Profile::load(profile.trim()).map_err(|e| {
            logging::error("cmd:gcode_generate", e.clone());
            e
        })?;
        for w in prof.lint() {
            logging::warn("cmd:gcode_generate", format!("profile '{}': {w}", prof.name));
        }
        logging::info(
            "cmd:gcode_generate",
            format!(
                "profile '{}': temps {:?}, flows {:?} mm3/s, bed {} C, nozzle {:.1}mm",
                prof.name, prof.cfg.temps, prof.cfg.flows, prof.cfg.bed, prof.cfg.nozzle
            ),
        );
        let r = prof.reference.clone().map(|r| expand(&r)).unwrap_or_else(|| refp.clone());
        (prof.cfg, r)
    } else {
        (flowgen::Cfg::default(), refp.clone())
    };
    if !std::path::Path::new(&chosen_ref).exists() {
        let msg = format!(
            "reference export {chosen_ref} not found -- it must be sliced with THIS \
             material and nozzle, since its start block carries the bed/chamber \
             temperatures and the levelling sequence"
        );
        logging::error("cmd:gcode_generate", msg.clone());
        return Err(msg);
    }

    // Identify the reference from its own settings footer and refuse to splice
    // one material's start block into another material's test. A PETG-named
    // profile was pointed at a PC-sliced export for this project's whole life
    // without anything noticing; the start block decides bed and chamber
    // temperature, so that silently prints at the wrong parameters.
    if let Ok(info) = flowgen::RefInfo::load(&chosen_ref) {
        logging::info("cmd:gcode_generate", format!("reference {chosen_ref}: {}", info.summary()));
        if let Some(mvs) = info.max_volumetric_speed {
            let hi = cfg.flows.iter().cloned().fold(0.0f64, f64::max);
            logging::info(
                "cmd:gcode_generate",
                format!(
                    "vendor publishes {mvs} mm3/s for this filament; ladder tops out at {hi} \
                     ({:.0}% of published)",
                    hi / mvs * 100.0
                ),
            );
        }
        if !profile.trim().is_empty() {
            let name = flowgen::Profile::load(profile.trim()).map(|p| p.name).unwrap_or_default();
            let bad = flowgen::check_match(&name, &info);
            if !bad.is_empty() {
                let msg = format!(
                    "reference/profile mismatch -- {}. Profile: '{name}'. Reference: {} ({}).",
                    bad.join("; "),
                    chosen_ref,
                    info.summary()
                );
                logging::error("cmd:gcode_generate", msg.clone());
                return Err(msg);
            }
        }
    } else {
        logging::warn(
            "cmd:gcode_generate",
            format!("could not read {chosen_ref} to identify its material"),
        );
    }

    cfg.out = outp.clone();
    cfg.reference = Some(chosen_ref);

    // Without M334/M331 the printer never streams loadcell_value or pos_z, so
    // the capture silently collects nothing and the whole run is wasted. Don't
    // depend on a UI field having been filled in -- derive the address from the
    // route to the printer, and fail only if even that is impossible.
    let host = if metrics_host.trim().is_empty() {
        let derived = local_ip(addr.clone()).map_err(|e| {
            let msg = format!(
                "no metrics host set, and it could not be derived from the route to {addr} ({e}). \
                 The printer would never stream loadcell_value/pos_z, so the run would capture nothing."
            );
            logging::error("cmd:gcode_generate", msg.clone());
            msg
        })?;
        logging::info(
            "cmd:gcode_generate",
            format!("metrics host was blank; derived {derived} from the route to {addr}"),
        );
        derived
    } else {
        metrics_host.trim().to_string()
    };
    cfg.metrics_host = Some(host);
    cfg.metrics_port = metrics_port as i64;
    // A form field left holding the previous material's numbers must not quietly
    // beat the profile it is shown next to. Overriding is still allowed -- that
    // is the point of the field -- but it is never silent.
    let using_profile = !profile.trim().is_empty();
    if !temps.trim().is_empty() {
        let t = flowgen::parse_i64_list(&temps)?;
        if using_profile && t != cfg.temps {
            logging::warn(
                "cmd:gcode_generate",
                format!("OVERRIDE: form temps {t:?} replace the profile's {:?}", cfg.temps),
            );
        }
        cfg.temps = t;
    }
    if !flows.trim().is_empty() {
        let f = flowgen::parse_f64_list(&flows)?;
        if using_profile && f != cfg.flows {
            logging::warn(
                "cmd:gcode_generate",
                format!("OVERRIDE: form flows {f:?} replace the profile's {:?}", cfg.flows),
            );
        }
        cfg.flows = f;
    }

    let cfg_diameter = cfg.diameter;
    let cfg_photo_pose = cfg.photo_pose;
    let cfg_park = (
        cfg.photo_park_x.unwrap_or(cfg.bed_x - 5.0),
        cfg.photo_park_y.unwrap_or(cfg.bed_y - 5.0),
    );
    let report = flowgen::generate(cfg).inspect_err(|e| {
        logging::error("cmd:gcode_generate", e.clone());
    })?;
    logging::info("cmd:gcode_generate", report.summary.clone());
    *state.last_layout.lock().unwrap() =
        Some((cfg_diameter, report.cylinder_positions.clone()));
    // A parked head INSIDE the camera frame is invisible to every motion-
    // and speed-based guard -- the one failure class only geometry catches.
    if cfg_photo_pose {
        if let Ok(calib) = std::fs::read_to_string("runs/vision.calib") {
            if let Ok(h) = flowvision::Homography::from_text(&calib) {
                let (ix, iy) = h.map(cfg_park.0, cfg_park.1);
                if (-100.0..2020.0).contains(&ix) && (-100.0..1180.0).contains(&iy) {
                    logging::warn(
                        "cmd:gcode_generate",
                        format!(
                            "photo park ({}, {}) projects INTO the camera frame at                              ({ix:.0}, {iy:.0}) -- every still would show the parked head;                              move photo_park_x/y out of view",
                            cfg_park.0, cfg_park.1
                        ),
                    );
                }
            }
        }
    }
    logging::trace(
        "cmd:gcode_generate",
        format!("layout stashed for vision: {} test objects", report.cylinder_positions.len()),
    );

    let widened = widen_m555(&outp, bed_w, bed_h)?;
    // Manifest LAST: it indexes the file's final bytes, and the M555 rewrite
    // above just shifted every offset. This is what gives the capture
    // deterministic sdpos band addressing.
    match flowgen::write_band_manifest(&outp) {
        Ok(m) => logging::info("cmd:gcode_generate", format!("band manifest: {m}")),
        Err(e) => logging::error("cmd:gcode_generate", format!("band manifest failed: {e}")),
    }
    logging::info("cmd:gcode_generate", format!("wrote {outp}, M555 widened: {widened}"));
    Ok(format!("{} | M555 widened: {widened} | {outp}", report.summary))
}

/// Where profiles live. The app's working directory depends on how it was
/// launched (cargo run from app/src-tauri, or a bundled .app), so try the
/// obvious candidates rather than assuming one.
fn profiles_dir() -> Option<PathBuf> {
    for cand in ["profiles", "../../profiles", "../profiles"] {
        let p = PathBuf::from(cand);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// Named material/nozzle profiles found on disk, with any lint warnings so
/// the UI can show a problem before an hour of printing rather than after.
#[tauri::command]
pub(crate) fn profiles_list() -> Result<Vec<serde_json::Value>, String> {
    let dir = profiles_dir().ok_or("no profiles/ directory found")?;
    logging::trace("cmd:profiles_list", format!("scanning {}", dir.display()));
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("profile") {
            continue;
        }
        let ps = path.to_string_lossy().into_owned();
        match flowgen::Profile::load(&ps) {
            Ok(p) => {
                let (lo, hi) = p.flow_span();
                // Identify the reference from its own contents rather than its
                // name, and carry any material mismatch straight into the UI.
                let mut lint = p.lint();
                let mut ref_desc = String::from("(none)");
                let mut published = serde_json::Value::Null;
                if let Some(r) = &p.reference {
                    let rp = expand(r);
                    match flowgen::RefInfo::load(&rp) {
                        Ok(info) => {
                            ref_desc = info.summary();
                            if let Some(m) = info.max_volumetric_speed {
                                published = serde_json::json!(m);
                            }
                            for w in flowgen::check_match(&p.name, &info) {
                                lint.push(format!("REFERENCE MISMATCH: {w}"));
                            }
                        }
                        Err(e) => {
                            ref_desc = format!("{r} -- UNREADABLE");
                            lint.push(format!("reference cannot be read: {e}"));
                        }
                    }
                }
                out.push(serde_json::json!({
                    "path": ps,
                    "name": p.name,
                    "notes": p.notes,
                    "reference": p.reference,
                    "refDesc": ref_desc,
                    "published": published,
                    "temps": p.cfg.temps,
                    "flows": p.cfg.flows,
                    "bed": p.cfg.bed,
                    "fan": p.cfg.fan,
                    "brim": p.cfg.brim,
                    "em": p.cfg.em,
                    "revs": p.cfg.revs,
                    "nozzle": p.cfg.nozzle,
                    "layerH": p.cfg.layer_h,
                    "firstLayerH": p.cfg.first_layer_h,
                    "flowLo": lo,
                    "flowHi": hi,
                    "lint": lint,
                    "error": serde_json::Value::Null,
                }));
            }
            Err(err) => {
                logging::warn("cmd:profiles_list", format!("{ps}: {err}"));
                out.push(serde_json::json!({ "path": ps, "name": ps, "error": err }));
            }
        }
    }
    out.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));
    logging::info("cmd:profiles_list", format!("{} profile(s)", out.len()));
    Ok(out)
}

/// Every reference export on disk, identified from its own settings footer.
///
/// A reference is not a neutral template: its start block fixes the bed and
/// chamber temperature, the levelling pass and the purge. Knowing what each
/// one actually is -- rather than what its filename suggests -- is the whole
/// point of keeping this catalogue.
#[tauri::command]
pub(crate) fn reference_catalog() -> Result<Vec<serde_json::Value>, String> {
    let dir = ["reference", "../../reference", "../reference"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_dir())
        .ok_or("no reference/ directory found")?;
    logging::trace("cmd:reference_catalog", format!("scanning {}", dir.display()));
    let found = flowgen::catalog(&dir.to_string_lossy());
    for i in &found {
        logging::info("cmd:reference_catalog", format!("{}: {}", i.path, i.summary()));
    }
    logging::info("cmd:reference_catalog", format!("{} reference export(s)", found.len()));
    Ok(found
        .into_iter()
        .map(|i| {
            serde_json::json!({
                "path": i.path,
                "summary": i.summary(),
                "filament": i.filament,
                "type": i.filament_type,
                "nozzle": i.nozzle,
                "highFlow": i.nozzle_high_flow,
                "abrasive": i.abrasive,
                "temp": i.temp,
                "bed": i.first_layer_bed,
                "chamber": i.chamber,
                "chamberMin": i.chamber_minimal,
                "published": i.max_volumetric_speed,
                "em": i.extrusion_multiplier,
                "printer": i.printer_model,
            })
        })
        .collect())
}

/// Widen M555 so mesh bed levelling covers the whole plate instead of the
/// dummy reference object's footprint. Returns how many lines were widened.
fn widen_m555(path: &str, bed_w: f64, bed_h: f64) -> Result<usize, String> {
    let body = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut widened = 0usize;
    let fixed: Vec<String> = body
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("M555 ") {
                widened += 1;
                format!("M555 X0 Y0 W{bed_w:.0} H{bed_h:.0} ; widened by Flowbench")
            } else {
                l.to_string()
            }
        })
        .collect();
    if widened > 0 {
        std::fs::write(path, fixed.join("\n") + "\n").map_err(|e| e.to_string())?;
    }
    Ok(widened)
}

/// Generate the camera calibration print (four pillars of known height at
/// the bench layout positions). Persists the pillar geometry to
/// runs/vision.pillars for the modal's guided click flow.
#[tauri::command]
pub(crate) fn gcode_generate_calibration(
    reference: String,
    out: String,
    profile: String,
    temps: String,
) -> Result<String, String> {
    let (refp, outp) = (expand(&reference), expand(&out));
    logging::trace(
        "cmd:gcode_generate_calibration",
        format!("reference={refp} out={outp} profile={profile:?}"),
    );
    let mut cfg = if profile.trim().is_empty() {
        flowgen::Cfg::default()
    } else {
        flowgen::Profile::load(&expand(profile.trim()))?.cfg
    };
    if let Ok(t) = flowgen::parse_i64_list(&temps) {
        if !t.is_empty() {
            cfg.temps = t;
        }
    }
    if !std::path::Path::new(&refp).exists() {
        return Err(format!("{refp} not found -- export an ASCII G-code from PrusaSlicer"));
    }
    cfg.reference = Some(refp);
    cfg.standalone = false;
    cfg.out = outp.clone();
    if let Some(parent) = std::path::Path::new(&outp).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let report = flowgen::generate_calibration(cfg.clone()).inspect_err(|e| {
        logging::error("cmd:gcode_generate_calibration", e.clone());
    })?;
    let widened = widen_m555(&outp, cfg.bed_x, cfg.bed_y)?;
    logging::info(
        "cmd:gcode_generate_calibration",
        format!("M555 widened: {widened}"),
    );
    std::fs::create_dir_all("runs").ok();
    let mut text = String::new();
    for p in &report.pillars {
        text.push_str(&format!("{} {} {}\n", p.x, p.y, p.height));
    }
    std::fs::write("runs/vision.pillars", text).map_err(|e| e.to_string())?;
    logging::info("cmd:gcode_generate_calibration", report.summary.clone());
    Ok(report.summary)
}
