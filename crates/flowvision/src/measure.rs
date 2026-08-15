// Per-frame outline measurement: the luma classification bounds (VisionCfg)
// and the column-wise scan of one ROI that yields a HeightMeasure.

use crate::geometry::Roi;
use crate::pgm::Pgm;

/// Luma classification bounds. Defaults measured on run 20260811-191503.
#[derive(Clone, Copy, Debug)]
pub struct VisionCfg {
    /// Pixel counts as object when luma >= bright_min (specular crown,
    /// white filament) ...
    pub bright_min: u8,
    /// ... or luma <= dark_max (dark filament wall).
    pub dark_max: u8,
    /// Consecutive object pixels a column needs before the run counts --
    /// filters 1-2 px wisps and sensor noise.
    pub min_run: usize,
    /// Stall vote when the top edge rides down at >= this fraction of the
    /// measured bed-descent rate. A healthy top is stationary in the image
    /// (it tracks the nozzle plane); a dead one descends with the bed.
    pub stall_frac: f64,
    /// Marginal vote between marginal_frac and stall_frac.
    pub marginal_frac: f64,
    /// Usable bands needed to regress the bed-descent rate.
    pub min_calib_bands: usize,
    /// Occlusion guard: fraction of 1/16-subsampled pixels moving more
    /// than 40 luma vs the cylinder's first usable frame. Measured on
    /// the real run: parked scenes 0.00-0.11, head-in-frame 0.14-0.39.
    pub occlusion_frac: f64,
}

impl Default for VisionCfg {
    fn default() -> Self {
        VisionCfg {
            bright_min: 200,
            dark_max: 30,
            min_run: 3,
            stall_frac: 0.75,
            marginal_frac: 0.4,
            min_calib_bands: 3,
            occlusion_frac: 0.15,
        }
    }
}

/// Per-frame outline measurement inside one ROI.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeightMeasure {
    /// Robust top edge: 10th percentile of per-column topmost object y --
    /// wispy crown spikes and neighbor contamination sit below p10.
    pub top_p10: f64,
    /// Bottommost object run, median across columns: the brim/base line.
    pub base_median: f64,
    /// base - top: the within-frame height. Immune to whole-scene drift.
    pub height_px: f64,
    /// Mean |top[x] - top[x+1]| across adjacent detected columns: the
    /// raggedness of the top edge.
    pub edge_energy: f64,
    /// Columns with any detection; a near-empty ROI is not usable.
    pub columns_hit: usize,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

/// Column-wise outline scan of `roi` in `img`.
pub fn measure(img: &Pgm, roi: &Roi, cfg: &VisionCfg) -> HeightMeasure {
    if roi.x0 > roi.x1 || roi.y0 > roi.y1 || roi.x0 >= img.w || roi.y0 >= img.h {
        return HeightMeasure::default();
    }
    let is_obj = |l: u8| l >= cfg.bright_min || l <= cfg.dark_max;
    let mut tops: Vec<f64> = Vec::new();
    let mut bases: Vec<f64> = Vec::new();
    let mut per_col_top: Vec<Option<f64>> = Vec::new();
    for x in roi.x0..=roi.x1.min(img.w - 1) {
        let mut top: Option<usize> = None;
        let mut base: Option<usize> = None;
        let mut run = 0usize;
        for y in roi.y0..=roi.y1.min(img.h - 1) {
            if is_obj(img.at(x, y)) {
                run += 1;
                if run >= cfg.min_run {
                    if top.is_none() {
                        top = Some(y + 1 - run);
                    }
                    base = Some(y);
                }
            } else {
                run = 0;
            }
        }
        per_col_top.push(top.map(|t| t as f64));
        if let (Some(t), Some(b)) = (top, base) {
            tops.push(t as f64);
            bases.push(b as f64);
        }
    }
    let mut tops_sorted = tops.clone();
    tops_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut bases_sorted = bases.clone();
    bases_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let top_p10 = percentile(&tops_sorted, 0.10);
    let base_median = percentile(&bases_sorted, 0.50);
    // top-edge energy over adjacent detected columns
    let mut diffs = Vec::new();
    for w in per_col_top.windows(2) {
        if let (Some(a), Some(b)) = (w[0], w[1]) {
            diffs.push((a - b).abs());
        }
    }
    let edge_energy = if diffs.is_empty() {
        f64::NAN
    } else {
        diffs.iter().sum::<f64>() / diffs.len() as f64
    };
    HeightMeasure {
        top_p10,
        base_median,
        height_px: base_median - top_p10,
        edge_energy,
        columns_hit: tops.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{frame, ROI};

    #[test]
    fn measure_reads_height_and_smooth_edges() {
        let img = frame(100, 250, 0);
        let m = measure(&img, &ROI, &VisionCfg::default());
        assert!((m.top_p10 - 100.0).abs() < 3.0, "{m:?}");
        assert!((m.base_median - 249.0).abs() < 3.0, "{m:?}");
        assert!((m.height_px - 149.0).abs() < 5.0, "{m:?}");
        assert!(m.edge_energy < 1.0, "smooth top must have ~zero energy: {m:?}");
    }

    #[test]
    fn jagged_top_edge_raises_energy() {
        let smooth = measure(&frame(100, 250, 0), &ROI, &VisionCfg::default());
        let jagged = measure(&frame(100, 250, 40), &ROI, &VisionCfg::default());
        assert!(
            jagged.edge_energy > 5.0 * smooth.edge_energy.max(0.1),
            "smooth {} vs jagged {}",
            smooth.edge_energy,
            jagged.edge_energy
        );
    }
}
