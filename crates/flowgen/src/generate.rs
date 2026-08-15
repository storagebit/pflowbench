// generate.rs -- assemble and write one calibration job.
//
// Order of operations is the whole point of this file: refuse impossible
// configs and layouts up front, splice the machine start block, arm metrics
// after the compatibility checks but before any motion, re-arm them after
// homing, emit every test object, stop the stream, validate the assembled
// body, and only then touch the disk.

use std::fs;

use crate::config::Cfg;
use crate::emit::cylinder;
use crate::geometry::extrusion_xsec;
use crate::layout::{layout_fits, layout_positions, object_pitch, test_objects, TestObject};
use crate::splice::{split_reference, standalone_blocks};
use crate::validate::validate_output;

/// What `generate()` hands back once the file is written -- everything the
/// old CLI used to print, structured instead of formatted to stdout.
#[derive(Debug)]
pub struct GenReport {
    pub summary: String,
    pub cylinder_count: usize,
    pub height: f64,
    /// (flow, z_lo, z_hi) per band, for the first cylinder.
    pub bands: Vec<(f64, f64, f64)>,
    /// (temp_c, x, y) per cylinder.
    pub cylinder_positions: Vec<(i64, f64, f64)>,
    /// Every test object printed, in the same order as `cylinder_positions`.
    pub test_objects: Vec<TestObject>,
    /// Per-test object band flow rates, in print order. Needed to interpret the
    /// capture: with a reversed or constant control, band N does NOT carry the
    /// same flow on every cylinder.
    pub band_flows: Vec<Vec<f64>>,
    /// False if any footprint falls off the bed under the chosen layout.
    pub fits_bed: bool,
}

/// Generate the flow-cliff test G-code and write it to `cfg.out`.
pub fn generate(mut c: Cfg) -> Result<GenReport, String> {
    if let Some(t) = c.only_temp {
        c.temps = vec![t];
    }
    if c.temps.is_empty() || c.flows.is_empty() {
        return Err("need at least one temperature and one flow rate".into());
    }
    if c.photo_park_x.is_some() != c.photo_park_y.is_some() {
        return Err("photo_park_x and photo_park_y must be set together".into());
    }
    if let (Some(px), Some(py)) = (c.photo_park_x, c.photo_park_y) {
        if !(0.0..=c.bed_x).contains(&px) || !(0.0..=c.bed_y).contains(&py) {
            return Err(format!(
                "photo park ({px}, {py}) is off the {}x{} bed",
                c.bed_x, c.bed_y
            ));
        }
    }

    let (start, end) = match (&c.reference, c.standalone) {
        (Some(p), _) => split_reference(&p.clone())?,
        (None, true) => standalone_blocks(&c),
        (None, false) => return Err("pick one: a reference file or standalone mode".into()),
    };

    let height = c.first_layer_h + c.flows.len() as f64 * c.revs as f64 * c.layer_h;
    if height + 5.0 > c.safe_z {
        c.safe_z = height + 8.0;
    }

    let specs = test_objects(&c);
    let place = layout_positions(&c, specs.len());
    let fits_bed = layout_fits(&c, &place);

    // Refuse, don't warn. This was a cosmetic string appended to the summary,
    // and a four-test object row duly generated with footprints running from
    // X=-27 to X=327 on a 300mm bed -- the head would have driven into the
    // frame. A geometric impossibility is not something to notify about.
    if !fits_bed {
        let r = c.diameter / 2.0 + c.brim as f64 * c.first_layer_w;
        let off: Vec<String> = place
            .iter()
            .enumerate()
            .filter(|(_, &(x, y))| x - r < 0.0 || x + r > c.bed_x || y - r < 0.0 || y + r > c.bed_y)
            .map(|(i, &(x, y))| {
                format!("#{i} at ({x:.0},{y:.0}) spans X {:.0}..{:.0} Y {:.0}..{:.0}",
                    x - r, x + r, y - r, y + r)
            })
            .collect();
        return Err(format!(
            "layout does not fit the {:.0}x{:.0} mm bed: {} test object(s) of radius {r:.1} mm \
             (dia {:.0} + {} brim rings) at pitch {:.0} mm run off it -- {}. \
             Use a stagger or grid layout, print fewer test objects, or reduce the brim.",
            c.bed_x,
            c.bed_y,
            place.len(),
            c.diameter,
            c.brim,
            object_pitch(&c),
            off.join("; ")
        ));
    }

    let xsec = extrusion_xsec(c.layer_h, c.width);

    // Arm metrics streaming FIRST, ahead of the spliced start block. That
    // block homes, runs mesh bed levelling and purges, which takes minutes;
    // arming afterwards would leave the capture blind for all of it (and
    // makes a "no loadcell_value yet" warning indistinguishable from a
    // genuinely broken job). M334/M331 depend on no machine state, so there
    // is no reason to wait.
    // Arm metrics EARLY -- the start block homes, levels and purges, which
    // takes minutes and should be covered -- but not before the file's own
    // opening. The firmware inspects the head of the file for its M862.x
    // compatibility checks, and prepending commands ahead of them changes how
    // a stock PrusaSlicer export begins. Insert just after the checks (M115)
    // instead: still ahead of all the motion, but the file starts normally.
    let mut metrics: Vec<String> = Vec::new();
    if let Some(host) = &c.metrics_host {
        metrics.push("; ---- metrics streaming (armed before any motion) ----".to_string());
        for m in &c.metrics_disable {
            metrics.push(format!("M332 {}", m));
        }
        metrics.push(format!("M334 {} {}", host, c.metrics_port));
        metrics.push("M331 loadcell_value".to_string());
        metrics.push("M331 pos_z".to_string());
        // X/Y too: Buddy exposes no speed or feedrate metric, so actual print
        // speed has to be differentiated from position. That matters because
        // a band's COMMANDED flow is only achieved if the printer reaches the
        // commanded feedrate -- acceleration and cornering limits can leave
        // the real flow well below the label on the band.
        metrics.push("M331 pos_x".to_string());
        metrics.push("M331 pos_y".to_string());
        // Temperatures belong in the capture, not in an HTTP poll: they are
        // on the same clock as the force samples, so a thermal explanation
        // for a knee can be checked directly against it.
        metrics.push("M331 temp_noz".to_string());
        metrics.push("M331 temp_bed".to_string());
        metrics.push("M331 chamber_temp".to_string());
        // Melt-limit evidence (all names verified in Buddy firmware source,
        // see docs/verdict-plan.md section 4): target temp for sag, heater PWM
        // duty for watt saturation, heatbreak for heat creep, planner slowdown
        // to prove the commanded flow was actually executed, filament sensor
        // for grinding. sdpos/stp_stall/heater_current/voltage are enabled by
        // default; arming is idempotent, so list them for explicitness.
        for m in ["ttemp_noz", "nozzle_pwm", "temp_hbr", "plan_slow", "fsensor", "sdpos"] {
            metrics.push(format!("M331 {m}"));
        }
        metrics.push(String::new());
    }
    let mut g: Vec<String> = Vec::new();
    if metrics.is_empty() {
        g.extend(start);
    } else {
        // after M115 if present, else after the last M862.x, else at the top
        let anchor = start
            .iter()
            .position(|l| l.trim_start().starts_with("M115"))
            .or_else(|| start.iter().rposition(|l| l.trim_start().starts_with("M862")))
            .map(|i| i + 1)
            .unwrap_or(0);
        g.extend(start[..anchor].iter().cloned());
        g.extend(metrics);
        g.extend(start[anchor..].iter().cloned());
    }
    g.push(String::new());
    g.push("; ==== flowcliff test ====".to_string());
    g.push(format!("; bead {:.2} x {:.2} -> xsec {:.4} mm^2, EM {:.3}",
                   c.layer_h, c.width, xsec, c.em));
    g.push(format!("; {} revs per band, {:.1} mm per band, cylinder height {:.2} mm",
                   c.revs, c.revs as f64 * c.layer_h, height));
    // Re-arm after the start block. Enabling pos_z before G28 -- while the Z
    // axis is unhomed and its position undefined -- produced a run with
    // 130k force samples and ZERO z samples, so the early arming alone is not
    // sufficient. M331 is idempotent, so arming again once the machine is
    // homed and levelled costs nothing and guarantees the position metrics
    // are live before the first test object.
    if let Some(host) = &c.metrics_host {
        g.push("; ---- re-arm metrics after homing/levelling ----".to_string());
        g.push(format!("M334 {} {}", host, c.metrics_port));
        for m in [
            "loadcell_value", "pos_z", "pos_x", "pos_y", "temp_noz", "temp_bed",
            "chamber_temp", "ttemp_noz", "nozzle_pwm", "temp_hbr", "plan_slow",
            "fsensor", "sdpos",
        ] {
            g.push(format!("M331 {m}"));
        }
    }
    g.push("G21".to_string());
    g.push("G90".to_string());
    g.push("M83".to_string());
    g.push("M221 S100".to_string());
    g.push(format!("M572 S{:.4}", c.pa));
    g.push("M142 S36".to_string());
    g.push(format!("M140 S{}", c.bed));
    g.push(format!("M190 S{}", c.bed));

    let mut first_map: Vec<(f64, f64, f64)> = Vec::new();
    let mut positions = Vec::new();
    let mut band_flows = Vec::new();
    for (i, (s, &(cx, cy))) in specs.iter().zip(place.iter()).enumerate() {
        positions.push((s.temp, cx, cy));
        let flows = s.flows(&c.flows);
        g.push(format!("; test object {i}: {}", s.label()));
        let m = cylinder(&mut g, &c, cx, cy, s.temp, i, &flows);
        band_flows.push(flows);
        if i == 0 {
            first_map = m;
        }
    }

    if c.metrics_host.is_some() {
        g.push(String::new());
        g.push("; ---- stop metrics streaming ----".to_string());
        g.push("M332 loadcell_value".to_string());
        g.push("M332 pos_z".to_string());
        g.push("M334 ; no parameters disables metrics".to_string());
    }

    g.push(";FBSEG kind=end".to_string());
    g.push(String::new());
    g.extend(end);
    let mut body = g.join("\n");
    body.push('\n');
    let problems = validate_output(&body, c.metrics_host.is_some());
    if !problems.is_empty() {
        return Err(format!(
            "generated G-code failed validation, refusing to write it:\n  - {}",
            problems.join("\n  - ")
        ));
    }
    fs::write(&c.out, body).map_err(|e| format!("{}: {}", c.out, e))?;

    let controls = specs.len() - c.temps.len();
    let summary = format!(
        "wrote {} : {} test object(s){}, {:.2} mm tall, bead {:.2} x {:.2} mm, nominal wall {:.2} mm at full extrusion{}",
        c.out,
        specs.len(),
        if controls > 0 { format!(" ({controls} Z control)") } else { String::new() },
        height,
        c.layer_h,
        c.width,
        c.width * c.em,
        "" // generation aborts above when the layout does not fit
    );

    Ok(GenReport {
        summary,
        cylinder_count: specs.len(),
        height,
        bands: first_map,
        cylinder_positions: positions,
        test_objects: specs,
        band_flows,
        fits_bed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Layout;

    #[test]
    fn generated_gcode_carries_per_object_flows() {
        let out = std::env::temp_dir().join(format!("flowgen_ctl_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: out.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255],
            flows: vec![8.0, 24.0],
            revs: 1,
            reversed_control: true,
            constant_control: Some(8.0),
            layout: Layout::Row,
            ..Default::default()
        };
        let r = generate(cfg).unwrap();
        assert_eq!(r.cylinder_count, 3);
        assert_eq!(r.band_flows[0], vec![8.0, 24.0]);   // ladder
        assert_eq!(r.band_flows[1], vec![24.0, 8.0]);   // reversed
        assert_eq!(r.band_flows[2], vec![8.0, 8.0]);    // constant
        let body = fs::read_to_string(&out).unwrap();
        assert!(body.contains("REVERSED ladder"));
        assert!(body.contains("constant 8.0 mm3/s"));
        let _ = fs::remove_file(&out);
    }

    #[test]
    fn standalone_generate_writes_expected_cylinders() {
        let out = std::env::temp_dir().join(format!("flowgen_test_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: out.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255, 265],
            flows: vec![8.0, 12.0],
            revs: 2,
            ..Default::default()
        };
        let report = generate(cfg).unwrap();
        assert_eq!(report.cylinder_count, 2);
        assert_eq!(report.bands.len(), 2); // one entry per flow rate
        assert_eq!(report.cylinder_positions.len(), 2);

        let body = fs::read_to_string(&out).unwrap();
        assert!(body.contains(";=== cylinder 0 : 255 C ==="));
        assert!(body.contains(";=== cylinder 1 : 265 C ==="));
        assert!(body.contains("HAND-WRITTEN START BLOCK"));
        let _ = fs::remove_file(&out);
    }

    #[test]
    fn reference_mode_requires_a_findable_start_and_end_block() {
        let bad = std::env::temp_dir().join(format!("flowgen_bad_ref_{}.gcode", std::process::id()));
        fs::write(&bad, "G28\nG1 X0 Y0\n").unwrap();
        let cfg = Cfg { reference: Some(bad.to_string_lossy().into_owned()), ..Default::default() };
        let err = generate(cfg).unwrap_err();
        assert!(err.contains("no ;LAYER_CHANGE"), "got: {err}");
        let _ = fs::remove_file(&bad);
    }

    #[test]
    fn missing_mode_is_an_error() {
        let cfg = Cfg { reference: None, standalone: false, ..Default::default() };
        let err = generate(cfg).unwrap_err();
        assert!(err.contains("standalone"));
    }

    /// The file must begin exactly as a stock PrusaSlicer export does, with
    /// its compatibility checks first. Prepending commands ahead of them
    /// changed how the printer greeted the job (an ATTENTION prompt on
    /// upload), so the metrics block goes AFTER the checks.
    #[test]
    fn metrics_never_precede_the_compatibility_checks() {
        let out = std::env::temp_dir().join(format!("flowgen_order_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: out.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255],
            flows: vec![8.0],
            revs: 1,
            metrics_host: Some("192.0.2.18".into()),
            ..Default::default()
        };
        generate(cfg).unwrap();
        let body = fs::read_to_string(&out).unwrap();
        let code: Vec<&str> = body
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with(';'))
            .collect();
        let m862 = code.iter().position(|l| l.starts_with("M862")).expect("a compat check");
        let m334 = code.iter().position(|l| l.starts_with("M334")).expect("metrics target");
        assert!(m862 < m334, "M862 checks must come before M334; got {m862} vs {m334}");
        // and pos_z must be armed AGAIN after homing: enabling it while the Z
        // axis is unhomed silently yields no samples at all
        let homed = code.iter().position(|l| l.starts_with("G28")).unwrap_or(0);
        assert!(
            code[homed..].iter().any(|l| l.trim() == "M331 pos_z"),
            "pos_z must be re-armed after G28, or the capture gets zero Z samples"
        );
        // but still before any motion, so levelling and purge are captured
        let motion = code.iter().position(|l| l.starts_with("G28") || l.starts_with("G29"));
        if let Some(mv) = motion {
            assert!(m334 < mv, "metrics must be armed before homing/levelling");
        }
        let _ = fs::remove_file(&out);
    }

    #[test]
    fn metrics_gcode_is_only_emitted_when_a_host_is_given() {
        let with_host = std::env::temp_dir().join(format!("flowgen_metrics_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: with_host.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255],
            flows: vec![8.0],
            revs: 1,
            metrics_host: Some("192.0.2.42".into()),
            metrics_port: 9999,
            ..Default::default()
        };
        generate(cfg).unwrap();
        let body = fs::read_to_string(&with_host).unwrap();
        assert!(body.contains("M334 192.0.2.42 9999"));
        assert!(body.contains("M331 loadcell_value"));
        let _ = fs::remove_file(&with_host);

        let without_host = std::env::temp_dir().join(format!("flowgen_nometrics_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: without_host.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255],
            flows: vec![8.0],
            revs: 1,
            ..Default::default()
        };
        generate(cfg).unwrap();
        let body = fs::read_to_string(&without_host).unwrap();
        assert!(!body.contains("M334"));
        let _ = fs::remove_file(&without_host);
    }
}
