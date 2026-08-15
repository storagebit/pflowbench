// Per-cylinder run analysis: attributing each PGM still to its (cylinder,
// band), the bed-descent calibration, and the Vote every band gets.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::geometry::Roi;
use crate::measure::{measure, HeightMeasure, VisionCfg};
use crate::pgm::Pgm;

/// One band frame's vision result.
#[derive(Clone, Debug)]
pub struct BandVision {
    pub cylinder: usize,
    /// 0-based, same addressing as the capture.
    pub band: usize,
    pub flow: f64,
    pub stale: bool,
    /// False when the frame cannot be attributed (stale name, duplicate
    /// bytes, empty ROI) -- excluded from votes.
    pub usable: bool,
    pub measure: HeightMeasure,
    /// Height gained vs the previous usable band (report only; height
    /// is a collapse detector, not an extrusion gauge).
    pub growth_px: Option<f64>,
    /// Top-edge descent rate as a fraction of the measured bed-descent
    /// rate: ~0 healthy, ~1 stalled. The vote's driving number.
    pub descent_ratio: Option<f64>,
    /// Edge energy relative to the cylinder's first usable band.
    /// Report only -- 0/8 true positives on the real failed run; it does
    /// not vote until a verified-healthy reference exists (Phase D).
    pub raggedness_ratio: Option<f64>,
    pub vote: Vote,
    pub note: String,
}

/// Vision's verdict contribution. Only ever downgrades: GROW is "no
/// objection", never a health certificate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vote {
    Grow,
    Marginal,
    /// The test object did not gain height where the program says it must, or
    /// the top edge is crown-jagged: the band FAILED visually.
    Stall,
    NoVote,
}

/// (cylinder, band, flow) parsed from a capture still filename like
/// `cyl11_band02_flow14.5.pgm` (optionally `..._stale.pgm`). Band numbers in
/// filenames are 1-based (matching the UI); returned 0-based.
pub fn parse_name(name: &str) -> Option<(usize, usize, f64, bool)> {
    let stem = name.strip_suffix(".pgm")?;
    let (stem, stale) = match stem.strip_suffix("_stale") {
        Some(s) => (s, true),
        None => (stem, false),
    };
    let mut cyl = None;
    let mut band = None;
    let mut flow = None;
    for part in stem.split('_') {
        if let Some(v) = part.strip_prefix("cyl") {
            cyl = v.parse().ok();
        } else if let Some(v) = part.strip_prefix("band") {
            band = v.parse::<usize>().ok();
        } else if let Some(v) = part.strip_prefix("flow") {
            flow = v.parse().ok();
        }
    }
    Some((cyl?, band?.checked_sub(1)?, flow?, stale))
}

/// FNV-1a over the raw bytes: cheap stale-duplicate detection. The first
/// real run held one frozen frame across whole cylinders; byte-identical
/// frames cannot be attributed to their band.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Analyze every band still of one test object (cylinder) in a run directory.
///
/// The vote rides on bed-descent physics (see docs/accuracy-plan.md): the
/// base regression calibrates px-per-band, the top edge's velocity against
/// it is the stall signal (measured z ~ 9 per band on the real run, vs ~2
/// for height differencing). Every vote carries its numbers in `note`.
pub fn analyze_cylinder(
    dir: &Path,
    cylinder: usize,
    roi: &Roi,
    cfg: &VisionCfg,
) -> Result<Vec<BandVision>, String> {
    // collect this cylinder's frames, band-ordered
    let mut frames: BTreeMap<usize, (f64, bool, Vec<u8>)> = BTreeMap::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((c, b, f, stale)) = parse_name(&name) {
            if c == cylinder {
                let bytes =
                    fs::read(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
                let replace = frames.get(&b).map(|(_, s, _)| *s).unwrap_or(true);
                if replace || !stale {
                    frames.insert(b, (f, stale, bytes));
                }
            }
        }
    }
    if frames.is_empty() {
        return Err(format!("no cyl{cylinder} PGM stills in {}", dir.display()));
    }

    // Pass 1: measure every frame, decide evaluability. Physics: the bed
    // descends revs x layer_h per band under a chamber-fixed camera, so the
    // brim (base) descends at a fixed pixel rate -- the free per-cylinder
    // ruler -- while a healthy test object's top edge stays put (it tracks the
    // nozzle plane). A stalled top rides down WITH the bed.
    struct FrameRow {
        band: usize,
        flow: f64,
        stale: bool,
        duplicate: bool,
        occluded: Option<f64>,
        pinned: bool,
        m: HeightMeasure,
    }
    let mut rows: Vec<FrameRow> = Vec::new();
    let mut seen_hashes = std::collections::HashSet::new();
    let mut reference_small: Option<Vec<u8>> = None;
    for (band, (flow, stale, bytes)) in &frames {
        let duplicate = !seen_hashes.insert(fnv1a(bytes));
        let img = Pgm::parse(bytes)?;
        let m = measure(&img, roi, cfg);
        // 1/16-subsampled luma for the occlusion guard
        let mut small = Vec::with_capacity((img.w / 16) * (img.h / 16));
        let mut y = 0;
        while y < img.h {
            let mut x = 0;
            while x < img.w {
                small.push(img.at(x, y));
                x += 16;
            }
            y += 16;
        }
        // scene change vs the cylinder's first usable frame: measured
        // separation is 0.00-0.11 parked vs 0.14-0.39 head-in-frame
        let occluded = reference_small.as_ref().map(|r| {
            let moved = r
                .iter()
                .zip(&small)
                .filter(|(a, b)| (**a as i16 - **b as i16).abs() > 40)
                .count();
            moved as f64 / r.len().max(1) as f64
        });
        // a top edge sitting ON the ROI's upper bound is clipped by the
        // window, not measured -- background leaked in or the ROI is wrong
        let pinned = m.columns_hit > 0 && m.top_p10 <= roi.y0 as f64 + 1.0;
        let usable = !stale
            && !duplicate
            && !pinned
            && m.columns_hit >= roi.x1.saturating_sub(roi.x0) / 10
            && occluded.map(|o| o < cfg.occlusion_frac).unwrap_or(true);
        if usable && reference_small.is_none() {
            reference_small = Some(small.clone());
        }
        rows.push(FrameRow {
            band: *band,
            flow: *flow,
            stale: *stale,
            duplicate,
            occluded,
            pinned,
            m,
        });
    }
    drop(reference_small);

    let usable = |r: &FrameRow, cfg: &VisionCfg| {
        !r.stale
            && !r.duplicate
            && !r.pinned
            && r.m.columns_hit >= roi.x1.saturating_sub(roi.x0) / 10
            && r.occluded.map(|o| o < cfg.occlusion_frac).unwrap_or(true)
    };

    // Bed-descent rate: least-squares slope of base_median vs band index
    // over usable frames. This is the ruler every vote divides by.
    let pts: Vec<(f64, f64)> = rows
        .iter()
        .filter(|r| usable(r, cfg))
        .map(|r| (r.band as f64, r.m.base_median))
        .collect();
    let s_bed = if pts.len() >= cfg.min_calib_bands {
        let n = pts.len() as f64;
        let (sx, sy): (f64, f64) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
        let (sxx, sxy): (f64, f64) =
            pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x * x, b + x * y));
        let denom = n * sxx - sx * sx;
        if denom.abs() > 1e-9 {
            Some((n * sxy - sx * sy) / denom)
        } else {
            None
        }
    } else {
        None
    };
    // sanity: the bed MUST descend visibly; a flat or negative slope means
    // the camera moved, the ROI is wrong, or the base was misdetected
    let s_bed = s_bed.filter(|s| *s > 1.0);

    // Pass 2: votes from top-edge velocity relative to the bed rate.
    let mut out = Vec::new();
    let mut prev_eval: Option<(usize, f64, f64)> = None; // band, top, height
    let mut ref_edge: Option<f64> = None;
    for r in rows {
        let ok = usable(&r, cfg);
        let note;
        let mut growth = None;
        let mut ratio = None;
        let mut rag = None;
        let vote = if !ok {
            note = if r.stale {
                "stale frame (no fresh image in the photo window)".into()
            } else if r.duplicate {
                "byte-identical to an earlier band's frame (frozen stream)".into()
            } else if r.pinned {
                "top edge pinned at the ROI bound -- background leak or bad ROI".into()
            } else if r.occluded.map(|o| o >= cfg.occlusion_frac).unwrap_or(false) {
                format!(
                    "scene changed vs reference ({:.0}% of pixels) -- head in frame?",
                    r.occluded.unwrap() * 100.0
                )
            } else {
                format!("ROI nearly empty ({} columns)", r.m.columns_hit)
            };
            Vote::NoVote
        } else {
            if ref_edge.is_none() && r.m.edge_energy.is_finite() && r.m.edge_energy > 0.0 {
                ref_edge = Some(r.m.edge_energy);
            }
            rag = ref_edge.map(|e| r.m.edge_energy / e);
            match (prev_eval, s_bed) {
                (Some((pb, ptop, ph)), Some(s)) => {
                    let gap = (r.band - pb).max(1) as f64;
                    let v_top = (r.m.top_p10 - ptop) / gap;
                    let rt = v_top / s;
                    ratio = Some(rt);
                    growth = Some((r.m.height_px - ph) / gap);
                    note = format!(
                        "top moved {v_top:+.1} px/band vs bed {s:.1} px/band (ratio {rt:.2})"
                    );
                    if rt >= cfg.stall_frac {
                        Vote::Stall
                    } else if rt >= cfg.marginal_frac {
                        Vote::Marginal
                    } else {
                        Vote::Grow
                    }
                }
                (Some(_), None) => {
                    note = format!(
                        "bed-descent rate not established ({} usable bands, need {}) -- no vote",
                        pts.len(),
                        cfg.min_calib_bands
                    );
                    Vote::NoVote
                }
                (None, _) => {
                    note = format!("first usable band, height {:.0} px", r.m.height_px);
                    Vote::Grow
                }
            }
        };
        if ok {
            prev_eval = Some((r.band, r.m.top_p10, r.m.height_px));
        }
        out.push(BandVision {
            cylinder,
            band: r.band,
            flow: r.flow,
            stale: r.stale,
            usable: ok,
            measure: r.m,
            growth_px: growth,
            descent_ratio: ratio,
            raggedness_ratio: rag,
            vote,
            note,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{frame, ROI};

    fn pgm_bytes(p: &Pgm) -> Vec<u8> {
        let mut out = format!("P5\n{} {}\n255\n", p.w, p.h).into_bytes();
        out.extend_from_slice(&p.data);
        out
    }

    #[test]
    fn filenames_parse_including_stale() {
        assert_eq!(parse_name("cyl11_band02_flow14.5.pgm"), Some((11, 1, 14.5, false)));
        assert_eq!(parse_name("cyl1_band10_flow32.0_stale.pgm"), Some((1, 9, 32.0, true)));
        assert_eq!(parse_name("timelapse-x.mp4"), None);
        assert_eq!(parse_name("cyl1_band00_flow8.0.pgm"), None, "band is 1-based on disk");
    }

    fn write_run(dir: &Path, frames: &[(usize, Pgm)]) {
        fs::create_dir_all(dir).unwrap();
        for (band, img) in frames {
            let name = format!("cyl1_band{:02}_flow{:.1}.pgm", band, 8.0 + *band as f64);
            fs::write(dir.join(name), pgm_bytes(img)).unwrap();
        }
    }

    #[test]
    fn growing_object_votes_grow_and_stall_is_caught() {
        let dir = std::env::temp_dir().join(format!("flowvision_grow_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // REAL geometry: bed descends 7 px/band. Bands 1-4 healthy (top
        // stationary at 150 while the base walks down), bands 5-6 stalled
        // (top rides down with the bed). Single-band onset must be caught.
        write_run(
            &dir,
            &[
                (1, frame(150, 200, 0)),
                (2, frame(150, 207, 0)),
                (3, frame(150, 214, 0)),
                (4, frame(150, 221, 0)),
                (5, frame(157, 228, 0)),
                (6, frame(164, 235, 0)),
            ],
        );
        let got = analyze_cylinder(&dir, 1, &ROI, &VisionCfg::default()).unwrap();
        let votes: Vec<Vote> = got.iter().map(|b| b.vote).collect();
        assert_eq!(
            votes,
            vec![Vote::Grow, Vote::Grow, Vote::Grow, Vote::Grow, Vote::Stall, Vote::Stall],
            "{:#?}",
            got.iter().map(|b| (b.band, b.vote, b.note.clone())).collect::<Vec<_>>()
        );
        // and the ratio numbers say what happened
        assert!(got[4].descent_ratio.unwrap() > 0.9, "{:?}", got[4].descent_ratio);
        assert!(got[1].descent_ratio.unwrap().abs() < 0.2, "{:?}", got[1].descent_ratio);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn aba_frozen_frame_patterns_get_no_vote() {
        let dir = std::env::temp_dir().join(format!("flowvision_aba_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // frame A, then B, then A again: the third band's "image" is the
        // first band's frozen frame resurfacing -- it must not vote
        write_run(
            &dir,
            &[(1, frame(150, 200, 0)), (2, frame(150, 207, 0)), (3, frame(150, 200, 0)),
              (4, frame(150, 221, 0))],
        );
        let got = analyze_cylinder(&dir, 1, &ROI, &VisionCfg::default()).unwrap();
        assert_eq!(got[2].vote, Vote::NoVote, "{:?}", got[2]);
        assert!(got[2].note.contains("identical"), "{}", got[2].note);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn never_growing_object_stalls_from_the_second_band() {
        let dir = std::env::temp_dir().join(format!("flowvision_dead_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // the PC-CF pattern: the whole ladder sits above the cliff, the
        // crown rides down with the bed from the very first band
        write_run(
            &dir,
            &[
                (1, frame(150, 200, 0)),
                (2, frame(157, 207, 0)),
                (3, frame(164, 214, 0)),
                (4, frame(171, 221, 0)),
            ],
        );
        let got = analyze_cylinder(&dir, 1, &ROI, &VisionCfg::default()).unwrap();
        let votes: Vec<Vote> = got.iter().map(|b| b.vote).collect();
        assert_eq!(
            votes,
            vec![Vote::Grow, Vote::Stall, Vote::Stall, Vote::Stall],
            "{got:#?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The real failed run: cylinder 11's distinct frames show a test object
    /// that never grows past band 2. Uses the ROI measured from those frames
    /// (see docs/verdict-plan.md §7 and the run's README). Needs the runs/
    /// directory, so ignored by default: `cargo test -p flowvision -- --ignored`.
    #[test]
    #[ignore]
    fn real_run_20260811_cyl11_growth_stall_is_detected() {
        let dir = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../runs/20260811-191503"
        ));
        if !dir.exists() {
            panic!("runs/20260811-191503 not present");
        }
        // Columns 940-1120: the centre test object only. Wider windows admit
        // the top-right crown (x >= ~1170) and the frame-cropped top-centre
        // crown (x ~700-975), both of which pin the detected top edge to the
        // frame top and fake a growing height.
        // y from 140: above that line the dark machine interior shows in
        // some frames and pins the top edge. ROI choice IS the calibration
        // step -- for live runs the homography derives it, for this fixed
        // historical data it is hand-measured off the frames.
        let roi = Roi { x0: 940, x1: 1120, y0: 140, y1: 430 };
        let got = analyze_cylinder(dir, 11, &roi, &VisionCfg::default()).unwrap();
        let eval: Vec<&BandVision> = got.iter().filter(|b| b.usable).collect();
        assert!(eval.len() >= 3, "need >=3 distinct frames, got {}", eval.len());
        // within-frame heights must be flat: the object does not grow
        let hs: Vec<f64> = eval.iter().map(|b| b.measure.height_px).collect();
        let (min, max) = hs.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        assert!(max - min < 40.0, "heights should be flat, got {hs:?}");
        // and vision must call the stall: every usable band after the
        // first votes Stall
        for b in &eval[1..] {
            assert_eq!(b.vote, Vote::Stall, "band {} {:?} {}", b.band + 1, b.vote, b.note);
        }
    }
}
