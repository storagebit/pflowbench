// layout.rs -- what gets printed where.
//
// Two decisions live here: which test objects a config produces (one per
// temperature, plus optional Z controls that decorrelate flow from height),
// and where each sits on the bed. Placement is a camera problem as much as a
// bed-size problem -- the camera sits at a fixed oblique angle, so the layout
// decides whether every test object is visible. The cold preview exists to
// settle that before any filament is committed.

use std::f64::consts::PI;

use crate::config::Cfg;

/// How the flow ladder is applied to one test object.
///
/// Band index is otherwise *perfectly correlated with Z height*, so a knee in
/// force or wall quality could equally be caused by height, cooling, or
/// thermal history rather than by flow rate. These variants turn that
/// correlation into a test: if the knee tracks flow rather than Z, it's real.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlowProgram {
    /// The configured ladder, low -> high with increasing Z.
    Ladder,
    /// The same ladder printed high -> low. A genuine flow knee moves to the
    /// opposite end of the test object; a height artifact stays put.
    Reversed,
    /// Every band at one flow rate. This control should show NO knee at all --
    /// if one appears, it is an artifact of Z, not of flow.
    Constant(f64),
}

/// One printed cylinder: a nozzle temperature plus a flow program.
#[derive(Clone, Debug, PartialEq)]
pub struct TestObject {
    pub temp: i64,
    pub program: FlowProgram,
}

impl TestObject {
    /// Flow rate per band in print order (bottom to top).
    pub fn flows(&self, ladder: &[f64]) -> Vec<f64> {
        match self.program {
            FlowProgram::Ladder => ladder.to_vec(),
            FlowProgram::Reversed => ladder.iter().rev().copied().collect(),
            FlowProgram::Constant(f) => vec![f; ladder.len()],
        }
    }
    pub fn label(&self) -> String {
        match self.program {
            FlowProgram::Ladder => format!("{} C, ladder", self.temp),
            FlowProgram::Reversed => format!("{} C, REVERSED ladder (Z control)", self.temp),
            FlowProgram::Constant(f) => format!("{} C, constant {f:.1} mm3/s (Z control)", self.temp),
        }
    }
}

/// Bed arrangement. The camera sits at a fixed oblique angle, so how the
/// test objects are placed decides whether it can see all of them.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Layout {
    /// Two-column grid centred on the bed. Compact, but a test object in the back
    /// row can sit directly behind a front one from the camera's viewpoint.
    #[default]
    Grid,
    /// One row across the bed. Nothing occludes anything else, so it is the
    /// best layout for the camera -- but it only fits a few test objects.
    Row,
    /// Two rows with the back row offset by half a pitch, so each back
    /// test object sits in the gap between two front ones. A compromise when a
    /// single row won't fit.
    Stagger,
}

/// Every test object this config will print, in order: one per temperature,
/// then any Z controls. Controls use the first temperature so they differ
/// from the baseline test object in flow program alone.
pub fn test_objects(c: &Cfg) -> Vec<TestObject> {
    let mut v: Vec<TestObject> = c
        .temps
        .iter()
        .map(|&temp| TestObject { temp, program: FlowProgram::Ladder })
        .collect();
    let base = c.temps.first().copied().unwrap_or(255);
    if c.reversed_control {
        v.push(TestObject { temp: base, program: FlowProgram::Reversed });
    }
    if let Some(f) = c.constant_control {
        v.push(TestObject { temp: base, program: FlowProgram::Constant(f) });
    }
    v
}

/// Centre-to-centre spacing that keeps brims from touching.
pub(crate) fn object_pitch(c: &Cfg) -> f64 {
    c.diameter + 2.0 * c.brim as f64 * c.first_layer_w + 35.0
}

/// Bed XY for each test object under the configured layout.
///
/// Y increases away from the camera, so a `Row` puts every test object at the
/// same distance and none can hide another. `Stagger` pushes odd-indexed
/// test objects to a second row, offset half a pitch sideways so they show
/// through the gaps rather than sitting directly behind.
pub fn layout_positions(c: &Cfg, n: usize) -> Vec<(f64, f64)> {
    let pitch = object_pitch(c);
    let (cx, cy) = (c.bed_x / 2.0 + c.layout_offset.0, c.bed_y / 2.0 + c.layout_offset.1);
    let mut out = Vec::with_capacity(n);
    match c.layout {
        Layout::Row => {
            let x0 = cx - (n as f64 - 1.0) * pitch / 2.0;
            for i in 0..n {
                out.push((x0 + i as f64 * pitch, cy));
            }
        }
        Layout::Stagger => {
            let front = n.div_ceil(2);
            let back = n - front;
            let fx0 = cx - (front as f64 - 1.0) * pitch / 2.0;
            let bx0 = cx - (back as f64 - 1.0) * pitch / 2.0;
            // front row nearer the camera (lower Y), back row half a pitch over
            for i in 0..n {
                if i % 2 == 0 {
                    out.push((fx0 + (i / 2) as f64 * pitch, cy - pitch / 2.0));
                } else {
                    out.push((bx0 + (i / 2) as f64 * pitch + pitch / 2.0, cy + pitch / 2.0));
                }
            }
        }
        Layout::Grid => {
            let cols = if n > 1 { 2 } else { 1 };
            let rows = n.div_ceil(cols);
            let ox = cx - (cols - 1) as f64 * pitch / 2.0;
            let oy = cy - (rows - 1) as f64 * pitch / 2.0 + 15.0;
            for i in 0..n {
                out.push((ox + (i % cols) as f64 * pitch, oy + (i / cols) as f64 * pitch));
            }
        }
    }
    out
}

/// True if every test object footprint (brim included) sits on the bed.
pub fn layout_fits(c: &Cfg, positions: &[(f64, f64)]) -> bool {
    let r = c.diameter / 2.0 + c.brim as f64 * c.first_layer_w;
    positions
        .iter()
        .all(|&(x, y)| x - r >= 0.0 && x + r <= c.bed_x && y - r >= 0.0 && y + r <= c.bed_y)
}

/// A no-extrusion tour of the test object footprints at safe Z, for aiming the
/// camera before committing filament: run it, watch the live view, and adjust
/// the camera (or `layout_offset`) until every circle is clearly in frame.
/// Cold -- it never heats, extrudes, or touches the bed.
pub fn layout_preview_gcode(c: &Cfg) -> String {
    let specs = test_objects(c);
    let pos = layout_positions(c, specs.len());
    let r = c.diameter / 2.0;
    let mut g = vec![
        "; pflowbench layout preview -- NO extrusion, NO heating.".to_string(),
        "; Traces each test object footprint so the camera can be aimed before printing.".to_string(),
        format!("M862.1 P{} A1 F0 ; nozzle check", c.nozzle),
        "M862.3 P \"COREONEL\" ; printer model check".to_string(),
        "M862.5 P2 ; g-code level check".to_string(),
        "M862.6 P\"Input shaper\" ; FW feature check".to_string(),
        "M115 U6.5.7+12836".to_string(),
        "G21".to_string(),
        "G90".to_string(),
        "M83".to_string(),
        "G28 ; home".to_string(),
    ];
    let z = c.safe_z.max(10.0);
    for (i, (&(cx, cy), s)) in pos.iter().zip(specs.iter()).enumerate() {
        g.push(format!("; test object {i}: {}", s.label()));
        g.push(format!("G1 Z{z:.2} F600"));
        g.push(format!("G1 X{:.3} Y{:.3} F{}", cx + r, cy, c.travel_f));
        for k in 1..=48 {
            let a = 2.0 * PI * k as f64 / 48.0;
            g.push(format!(
                "G1 X{:.3} Y{:.3} F{}",
                cx + r * a.cos(),
                cy + r * a.sin(),
                c.travel_f / 2
            ));
        }
        g.push(format!("G4 S2 ; pause on test object {i} for a clear look"));
    }
    g.push(format!("G1 Z{:.2} F600", z + 20.0));
    g.push("M84".to_string());
    g.push("; filament used [g] = 0.00".to_string());
    g.push("; estimated printing time (normal mode) = 1m".to_string());
    g.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Profile;

    #[test]
    fn reversed_program_flips_the_ladder_and_constant_flattens_it() {
        let ladder = vec![8.0, 10.0, 12.0];
        let s = |p| TestObject { temp: 255, program: p };
        assert_eq!(s(FlowProgram::Ladder).flows(&ladder), vec![8.0, 10.0, 12.0]);
        assert_eq!(s(FlowProgram::Reversed).flows(&ladder), vec![12.0, 10.0, 8.0]);
        assert_eq!(s(FlowProgram::Constant(9.0)).flows(&ladder), vec![9.0, 9.0, 9.0]);
    }

    #[test]
    fn controls_are_appended_at_the_first_temperature() {
        let cfg = Cfg {
            temps: vec![255, 265],
            reversed_control: true,
            constant_control: Some(8.0),
            ..Default::default()
        };
        let s = test_objects(&cfg);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0], TestObject { temp: 255, program: FlowProgram::Ladder });
        assert_eq!(s[1], TestObject { temp: 265, program: FlowProgram::Ladder });
        // controls share the baseline temperature so flow program is the only variable
        assert_eq!(s[2], TestObject { temp: 255, program: FlowProgram::Reversed });
        assert_eq!(s[3], TestObject { temp: 255, program: FlowProgram::Constant(8.0) });
    }

    #[test]
    fn row_layout_keeps_every_object_at_one_depth() {
        let cfg = Cfg { layout: Layout::Row, ..Default::default() };
        let p = layout_positions(&cfg, 3);
        assert!(p.iter().all(|&(_, y)| (y - p[0].1).abs() < 1e-9), "row must share one Y: {p:?}");
        // and be evenly spaced, centred on the bed
        let mid = p[1].0;
        assert!((mid - cfg.bed_x / 2.0).abs() < 1e-9, "{p:?}");
        assert!((p[2].0 - p[1].0) - (p[1].0 - p[0].0) < 1e-9);
    }

    #[test]
    fn stagger_layout_alternates_depth_and_offsets_sideways() {
        let cfg = Cfg { layout: Layout::Stagger, ..Default::default() };
        let p = layout_positions(&cfg, 4);
        // even indices in the front row, odd in the back
        assert!(p[0].1 < p[1].1, "{p:?}");
        assert!((p[0].1 - p[2].1).abs() < 1e-9);
        assert!((p[1].1 - p[3].1).abs() < 1e-9);
        // back row must not sit directly behind a front one
        for &(bx, _) in [p[1], p[3]].iter() {
            assert!(p.iter().step_by(2).all(|&(fx, _)| (fx - bx).abs() > 1.0), "{p:?}");
        }
    }

    #[test]
    fn layout_offset_shifts_the_whole_arrangement() {
        let base = Cfg { layout: Layout::Row, ..Default::default() };
        let moved = Cfg { layout: Layout::Row, layout_offset: (20.0, -30.0), ..Default::default() };
        let (a, b) = (layout_positions(&base, 3), layout_positions(&moved, 3));
        for (p, q) in a.iter().zip(b.iter()) {
            assert!((q.0 - p.0 - 20.0).abs() < 1e-9);
            assert!((q.1 - p.1 + 30.0).abs() < 1e-9);
        }
    }

    #[test]
    fn layout_fits_detects_an_overfull_bed() {
        // four 50mm test objects in one row need ~330mm; a 300mm bed can't take it
        let cfg = Cfg { layout: Layout::Row, ..Default::default() };
        assert!(!layout_fits(&cfg, &layout_positions(&cfg, 4)));
        assert!(layout_fits(&cfg, &layout_positions(&cfg, 3)));
    }

    #[test]
    fn layout_preview_is_cold_and_never_extrudes() {
        let cfg = Cfg { temps: vec![255, 265], layout: Layout::Row, ..Default::default() };
        let g = layout_preview_gcode(&cfg);
        // no extrusion moves, no heating anywhere
        assert!(!g.contains(" E"), "preview must not extrude");
        assert!(!g.contains("M104") && !g.contains("M109"), "preview must not heat the nozzle");
        assert!(!g.contains("M140") && !g.contains("M190"), "preview must not heat the bed");
        // but it must still pass the firmware compatibility gate
        assert!(g.contains("M862.3 P \"COREONEL\""));
        assert!(g.contains("G28"));
        // one header comment per test object (the word also appears in the file
        // preamble and in each dwell comment, so match the header form)
        assert_eq!(g.matches("; test object ").count(), 2);
    }

    #[test]
    fn layout_reality_check() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let p = Profile::load(&format!("{root}/profiles/pc-blend-cf-0.8-diamondback.profile")).unwrap();
        for lay in [Layout::Row, Layout::Stagger, Layout::Grid] {
            for n in [2usize, 3, 4] {
                let mut c = p.cfg.clone();
                c.layout = lay;
                let pos = layout_positions(&c, n);
                let r = c.diameter / 2.0 + c.brim as f64 * c.first_layer_w;
                let fits = layout_fits(&c, &pos);
                let xs: Vec<String> = pos.iter().map(|(x,y)| format!("({x:.0},{y:.0})")).collect();
                println!("{:?} n={n} pitch={:.1} r={r:.1} fits={fits}  centers {}",
                    lay, object_pitch(&c), xs.join(" "));
            }
        }
    }
}
