// The capture session: hooks, shared state, the polled snapshot view, and the
// handle owning the UDP receive thread. The receive loop itself lives in
// capture/run.rs; the loopback tests for the whole session in capture/tests.rs.

use std::collections::BTreeMap;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::bandmap::BandMap;
use crate::sdmap::{SdMap, SegKind};
use crate::stats::Acc;

mod run;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------- logging hook

/// (level, message) sink for trace/info/warn/error events from the capture
/// loop -- level is one of "trace"/"info"/"warn"/"error", matching the
/// on-screen console in app/ui. `None` is a silent no-op, used by this
/// crate's own tests so they stay independent of any log sink; the app layer
/// supplies a real one that also reaches the webview.
pub type LogFn = std::sync::Arc<dyn Fn(&'static str, String) + Send + Sync>;

fn noop_logger() -> LogFn {
    std::sync::Arc::new(|_level, _msg| {})
}

/// Fired when the spiral crosses into a new flow band. Each band is a single
/// commanded flow rate, so this is the natural moment to capture evidence
/// (e.g. a camera still) attributable to exactly one point on the ladder.
///
/// Runs on the capture thread: keep the handler quick and non-blocking, or
/// the UDP receive loop stalls and samples are dropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandChange {
    pub cylinder: usize,
    /// 0-based index into `BandMap::flows`.
    pub band: usize,
    pub flow: f64,
    pub z: f64,
}

pub type BandChangeFn = std::sync::Arc<dyn Fn(BandChange) + Send + Sync>;

/// Optional callbacks for a capture session. Grouped into a struct so adding
/// another hook later doesn't churn every `Capture::start` call site.
#[derive(Default, Clone)]
pub struct CaptureHooks {
    pub logger: Option<LogFn>,
    pub on_band_change: Option<BandChangeFn>,
    /// Fired when sdpos enters a `kind=photo` segment: the head is parked out
    /// of the camera's view and dwelling, so THIS -- not the band change --
    /// is the moment to save a still. `BandChange` carries the band the
    /// window belongs to. Requires a manifest with photo segments.
    pub on_photo_window: Option<BandChangeFn>,
}

impl CaptureHooks {
    pub fn with_logger(logger: LogFn) -> Self {
        CaptureHooks { logger: Some(logger), on_band_change: None, on_photo_window: None }
    }
}

/// One consistent view of the capture, handed to the UI each poll. A struct
/// rather than a tuple: this grew past the point where positional returns
/// were readable or safe to extend.
#[derive(Default, Clone, Debug)]
pub struct Snapshot {
    pub seq: u64,
    pub force: Vec<(f64, f64)>,
    pub z: Vec<(f64, f64)>,
    pub speed: Vec<(f64, f64)>,
    pub temp_noz: Vec<(f64, f64)>,
    pub temp_bed: Vec<(f64, f64)>,
    pub temp_chamber: Vec<(f64, f64)>,
    pub bands: Vec<BandStat>,
    pub cyl: usize,
    pub z_now: f64,
    pub now_noz: Option<f64>,
    pub now_bed: Option<f64>,
    pub now_chamber: Option<f64>,
    /// (cylinder, tare mean g, tare n) -- the loadcell zero per cylinder.
    pub tares: Vec<(usize, f64, u64)>,
    /// (cylinder, band, t_enter, t_exit) in capture seconds.
    pub band_windows: Vec<(usize, usize, f64, f64)>,
    /// (cylinder, nozzle temp C) from the manifest; empty without one.
    pub cylinder_temps: Vec<(usize, i64)>,
}

/// Anything faster than this is a repositioning move, not printing. The
/// generator travels at 9000 mm/min = 150 mm/s, while the fastest print band
/// (24 mm3/s through a 0.33 mm^2 bead) is about 74 mm/s -- so the gap is
/// wide and this cleanly separates the two.
pub const TRAVEL_SPEED_CUTOFF: f64 = 110.0;

// ---------------------------------------------------------------- capture

#[derive(Clone, Debug, Default)]
pub struct BandStat {
    pub cylinder: usize,
    pub band: usize, // 1-based for display
    /// Flow this band was COMMANDED to print at.
    pub flow: f64,
    pub n: u64,
    pub mean: f64,
    pub sd: f64,
    /// How many head-speed samples this band got. Distinct from `n`, which
    /// counts loadcell samples: position streams far slower than force, and a
    /// band can be well covered by one and not the other.
    pub speed_n: u64,
    /// Mean head speed measured over the band, mm/s. `None` when the band got
    /// no speed samples at all -- otherwise "never measured" and "measured as
    /// stationary" both render as 0.0 and cannot be told apart.
    pub speed_mean: Option<f64>,
    pub speed_sd: Option<f64>,
    /// speed x bead cross-section: the flow actually delivered. `None` when
    /// the bead geometry wasn't supplied OR the band has no speed samples.
    /// Compare against `flow` -- a gap means the printer never reached the
    /// commanded feedrate, so the band's label overstates what was really
    /// tested. A `None` here means "unknown", never "zero".
    pub actual_flow: Option<f64>,
    /// Force statistics over the band's SETTLED window only (first revolution
    /// excluded; sdpos addressing required). This is what the verdict judges:
    /// early revolutions carry the previous band's pressure and lean on its
    /// still-sound wall.
    pub settled_n: u64,
    pub settled_mean: Option<f64>,
    pub settled_sd: Option<f64>,
}

#[derive(Default)]
pub struct CaptureState {
    pub seq: u64, // bumps on every appended sample
    pub t0: Option<Instant>,
    /// Firmware `tm=` of the first point seen, in microseconds -- the origin
    /// for the per-sample clock. See the receive loop for why arrival time is
    /// not good enough.
    pub t0_us: Option<i64>,
    pub force: Vec<(f64, f64)>, // (seconds since start, grams)
    pub z: Vec<(f64, f64)>,     // (seconds since start, mm)
    pub acc: BTreeMap<(usize, usize), Acc>,
    /// Head speed (mm/s) accumulated per (cylinder, band).
    pub speed_acc: BTreeMap<(usize, usize), Acc>,
    /// (seconds, mm/s) series, for overlaying on the force plot.
    pub speed: Vec<(f64, f64)>,
    /// Temperature series on the same clock as the force samples, so a
    /// thermal cause for a knee can be checked against it directly.
    pub temp_noz: Vec<(f64, f64)>,
    pub temp_bed: Vec<(f64, f64)>,
    pub temp_chamber: Vec<(f64, f64)>,
    /// Latest values, for the live readout. `Option` because 0.0 is a
    /// legitimate reading: defaulting to it made the UI paint a confident
    /// "0.0 C" over a correct value while the nozzle metric was being dropped.
    pub now_noz: Option<f64>,
    pub now_bed: Option<f64>,
    pub now_chamber: Option<f64>,
    pub cyl: usize,
    pub z_now: f64,
    pub metric_names: BTreeMap<String, u64>,
    pub packets: u64,
    /// Per-cylinder loadcell zero, read during the parked tare dwell. All
    /// force features downstream are deltas from this -- the tare drifts with
    /// temperature (run 1: band means of -13..-104 g).
    pub tare: BTreeMap<usize, Acc>,
    /// Force restricted to each band's settled window (first revolution
    /// excluded) -- what the verdict engine judges on.
    pub acc_settled: BTreeMap<(usize, usize), Acc>,
    /// (cylinder, band, t_enter, t_exit) in capture seconds; t_exit updated
    /// while inside. Lets the verdict slice the raw force series per band.
    pub band_windows: Vec<(usize, usize, f64, f64)>,
}

pub struct Capture {
    pub state: Arc<Mutex<CaptureState>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub map: BandMap,
    /// (cylinder, nozzle temp C) from the manifest, for the verdict.
    pub cylinder_temps: Vec<(usize, i64)>,
    logger: LogFn,
}

impl Capture {
    /// Bind and start the UDP receive thread. Fails fast if the port is taken.
    /// `hooks.logger` receives packet/band/cylinder milestones, idle warnings
    /// and parse failures as they happen; `hooks.on_band_change` fires once per
    /// flow-band transition. Both are optional -- `CaptureHooks::default()` is
    /// entirely silent.
    pub fn start(
        bind: &str,
        port: u16,
        map: BandMap,
        sd_map: Option<SdMap>,
        hooks: CaptureHooks,
    ) -> std::io::Result<Capture> {
        let logger = hooks.logger.unwrap_or_else(noop_logger);
        logger("trace", format!("binding UDP {bind}:{port}"));
        match &sd_map {
            Some(m) => logger("info", format!(
                "sdpos band addressing active: {} segments, {} bands -- deterministic binning",
                m.segs.len(),
                m.segs.iter().filter(|s| s.kind == SegKind::Band).count()
            )),
            None => logger("warn",
                "no band manifest -- falling back to the Z heuristic (phantom-cylinder prone; \
                 regenerate the G-code to get <out>.bands.txt)".to_string()),
        }
        let sock = match UdpSocket::bind((bind, port)) {
            Ok(s) => s,
            Err(e) => {
                logger("error", format!("bind {bind}:{port} failed: {e}"));
                return Err(e);
            }
        };
        sock.set_read_timeout(Some(Duration::from_millis(200)))?;
        logger("info", format!("listening on UDP {bind}:{port}"));
        let state = Arc::new(Mutex::new(CaptureState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp, m, lg) = (state.clone(), stop.clone(), map.clone(), logger.clone());
        let cylinder_temps = sd_map.as_ref().map(|m| m.cylinder_temps()).unwrap_or_default();
        let on_band = hooks.on_band_change.clone();
        let on_photo = hooks.on_photo_window.clone();
        let handle =
            std::thread::spawn(move || run::run(sock, st, sp, m, sd_map, lg, on_band, on_photo));
        Ok(Capture { state, stop, handle: Some(handle), map, cylinder_temps, logger })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
            let s = self.state.lock().unwrap();
            (self.logger)("info", format!(
                "capture stopped: {} packets, {} force samples, {} z samples, {} bands touched",
                s.packets, s.force.len(), s.z.len(), s.acc.len()
            ));
        }
    }

    /// The complete capture, never gated. Export must not go through `delta`:
    /// that returns empty series whenever `since` happens to equal the current
    /// seq, which would write a file full of headers and no data.
    pub fn snapshot_all(&self) -> Snapshot {
        let mut snap = self.delta(u64::MAX);
        let s = self.state.lock().unwrap();
        snap.seq = s.seq;
        snap
    }

    /// Everything appended since `since`, plus current band stats.
    #[allow(clippy::type_complexity)]
    pub fn delta(&self, since: u64) -> Snapshot {
        let s = self.state.lock().unwrap();
        let take = |v: &Vec<(f64, f64)>| -> Vec<(f64, f64)> {
            // seq counts total samples across both streams; a conservative
            // over-send is fine, so slice by how much each stream grew.
            v.clone()
        };
        // Simple contract: when since == current seq, send nothing; otherwise
        // send full arrays. Frontend replaces wholesale. At 180 Hz x 15 min the
        // arrays are ~160k pairs, well inside uPlot's comfort zone; delta
        // slicing is an optimisation for later.
        let fresh = since != s.seq;
        Snapshot {
            seq: s.seq,
            force: if fresh { take(&s.force) } else { Vec::new() },
            z: if fresh { take(&s.z) } else { Vec::new() },
            speed: if fresh { take(&s.speed) } else { Vec::new() },
            temp_noz: if fresh { take(&s.temp_noz) } else { Vec::new() },
            temp_bed: if fresh { take(&s.temp_bed) } else { Vec::new() },
            temp_chamber: if fresh { take(&s.temp_chamber) } else { Vec::new() },
            bands: stats(&s, &self.map),
            cyl: s.cyl,
            z_now: s.z_now,
            now_noz: s.now_noz,
            now_bed: s.now_bed,
            now_chamber: s.now_chamber,
            tares: s.tare.iter().map(|(&c, a)| (c, a.mean(), a.n)).collect(),
            cylinder_temps: self.cylinder_temps.clone(),
            band_windows: s.band_windows.clone(),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn stats(s: &CaptureState, map: &BandMap) -> Vec<BandStat> {
    s.acc
        .iter()
        .map(|(&(cy, b), a)| {
            let sp = s.speed_acc.get(&(cy, b)).copied().unwrap_or_default();
            // An empty speed accumulator means UNMEASURED. Reporting mean()'s
            // 0.0 would claim the head stood still and, multiplied by the bead
            // cross-section, that the band delivered no material at all.
            let measured = sp.n > 0;
            let st = s.acc_settled.get(&(cy, b)).copied().unwrap_or_default();
            BandStat {
                cylinder: cy,
                band: b + 1,
                flow: map.flow_for(cy, b),
                n: a.n,
                mean: a.mean(),
                sd: a.sd(),
                speed_n: sp.n,
                speed_mean: measured.then(|| sp.mean()),
                speed_sd: measured.then(|| sp.sd()),
                actual_flow: match (map.bead_xsec, measured) {
                    (Some(x), true) => Some(sp.mean() * x),
                    _ => None,
                },
                settled_n: st.n,
                settled_mean: (st.n > 0).then(|| st.mean()),
                settled_sd: (st.n > 0).then(|| st.sd()),
            }
        })
        .collect()
}
