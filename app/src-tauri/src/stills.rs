// stills.rs -- per-band camera stills. The flowcore capture thread fires a
// hook at each band boundary (or photo window); the worker built here grabs
// one full-resolution frame per firing, on its own thread so the UDP receive
// loop never blocks, and names the file *_stale when no fresh frame arrived.

use crate::logging;
use crate::snapshots;
use crate::state::DropFlag;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Pushed to the UI as each flow band completes, for the snapshot strip.
#[derive(Serialize, Clone)]
struct BandSnapshot {
    cylinder: usize,
    /// 1-based, matching the stats table.
    band: usize,
    flow: f64,
    z: f64,
    image: String,
    /// Full-resolution file on disk, for later caliper cross-checking.
    file: String,
}

/// Save a full-resolution still per band. The saver runs on its OWN thread
/// -- the flowcore hooks run on the UDP receive loop, and a blocking wait
/// there drops samples. Freshness gate: the camera decodes keyframes only
/// (~3s cadence), so the worker flips to full-rate decode, waits for a
/// frame NEWER than the last one it saved, then flips back. A frame that
/// never freshens within the window is still saved but named *_stale --
/// the first real run held one frozen frame across whole cylinders and
/// nothing flagged it.
pub(crate) fn save_still(
    camera: Arc<Mutex<Option<flowcam::Camera>>>,
    run_dir: Arc<Mutex<Option<PathBuf>>>,
) -> Arc<dyn Fn(flowcore::BandChange) + Send + Sync> {
    let last_saved_seq = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let saver_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    Arc::new(move |bc: flowcore::BandChange| {
        if saver_busy.swap(true, Ordering::SeqCst) {
            logging::warn(
                "camera",
                format!("snapshot for band {} skipped: previous save still running", bc.band + 1),
            );
            return;
        }
        let camera = camera.clone();
        let run_dir = run_dir.clone();
        let last_seq = last_saved_seq.clone();
        let busy = saver_busy.clone();
        std::thread::spawn(move || {
            let _reset = DropFlag(busy); // clears busy however this exits
            let dir = match run_dir.lock().unwrap().clone() {
                Some(d) => d,
                None => return,
            };
            // flowcore fires this hook when the head is demonstrably
            // stationary (3 consecutive derived speeds < 0.5 mm/s inside
            // the photo segment), or on its 5 s fallback. A short settle
            // then covers auto-exposure recovery after the head leaves
            // the frame; the re-baseline ensures the accepted frame was
            // decoded after it.
            let cheap_seq = |camera: &Arc<Mutex<Option<flowcam::Camera>>>| {
                camera.lock().unwrap().as_ref().and_then(|c| c.preview()).map(|p| p.seq)
            };
            let was_live = {
                let slot = camera.lock().unwrap();
                match slot.as_ref() {
                    Some(c) => {
                        let l = c.is_live();
                        c.set_live(true);
                        l
                    }
                    None => return,
                }
            };
            const PARK_LATENCY: std::time::Duration = std::time::Duration::from_millis(600);
            const DEADLINE: std::time::Duration = std::time::Duration::from_secs(6);
            let t0 = std::time::Instant::now();
            std::thread::sleep(PARK_LATENCY);
            // Re-baseline AFTER the wait: also self-heals a mid-run RTSP
            // reconnect, which resets seq to 0.
            let baseline = cheap_seq(&camera).unwrap_or(0);
            let mut fresh = false;
            while std::time::Instant::now() - t0 < DEADLINE {
                match cheap_seq(&camera) {
                    Some(s) if s > baseline => {
                        fresh = true;
                        break;
                    }
                    Some(_) => {}
                    None => break, // camera disconnected mid-save
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            // One lock scope, one internal frame: the JPEG the gate
            // vouches for and the PGM vision reads MUST be the same
            // frame (flowcam::full_snapshot_with_luma).
            let grabbed = {
                let slot = camera.lock().unwrap();
                let cam = match slot.as_ref() {
                    Some(c) => c,
                    None => return,
                };
                let g = cam.full_snapshot_with_luma(95);
                cam.set_live(was_live);
                g
            };
            let (snap, pgm) = match grabbed {
                Some(p) => p,
                None => {
                    logging::warn(
                        "camera",
                        format!("band {}: no frame decoded at all", bc.band + 1),
                    );
                    return;
                }
            };
            last_seq.store(snap.seq, Ordering::SeqCst);
            let mut name = snapshots::band_image_name(bc.cylinder, bc.band, bc.flow);
            if !fresh {
                name = name.replace(".jpg", "_stale.jpg");
                logging::warn(
                    "camera",
                    format!(
                        "band {} still is STALE (no fresh frame within the photo window)",
                        bc.band + 1
                    ),
                );
            }
            let path = dir.join(&name);
            match std::fs::write(&path, &snap.jpeg) {
                Ok(_) => logging::info(
                    "camera",
                    format!(
                        "band {} @ {:.1} mm3/s -> {} ({} KB{})",
                        bc.band + 1,
                        bc.flow,
                        path.display(),
                        snap.jpeg.len() / 1024,
                        if fresh { "" } else { ", stale" },
                    ),
                ),
                Err(e) => logging::warn("camera", format!("writing {}: {e}", path.display())),
            }
            // Lossless luma companion for vision -- same frame as the JPEG.
            let ppath = path.with_extension("pgm");
            if let Err(e) = std::fs::write(&ppath, &pgm) {
                logging::warn("camera", format!("writing {}: {e}", ppath.display()));
            }
            let slot = camera.lock().unwrap();
            if let Some(cam) = slot.as_ref() {
                if let Some(p) = cam.preview() {
                    logging::emit_event(
                        "band-snapshot",
                        BandSnapshot {
                            cylinder: bc.cylinder,
                            band: bc.band + 1,
                            flow: bc.flow,
                            z: bc.z,
                            image: snapshots::jpeg_data_url(&p.jpeg),
                            file: path.to_string_lossy().into_owned(),
                        },
                    );
                }
            }
        });
    })
}
