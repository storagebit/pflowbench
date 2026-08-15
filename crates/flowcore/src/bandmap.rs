// Band and cylinder addressing from the Z stream: band index from a Z window
// (first layer and overtravel excluded), cylinder boundaries from Z dropping
// back to the first layer. This is the legacy fallback path -- with a band
// manifest present, sdmap addresses bands deterministically instead.

#[derive(Clone, Debug)]
pub struct BandMap {
    /// Default flow per band, bottom to top.
    pub flows: Vec<f64>,
    /// Optional per-cylinder override, indexed by cylinder. Needed because a
    /// run may include Z-control test objects (a reversed ladder, or one constant
    /// flow throughout) where band N does NOT carry the same flow it does on
    /// the baseline cylinder. Empty = every cylinder uses `flows`.
    pub per_cylinder_flows: Vec<Vec<f64>>,
    pub revs: usize,
    pub layer_h: f64,
    pub first_layer_h: f64,
    /// Bead cross-section in mm^2 (PrusaSlicer's rectangle-with-round-ends
    /// model). With it, measured head speed converts to the volumetric flow
    /// the printer ACTUALLY delivered, which is not necessarily the flow the
    /// band commanded -- acceleration and cornering limits can hold it below.
    pub bead_xsec: Option<f64>,
}

impl BandMap {
    /// Flow rate actually commanded for this (cylinder, band).
    pub fn flow_for(&self, cylinder: usize, band: usize) -> f64 {
        self.per_cylinder_flows
            .get(cylinder)
            .and_then(|f| f.get(band))
            .or_else(|| self.flows.get(band))
            .copied()
            .unwrap_or(0.0)
    }
}

impl BandMap {
    pub fn band_h(&self) -> f64 {
        self.revs as f64 * self.layer_h
    }
    pub fn height(&self) -> f64 {
        self.first_layer_h + self.flows.len() as f64 * self.band_h()
    }
    /// Band index for a Z inside the spiral; None for purge/first layer/overtravel.
    pub fn band_for(&self, z: f64) -> Option<usize> {
        if z <= self.first_layer_h + 0.05 || z > self.height() {
            return None;
        }
        Some(
            (((z - self.first_layer_h) / self.band_h()).floor() as usize)
                .min(self.flows.len() - 1),
        )
    }
}

/// Detects cylinder changes from the Z stream alone: a drop back to the first
/// layer after the stack has been climbed means the next cylinder started.
#[derive(Default)]
pub struct CylTracker {
    pub cylinder: usize,
    z_max: f64,
}

impl CylTracker {
    pub fn observe(&mut self, z: f64, map: &BandMap) {
        if z < map.first_layer_h + 0.5 && self.z_max > map.height() * 0.5 {
            self.cylinder += 1;
            self.z_max = 0.0;
        }
        if z > self.z_max {
            self.z_max = z;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_mapping_excludes_first_layer_and_clamps() {
        let m = BandMap { flows: vec![8., 10., 12.], per_cylinder_flows: Vec::new(), revs: 4, layer_h: 0.4, first_layer_h: 0.2, bead_xsec: None };
        assert_eq!(m.band_for(0.20), None); // first layer
        assert_eq!(m.band_for(0.26), Some(0));
        assert_eq!(m.band_for(1.79), Some(0));
        assert_eq!(m.band_for(1.81), Some(1));
        assert_eq!(m.band_for(5.0), Some(2)); // exactly top -> clamp into last band
        assert_eq!(m.band_for(5.1), None); // overtravel
    }

    #[test]
    fn cylinder_boundary_from_z_drop() {
        let m = BandMap { flows: vec![8., 10.], per_cylinder_flows: Vec::new(), revs: 4, layer_h: 0.4, first_layer_h: 0.2, bead_xsec: None };
        let mut t = CylTracker::default();
        for z in [0.2, 1.0, 2.0, 3.4] {
            t.observe(z, &m);
        }
        assert_eq!(t.cylinder, 0);
        t.observe(0.2, &m); // back to the deck after climbing -> next cylinder
        assert_eq!(t.cylinder, 1);
    }

    #[test]
    fn flow_for_uses_the_per_cylinder_schedule_when_present() {
        // cylinder 0 runs the ladder, cylinder 1 is a reversed Z control,
        // cylinder 2 is a constant-flow control.
        let m = BandMap {
            flows: vec![8., 10., 12.],
            per_cylinder_flows: vec![
                vec![8., 10., 12.],
                vec![12., 10., 8.],
                vec![8., 8., 8.],
            ],
            revs: 4,
            layer_h: 0.4,
            first_layer_h: 0.2,
            bead_xsec: None,
        };
        assert_eq!(m.flow_for(0, 0), 8.0);
        assert_eq!(m.flow_for(1, 0), 12.0, "reversed control must not report the ladder's flow");
        assert_eq!(m.flow_for(2, 2), 8.0);
        // a cylinder beyond the schedule falls back to the default ladder
        assert_eq!(m.flow_for(9, 1), 10.0);
    }

    #[test]
    fn flow_for_falls_back_to_the_ladder_without_a_schedule() {
        let m = BandMap {
            flows: vec![8., 10.],
            per_cylinder_flows: Vec::new(),
            revs: 4,
            layer_h: 0.4,
            first_layer_h: 0.2,
            bead_xsec: None,
        };
        assert_eq!(m.flow_for(0, 1), 10.0);
        assert_eq!(m.flow_for(3, 0), 8.0);
        assert_eq!(m.flow_for(0, 99), 0.0, "out-of-range band must not panic");
    }
}
