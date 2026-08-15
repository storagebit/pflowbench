// nozzle.rs -- the Nextruder nozzle database.
//
// One file per physical nozzle product under profiles/nozzles/*.nozzle, plus
// one *.machine file per printer family for motion limits. Same flat
// `key = value` format as profile.rs, same reasons: dependency-free, hand-
// editable, diffs cleanly.
//
// The rule (PETG-CF published 22 -> visual
// knee 16-18; PC Blend CF published 18 -> ceiling bracketed 8-12): vendor
// and slicer-preset numbers are CLAIMS, not specs. They are stored under
// `claim_*` keys so nothing downstream can mistake them for measurements,
// and they exist only to place a test ladder and compute a derate. The only
// trusted flow numbers are `measured` entries, which are appended by hand
// after a bench run (loadcell knee) or a flowstep run (melt ceiling).

use std::collections::BTreeMap;
use std::fs;

/// A physical nozzle product and everything known about it, with claims and
/// measurements kept apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nozzle {
    pub name: String,
    /// Who manufactures it (prusa, e3d, ...).
    pub vendor: String,
    /// Tip material: brass | hardened-steel | obxidian | pcd | sic | ruby
    /// | tungsten-carbide.
    pub tip: String,
    pub diameter: f64,
    /// Pairs with the HF slicer presets (melt-zone geometry, not a speed).
    pub high_flow: bool,
    /// Safe for CF/GF filaments.
    pub abrasive_ok: bool,
    /// PrusaSlicer printer-preset variant suffix this product pairs with,
    /// e.g. "0.6" or "HF0.6" -- the same string for CORE One and CORE One L.
    pub preset_variant: String,
    /// Printer-preset retract defaults for this nozzle (claims).
    pub retract: Option<f64>,
    pub retract_speed: Option<f64>,
    /// Default quality print-preset anchors for this nozzle (claims).
    pub default_layer_h: Option<f64>,
    pub default_width: Option<f64>,
    pub default_speed_external: Option<f64>,
    pub default_speed_perimeter: Option<f64>,
    pub default_speed_infill: Option<f64>,
    pub default_speed_travel: Option<f64>,
    /// Prusa's published pressure-advance anchor for this diameter (M572 S).
    pub claim_pa: Option<f64>,
    /// Published/preset volumetric ceilings by material key (lowercased,
    /// e.g. "pla", "petg", "pla-cf"). CLAIMS ONLY -- ladder placement, never
    /// a ceiling. `claim_mvs_pla = 15.5` parses to ("pla", 15.5).
    pub claim_mvs: BTreeMap<String, f64>,
    /// Measured results, appended after real runs. Free text, one entry per
    /// `measured =` line, newest last; convention:
    /// "petg-cf: knee 16-18 mm3/s (loadcell, undried spool, 2026-08)".
    pub measured: Vec<String>,
    /// Source notes ([sourced]/[inferred] ...), same as Profile.
    pub notes: Vec<String>,
}

/// Motion limits for a printer family, from its slicer printer preset --
/// claims from the vendor bundle, recorded once instead of per nozzle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub name: String,
    pub max_feedrate_xy: Option<f64>,
    pub max_feedrate_z: Option<f64>,
    pub max_feedrate_e: Option<f64>,
    pub max_accel_print: Option<f64>,
    pub max_accel_travel: Option<f64>,
    pub max_jerk_xy: Option<f64>,
    pub notes: Vec<String>,
}

fn kv_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines().enumerate().filter_map(|(n, raw)| {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() { None } else { Some((n, line)) }
    })
}

impl Nozzle {
    pub fn parse(text: &str) -> Result<Nozzle, String> {
        let mut z = Nozzle::default();
        for (n, line) in kv_lines(text) {
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected `key = value`, got {line:?}", n + 1))?;
            let (k, v) = (k.trim(), v.trim());
            let num = |key: &str| -> Result<f64, String> {
                v.parse::<f64>().map_err(|_| format!("{key}: not a number: {v:?}"))
            };
            let flag = || matches!(v, "true" | "1" | "yes");
            match k {
                "name" => z.name = v.to_string(),
                "vendor" => z.vendor = v.to_string(),
                "tip" => z.tip = v.to_string(),
                "diameter" => z.diameter = num(k)?,
                "high_flow" => z.high_flow = flag(),
                "abrasive_ok" => z.abrasive_ok = flag(),
                "preset_variant" => z.preset_variant = v.to_string(),
                "retract" => z.retract = Some(num(k)?),
                "retract_speed" => z.retract_speed = Some(num(k)?),
                "default_layer_h" => z.default_layer_h = Some(num(k)?),
                "default_width" => z.default_width = Some(num(k)?),
                "default_speed_external" => z.default_speed_external = Some(num(k)?),
                "default_speed_perimeter" => z.default_speed_perimeter = Some(num(k)?),
                "default_speed_infill" => z.default_speed_infill = Some(num(k)?),
                "default_speed_travel" => z.default_speed_travel = Some(num(k)?),
                "claim_pa" => z.claim_pa = Some(num(k)?),
                "measured" => z.measured.push(v.to_string()),
                "note" => z.notes.push(v.to_string()),
                mvs if mvs.starts_with("claim_mvs_") => {
                    let mat = mvs.trim_start_matches("claim_mvs_");
                    if mat.is_empty() {
                        return Err(format!("line {}: claim_mvs_ needs a material suffix", n + 1));
                    }
                    z.claim_mvs.insert(mat.to_string(), num(mvs)?);
                }
                other => return Err(format!("line {}: unknown key {other:?}", n + 1)),
            }
        }
        Ok(z)
    }

    pub fn load(path: &str) -> Result<Nozzle, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let z = Self::parse(&text)?;
        let bad = z.lint();
        if bad.is_empty() { Ok(z) } else { Err(format!("{path}: {}", bad.join("; "))) }
    }

    /// Structural checks. A nozzle file that fails these is unusable for
    /// profile generation, not merely incomplete.
    pub fn lint(&self) -> Vec<String> {
        let mut bad = Vec::new();
        if self.name.is_empty() { bad.push("no name".into()); }
        if self.vendor.is_empty() { bad.push("no vendor".into()); }
        const TIPS: &[&str] =
            &["brass", "hardened-steel", "obxidian", "pcd", "sic", "ruby", "tungsten-carbide"];
        if !TIPS.contains(&self.tip.as_str()) {
            bad.push(format!("tip: expected one of {TIPS:?}, got {:?}", self.tip));
        }
        if !(0.1..=1.2).contains(&self.diameter) {
            bad.push(format!("diameter {} outside 0.1-1.2 mm", self.diameter));
        }
        if self.preset_variant.is_empty() {
            bad.push("no preset_variant (which slicer presets does it pair with?)".into());
        }
        // The variant string must agree with the flags it encodes.
        let hf_variant = self.preset_variant.starts_with("HF");
        if hf_variant != self.high_flow {
            bad.push(format!(
                "preset_variant {:?} contradicts high_flow = {} -- the HF/standard split \
                 decides which preset family's numbers apply",
                self.preset_variant, self.high_flow
            ));
        }
        let dia_in_variant = self.preset_variant.trim_start_matches("HF");
        if dia_in_variant.parse::<f64>().map(|d| (d - self.diameter).abs() > 1e-9).unwrap_or(true) {
            bad.push(format!(
                "preset_variant {:?} does not encode diameter {}",
                self.preset_variant, self.diameter
            ));
        }
        if self.tip == "brass" && self.abrasive_ok {
            bad.push("brass cannot be abrasive_ok".into());
        }
        for (m, v) in &self.claim_mvs {
            if !(0.5..=60.0).contains(v) {
                bad.push(format!("claim_mvs_{m} = {v} outside plausible 0.5-60 mm3/s"));
            }
            if m.chars().any(|c| c.is_ascii_uppercase()) {
                bad.push(format!("claim_mvs material {m:?} must be lowercase"));
            }
        }
        // Claims must be sourced: a claim without a [sourced] note somewhere
        // in the file is indistinguishable from a guess.
        if !self.claim_mvs.is_empty() && !self.notes.iter().any(|s| s.contains("[sourced]")) {
            bad.push("claim_mvs_* present but no [sourced] note says where they came from".into());
        }
        bad
    }
}

impl Machine {
    pub fn parse(text: &str) -> Result<Machine, String> {
        let mut m = Machine::default();
        for (n, line) in kv_lines(text) {
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected `key = value`, got {line:?}", n + 1))?;
            let (k, v) = (k.trim(), v.trim());
            let num = |key: &str| -> Result<f64, String> {
                v.parse::<f64>().map_err(|_| format!("{key}: not a number: {v:?}"))
            };
            match k {
                "name" => m.name = v.to_string(),
                "max_feedrate_xy" => m.max_feedrate_xy = Some(num(k)?),
                "max_feedrate_z" => m.max_feedrate_z = Some(num(k)?),
                "max_feedrate_e" => m.max_feedrate_e = Some(num(k)?),
                "max_accel_print" => m.max_accel_print = Some(num(k)?),
                "max_accel_travel" => m.max_accel_travel = Some(num(k)?),
                "max_jerk_xy" => m.max_jerk_xy = Some(num(k)?),
                "note" => m.notes.push(v.to_string()),
                other => return Err(format!("line {}: unknown key {other:?}", n + 1)),
            }
        }
        if m.name.is_empty() { return Err("no name".into()); }
        Ok(m)
    }

    pub fn load(path: &str) -> Result<Machine, String> {
        Self::parse(&fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?)
            .map_err(|e| format!("{path}: {e}"))
    }
}

/// Every nozzle in a database directory, sorted by (diameter, name) so the
/// listing is stable for UI and logs.
pub fn nozzle_catalog(dir: &str) -> Result<Vec<Nozzle>, String> {
    let mut out = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("{dir}: {e}"))?;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("nozzle") {
            continue;
        }
        out.push(Nozzle::load(path.to_str().ok_or("non-utf8 path")?)?);
    }
    out.sort_by(|a, b| {
        a.diameter.partial_cmp(&b.diameter).unwrap().then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// Find the nozzle a profile should build against.
pub fn find<'a>(catalog: &'a [Nozzle], diameter: f64, high_flow: bool) -> Option<&'a Nozzle> {
    catalog.iter().find(|z| (z.diameter - diameter).abs() < 1e-9 && z.high_flow == high_flow)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
name = E3D HF ObXidian 0.6
vendor = e3d
tip = obxidian
diameter = 0.6
high_flow = 1
abrasive_ok = 1
preset_variant = HF0.6
retract = 0.6
retract_speed = 25
claim_pa = 0.014
claim_mvs_pla = 30
claim_mvs_petg = 22
measured = pla-cf: pending first bench run
note = [sourced] E3D flow table + PrusaSlicer vendor bundle 2.5.5
";

    #[test]
    fn parses_a_full_nozzle_file() {
        let z = Nozzle::parse(GOOD).unwrap();
        assert_eq!(z.name, "E3D HF ObXidian 0.6");
        assert_eq!(z.diameter, 0.6);
        assert!(z.high_flow && z.abrasive_ok);
        assert_eq!(z.claim_mvs.get("pla"), Some(&30.0));
        assert_eq!(z.claim_mvs.get("petg"), Some(&22.0));
        assert_eq!(z.measured.len(), 1);
        assert!(z.lint().is_empty(), "{:?}", z.lint());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Nozzle::parse("bogus = 1").unwrap_err().contains("unknown key"));
    }

    #[test]
    fn lint_catches_contradictions() {
        // HF flag vs variant string disagreement
        let z = Nozzle::parse(&GOOD.replace("high_flow = 1", "high_flow = 0")).unwrap();
        assert!(z.lint().iter().any(|w| w.contains("contradicts high_flow")));
        // variant not encoding the diameter
        let z = Nozzle::parse(&GOOD.replace("preset_variant = HF0.6", "preset_variant = HF0.4")).unwrap();
        assert!(z.lint().iter().any(|w| w.contains("does not encode diameter")));
        // abrasive brass
        let z = Nozzle::parse(
            &GOOD.replace("tip = obxidian", "tip = brass"),
        ).unwrap();
        assert!(z.lint().iter().any(|w| w.contains("brass cannot")));
        // a claim with no [sourced] note anywhere
        let z = Nozzle::parse(&GOOD.replace("[sourced]", "")).unwrap();
        assert!(z.lint().iter().any(|w| w.contains("where they came from")));
    }

    #[test]
    fn claims_and_measurements_never_share_a_field() {
        // The type system is the guarantee: claims are numeric map entries,
        // measurements are opaque strings. This test documents the intent.
        let z = Nozzle::parse(GOOD).unwrap();
        assert!(z.claim_mvs.values().all(|v| v.is_finite()));
        assert!(z.measured.iter().all(|s| !s.trim().is_empty()));
    }

    #[test]
    fn machine_file_parses() {
        let m = Machine::parse(
            "name = CORE One family\nmax_feedrate_xy = 400\nmax_accel_travel = 8000\nnote = [sourced] vendor bundle",
        ).unwrap();
        assert_eq!(m.max_feedrate_xy, Some(400.0));
        assert!(Machine::parse("max_feedrate_xy = 400").is_err(), "nameless machine must fail");
    }

    #[test]
    fn find_selects_on_diameter_and_melt_geometry() {
        let hf = Nozzle::parse(GOOD).unwrap();
        let std_ = Nozzle::parse(
            &GOOD.replace("high_flow = 1", "high_flow = 0")
                .replace("preset_variant = HF0.6", "preset_variant = 0.6")
                .replace("name = E3D HF ObXidian 0.6", "name = E3D DiamondBack 0.6")
                .replace("tip = obxidian", "tip = pcd"),
        ).unwrap();
        let cat = vec![hf, std_];
        assert_eq!(find(&cat, 0.6, true).unwrap().name, "E3D HF ObXidian 0.6");
        assert_eq!(find(&cat, 0.6, false).unwrap().name, "E3D DiamondBack 0.6");
        assert!(find(&cat, 0.4, true).is_none());
    }

    #[test]
    fn the_shipped_nozzle_db_parses_and_lints_clean() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../profiles/nozzles");
        if !std::path::Path::new(dir).exists() {
            return; // db is optional in a bare checkout
        }
        let cat = nozzle_catalog(dir).unwrap_or_else(|e| panic!("{e}"));
        assert!(!cat.is_empty(), "profiles/nozzles exists but holds no .nozzle files");
        for e in fs::read_dir(dir).unwrap().flatten() {
            if e.path().extension().and_then(|s| s.to_str()) == Some("machine") {
                Machine::load(e.path().to_str().unwrap()).unwrap_or_else(|e| panic!("{e}"));
            }
        }
    }
}
