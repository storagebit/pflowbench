// emit.rs -- the G-code emitters for one test cylinder.
//
// Everything here appends lines to the job body: purge lanes, brim and first
// layer, the spiral band ladder, photo windows, and the wipe/retract moves
// between them. The recurring constraint is ooze: a 0.8mm high-flow nozzle
// carrying molten CF PETG strings on every unguarded travel, so no emitter
// may leave the nozzle pressurised before a move off the part.

use std::f64::consts::PI;

use crate::config::Cfg;
use crate::geometry::{e_for, extrusion_xsec, feed_for};

/// Retract *while still moving along the just-printed path*, so the residual
/// pressure is dragged out onto existing material instead of hanging off the
/// nozzle as a string.
///
/// This is what PrusaSlicer's `wipe = 1` does, and the reference profile for
/// this printer has it enabled. Retract distance alone does not substitute
/// for it: the reference retracts only 0.6mm yet does not string, because it
/// wipes. A 0.8mm high-flow nozzle carrying molten CF PETG will string on
/// every travel without this.
fn wipe_retract_arc(g: &mut Vec<String>, c: &Cfg, cx: f64, cy: f64, r: f64,
                    start_angle: f64, z: Option<f64>) {
    const WIPE_SEGS: usize = 8;
    const WIPE_ARC: f64 = 0.35; // radians of circumference to wipe over
    let e_step = c.retract / WIPE_SEGS as f64;
    g.push("; wipe while retracting".to_string());
    for i in 1..=WIPE_SEGS {
        let a = start_angle + WIPE_ARC * i as f64 / WIPE_SEGS as f64;
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        match z {
            Some(zz) => g.push(format!("G1 X{x:.3} Y{y:.3} Z{zz:.3} E-{e_step:.5} F1500")),
            None => g.push(format!("G1 X{x:.3} Y{y:.3} E-{e_step:.5} F1500")),
        }
    }
}

/// Lift clear before a travel. The reference profile ramps up to 1.5mm; a
/// flat hop is enough here and keeps the nozzle from dragging over the parts.
fn hop(g: &mut Vec<String>, z: f64) {
    g.push(format!("G1 Z{:.2} F900 ; hop before travel", z));
}

/// Lay a purge line at the front of the bed, one lane per temperature.
fn purge_line(g: &mut Vec<String>, c: &Cfg, index: usize, temp: i64) {
    let y = c.purge_y + index as f64 * c.purge_pitch;
    let (x0, x1) = (c.purge_x, c.purge_x + c.purge_len);
    let xsec = extrusion_xsec(c.first_layer_h, c.purge_w);
    g.push(format!("; purge lane {} at {} C", index, temp));
    g.push(format!("G1 Z{:.2} F600", c.safe_z));
    g.push(format!("G1 X{:.3} Y{:.3} F{}", x0, y, c.travel_f));
    g.push(format!("G1 Z{:.2} F600", c.first_layer_h));
    g.push("G1 E4 F1200".to_string());
    g.push(format!("G1 X{:.3} Y{:.3} E{:.5} F{}", x1, y,
                   e_for(c.purge_len, xsec, c.em), 1500));
    g.push(format!("G1 X{:.3} Y{:.3} E{:.5} F{}", x1, y + c.purge_w,
                   e_for(c.purge_w, xsec, c.em), 1500));
    g.push(format!("G1 X{:.3} Y{:.3} E{:.5} F{}", x0, y + c.purge_w,
                   e_for(c.purge_len, xsec, c.em), 1500));
    // Wipe back along the purge stroke while retracting -- a bare retract here
    // leaves a blob that gets dragged across the plate on the way to the
    // test object (visible as a long string in the camera stills).
    g.push("; wipe back along the purge stroke while retracting".to_string());
    let steps = 8;
    let e_step = c.retract / steps as f64;
    for i in 1..=steps {
        let x = x0 + (c.purge_len * 0.25) * i as f64 / steps as f64;
        g.push(format!("G1 X{x:.3} Y{:.3} E-{e_step:.5} F1500", y + c.purge_w));
    }
    g.push(format!("G1 Z{:.2} F600", c.safe_z));
}

/// One flat circular loop at constant Z.
///
/// The move onto the ring's start point extrudes nothing, so it is bracketed
/// by a retract/deretract: without it the nozzle drools across the gap between
/// concentric brim rings and leaves strings on the part.
fn ring(g: &mut Vec<String>, c: &Cfg, cx: f64, cy: f64, radius: f64, nseg: usize,
        xsec: f64, em: f64, feed: i64) {
    let seg = 2.0 * PI * radius / nseg as f64;
    let e = e_for(seg, xsec, em);
    g.push(format!("G1 E-{:.2} F2400 ; retract for ring approach", c.retract));
    g.push(format!("G1 X{:.3} Y{:.3} F{}", cx + radius, cy, c.travel_f));
    g.push(format!("G1 E{:.2} F1200 ; deretract", c.retract));
    g.push(format!("G1 F{}", feed));
    for i in 1..=nseg {
        let a = 2.0 * PI * i as f64 / nseg as f64;
        g.push(format!("G1 X{:.3} Y{:.3} E{:.5}",
                       cx + radius * a.cos(), cy + radius * a.sin(), e));
    }
}

/// Vision Tier 0 photo window: wipe-retract off the wall, hop, park out of
/// the camera's view, dwell. The `;FBSEG kind=photo` segment starts with
/// ~2.3KB of comment padding so that sdpos minus the prefetch guard still
/// lands INSIDE this segment while the printer sits on the dwell -- without
/// it the window is a handful of bytes and attribution falls back into the
/// previous band. The caller emits the return/deretract only when another
/// band follows.
fn photo_window(g: &mut Vec<String>, c: &Cfg, cx: f64, cy: f64, r: f64, z: f64,
                stack_top: f64, cyl: usize, band: usize, flow: f64, temp: i64) {
    let px = c.photo_park_x.unwrap_or(c.bed_x - 5.0);
    let py = c.photo_park_y.unwrap_or(c.bed_y - 5.0);
    g.push(format!(";FBSEG kind=photo cyl={cyl} band={band} flow={flow:.2} temp={temp}"));
    wipe_retract_arc(g, c, cx, cy, r, 0.0, None);
    hop(g, z + 1.0);
    // Clear finished test objects (all the same stack height) with 2 mm margin
    // -- not the flat safe_z, which costs ~1.6 s per pose at 25 mm. Z runs
    // at F900 (machine limit 15 mm/s); the pose tax is stationary hot-melt
    // time, a viscosity confound for the loadcell knee (accuracy-plan).
    let clearance = (stack_top + 2.0).max(z + 1.0);
    g.push(format!("G1 Z{clearance:.2} F900"));
    g.push(format!("G1 X{px:.1} Y{py:.1} F{} ; park out of camera view", c.travel_f));
    g.push(format!("G4 P{} ; photo window", (c.photo_dwell * 1000.0).round() as i64));
    // Padding AFTER the dwell: while the head sits on the G4 the firmware's
    // reader pre-fetches the following moves into the planner queue, so raw
    // sdpos runs ahead by roughly the queue depth (~2KB of short spiral
    // lines). These bytes keep sdpos minus SD_GUARD_BYTES (2048, flowcore)
    // inside THIS segment for the whole dwell -- without them the dwell
    // attributes to the next band. They cost no print time: comments parse
    // in microseconds.
    let pad = format!(";FBPAD {}", "-".repeat(120));
    for _ in 0..18 {
        g.push(pad.clone());
    }
}

/// One full test cylinder: purge, first layer + brim, then the spiral ladder.
/// Returns (flow, z_lo, z_hi) per band.
/// One calibration pillar: brim, first layer, then a constant-flow spiral
/// of `layers` whole layers. No metrics, no bands -- the geometry is the
/// product. Flow is the profile's first-layer rate: safe for any material.
pub(crate) fn pillar(g: &mut Vec<String>, c: &Cfg, cx: f64, cy: f64,
                     index: usize, layers: usize) {
    let xsec1 = extrusion_xsec(c.first_layer_h, c.first_layer_w);
    let xsec = extrusion_xsec(c.layer_h, c.width);
    let r = c.diameter / 2.0;
    let circ = 2.0 * PI * r;
    let nseg = std::cmp::max(48, (circ / c.seg_len).round() as usize);
    let seg = circ / nseg as f64;
    let e_seg = e_for(seg, xsec, c.em);
    let dz_seg = c.layer_h / nseg as f64;

    g.push(String::new());
    g.push(format!(";=== pillar {} : {:.1}mm ===", index, layers as f64 * c.layer_h));
    g.push(format!("G1 E-{:.2} F2400 ; retract for travel", c.retract));
    g.push(format!("G1 Z{:.2} F900", c.safe_z));
    g.push(format!("G1 X{:.3} Y{:.3} F{}", cx + r, cy, c.travel_f));
    g.push(format!("G1 Z{:.2} F600", c.first_layer_h));
    g.push(format!("G1 E{:.2} F1200 ; deretract", c.retract));
    g.push("M107".to_string());
    let f1 = feed_for(c.first_layer_flow, xsec1) as i64;
    for b in (1..=c.brim).rev() {
        ring(g, c, cx, cy, r + b as f64 * c.first_layer_w, nseg, xsec1, c.em, f1);
    }
    ring(g, c, cx, cy, r, nseg, xsec1, c.em, f1);
    g.push(format!("M106 S{}", c.fan));
    // constant-flow spiral at the first-layer rate
    let f = feed_for(c.first_layer_flow, xsec);
    g.push(format!("G1 F{f:.0}"));
    let mut z = c.first_layer_h;
    for _ in 0..layers {
        for i in 1..=nseg {
            let a = 2.0 * PI * (i % nseg) as f64 / nseg as f64;
            z += dz_seg;
            g.push(format!("G1 X{:.3} Y{:.3} Z{:.3} E{:.5}",
                           cx + r * a.cos(), cy + r * a.sin(), z, e_seg));
        }
    }
    wipe_retract_arc(g, c, cx, cy, r, 0.0, None);
    g.push("M107".to_string());
    g.push(format!("G1 Z{:.2} F900", c.safe_z));
}

pub(crate) fn cylinder(g: &mut Vec<String>, c: &Cfg, cx: f64, cy: f64, temp: i64,
            index: usize, band_flows: &[f64]) -> Vec<(f64, f64, f64)> {
    let xsec1 = extrusion_xsec(c.first_layer_h, c.first_layer_w);
    let xsec = extrusion_xsec(c.layer_h, c.width);
    let r = c.diameter / 2.0;
    let circ = 2.0 * PI * r;
    let nseg = std::cmp::max(48, (circ / c.seg_len).round() as usize);
    let seg = circ / nseg as f64;
    let e_seg = e_for(seg, xsec, c.em);
    let dz_seg = c.layer_h / nseg as f64;

    g.push(String::new());
    g.push(format!(";=== cylinder {} : {} C ===", index, temp));
    // Segment markers: sdpos (the printer's byte offset into this file,
    // streamed every 100ms) is mapped back through these to give the capture
    // DETERMINISTIC band addressing -- no Z heuristics. See docs/verdict-plan.md.
    g.push(format!(";FBSEG kind=travel cyl={index} temp={temp}"));
    // Retract and park clear of the plate BEFORE heating and dwelling. The
    // dwell is tens of seconds at full temperature: leaving the nozzle
    // pressurised over the previous cylinder drools molten filament onto a
    // finished test object and ruins it.
    g.push(format!("G1 E-{:.2} F2400 ; retract before the equilibration dwell", c.retract));
    g.push(format!("G1 Z{:.2} F600", c.safe_z));
    g.push(format!("G1 X{:.1} Y{:.1} F{} ; park clear of the test objects",
                   c.bed_x - 5.0, c.bed_y - 5.0, c.travel_f));
    g.push(format!("M104 S{}", temp));
    g.push(format!("M109 S{}", temp));
    // M109 already blocks until the nozzle reaches temperature, so an extra
    // dwell buys nothing and actively hurts: a carbon-filled melt sitting
    // stationary at PC temperatures degrades and can clog. Off unless asked.
    if c.dwell > 0 {
        g.push(format!("G4 S{} ; optional soak", c.dwell));
    }
    // Loadcell tare window: nozzle parked, retracted, stationary -- the
    // loadcell reads its zero for THIS cylinder at THIS temperature. Every
    // force feature downstream is a delta from this, because the tare drifts
    // with temperature (the first run produced negative band means).
    g.push(format!(";FBSEG kind=tare cyl={index} temp={temp}"));
    g.push("G4 S8 ; loadcell tare -- no motion, no extrusion".to_string());
    g.push(format!(";FBSEG kind=purge cyl={index} temp={temp}"));
    purge_line(g, c, index, temp);

    g.push(format!(";FBSEG kind=first cyl={index} temp={temp}"));
    g.push("; first layer".to_string());
    g.push("M107".to_string());
    // purge_line leaves the nozzle retracted and lifted; stay that way across
    // the travel and only deretract once parked on the start point.
    g.push(format!("G1 X{:.3} Y{:.3} F{}", cx + r, cy, c.travel_f));
    g.push(format!("G1 Z{:.2} F600", c.first_layer_h));
    let f1 = feed_for(c.first_layer_flow, xsec1) as i64;
    for b in (1..=c.brim).rev() {
        ring(g, c, cx, cy, r + b as f64 * c.first_layer_w, nseg, xsec1, c.em, f1);
    }
    ring(g, c, cx, cy, r, nseg, xsec1, c.em, f1);

    g.push(format!("M106 S{}", c.fan));

    let mut z = c.first_layer_h;
    let mut zmap = Vec::new();
    let nbands = band_flows.len();
    for &flow in band_flows {
        let f = feed_for(flow, xsec);
        let z_lo = z;
        let band_idx = zmap.len();
        g.push(format!(
            ";FBSEG kind=band cyl={index} band={band_idx} flow={flow:.2} revs={} temp={temp}",
            c.revs
        ));
        g.push(format!("; band: {:.1} mm3/s  F{:.0}  Z {:.2} -> {:.2}",
                       flow, f, z_lo, z_lo + c.revs as f64 * c.layer_h));
        g.push(format!("G1 F{:.0}", f));
        for _ in 0..c.revs {
            for i in 1..=nseg {
                let a = 2.0 * PI * (i % nseg) as f64 / nseg as f64;
                z += dz_seg;
                g.push(format!("G1 X{:.3} Y{:.3} Z{:.3} E{:.5}",
                               cx + r * a.cos(), cy + r * a.sin(), z, e_seg));
            }
        }
        zmap.push((flow, z_lo, z));
        if c.photo_pose {
            let stack_top = c.first_layer_h + nbands as f64 * c.revs as f64 * c.layer_h;
            photo_window(g, c, cx, cy, r, z, stack_top, index, band_idx, flow, temp);
            if band_idx + 1 < nbands {
                // back over the seam at clearance height, then down
                g.push(format!("G1 X{:.3} Y{:.3} F{}", cx + r, cy, c.travel_f));
                g.push(format!("G1 Z{:.3} F900", z));
                g.push(format!("G1 E{:.2} F1200 ; deretract", c.retract));
            }
        }
    }

    g.push(format!(";FBSEG kind=travel cyl={index} temp={temp}"));
    if c.photo_pose {
        // The last photo window already wiped, retracted and parked clear;
        // just kill the fan and climb to the travel height.
        g.push("M107".to_string());
        g.push(format!("G1 Z{:.2} F600", c.safe_z));
    } else {
        // The spiral ends at full height with the nozzle still pressurised;
        // wipe along the wall while retracting so nothing hangs off it for
        // the travel to the next test object.
        wipe_retract_arc(g, c, cx, cy, r, 0.0, None);
        g.push("M107".to_string());
        hop(g, z + 2.0);
        g.push(format!("G1 Z{:.2} F600", c.safe_z));
    }
    zmap
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{generate, write_band_manifest, Cfg};

    #[test]
    fn photo_pose_emits_a_guarded_window_per_band() {
        let out = std::env::temp_dir().join(format!("flowgen_photo_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: out.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255, 265],
            flows: vec![8.0, 12.0, 16.0],
            revs: 1,
            photo_pose: true,
            ..Default::default()
        };
        generate(cfg).unwrap();
        let body = fs::read_to_string(&out).unwrap();

        // one photo window per (cylinder, band)
        let markers: Vec<&str> =
            body.lines().filter(|l| l.starts_with(";FBSEG kind=photo")).collect();
        assert_eq!(markers.len(), 2 * 3, "{markers:?}");

        // each window: padding beats the sdpos prefetch guard, head is
        // retracted before it travels, and the dwell happens while parked
        for m in &markers {
            let at = body.find(m).unwrap();
            let seg = &body[at..(at + 4000).min(body.len())];
            let pad_bytes: usize =
                seg.lines().filter(|l| l.starts_with(";FBPAD")).map(|l| l.len() + 1).sum();
            assert!(pad_bytes > 2048, "photo segment padding {pad_bytes} <= guard");
            let dwell = seg.find("G4 P5000").expect("dwell in window");
            let retract = seg.find("E-").expect("wipe-retract in window");
            let park = seg.find("park out of camera view").expect("park travel");
            let pad = seg.find(";FBPAD").expect("guard padding");
            assert!(
                retract < park && park < dwell && dwell < pad,
                "order: retract < park < dwell < pad (pad guards the dwell's sdpos)"
            );
            let climb = seg.find("G1 Z").expect("climb");
            assert!(climb < park, "park travel must happen at safe_z, not band height");
        }

        // the manifest carries the photo segments with their band identity
        // (written by the caller after post-processing, same as the app does)
        let mpath = write_band_manifest(out.to_str().unwrap()).unwrap();
        let manifest = fs::read_to_string(mpath).unwrap();
        assert_eq!(manifest.matches(" photo ").count(), 6, "{manifest}");

        // non-final windows re-pressurise for the next band; the final one
        // leaves the nozzle retracted for the inter-cylinder travel
        let deretracts = body.matches("F1200 ; deretract").count();
        // 2 cylinders x 2 non-final bands, plus first-layer ring approaches
        assert!(deretracts >= 4, "expected the return deretracts, got {deretracts}");
    }

    #[test]
    fn photo_pose_is_off_by_default() {
        let out = std::env::temp_dir().join(format!("flowgen_nophoto_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: out.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255],
            flows: vec![8.0],
            revs: 1,
            ..Default::default()
        };
        generate(cfg).unwrap();
        let body = fs::read_to_string(&out).unwrap();
        assert!(!body.contains("kind=photo"), "photo windows must be opt-in");
        assert!(!body.contains(";FBPAD"));
    }

    /// Every long travel must be preceded by a retract, and the two moments
    /// that actually strung on real hardware -- the equilibration dwell and
    /// the end of each spiral -- must wipe. Regression test for visible ooze
    /// strings dragged between test objects.
    #[test]
    fn no_travel_happens_with_a_pressurised_nozzle() {
        let out = std::env::temp_dir().join(format!("flowgen_ooze_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: out.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255, 265],
            flows: vec![8.0, 12.0],
            revs: 1,
            ..Default::default()
        };
        generate(cfg.clone()).unwrap();
        let body = fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = body.lines().collect();

        // heating must be entered retracted and parked clear, not with a
        // pressurised nozzle sitting over the previous test object
        // scope to the first TEST OBJECT, past the machine start block (which has
        // its own M109 for mesh-levelling temperature)
        let cyl = lines.iter().position(|l| l.starts_with(";=== cylinder")).expect("a cylinder");
        let heat = cyl + lines[cyl..].iter().position(|l| l.starts_with("M109 S")).expect("a heat-and-wait");
        let before: Vec<&str> = lines[heat.saturating_sub(6)..heat].to_vec();
        assert!(
            before.iter().any(|l| l.starts_with("G1 E-")),
            "heating entered without retracting: {before:?}"
        );
        assert!(
            before.iter().any(|l| l.contains("park clear")),
            "nozzle not parked clear before heating: {before:?}"
        );
        // and no soak dwell by default -- it degrades filled filaments. The
        // per-cylinder loadcell TARE dwell (8s, retracted and parked) is the
        // one legitimate exception; anything else is a regression.
        assert!(!body.contains("optional soak"), "a soak was emitted despite dwell = 0");
        for l in body.lines().filter(|l| l.trim_start().starts_with("G4 S")) {
            assert!(l.contains("loadcell tare"), "unexpected dwell: {l}");
        }
        assert_eq!(body.matches("loadcell tare").count(), 2, "one tare per cylinder");

        // both wipe sites must be present, once per test object
        assert_eq!(body.matches("wipe while retracting").count(), 2, "one wipe per spiral");
        assert!(body.contains("wipe back along the purge stroke"));

        // a wipe must actually move AND retract in the same move
        assert!(
            body.lines().any(|l| l.starts_with("G1 X") && l.contains("E-")),
            "wipe must combine motion with retraction, not retract in place"
        );

        // every ring approach retracts before repositioning
        assert!(body.contains("retract for ring approach"));
        let _ = fs::remove_file(&out);
    }
}
