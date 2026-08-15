// Shared test fixtures for the measure and analyze tests: the synthetic
// frame renderer and the ROI both scan.

use crate::geometry::Roi;
use crate::pgm::Pgm;

/// Render a synthetic 200x300 frame at REAL geometry: mid-gray bed
/// (luma 120), a dark test object from `top_y` down to `base_y` across
/// x 60..140, optional jagged top. The bed-descent physics the detector
/// rides on: base_y descends ~7 px per band (the real measured rate);
/// a healthy top stays put, a stalled top descends with the bed.
pub(crate) fn frame(top_y: usize, base_y: usize, jag: usize) -> Pgm {
    let (w, h) = (200usize, 300usize);
    let mut data = vec![120u8; w * h];
    for x in 60..140 {
        let t = top_y + if jag > 0 { (x * 7919) % jag } else { 0 };
        for y in t..base_y {
            data[y * w + x] = 10;
        }
    }
    Pgm { w, h, data }
}

pub(crate) const ROI: Roi = Roi { x0: 50, x1: 150, y0: 0, y1: 280 };
