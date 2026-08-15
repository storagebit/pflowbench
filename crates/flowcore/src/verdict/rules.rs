// The individual verdict rules. Numbers cited in comments are from the real
// runs that calibrated them (docs/verdict-plan.md section 6).

use super::{BandClass, BandVerdict, CylinderInput, VisionVote};

/// Rule 1 -- artifact gate. The Z-heuristic era invented cylinders from
/// travel moves (11 counted, 4 printed); sdpos addressing mostly fixed the
/// source, but the gate stays as defense in depth. A real ladder cylinder
/// has sample counts that scale like revs/flow (band time = revs x
/// circumference / speed, speed proportional to flow) and therefore FALL as
/// the ladder climbs. Phantoms show 30-70 samples with no such pattern.
pub(super) fn artifact_gate(c: &CylinderInput, revs: usize) -> (bool, String) {
    if c.bands.len() < 3 {
        return (false, format!("{} bands", c.bands.len()));
    }
    // n x flow should be roughly constant; derive the constant from the
    // cylinder's own median rather than hard-coding a sample count.
    let mut nf: Vec<f64> = c.bands.iter().map(|b| b.n as f64 * b.flow / revs as f64).collect();
    nf.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let c_est = nf[nf.len() / 2];
    if c_est <= 0.0 {
        return (false, "no samples".into());
    }
    for b in &c.bands {
        let expected = c_est * revs as f64 / b.flow;
        let ratio = b.n as f64 / expected;
        // Band 1 runs wide: purge/first-layer samples leak into it (the
        // real run measured n1/n2 = 2.9 vs the 1.21 physics predicts, and
        // real cylinders sat at 2.5x expected). Phantoms overshoot by 4-280x.
        let hi = if b.band <= 1 { 3.5 } else { 2.5 };
        if !(0.4..=hi).contains(&ratio) {
            return (
                false,
                format!("band {} has {} samples, expected ~{expected:.0}", b.band, b.n),
            );
        }
    }
    // Sample counts must fall as flow climbs (equal revs per band). One
    // tolerated inversion absorbs a band boundary blip; phantoms fail by
    // several.
    let mut inversions = 0;
    for w in c.bands.windows(2) {
        if w[1].flow > w[0].flow && w[1].n as f64 > w[0].n as f64 * 1.05 {
            inversions += 1;
        }
    }
    if inversions > 1 {
        return (false, format!("{inversions} sample-count inversions across the ladder"));
    }
    (true, String::new())
}

/// Rule 4 -- cross-temperature check over all real cylinders. At a fixed
/// flow, force strictly falling as temperature rises means the nozzle is
/// melt-limited there (hotter melts more, back-pressure drops: measured
/// 12298 -> 9386 g across 275 -> 290 C at 27 mm3/s). Force RISING with
/// temperature at a flow marks deep saturation (measured at 32).
pub(super) fn cross_temperature(real: &[&CylinderInput]) -> Vec<String> {
    let mut flags = Vec::new();
    let with_temp: Vec<&&CylinderInput> = real.iter().filter(|c| c.temp.is_some()).collect();
    if with_temp.len() < 3 {
        return flags;
    }
    // union of flows across all cylinders -- a flow missing from the first
    // cylinder must still be checked (reversed programs order differently)
    let mut flows: Vec<f64> = with_temp.iter().flat_map(|c| c.bands.iter().map(|b| b.flow)).collect();
    flows.sort_by(|a, b| a.partial_cmp(b).unwrap());
    flows.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    for flow in flows {
        // group by temperature; duplicate temps (the probe cylinder repeats
        // the first) average, so the trend compares DISTINCT temperatures
        let mut by_temp: std::collections::BTreeMap<i64, (f64, u32)> = Default::default();
        for c in &with_temp {
            if let Some(b) = c.bands.iter().find(|b| (b.flow - flow).abs() < 1e-9) {
                let e = by_temp.entry(c.temp.unwrap_or(0)).or_insert((0.0, 0));
                e.0 += effective_mean(c, b);
                e.1 += 1;
            }
        }
        if by_temp.len() < 3 {
            continue;
        }
        let series: Vec<f64> = by_temp.values().map(|(s, n)| s / *n as f64).collect();
        let falling = series.windows(2).all(|w| w[1] < w[0]);
        let rising = series.windows(2).all(|w| w[1] > w[0]);
        if falling {
            flags.push(format!(
                "flow {flow:.1}: force falls with temperature across {} temps -- melt-limited",
                by_temp.len()
            ));
        } else if rising {
            flags.push(format!(
                "flow {flow:.1}: force RISES with temperature across {} temps -- deep saturation",
                by_temp.len()
            ));
        }
    }
    flags
}

/// The mean the rules judge on: settled-window stats when the capture has
/// them (first revolution excluded -- the plan's mandated input), full-band
/// otherwise; tared when a tare exists.
fn effective_mean(c: &CylinderInput, b: &crate::capture::BandStat) -> f64 {
    let raw = match b.settled_mean {
        Some(m) if b.settled_n > 0 => m,
        _ => b.mean,
    };
    match c.tare {
        Some((t, n)) if n > 0 => raw - t,
        _ => raw,
    }
}

fn effective_sd(b: &crate::capture::BandStat) -> f64 {
    match b.settled_sd {
        Some(s) if b.settled_n > 0 => s,
        _ => b.sd,
    }
}

fn vision_for(c: &CylinderInput, band: usize) -> VisionVote {
    c.vision
        .iter()
        .find(|(b, _)| *b == band)
        .map(|(_, v)| *v)
        .unwrap_or(VisionVote::NoVote)
}

/// Rules 2, 3, 8, 9, 10 for one real cylinder.
pub(super) fn judge_cylinder(c: &CylinderInput) -> (Vec<BandVerdict>, Vec<String>) {
    let mut flags = Vec::new();

    let withhold = |why: &str, flags: &mut Vec<String>| -> Vec<BandVerdict> {
        flags.push(format!("cylinder {}: {why} -- verdict withheld", c.cylinder));
        c.bands
            .iter()
            .map(|b| BandVerdict {
                cylinder: c.cylinder,
                band: b.band,
                flow: b.flow,
                class: BandClass::NoVote,
                fired: vec![why.to_string()],
                confidence: 0.0,
            })
            .collect()
    };

    // The force rules assume an ascending ladder. Reversed and constant
    // control cylinders (and anything else that is not a ladder) cannot be
    // judged by them -- withholding here is what keeps a control's bands
    // from ever reading SUSTAINED and poisoning the recommendation.
    let ascending = c.bands.windows(2).all(|w| w[1].flow > w[0].flow);
    if !ascending {
        return (withhold("flows are not an ascending ladder (control cylinder?)", &mut flags), flags);
    }

    if c.tare.is_none() {
        flags.push(format!(
            "cylinder {}: no tare window -- judging on raw force (pre-tare G-code?)",
            c.cylinder
        ));
    }
    if c.bands.iter().any(|b| b.settled_n == 0 || b.settled_mean.is_none()) {
        flags.push(format!(
            "cylinder {}: no settled-window stats -- judging on full-band means (legacy data)",
            c.cylinder
        ));
    }
    let means: Vec<f64> = c.bands.iter().map(|b| effective_mean(c, b)).collect();
    if c.tare.is_some() && means.iter().any(|m| *m < 0.0) {
        return (withhold("negative tared band means -- tare drifted", &mut flags), flags);
    }

    // Rule 3 references. The reference band must itself be usable: a mean
    // buried in its own noise makes every ratio meaningless. d_ref is the
    // first CLEAN positive increment -- clean means clearing half the
    // reference band's noise, so a noise-scale wiggle cannot become the
    // normalizer that condemns the whole ladder.
    let mean_ref = means.first().cloned().unwrap_or(0.0);
    let sd_ref = c.bands.first().map(effective_sd).unwrap_or(0.0);
    let q_ref = c.bands.first().map(|b| b.flow).unwrap_or(1.0);
    if mean_ref <= 2.0 * sd_ref {
        return (
            withhold("reference band mean is inside its own noise band", &mut flags),
            flags,
        );
    }
    let d: Vec<f64> = means.windows(2).map(|w| w[1] - w[0]).collect();
    let d_ref = d.iter().cloned().find(|x| *x > 0.5 * sd_ref.max(f64::EPSILON));
    let Some(d_ref) = d_ref else {
        return (
            withhold("no clean positive force increment -- force family cannot judge", &mut flags),
            flags,
        );
    };

    // Pass 1: force family + vision for EVERY band. Rule 10 is applied as a
    // second pass so the saturation branch stays reachable -- the slip
    // plateau above the first failure must read SATURATED, not silently
    // vanish into no-votes (it was validated on the real run's 19.5-32).
    let mut out = Vec::new();
    let mut max_prior_r: f64 = 0.0;
    for (i, b) in c.bands.iter().enumerate() {
        let mut fired = Vec::new();
        let mut class = BandClass::Sustained;
        let mut confidence = 0.6f64;

        if i > 0 {
            let r = d[i - 1] / d_ref;
            // saturation first: increments collapsing after strong growth is
            // slip, not recovery
            if max_prior_r > 2.0 && r < 0.75 * max_prior_r {
                fired.push(format!(
                    "increment collapsed after strong growth (R {r:.2} vs prior max {max_prior_r:.2}) -- slip plateau"
                ));
                class = BandClass::Saturated;
                confidence = 0.75;
            } else if r > 1.5 {
                fired.push(format!("increment ratio R {r:.2} > 1.5"));
                class = BandClass::Failed;
                confidence = 0.75;
            }
            max_prior_r = max_prior_r.max(r);

            let e = (means[i] / mean_ref) / (b.flow / q_ref);
            if e > 1.3 {
                fired.push(format!("excess factor E {e:.2} > 1.3"));
                if class == BandClass::Sustained {
                    class = BandClass::Failed;
                    confidence = 0.75;
                } else {
                    confidence = 1.0; // two force signals agree
                }
            } else if e > 1.15 && class == BandClass::Sustained {
                fired.push(format!("excess factor E {e:.2} in 1.15-1.3"));
                class = BandClass::Marginal;
                confidence = 0.5;
            }
        }

        // Rule 8: vision votes, downgrade only.
        match vision_for(c, b.band) {
            VisionVote::Stall => {
                fired.push("vision: growth stalled".into());
                match class {
                    // Rule 9: two independent families agreeing = hard FAILED
                    BandClass::Failed | BandClass::Saturated => confidence = 1.0,
                    BandClass::Marginal => {
                        class = BandClass::Failed;
                        confidence = 1.0;
                    }
                    // vision alone is a single family: cap at MARGINAL
                    BandClass::Sustained => {
                        fired.push("force family disagrees -- capped at MARGINAL".into());
                        class = BandClass::Marginal;
                        confidence = 0.5;
                    }
                    BandClass::NoVote => {}
                }
            }
            VisionVote::Marginal => {
                if class == BandClass::Sustained {
                    fired.push("vision: marginal growth".into());
                    class = BandClass::Marginal;
                    confidence = 0.5;
                }
            }
            VisionVote::Grow | VisionVote::NoVote => {}
        }

        out.push(BandVerdict {
            cylinder: c.cylinder,
            band: b.band,
            flow: b.flow,
            class,
            fired,
            confidence,
        });
    }

    // Pass 2, rule 10: above the first failure nothing can be judged
    // HEALTHY (it printed on a broken foundation) -- but a condemned class
    // (Failed/Saturated) stays, because "this also failed" is evidence, not
    // judgment on a healthy wall.
    if let Some(first_failed) = out.iter().position(|b| b.class == BandClass::Failed) {
        for b in &mut out[first_failed + 1..] {
            if matches!(b.class, BandClass::Sustained | BandClass::Marginal) {
                b.class = BandClass::NoVote;
                b.fired.push("above the first failed band".into());
                b.confidence = 0.0;
            }
        }
    }
    (out, flags)
}

/// Rule 10's ceiling: highest flow of the contiguous SUSTAINED prefix.
pub(super) fn sustained_prefix_ceiling(bands: &[BandVerdict]) -> Option<f64> {
    let mut ceiling = None;
    for b in bands {
        if b.class == BandClass::Sustained {
            ceiling = Some(b.flow);
        } else {
            break;
        }
    }
    ceiling
}

/// The per-temperature summary line, e.g.
/// "8.0-10.0 SUSTAINED - 12.0 MARGINAL - 14.0+ FAILED".
pub(super) fn sentence(bands: &[BandVerdict]) -> String {
    // half-rung ladders are real (12, 14.5, 17 ...): print the decimal
    // only when there is one, never round 19.5 into "20"
    fn ff(f: f64) -> String {
        if (f - f.round()).abs() < 1e-9 {
            format!("{f:.0}")
        } else {
            format!("{f:.1}")
        }
    }
    let mut parts = Vec::new();
    let mut push_span = |lo: f64, hi: f64, label: &str| {
        if (lo - hi).abs() < 1e-9 {
            parts.push(format!("{} {label}", ff(lo)));
        } else {
            parts.push(format!("{}-{} {label}", ff(lo), ff(hi)));
        }
    };
    let mut i = 0;
    while i < bands.len() {
        let class = bands[i].class;
        let lo = bands[i].flow;
        let mut hi = lo;
        while i + 1 < bands.len() && bands[i + 1].class == class {
            i += 1;
            hi = bands[i].flow;
        }
        let label = match class {
            BandClass::Sustained => "SUSTAINED",
            BandClass::Marginal => "MARGINAL",
            BandClass::Failed => "FAILED",
            BandClass::Saturated => "SATURATED",
            BandClass::NoVote => "no vote",
        };
        // everything above the first failure reads as "N+ FAILED"
        if class == BandClass::Failed && i + 1 < bands.len()
            && bands[i + 1..].iter().all(|b| b.class == BandClass::NoVote)
        {
            parts.push(format!("{}+ FAILED", ff(lo)));
            break;
        }
        push_span(lo, hi, label);
        i += 1;
    }
    parts.join(" - ")
}
