// flowvision -- Tier 1 local vision analysis for PFlowBench (verdict-plan §7).
//
// Classical CV over the per-band PGM lumas the capture saves; no models, no
// network, std-only like every other crate here. Two measures, both chosen
// because they are exactly what a human used to diagnose the first failed
// run from its photos:
//
//   * growth: the test object's outline height, measured WITHIN each frame
//     as (base - top). The camera scene drifts tens of pixels between
//     keyframes (measured on run 20260811-191503: up to 38 px), so absolute
//     top-edge positions are useless -- the within-frame difference is not.
//   * raggedness: high-frequency energy along the detected top edge. A
//     healthy spiral wall has a smooth top; the melt-failure crown is
//     jagged.
//
// Everything self-normalizes against the same test object's earlier bands --
// no absolute pixel thresholds decide anything, matching the force rules.
// Vision votes only downgrade a band, never rescue it.
//
// Pixel classification: an "object" pixel is far from the bed's mid-gray --
// either specular-bright (the crown highlights, white filament) or
// near-black (dark filament walls). Measured on the real run: bed luma
// spans ~40-170 across the frame, crown spikes >= 210, PC-CF walls < 30.
//
// Layout: pgm (P5 parse/load), geometry (Homography, Roi), measure
// (VisionCfg, the per-frame outline scan), analyze (per-cylinder votes).

mod analyze;
mod geometry;
mod measure;
mod pgm;

#[cfg(test)]
pub(crate) mod test_util;

pub use analyze::{analyze_cylinder, parse_name, BandVision, Vote};
pub use geometry::{Homography, Projection, Roi};
pub use measure::{measure, HeightMeasure, VisionCfg};
pub use pgm::Pgm;
