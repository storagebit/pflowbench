// verdict -- turns a run's per-band statistics into the sentence the bench
// exists to print (docs/verdict-plan.md section 6):
//
//   8 mm3/s SUSTAINED - 10 MARGINAL - 12+ FAILED
//   recommended filament_max_volumetric_speed = 8
//
// All rules self-normalize against the same run; no absolute gram or percent
// thresholds anywhere. Vision votes only downgrade. A rule that cannot run
// (missing tare, too few samples) withholds judgment and says so instead of
// guessing -- run_flags carry every withheld or suspicious condition.

mod rules;
#[cfg(test)]
mod tests;

use crate::capture::BandStat;

/// Verdict class for one band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandClass {
    /// Printed clean; force on the viscous baseline.
    Sustained,
    /// A weak signal fired, or families disagree: usable with caution.
    Marginal,
    /// The band failed: force departure, or corroborated vision stall.
    Failed,
    /// Force plateaued after strong growth: extruder slip, not health.
    Saturated,
    /// Not judgeable: artifact cylinder, broken foundation, missing data.
    NoVote,
}

/// Vision's per-band contribution, mirroring flowvision's vote without a
/// crate dependency -- flowcore stays std-only and app-agnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisionVote {
    Grow,
    Marginal,
    Stall,
    NoVote,
}

/// One cylinder's inputs: the capture's band stats plus what the manifest
/// and the app know about it.
#[derive(Clone, Debug)]
pub struct CylinderInput {
    pub cylinder: usize,
    /// Nozzle temperature this cylinder printed at, from the manifest.
    pub temp: Option<i64>,
    /// Loadcell zero for this cylinder: (mean g, sample count).
    pub tare: Option<(f64, u64)>,
    /// Band stats in band order (BandStat.band is 1-based).
    pub bands: Vec<BandStat>,
    /// Vision votes by 1-based band number.
    pub vision: Vec<(usize, VisionVote)>,
}

#[derive(Clone, Debug)]
pub struct VerdictInput {
    pub cylinders: Vec<CylinderInput>,
    /// Ladder revolutions per band -- the artifact gate's physics constant.
    pub revs: usize,
    /// The temperature whose ceiling becomes the headline recommendation;
    /// None = the temperature with the highest ceiling.
    pub primary_temp: Option<i64>,
    /// Safety margin in ladder rungs subtracted from the recommendation.
    pub margin_rungs: usize,
}

/// One band's judgment with the evidence that produced it.
#[derive(Clone, Debug)]
pub struct BandVerdict {
    pub cylinder: usize,
    /// 1-based, matching BandStat and the UI.
    pub band: usize,
    pub flow: f64,
    pub class: BandClass,
    /// Which rules fired, human-readable, with their numbers.
    pub fired: Vec<String>,
    /// 0.0-1.0: how much evidence agrees. Strong force signal 0.75; two
    /// families 1.0; single weak signal 0.5; no signal (Sustained) 0.6.
    pub confidence: f64,
}

/// Per-temperature outcome.
#[derive(Clone, Debug)]
pub struct TempVerdict {
    pub cylinder: usize,
    pub temp: Option<i64>,
    /// Highest flow of the contiguous SUSTAINED prefix; None when even the
    /// first band failed or the cylinder was withheld.
    pub ceiling: Option<f64>,
    /// "8.0 SUSTAINED - 10.0 MARGINAL - 12.0+ FAILED" style summary.
    pub sentence: String,
}

#[derive(Clone, Debug, Default)]
pub struct Verdict {
    pub bands: Vec<BandVerdict>,
    pub temps: Vec<TempVerdict>,
    /// Ceiling of the primary temperature minus the rung margin -- the
    /// number that goes into the slicer. None when no cylinder produced a
    /// usable ceiling.
    pub recommendation: Option<f64>,
    /// Everything withheld, suspicious, or cross-checked, for the report.
    pub run_flags: Vec<String>,
}

/// Judge a run. Never panics on weird data: a cylinder or band the rules
/// cannot judge gets NoVote and a flag, not a guess.
pub fn judge(input: &VerdictInput) -> Verdict {
    let mut v = Verdict::default();

    // Rule 1: artifact gate -- which cylinders are real test objects.
    let real: Vec<&CylinderInput> = input
        .cylinders
        .iter()
        .filter(|c| {
            let (ok, why) = rules::artifact_gate(c, input.revs);
            if !ok {
                v.run_flags.push(format!("cylinder {}: artifact ({why})", c.cylinder));
                for b in &c.bands {
                    v.bands.push(BandVerdict {
                        cylinder: c.cylinder,
                        band: b.band,
                        flow: b.flow,
                        class: BandClass::NoVote,
                        fired: vec![format!("artifact gate: {why}")],
                        confidence: 0.0,
                    });
                }
            }
            ok
        })
        .collect();

    if real.is_empty() {
        v.run_flags.push("no real cylinders survived the artifact gate".into());
        return v;
    }

    // Rule 4 needs all real cylinders together: flag melt-limited flows.
    let melt_flags = rules::cross_temperature(&real);
    for f in &melt_flags {
        v.run_flags.push(f.clone());
    }

    // Rules 2, 3, 8, 9, 10 per cylinder.
    for c in &real {
        let (mut bands, flags) = rules::judge_cylinder(c);
        v.run_flags.extend(flags);
        let ceiling = rules::sustained_prefix_ceiling(&bands);
        let sentence = rules::sentence(&bands);
        v.temps.push(TempVerdict {
            cylinder: c.cylinder,
            temp: c.temp,
            ceiling,
            sentence,
        });
        v.bands.append(&mut bands);
    }

    // Rule 11: the headline number.
    let primary = match input.primary_temp {
        Some(t) => v.temps.iter().find(|tv| tv.temp == Some(t)),
        None => v
            .temps
            .iter()
            .filter(|tv| tv.ceiling.is_some())
            .max_by(|a, b| a.ceiling.partial_cmp(&b.ceiling).unwrap()),
    };
    if let Some(tv) = primary {
        if let (Some(ceiling), Some(c)) =
            (tv.ceiling, real.iter().find(|c| c.cylinder == tv.cylinder))
        {
            // margin in rungs of THIS cylinder's ladder
            let flows: Vec<f64> = c.bands.iter().map(|b| b.flow).collect();
            let idx = flows.iter().position(|f| (*f - ceiling).abs() < 1e-9);
            let rec = match idx {
                Some(i) => flows[i.saturating_sub(input.margin_rungs)],
                None => ceiling,
            };
            v.recommendation = Some(rec);
        }
    }
    if v.recommendation.is_none() {
        v.run_flags
            .push("no usable ceiling -- every ladder sat above the cliff or was withheld".into());
    }
    v
}
