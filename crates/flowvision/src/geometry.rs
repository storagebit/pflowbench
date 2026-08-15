// Bed-plane geometry: the homography solved from the four clicked brim
// centres, and the image ROI it projects for a test object.

/// Bed-plane (x, y, mm) to image (x, y, px) homography, solved from the four
/// brim centres the user clicks once per camera setup.
#[derive(Clone, Copy, Debug)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    /// Solve from exactly four correspondences ((bed_x, bed_y), (img_x, img_y)).
    /// Standard DLT: 8 equations, 8 unknowns, h9 = 1.
    pub fn solve(pairs: &[((f64, f64), (f64, f64)); 4]) -> Result<Homography, String> {
        let mut a = [[0.0f64; 9]; 8];
        for (i, ((bx, by), (ix, iy))) in pairs.iter().enumerate() {
            a[2 * i] = [*bx, *by, 1.0, 0.0, 0.0, 0.0, -ix * bx, -ix * by, *ix];
            a[2 * i + 1] = [0.0, 0.0, 0.0, *bx, *by, 1.0, -iy * bx, -iy * by, *iy];
        }
        // Gaussian elimination with partial pivoting on the 8x9 system.
        for col in 0..8 {
            let piv = (col..8)
                .max_by(|&r1, &r2| a[r1][col].abs().partial_cmp(&a[r2][col].abs()).unwrap())
                .unwrap();
            if a[piv][col].abs() < 1e-12 {
                return Err("degenerate calibration points (collinear?)".into());
            }
            a.swap(col, piv);
            let d = a[col][col];
            for v in a[col].iter_mut() {
                *v /= d;
            }
            for row in 0..8 {
                if row != col {
                    let f = a[row][col];
                    for k in 0..9 {
                        a[row][k] -= f * a[col][k];
                    }
                }
            }
        }
        let mut h = [0.0f64; 9];
        for i in 0..8 {
            h[i] = a[i][8];
        }
        h[8] = 1.0;
        Ok(Homography(h))
    }

    /// Bed mm -> image px.
    pub fn map(&self, bx: f64, by: f64) -> (f64, f64) {
        let h = &self.0;
        let w = h[6] * bx + h[7] * by + h[8];
        ((h[0] * bx + h[1] * by + h[2]) / w, (h[3] * bx + h[4] * by + h[5]) / w)
    }

    /// Serialize/deserialize as one number per line -- the run directory's
    /// vision.calib file.
    pub fn to_text(&self) -> String {
        self.0.iter().map(|v| format!("{v:.17e}\n")).collect()
    }

    pub fn from_text(text: &str) -> Result<Homography, String> {
        let vals: Vec<f64> = text
            .split_whitespace()
            .map(|t| t.parse().map_err(|_| format!("bad calib value {t:?}")))
            .collect::<Result<_, _>>()?;
        if vals.len() != 9 {
            return Err(format!("calib needs 9 values, got {}", vals.len()));
        }
        let mut h = [0.0; 9];
        h.copy_from_slice(&vals);
        Ok(Homography(h))
    }
}

/// Image-pixel region a test object is measured in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Roi {
    pub x0: usize,
    pub x1: usize,
    pub y0: usize,
    pub y1: usize,
}

impl Roi {
    /// TestObject ROI from the calibration: project the cylinder's bed-plane
    /// extremes for the x-range and base line, then open the window upward
    /// to catch the growing outline. `margin` widens x on both sides.
    pub fn from_homography(
        h: &Homography,
        cx: f64,
        cy: f64,
        dia_mm: f64,
        img_w: usize,
        img_h: usize,
        margin: usize,
    ) -> Roi {
        let r = dia_mm / 2.0;
        let pts = [
            h.map(cx - r, cy),
            h.map(cx + r, cy),
            h.map(cx, cy - r),
            h.map(cx, cy + r),
        ];
        let xs: Vec<f64> = pts.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = pts.iter().map(|p| p.1).collect();
        let xmin = xs.iter().cloned().fold(f64::MAX, f64::min);
        let xmax = xs.iter().cloned().fold(f64::MIN, f64::max);
        let ybase = ys.iter().cloned().fold(f64::MIN, f64::max);
        let x0 = (xmin as isize - margin as isize).max(0) as usize;
        let x1 = (xmax as usize + margin).min(img_w.saturating_sub(1));
        let ytop = ys.iter().cloned().fold(f64::MAX, f64::min);
        // NOT the frame top: opening y0 to 0 admitted the dark chamber
        // interior, which classifies as object and pinned the top edge
        // (+140-160 px fabricated height on the real frames). The test object
        // stack is bounded by ~0.5x the projected footprint width above the
        // projected rim -- generous for any stack this bench prints.
        let stack_allowance = (xmax - xmin) * 0.5;
        let y0 = ((ytop - stack_allowance).max(0.0)) as usize;
        let y1 = ((ybase as usize) + margin).min(img_h.saturating_sub(1));
        Roi { x0, x1, y0, y1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_maps_the_calibration_points_back() {
        // a perspective-ish quad
        let pairs = [
            ((0.0, 0.0), (100.0, 800.0)),
            ((100.0, 0.0), (900.0, 780.0)),
            ((100.0, 100.0), (700.0, 200.0)),
            ((0.0, 100.0), (250.0, 230.0)),
        ];
        let h = Homography::solve(&pairs).unwrap();
        for ((bx, by), (ix, iy)) in pairs {
            let (mx, my) = h.map(bx, by);
            assert!((mx - ix).abs() < 1e-6 && (my - iy).abs() < 1e-6, "{mx},{my}");
        }
        // round-trips through text
        let h2 = Homography::from_text(&h.to_text()).unwrap();
        let (a, b) = h.map(50.0, 50.0);
        let (c, d) = h2.map(50.0, 50.0);
        assert!((a - c).abs() < 1e-9 && (b - d).abs() < 1e-9);
        // degenerate points must not silently "solve"
        let bad = [
            ((0.0, 0.0), (0.0, 0.0)),
            ((1.0, 1.0), (1.0, 1.0)),
            ((2.0, 2.0), (2.0, 2.0)),
            ((3.0, 3.0), (3.0, 3.0)),
        ];
        assert!(Homography::solve(&bad).is_err());
    }
}

/// Full camera model: 3x4 projection matrix mapping bed-space (x, y, z, mm)
/// to image (px). Solved from the calibration print's pillar bases and tops
/// -- measured correspondences at the positions where test objects print,
/// so vertical scale and viewing angle are known everywhere they are used.
#[derive(Clone, Copy, Debug)]
pub struct Projection(pub [f64; 12]);

impl Projection {
    /// Solve by direct linear transform from world->image correspondences.
    /// Needs at least 6 points that are not all coplanar (pillar bases are
    /// all at z = 0; the tops' differing heights provide depth).
    pub fn solve(points: &[((f64, f64, f64), (f64, f64))]) -> Result<Projection, String> {
        if points.len() < 6 {
            return Err(format!("projection needs >= 6 points, got {}", points.len()));
        }
        let zs: Vec<f64> = points.iter().map(|((_, _, z), _)| *z).collect();
        let z_spread = zs.iter().cloned().fold(f64::MIN, f64::max)
            - zs.iter().cloned().fold(f64::MAX, f64::min);
        if z_spread < 1.0 {
            return Err("calibration points are coplanar -- pillar heights must differ".into());
        }
        // Normal equations for the 11-parameter DLT (p12 = 1). The system is
        // small (11x11) and the points are user-clicked, so Gaussian
        // elimination with pivoting is accurate enough.
        let mut ata = [[0.0f64; 12]; 11];
        for ((x, y, z), (u, v)) in points {
            let rows: [[f64; 12]; 2] = [
                [*x, *y, *z, 1.0, 0.0, 0.0, 0.0, 0.0, -u * x, -u * y, -u * z, *u],
                [0.0, 0.0, 0.0, 0.0, *x, *y, *z, 1.0, -v * x, -v * y, -v * z, *v],
            ];
            for r in rows {
                for i in 0..11 {
                    for j in 0..11 {
                        ata[i][j] += r[i] * r[j];
                    }
                    ata[i][11] += r[i] * r[11];
                }
            }
        }
        for col in 0..11 {
            let piv = (col..11)
                .max_by(|&a, &b| ata[a][col].abs().partial_cmp(&ata[b][col].abs()).unwrap())
                .unwrap();
            if ata[piv][col].abs() < 1e-9 {
                return Err("degenerate calibration geometry".into());
            }
            ata.swap(col, piv);
            let d = ata[col][col];
            for v in ata[col].iter_mut() {
                *v /= d;
            }
            for row in 0..11 {
                if row != col {
                    let f = ata[row][col];
                    for k in 0..12 {
                        ata[row][k] -= f * ata[col][k];
                    }
                }
            }
        }
        let mut p = [0.0f64; 12];
        for i in 0..11 {
            p[i] = ata[i][11];
        }
        p[11] = 1.0;
        Ok(Projection(p))
    }

    /// Bed-space mm -> image px.
    pub fn map3(&self, x: f64, y: f64, z: f64) -> (f64, f64) {
        let p = &self.0;
        let w = p[8] * x + p[9] * y + p[10] * z + p[11];
        (
            (p[0] * x + p[1] * y + p[2] * z + p[3]) / w,
            (p[4] * x + p[5] * y + p[6] * z + p[7]) / w,
        )
    }

    /// Vertical image scale at a bed position: px per mm of height, measured
    /// over the first 10 mm. This is the number the plane homography cannot
    /// provide and the vote thresholds depend on.
    pub fn vertical_px_per_mm(&self, x: f64, y: f64) -> f64 {
        let (_, v0) = self.map3(x, y, 0.0);
        let (_, v10) = self.map3(x, y, 10.0);
        (v0 - v10) / 10.0
    }

    /// Worst reprojection distance over the calibration points, in px --
    /// unlike an exact 4-point homography this is overdetermined, so the
    /// residual is a real quality number. Above ~5 px means a mis-click.
    pub fn worst_residual(&self, points: &[((f64, f64, f64), (f64, f64))]) -> f64 {
        points.iter().fold(0.0f64, |acc, ((x, y, z), (u, v))| {
            let (mu, mv) = self.map3(*x, *y, *z);
            acc.max(((mu - u).powi(2) + (mv - v).powi(2)).sqrt())
        })
    }

    pub fn to_text(&self) -> String {
        self.0.iter().map(|v| format!("{v:.17e}\n")).collect()
    }

    pub fn from_text(text: &str) -> Result<Projection, String> {
        let vals: Vec<f64> = text
            .split_whitespace()
            .map(|t| t.parse().map_err(|_| format!("bad camera value {t:?}")))
            .collect::<Result<_, _>>()?;
        if vals.len() != 12 {
            return Err(format!("camera model needs 12 values, got {}", vals.len()));
        }
        let mut p = [0.0; 12];
        p.copy_from_slice(&vals);
        Ok(Projection(p))
    }
}

impl Roi {
    /// Test-object ROI from the full camera model: project the base circle
    /// for the x-range and base line, the expected final height for the top
    /// bound. No stack allowance guesswork -- the model knows the angles.
    pub fn from_projection(
        p: &Projection,
        cx: f64,
        cy: f64,
        dia_mm: f64,
        max_h_mm: f64,
        img_w: usize,
        img_h: usize,
        margin: usize,
    ) -> Roi {
        let r = dia_mm / 2.0;
        let base = [
            p.map3(cx - r, cy, 0.0),
            p.map3(cx + r, cy, 0.0),
            p.map3(cx, cy - r, 0.0),
            p.map3(cx, cy + r, 0.0),
        ];
        let top = [
            p.map3(cx - r, cy, max_h_mm),
            p.map3(cx + r, cy, max_h_mm),
            p.map3(cx, cy - r, max_h_mm),
            p.map3(cx, cy + r, max_h_mm),
        ];
        let xs = base.iter().chain(&top).map(|q| q.0);
        let xmin = xs.clone().fold(f64::MAX, f64::min);
        let xmax = xs.fold(f64::MIN, f64::max);
        let ybase = base.iter().map(|q| q.1).fold(f64::MIN, f64::max);
        let ytop = top.iter().map(|q| q.1).fold(f64::MAX, f64::min);
        Roi {
            x0: (xmin as isize - margin as isize).max(0) as usize,
            x1: (xmax as usize + margin).min(img_w.saturating_sub(1)),
            y0: (ytop as isize - margin as isize).max(0) as usize,
            y1: ((ybase as usize) + margin).min(img_h.saturating_sub(1)),
        }
    }
}

#[cfg(test)]
mod projection_tests {
    use super::*;

    /// A synthetic oblique camera: u depends on x with perspective, v on y
    /// and height. Chosen so no coefficient is zero and w varies with depth.
    fn cam(x: f64, y: f64, z: f64) -> (f64, f64) {
        let w = 0.0008 * y + 1.0;
        ((5.6 * x + 0.3 * y + 210.0) / w, (0.4 * x + 3.1 * y - 4.4 * z + 95.0) / w)
    }

    fn pillar_points() -> Vec<((f64, f64, f64), (f64, f64))> {
        // four pillar positions, base + top each, heights 6/10/14/18
        let spots = [(104.5, 119.5, 6.0), (195.5, 119.5, 10.0), (104.5, 210.5, 14.0), (195.5, 210.5, 18.0)];
        let mut pts = Vec::new();
        for (x, y, h) in spots {
            pts.push(((x, y, 0.0), cam(x, y, 0.0)));
            pts.push(((x, y, h), cam(x, y, h)));
        }
        pts
    }

    #[test]
    fn projection_recovers_a_synthetic_camera() {
        let pts = pillar_points();
        let p = Projection::solve(&pts).unwrap();
        assert!(p.worst_residual(&pts) < 1e-6, "{}", p.worst_residual(&pts));
        // predicts an UNSEEN point, not just the calibration set
        let (u, v) = p.map3(150.0, 165.0, 12.0);
        let (eu, ev) = cam(150.0, 165.0, 12.0);
        assert!((u - eu).abs() < 1e-6 && (v - ev).abs() < 1e-6, "{u},{v} vs {eu},{ev}");
        // vertical scale matches the synthetic camera's 4.4 px/mm (w ~ 1.1)
        let s = p.vertical_px_per_mm(150.0, 165.0);
        let expect = 4.4 / (0.0008 * 165.0 + 1.0);
        assert!((s - expect).abs() < 0.01, "{s} vs {expect}");
    }

    #[test]
    fn coplanar_and_underdetermined_points_are_rejected() {
        let flat: Vec<_> = pillar_points().into_iter().map(|((x, y, _), uv)| ((x, y, 0.0), uv)).collect();
        assert!(Projection::solve(&flat).is_err(), "all-coplanar must fail");
        assert!(Projection::solve(&pillar_points()[..5]).is_err(), "5 points must fail");
    }

    #[test]
    fn projection_text_round_trips() {
        let p = Projection::solve(&pillar_points()).unwrap();
        let q = Projection::from_text(&p.to_text()).unwrap();
        let (a, b) = p.map3(120.0, 150.0, 7.0);
        let (c, d) = q.map3(120.0, 150.0, 7.0);
        assert!((a - c).abs() < 1e-9 && (b - d).abs() < 1e-9);
    }

    #[test]
    fn projection_roi_bounds_the_object_at_both_ends() {
        let p = Projection::solve(&pillar_points()).unwrap();
        let roi = Roi::from_projection(&p, 150.0, 165.0, 50.0, 15.0, 1920, 1080, 10);
        let (_, v_base) = p.map3(150.0, 165.0, 0.0);
        let (_, v_top) = p.map3(150.0, 165.0, 15.0);
        assert!(roi.y0 as f64 <= v_top && v_base <= roi.y1 as f64, "{roi:?} vs {v_top}..{v_base}");
        assert!(roi.x0 < roi.x1 && roi.y0 < roi.y1);
    }
}
