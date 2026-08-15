// calibrate.rs -- the camera calibration print.
//
// One pillar per bench layout position, each with a different known height.
// Pillar bases sit at known bed XY (z = 0); the tops add the depth spread a
// full projection solve needs (flowvision::Projection). Clicking each base
// and top in the app yields measured 3D -> 2D pairs AT the positions where
// test objects print, so vertical scale and viewing angle carry no
// cross-position transfer error.
//
// The print is deliberately cheap and safe: small spiral pillars at the
// profile's first-layer flow, one temperature, any material.

use crate::config::Cfg;
use crate::emit;
use crate::layout::layout_positions;
use crate::splice::{split_reference, standalone_blocks};
use std::fmt::Write as _;

/// One calibration pillar: bed position and printed height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pillar {
    pub x: f64,
    pub y: f64,
    pub height: f64,
}

#[derive(Debug)]
pub struct CalReport {
    pub pillars: Vec<Pillar>,
    pub summary: String,
}

/// Pillar diameter: small enough to print fast, wide enough that base and
/// top are clickable at ~4-6 px/mm image scale.
const PILLAR_DIA: f64 = 12.0;
/// Nominal pillar heights, mm; snapped to whole layers at generation.
const HEIGHTS: [f64; 4] = [6.0, 10.0, 14.0, 18.0];

/// Generate the calibration print into `c.out`. Uses the same reference
/// splice, purge, layout and safety rules as the bench job; ignores the
/// flow ladder (one safe flow) and extra temperatures (first temp only).
pub fn generate_calibration(mut c: Cfg) -> Result<CalReport, String> {
    if c.temps.is_empty() {
        return Err("need a temperature".into());
    }
    let temp = c.temps[0];
    let (start, end) = match (&c.reference, c.standalone) {
        (Some(p), _) => split_reference(&p.clone())?,
        (None, true) => standalone_blocks(&c),
        (None, false) => return Err("pick one: a reference file or standalone mode".into()),
    };

    let max_h = HEIGHTS.iter().cloned().fold(0.0f64, f64::max);
    if max_h + 5.0 > c.safe_z {
        c.safe_z = max_h + 8.0;
    }

    // Pillars stand where the bench's test objects stand: same layout code,
    // four positions. The bench prints one object per temperature; the
    // calibration print reuses those positions with the ladder untouched.
    let place = layout_positions(&c, HEIGHTS.len());
    let mut pillars = Vec::new();

    let mut g: Vec<String> = Vec::new();
    g.extend(start);
    g.push(String::new());
    g.push(format!("; camera calibration print: {} pillars, dia {PILLAR_DIA}", HEIGHTS.len()));
    g.push(format!("M104 S{temp}"));
    g.push(format!("M140 S{}", c.bed));
    g.push(format!("M109 S{temp}"));
    g.push(format!("M190 S{}", c.bed));

    let mut cal = c.clone();
    cal.diameter = PILLAR_DIA;
    // A reference start block ends primed (its own purge); the standalone
    // block does not -- prime the nozzle or pillar 0's first loops print air.
    if c.reference.is_none() {
        emit::purge_line_pub(&mut g, &cal, 0, temp);
    }
    for (i, ((cx, cy), h_nom)) in place.iter().zip(HEIGHTS).enumerate() {
        let layers = (h_nom / cal.layer_h).round().max(1.0);
        // recorded height is the PHYSICAL top: first layer + spiral layers --
        // this is the number the calibration clicks are paired with
        let h = cal.first_layer_h + layers as f64 * cal.layer_h;
        // pillars after the first start retracted (the previous pillar ends
        // on a wipe-retract); the first starts primed
        emit::pillar(&mut g, &cal, *cx, *cy, i, layers as usize, i > 0);
        pillars.push(Pillar { x: *cx, y: *cy, height: h });
    }
    g.push(format!("G1 Z{:.2} F900", cal.safe_z));
    g.extend(end);

    let body = g.join("\n") + "\n";
    std::fs::write(&c.out, &body).map_err(|e| format!("{}: {e}", c.out))?;

    let mut summary = format!("calibration print: {} pillars at ", pillars.len());
    for p in &pillars {
        let _ = write!(summary, "({:.0},{:.0})h{:.1} ", p.x, p.y, p.height);
    }
    let _ = write!(summary, "-> {}", c.out);
    Ok(CalReport { pillars, summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(out: &str) -> Cfg {
        Cfg {
            out: out.into(),
            standalone: true,
            temps: vec![215],
            flows: vec![8.0],
            ..Default::default()
        }
    }

    #[test]
    fn calibration_print_has_four_distinct_pillars() {
        let out = std::env::temp_dir().join(format!("flowgen_cal_{}.gcode", std::process::id()));
        let r = generate_calibration(cfg(out.to_str().unwrap())).unwrap();
        assert_eq!(r.pillars.len(), 4);
        // heights all different (the projection solve needs depth spread)
        let mut hs: Vec<f64> = r.pillars.iter().map(|p| p.height).collect();
        hs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        hs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert_eq!(hs.len(), 4, "{:?}", r.pillars);
        // heights are the physical top: first layer + whole spiral layers
        for p in &r.pillars {
            let layers = (p.height - 0.2) / 0.4;
            assert!((layers - layers.round()).abs() < 1e-9, "{}", p.height);
        }
        // distinct positions
        let mut xy: Vec<(i64, i64)> =
            r.pillars.iter().map(|p| (p.x as i64, p.y as i64)).collect();
        xy.sort_unstable();
        xy.dedup();
        assert_eq!(xy.len(), 4);
        let body = std::fs::read_to_string(&out).unwrap();
        assert!(body.contains("calibration print"));
        // retract ledger: pillar 0 starts primed (1 travel retract);
        // pillars 1-3 start wipe-retracted (no travel retract). Rings pair
        // their own retract/deretract (4 pillars x 4 rings). Deretracts:
        // 16 ring + 4 pillar-travel = 20; wipes leave the file retracted
        // once at the end. Doubles starve the seam -- hence exact counts.
        assert_eq!(body.matches("retract for travel").count(), 1);
        assert_eq!(body.matches("retract for ring approach").count(), 16);
        assert_eq!(body.matches("; deretract").count(), 20);
        // standalone start block does not prime: the print must purge
        assert!(body.contains("purge lane"), "no purge in standalone calibration print");
        std::fs::remove_file(out).ok();
    }

    #[test]
    fn calibration_needs_a_start_block_source() {
        let mut c = cfg("/tmp/never_written.gcode");
        c.standalone = false;
        assert!(generate_calibration(c).is_err());
    }
}
