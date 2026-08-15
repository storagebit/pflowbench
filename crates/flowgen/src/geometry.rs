// geometry.rs -- bead cross-section and extrusion math.
//
// Everything downstream prices a move in these units: a bead's cross-section
// (PrusaSlicer's rectangle-with-semicircular-ends model) converts mm of path
// into mm of filament, and a commanded volumetric flow into an F value. Kept
// together so the one filament-diameter constant has a single home.

use std::f64::consts::PI;

const FILAMENT_D: f64 = 1.75;

fn filament_xsec() -> f64 {
    PI / 4.0 * FILAMENT_D * FILAMENT_D // 2.4053 mm^2
}

/// PrusaSlicer's extrusion cross-section: rectangle with semicircular ends.
pub fn extrusion_xsec(h: f64, w: f64) -> f64 {
    h * (w - h * (1.0 - PI / 4.0))
}

/// mm of filament needed to lay `len` mm of bead (relative E).
pub(crate) fn e_for(len: f64, xsec: f64, em: f64) -> f64 {
    len * xsec * em / filament_xsec()
}

/// F value (mm/min) that yields `flow` mm^3/s at this bead cross-section.
pub(crate) fn feed_for(flow: f64, xsec: f64) -> f64 {
    flow / xsec * 60.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrusion_xsec_matches_hand_calc() {
        // 0.4mm layer, 0.9mm wide bead: rectangle + semicircular ends.
        let xsec = extrusion_xsec(0.4, 0.9);
        assert!((xsec - 0.4 * (0.9 - 0.4 * (1.0 - PI / 4.0))).abs() < 1e-12);
        assert!(xsec > 0.0 && xsec < 0.9 * 0.4); // less than the bounding rectangle
    }
}
