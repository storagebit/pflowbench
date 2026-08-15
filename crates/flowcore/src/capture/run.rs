// The UDP receive loop: decode each datagram, bin loadcell/speed samples by
// (cylinder, band) -- deterministically via sdpos when a manifest is present,
// by the Z heuristic otherwise -- fire the band/photo hooks, and report
// milestones through the logger at a bounded rate, never per-packet.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::bandmap::{BandMap, CylTracker};
use crate::parse::{parse_point, strip_header_tm};
use crate::sdmap::{SdMap, SegKind, SD_GUARD_BYTES};

use super::{BandChange, BandChangeFn, CaptureState, LogFn, TRAVEL_SPEED_CUTOFF};

pub(super) fn run(
    sock: UdpSocket,
    state: Arc<Mutex<CaptureState>>,
    stop: Arc<AtomicBool>,
    map: BandMap,
    sd_map: Option<SdMap>,
    logger: LogFn,
    on_band_change: Option<BandChangeFn>,
    on_photo_window: Option<BandChangeFn>,
) {
    // sdpos-derived position: index of the current segment, the raw byte
    // offset (for the settle check), and the time we entered it.
    let mut cur_seg: Option<usize> = None;
    let mut photo_fired: std::collections::HashSet<(usize, usize)> = Default::default();
    // Armed photo window: (cylinder, band, flow, t_entry). The hook fires
    // when the head has demonstrably STOPPED (3 consecutive derived-speed
    // samples < 0.5 mm/s), not at segment entry -- sdpos leads execution by
    // the planner queue, so entry precedes the park by seconds. Firmware-
    // clock fallback at +5 s covers the case where no position metrics
    // arrive during the G4 dwell (untested on hardware; see accuracy-plan).
    let mut photo_armed: Option<(usize, usize, f64, Option<f64>)> = None;
    let mut photo_static_count: u32 = 0;
    let mut cur_sd: u64 = 0;
    let mut buf = [0u8; 4096];
    let mut tracker = CylTracker::default();
    let mut z: Option<f64> = None;
    // Buddy publishes no velocity metric, so speed is differentiated from
    // successive positions. pos_x and pos_y arrive as separate points, so a
    // sample is only complete once both have been seen at least once.
    let (mut px, mut py): (Option<f64>, Option<f64>) = (None, None);
    let mut last_pos: Option<(f64, f64, f64, f64)> = None; // t, x, y, z
    // Milestone/idle tracking for logging -- deliberately NOT per-packet or
    // per-point: at 180Hz over a 15-minute run that would be hundreds of
    // thousands of log lines and would make the on-screen console useless.
    // Band/cylinder transitions and a periodic summary give full visibility
    // into progress without flooding it.
    let mut first_packet = true;
    let mut last_packet_at = Instant::now();
    let mut idle_warned = false;
    let mut last_band: Option<usize> = None;
    let mut last_cyl = 0usize;
    let mut last_summary = Instant::now();
    let mut parse_failures = 0u32;
    // Packets arriving while NOTHING binds means the printer is streaming its
    // default telemetry but loadcell_value/pos_z were never enabled -- i.e. the
    // job is missing its M334/M331 block. That silently wastes an entire print,
    // so say so loudly and name what IS arriving.
    let mut wanted_seen = false;
    let mut wrong_metrics_warned = false;
    let mut first_wanted_deadline: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let n = match sock.recv_from(&mut buf) {
            Ok((n, _)) => n,
            Err(_) => {
                if !first_packet && !idle_warned && last_packet_at.elapsed() > Duration::from_secs(10) {
                    idle_warned = true;
                    logger("warn", "no UDP packets in 10s -- check firewall, the M334 target/port, \
                        and that Metrics & Log is enabled on the printer's touchscreen".to_string());
                }
                continue; // timeout tick; re-check stop flag
            }
        };
        last_packet_at = Instant::now();
        idle_warned = false;
        first_wanted_deadline.get_or_insert_with(Instant::now);
        if first_packet {
            first_packet = false;
            logger("info", "first UDP packet received, printer is streaming metrics".to_string());
        }
        let pkt = String::from_utf8_lossy(&buf[..n]);
        let (body, tm_us) = strip_header_tm(&pkt);
        let mut s = state.lock().unwrap();
        s.packets += 1;
        let t0 = *s.t0.get_or_insert_with(Instant::now);
        // Arrival time, used only as a fallback and for the idle/summary logs.
        let t_arrival = t0.elapsed().as_secs_f64();
        for line in body.split('\n') {
            let (name, v, off_us) = match parse_point(line) {
                Some(p) => p,
                None => {
                    if !line.trim().is_empty() {
                        parse_failures += 1;
                        if parse_failures <= 3 {
                            logger("warn", format!("unparseable metric line ({parse_failures}): {:?}", line.trim()));
                        } else if parse_failures == 4 {
                            logger("warn", "further unparseable lines suppressed".to_string());
                        }
                    }
                    continue;
                }
            };
            *s.metric_names.entry(name.to_string()).or_insert(0) += 1;

            // Deterministic position from sdpos, when a manifest is present.
            if name == "sdpos" {
                if let Some(m) = &sd_map {
                    cur_sd = v.max(0.0) as u64;
                    let found = m.locate(cur_sd);
                    let found_idx = found.map(|seg| {
                        m.segs.iter().position(|x| std::ptr::eq(x, seg)).unwrap_or(usize::MAX)
                    });
                    if found_idx != cur_seg {
                        // close the previous band window
                        if let Some(i) = cur_seg {
                            if m.segs[i].kind == SegKind::Band {
                                if let Some(w) = s.band_windows.last_mut() {
                                    w.3 = 0.0_f64.max(w.3); // keep; t_exit set below on each sample
                                }
                            }
                        }
                        cur_seg = found_idx;
                        if let Some(seg) = found {
                            match seg.kind {
                                SegKind::Band => {
                                    s.cyl = seg.cylinder;
                                    s.band_windows.push((seg.cylinder, seg.band, 0.0, 0.0));
                                    logger("info", format!(
                                        "sdpos: cylinder {} band {} ({:.1} mm3/s, {} C)",
                                        seg.cylinder, seg.band + 1, seg.flow, seg.temp
                                    ));
                                    if let Some(cb) = &on_band_change {
                                        cb(BandChange {
                                            cylinder: seg.cylinder,
                                            band: seg.band,
                                            flow: seg.flow,
                                            z: s.z_now,
                                        });
                                    }
                                }
                                SegKind::Photo => {
                                    // arm once per window; the actual fire
                                    // waits for stationarity below
                                    if !photo_fired.contains(&(seg.cylinder, seg.band))
                                        && photo_armed.is_none()
                                    {
                                        logger("info", format!(
                                            "sdpos: photo window for cylinder {} band {} ({:.1} mm3/s) -- armed, waiting for the head to stop",
                                            seg.cylinder, seg.band + 1, seg.flow
                                        ));
                                        photo_armed = Some((seg.cylinder, seg.band, seg.flow, None));
                                        photo_static_count = 0;
                                    }
                                }
                                SegKind::Tare => logger("info", format!(
                                    "sdpos: tare window for cylinder {} ({} C)",
                                    seg.cylinder, seg.temp
                                )),
                                SegKind::End => logger("info",
                                    "sdpos: past the last band -- end block".to_string()),
                                _ => logger("trace", format!(
                                    "sdpos: {:?} segment, cylinder {}", seg.kind, seg.cylinder
                                )),
                            }
                        }
                    }
                }
            }

            // Per-sample clock from the firmware's own microsecond stamp.
            // Buddy packs many points into one datagram, so timing them by
            // arrival gives every point in a batch an identical `t`: the
            // position differentiator then sees dt == 0 for all but the first
            // (discarded by its own guard) while that survivor's dt spans the
            // whole inter-datagram gap against a distance covering just one
            // sample interval. The result is a speed -- and therefore an
            // `actual_flow` -- biased low by roughly the batch size on every
            // run. Fall back to arrival time if the packet had no usable `tm`
            // or the stamp is implausible, so a firmware quirk degrades to the
            // old behaviour instead of producing garbage.
            let t = match tm_us {
                Some(base) => {
                    let abs = base.saturating_add(off_us);
                    let t0u = *s.t0_us.get_or_insert(abs);
                    let secs = (abs - t0u) as f64 / 1_000_000.0;
                    if secs.is_finite() && (-1.0..86_400.0).contains(&secs) {
                        secs.max(0.0)
                    } else {
                        t_arrival
                    }
                }
                None => t_arrival,
            };

            match name {
                "pos_z" => {
                    wanted_seen = true;
                    if sd_map.is_none() {
                        // Legacy cylinder detection -- only without a manifest:
                        // this heuristic counted 11 cylinders for 4 test objects.
                        tracker.observe(v, &map);
                        if tracker.cylinder != last_cyl {
                            last_cyl = tracker.cylinder;
                            logger("info", format!("cylinder boundary detected -- now on cylinder {last_cyl}"));
                        }
                        s.cyl = tracker.cylinder;
                    }
                    s.z_now = v;
                    s.z.push((t, v));
                    s.seq += 1;
                    z = Some(v);

                    // Differentiate position -> speed. Guard on a sane dt:
                    // a zero/backwards interval yields a nonsense spike, and
                    // a long gap spans a pause where the mean is meaningless.
                    if let (Some(x), Some(y)) = (px, py) {
                        if let Some((lt, lx, ly, lz)) = last_pos {
                            let dt = t - lt;
                            if dt > 0.002 && dt < 1.0 {
                                let d = ((x - lx).powi(2) + (y - ly).powi(2) + (v - lz).powi(2)).sqrt();
                                let mm_s = d / dt;
                                s.speed.push((t, mm_s));
                                if let Some((cy, bi, fl, t0)) = photo_armed {
                                    let t0 = match t0 {
                                        Some(v) => v,
                                        None => {
                                            photo_armed = Some((cy, bi, fl, Some(t)));
                                            t
                                        }
                                    };
                                    if mm_s < 0.5 {
                                        photo_static_count += 1;
                                    } else {
                                        photo_static_count = 0;
                                    }
                                    if photo_static_count >= 3 || t - t0 > 5.0 {
                                        photo_armed = None;
                                        photo_fired.insert((cy, bi));
                                        logger("info", format!(
                                            "photo window fired for cylinder {} band {} ({})",
                                            cy, bi + 1,
                                            if photo_static_count >= 3 { "head stationary" }
                                            else { "5s fallback -- no stationarity seen" }
                                        ));
                                        if let Some(cb) = &on_photo_window {
                                            cb(BandChange { cylinder: cy, band: bi, flow: fl, z: s.z_now });
                                        }
                                    }
                                }
                                // Only bin speed while inside the band stack,
                                // and reject travel moves: those run far above
                                // print speed and would inflate the mean.
                                match (&sd_map, cur_seg) {
                                    (Some(m), Some(i)) if m.segs[i].kind == SegKind::Band => {
                                        if mm_s < TRAVEL_SPEED_CUTOFF {
                                            let seg = &m.segs[i];
                                            s.speed_acc.entry((seg.cylinder, seg.band)).or_default().add(mm_s);
                                        }
                                    }
                                    (Some(_), _) => {}
                                    (None, _) => {
                                        if let Some(bi) = map.band_for(v) {
                                            if mm_s < TRAVEL_SPEED_CUTOFF {
                                                let cy = tracker.cylinder;
                                                s.speed_acc.entry((cy, bi)).or_default().add(mm_s);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        last_pos = Some((t, x, y, v));
                    }
                    let band = if sd_map.is_none() { map.band_for(v) } else { last_band };
                    if band != last_band {
                        last_band = band;
                        match band {
                            Some(b) => {
                                logger("trace", format!(
                                    "entered band {} (flow {:.1} mm3/s) at Z={v:.2}", b + 1, map.flow_for(tracker.cylinder, b)
                                ));
                                if let Some(cb) = &on_band_change {
                                    // Handler runs inline on this thread; see
                                    // BandChange's docs -- it must not block.
                                    cb(BandChange {
                                        cylinder: tracker.cylinder,
                                        band: b,
                                        flow: map.flow_for(tracker.cylinder, b),
                                        z: v,
                                    });
                                }
                            }
                            None => logger("trace", format!(
                                "left the band stack at Z={v:.2} (first layer / purge / overtravel)"
                            )),
                        }
                    }
                }
                "pos_x" => px = Some(v),
                "pos_y" => py = Some(v),
                "temp_noz" => { s.now_noz = Some(v); s.temp_noz.push((t, v)); s.seq += 1; }
                "temp_bed" => { s.now_bed = Some(v); s.temp_bed.push((t, v)); s.seq += 1; }
                "chamber_temp" => { s.now_chamber = Some(v); s.temp_chamber.push((t, v)); s.seq += 1; }
                "loadcell_value" => {
                    wanted_seen = true;
                    s.force.push((t, v));
                    s.seq += 1;
                    match (&sd_map, cur_seg) {
                        // Deterministic: bin by the segment sdpos says we're in.
                        (Some(m), Some(i)) => {
                            let seg = &m.segs[i];
                            match seg.kind {
                                SegKind::Band => {
                                    let key = (seg.cylinder, seg.band);
                                    s.acc.entry(key).or_default().add(v);
                                    if cur_sd.saturating_sub(SD_GUARD_BYTES) >= seg.settle {
                                        s.acc_settled.entry(key).or_default().add(v);
                                    }
                                    if let Some(w) = s.band_windows.last_mut() {
                                        if w.0 == seg.cylinder && w.1 == seg.band {
                                            if w.2 == 0.0 {
                                                w.2 = t;
                                            }
                                            w.3 = t;
                                        }
                                    }
                                }
                                SegKind::Tare => {
                                    s.tare.entry(seg.cylinder).or_default().add(v);
                                }
                                _ => {}
                            }
                        }
                        // Legacy: Z-window heuristic (no manifest available).
                        _ => {
                            if let Some(zz) = z {
                                if let Some(b) = map.band_for(zz) {
                                    let cy = s.cyl;
                                    s.acc.entry((cy, b)).or_default().add(v);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        // Time-based, not just packet-count: with the metrics block armed
        // ahead of the start block a correct job streams pos_z from the very
        // first move, so 30s of telemetry with none of it means the job really
        // is missing M334/M331 -- not merely that homing hasn't finished.
        let quiet_too_long = first_wanted_deadline
            .map(|t: Instant| t.elapsed() > Duration::from_secs(30))
            .unwrap_or(false);
        if !wanted_seen && !wrong_metrics_warned && s.packets > 50 && quiet_too_long {
            wrong_metrics_warned = true;
            let names: Vec<String> = s.metric_names.keys().take(8).cloned().collect();
            logger("error", format!(
                "{} packets received but NO loadcell_value or pos_z -- this run will \
                 capture nothing. The G-code is almost certainly missing its M334/M331 \
                 metrics block (regenerate with a metrics host set). Metrics arriving: {}",
                s.packets, names.join(", ")
            ));
        }

        if last_summary.elapsed() > Duration::from_secs(5) {
            last_summary = Instant::now();
            // Name every metric actually arriving, with its count. When a
            // series is missing from a chart the first question is always
            // whether the printer is streaming it at all -- previously that
            // was only answerable from the total-failure error path, so a
            // single absent metric (chamber temperature, head position) had
            // to be diagnosed by reading source instead of the log.
            let names: Vec<String> =
                s.metric_names.iter().map(|(k, n)| format!("{k}={n}")).collect();
            logger("trace", format!(
                "{} packets | force {} | z {} | speed {} | noz {} | bed {} | chamber {} || arriving: {}",
                s.packets,
                s.force.len(),
                s.z.len(),
                s.speed.len(),
                s.temp_noz.len(),
                s.temp_bed.len(),
                s.temp_chamber.len(),
                if names.is_empty() { "(nothing)".to_string() } else { names.join(" ") }
            ));
        }
    }
}
