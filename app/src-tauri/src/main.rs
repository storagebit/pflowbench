// pflowbench-app -- thin Tauri glue. PFlowBench = Prusa FlowBench. All logic lives in flowcore / prusalink /
// flowgen, which are compiled and unit-tested independently of this crate.
//
// Secrets policy: the PrusaLink API key or digest password is stored ONLY in the
// OS keychain via the `keyring` crate (service "flowbench"). It is never written
// to config files, never logged, and never echoed back to the frontend --
// key_status() returns only whether an entry exists.
//
// Logging policy: every command traces its entry (parameters, minus secrets)
// and logs warn/error on anything that didn't go as expected. See logging.rs.
// Two deliberate exceptions for commands on a tight polling loop, where
// per-call tracing would flood the console with noise for a run's whole
// duration instead of surfacing anything that changed:
//   - capture_delta: polled every 200ms while a capture is running. The
//     capture thread itself (flowcore::capture, via logging::sink) already
//     reports its own milestones at a sane, bounded rate.
//   - printer_status: polled every 2s while a capture is running (~450
//     calls over a 15-minute run). Logs only on printer.state transitions
//     (READY -> PRINTING -> FINISHED), via prusalink_client(..., false) to
//     also suppress the per-request "-> GET / <- 200" noise underneath it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod capture;
mod gcode;
mod keys;
mod logging;
mod printer;
mod snapshots;
mod state;
mod stills;
mod vision;

use state::AppState;
use std::sync::{Arc, Mutex};

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            logging::init(app.handle().clone());
            logging::info("app", "PFlowBench backend started".to_string());
            Ok(())
        })
        .manage(AppState {
            capture: Mutex::new(None),
            last_printer_state: Mutex::new(None),
            camera: Arc::new(Mutex::new(None)),
            run_dir: Arc::new(Mutex::new(None)),
            last_run: Mutex::new(None),
            last_layout: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            keys::key_save,
            keys::key_status,
            logging::ui_log,
            keys::key_clear,
            printer::printer_info,
            printer::printer_status,
            printer::printer_storage,
            printer::printer_upload,
            printer::printer_beep,
            gcode::local_ip,
            gcode::gcode_generate,
            gcode::gcode_generate_calibration,
            gcode::profiles_list,
            gcode::reference_catalog,
            capture::capture_start,
            capture::capture_stop,
            capture::capture_delta,
            capture::capture_export,
            camera::camera_start,
            camera::camera_stop,
            camera::camera_preview,
            camera::camera_test,
            camera::camera_stats,
            camera::camera_set_live,
            camera::camera_record,
            camera::camera_write_timelapse,
            vision::vision_calibrate,
            vision::vision_analyze,
            vision::vision_camera_status,
            vision::vision_camera_calibrate
        ])
        .run(tauri::generate_context!())
        .expect("error while running PFlowBench");
}
