// profile.rs -- named material/nozzle profiles.
//
// The generator's `Cfg` has ~35 tunables and its defaults were originally
// PETG-CF-shaped: switching material meant remembering which
// of those to change, and forgetting one (bed temperature, say) silently
// produces a job that prints a different material's parameters. A profile
// names the whole set, so a run is reproducible and reviewable.
//
// The format is deliberately a flat `key = value` text file rather than TOML
// or JSON: it keeps this crate dependency-free, it diffs cleanly in git, and
// it is meant to be edited by hand between runs.
//
// Anything not named in the file keeps its `Cfg::default()` value, so a
// profile only states what is characteristic of that material.

use crate::{Cfg, Layout};

/// A named set of generator parameters, plus the source notes needed to trust
/// it later: where each number came from, and which reference export the
/// machine start block must be spliced from.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    /// Free-text source notes: which values are from a vendor datasheet, which
    /// were measured, which are guesses awaiting a run.
    pub notes: Vec<String>,
    /// PrusaSlicer export whose start/end blocks get spliced in. MUST be
    /// sliced with this material + nozzle: the start block carries bed and
    /// chamber temperatures and the whole levelling/purge sequence.
    pub reference: Option<String>,
    pub cfg: Cfg,
}

fn parse_num_list<T: std::str::FromStr>(v: &str, key: &str) -> Result<Vec<T>, String> {
    v.split(',')
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().parse::<T>().map_err(|_| format!("{key}: bad number {:?}", t.trim())))
        .collect()
}

impl Profile {
    /// Parse the `key = value` form. Blank lines and `#` comments are skipped.
    pub fn parse(text: &str) -> Result<Profile, String> {
        let mut p = Profile {
            name: String::new(),
            notes: Vec::new(),
            reference: None,
            cfg: Cfg::default(),
        };
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected `key = value`, got {raw:?}", n + 1))?;
            let (k, v) = (k.trim(), v.trim());
            let num = |key: &str| -> Result<f64, String> {
                v.parse::<f64>().map_err(|_| format!("{key}: not a number: {v:?}"))
            };
            let int = |key: &str| -> Result<i64, String> {
                v.parse::<i64>().map_err(|_| format!("{key}: not an integer: {v:?}"))
            };
            match k {
                "name" => p.name = v.to_string(),
                "note" => p.notes.push(v.to_string()),
                "reference" => p.reference = Some(v.to_string()),

                "temps" => p.cfg.temps = parse_num_list::<i64>(v, k)?,
                "flows" => p.cfg.flows = parse_num_list::<f64>(v, k)?,
                "revs" => p.cfg.revs = int(k)? as usize,

                "bed" => p.cfg.bed = int(k)?,
                "fan" => p.cfg.fan = int(k)?,
                "dwell" => p.cfg.dwell = int(k)?,

                "nozzle" => p.cfg.nozzle = num(k)?,
                "layer_h" => p.cfg.layer_h = num(k)?,
                "width" => p.cfg.width = num(k)?,
                "first_layer_h" => p.cfg.first_layer_h = num(k)?,
                "first_layer_w" => p.cfg.first_layer_w = num(k)?,
                "first_layer_flow" => p.cfg.first_layer_flow = num(k)?,
                "em" => p.cfg.em = num(k)?,
                "pa" => p.cfg.pa = num(k)?,
                "retract" => p.cfg.retract = num(k)?,
                "diameter" => p.cfg.diameter = num(k)?,
                "brim" => p.cfg.brim = int(k)? as usize,
                "safe_z" => p.cfg.safe_z = num(k)?,
                "travel_f" => p.cfg.travel_f = int(k)?,
                "seg_len" => p.cfg.seg_len = num(k)?,

                "bed_x" => p.cfg.bed_x = num(k)?,
                "bed_y" => p.cfg.bed_y = num(k)?,

                "reversed_control" => p.cfg.reversed_control = matches!(v, "true" | "1" | "yes"),
                "photo_pose" => p.cfg.photo_pose = matches!(v, "true" | "1" | "yes"),
                "photo_dwell" => p.cfg.photo_dwell = num(k)?,
                "photo_park_x" => p.cfg.photo_park_x = Some(num(k)?),
                "photo_park_y" => p.cfg.photo_park_y = Some(num(k)?),
                "constant_control" => p.cfg.constant_control = Some(num(k)?),
                "layout" => {
                    p.cfg.layout = match v {
                        "row" => Layout::Row,
                        "stagger" => Layout::Stagger,
                        "grid" => Layout::Grid,
                        _ => return Err(format!("layout: expected row|stagger|grid, got {v:?}")),
                    }
                }
                "layout_offset" => {
                    let xy = parse_num_list::<f64>(v, k)?;
                    if xy.len() != 2 {
                        return Err("layout_offset: expected `x,y`".into());
                    }
                    p.cfg.layout_offset = (xy[0], xy[1]);
                }
                other => return Err(format!("line {}: unknown key {other:?}", n + 1)),
            }
        }
        if p.name.is_empty() {
            return Err("profile has no `name`".into());
        }
        Ok(p)
    }

    pub fn load(path: &str) -> Result<Profile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        Profile::parse(&text).map_err(|e| format!("{path}: {e}"))
    }

    /// Highest and lowest commanded flow, for a quick sanity readout.
    pub fn flow_span(&self) -> (f64, f64) {
        let lo = self.cfg.flows.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = self.cfg.flows.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        (lo, hi)
    }

    /// Obvious mistakes worth catching before an hour of printing, returned
    /// as human-readable warnings rather than hard errors -- an unusual
    /// profile may still be deliberate.
    pub fn lint(&self) -> Vec<String> {
        let c = &self.cfg;
        let mut w = Vec::new();
        if c.flows.is_empty() {
            w.push("no flow ladder: nothing would be measured".into());
        }
        if c.flows.windows(2).any(|p| p[1] <= p[0]) {
            w.push("flow ladder is not strictly ascending; band->flow mapping will be confusing".into());
        }
        if c.layer_h > c.nozzle * 0.8 {
            w.push(format!(
                "layer height {:.2} exceeds 80% of the {:.1}mm nozzle: poor layer bonding",
                c.layer_h, c.nozzle
            ));
        }
        if c.width < c.nozzle {
            w.push(format!(
                "extrusion width {:.2} is narrower than the {:.1}mm nozzle",
                c.width, c.nozzle
            ));
        }
        if c.dwell > 0 {
            w.push(format!(
                "dwell of {}s parks a hot melt stationary; with filled filaments this risks degradation",
                c.dwell
            ));
        }
        if self.reference.is_none() {
            w.push("no reference export set: the machine start block would have to come from the \
                    hand-written standalone block, which carries no chamber or levelling sequence".into());
        }
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# a profile is only what is characteristic of the material
name = Test Material @ 0.8
note = flows from vendor datasheet
reference = reference/ref-test.gcode
temps = 270, 280, 290
flows = 10, 14, 18, 22
bed = 110
fan = 0
layer_h = 0.4
width = 0.9
nozzle = 0.8
layout = row
constant_control = 10
reversed_control = true
layout_offset = 12.5, -4
"#;

    #[test]
    fn parses_a_profile_and_leaves_unmentioned_fields_at_default() {
        let p = Profile::parse(SAMPLE).unwrap();
        assert_eq!(p.name, "Test Material @ 0.8");
        assert_eq!(p.notes, vec!["flows from vendor datasheet"]);
        assert_eq!(p.reference.as_deref(), Some("reference/ref-test.gcode"));
        assert_eq!(p.cfg.temps, vec![270, 280, 290]);
        assert_eq!(p.cfg.flows, vec![10.0, 14.0, 18.0, 22.0]);
        assert_eq!(p.cfg.bed, 110);
        assert_eq!(p.cfg.fan, 0);
        assert_eq!(p.cfg.layout, Layout::Row);
        assert_eq!(p.cfg.constant_control, Some(10.0));
        assert!(p.cfg.reversed_control);
        assert_eq!(p.cfg.layout_offset, (12.5, -4.0));
        // untouched keys keep their defaults
        assert_eq!(p.cfg.revs, Cfg::default().revs);
        assert_eq!(p.cfg.em, Cfg::default().em);
    }

    #[test]
    fn rejects_unknown_keys_rather_than_ignoring_them() {
        // silently dropping a typo'd key would print the wrong parameters
        let e = Profile::parse("name = x\nbed_temp = 110\n").unwrap_err();
        assert!(e.contains("unknown key"), "{e}");
        assert!(e.contains("bed_temp"), "{e}");
    }

    #[test]
    fn rejects_a_profile_without_a_name() {
        assert!(Profile::parse("bed = 110\n").unwrap_err().contains("no `name`"));
    }

    #[test]
    fn reports_the_line_number_for_a_malformed_line() {
        let e = Profile::parse("name = x\nthis is not a pair\n").unwrap_err();
        assert!(e.contains("line 2"), "{e}");
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let p = Profile::parse("# leading comment\n\nname = y   # trailing\nbed = 60\n").unwrap();
        assert_eq!(p.name, "y");
        assert_eq!(p.cfg.bed, 60);
    }

    #[test]
    fn lint_catches_the_mistakes_that_waste_a_print() {
        let p = Profile::parse(
            "name = bad\nnozzle = 0.8\nlayer_h = 0.7\nwidth = 0.6\nflows = 10,8,12\ndwell = 45\n",
        )
        .unwrap();
        let w = p.lint().join(" | ");
        assert!(w.contains("layer height"), "{w}");
        assert!(w.contains("narrower"), "{w}");
        assert!(w.contains("ascending"), "{w}");
        assert!(w.contains("dwell"), "{w}");
        assert!(w.contains("no reference export"), "{w}");
    }

    #[test]
    fn flow_span_reports_the_ladder_range() {
        let p = Profile::parse("name = z\nflows = 8, 16, 24\n").unwrap();
        assert_eq!(p.flow_span(), (8.0, 24.0));
    }

    #[test]
    fn the_shipped_profiles_all_parse_and_lint_clean() {
        // every profile in profiles/ must stay valid; a broken one would only
        // be discovered when a print was about to start
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../profiles");
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return, // profiles/ is optional in a bare checkout
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("profile") {
                continue;
            }
            let p = Profile::load(path.to_str().unwrap())
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            let bad: Vec<_> = p
                .lint()
                .into_iter()
                .filter(|w| !w.contains("no reference export"))
                .collect();
            assert!(bad.is_empty(), "{} lints: {bad:?}", path.display());
        }
    }
}
