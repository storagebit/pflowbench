// config.rs -- the full parameter set for one generated job.
//
// Every tunable the generator honours lives on this one struct, so a run is
// a single reviewable value: a profile fills it, the app's form edits it,
// `generate()` consumes it. Defaults must stay printable as-is on the target
// machine; anything material-specific belongs in a profile, not here.

use crate::layout::Layout;

#[derive(Clone, Debug, PartialEq)]
pub struct Cfg {
    pub out: String,
    pub reference: Option<String>,
    pub standalone: bool,
    pub temps: Vec<i64>,
    pub flows: Vec<f64>,
    /// Add a test object printing the ladder top-to-bottom, as a Z control.
    pub reversed_control: bool,
    /// Add a test object at this constant flow for every band, as a Z control.
    pub constant_control: Option<f64>,
    pub layout: Layout,
    /// Shifts the whole arrangement on the bed, to aim it at the camera.
    pub layout_offset: (f64, f64),
    pub revs: usize,
    pub diameter: f64,
    pub layer_h: f64,
    pub width: f64,
    pub first_layer_h: f64,
    pub first_layer_w: f64,
    pub first_layer_flow: f64,
    pub brim: usize,
    pub em: f64,
    pub pa: f64,
    pub bed: i64,
    pub fan: i64,
    pub dwell: i64,
    pub nozzle: f64,
    pub retract: f64,
    pub seg_len: f64,
    pub safe_z: f64,
    pub travel_f: i64,
    pub bed_x: f64,
    pub bed_y: f64,
    pub purge_x: f64,
    pub purge_y: f64,
    pub purge_len: f64,
    pub purge_w: f64,
    pub purge_pitch: f64,
    pub only_temp: Option<i64>,
    /// Vision Tier 0: park the head out of the camera's view at every band
    /// boundary and dwell, so the capture can save a fresh, unoccluded frame.
    pub photo_pose: bool,
    /// Seconds parked per photo window. Must cover the app's park-latency
    /// wait (the hook fires at segment entry, up to several seconds before
    /// the head actually parks -- planner queue plus the safe_z climb and
    /// cross-bed travel) with margin for one full-rate frame.
    pub photo_dwell: f64,
    /// Park position. None = bed back-right corner (clear of test objects and,
    /// with the stock chamber camera, out of frame).
    pub photo_park_x: Option<f64>,
    pub photo_park_y: Option<f64>,
    pub metrics_host: Option<String>,
    pub metrics_port: i64,
    pub metrics_disable: Vec<String>,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            out: "flowcliff.gcode".into(),
            reference: None,
            standalone: false,
            temps: vec![255, 265, 275, 285],
            flows: vec![8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0],
            reversed_control: false,
            constant_control: None,
            layout: Layout::default(),
            layout_offset: (0.0, 0.0),
            revs: 4,
            diameter: 50.0,
            layer_h: 0.4,
            width: 0.9,
            first_layer_h: 0.2,
            first_layer_w: 1.0,
            first_layer_flow: 8.0,
            brim: 3,
            em: 1.03,
            pa: 0.018,
            bed: 85,
            fan: 128,
            dwell: 0,
            nozzle: 0.8,
            retract: 0.8,
            seg_len: 1.0,
            safe_z: 25.0,
            travel_f: 9000,
            bed_x: 300.0,
            bed_y: 300.0,
            purge_x: 60.0,
            purge_y: 8.0,
            purge_len: 120.0,
            purge_w: 1.2,
            purge_pitch: 4.0,
            only_temp: None,
            photo_pose: false,
            photo_dwell: 5.0,
            photo_park_x: None,
            photo_park_y: None,
            metrics_host: None,
            metrics_port: 8514,
            metrics_disable: Vec::new(),
        }
    }
}
