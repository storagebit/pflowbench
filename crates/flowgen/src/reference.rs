// reference.rs -- identify what a PrusaSlicer export ACTUALLY is.
//
// The generator splices its machine start/end blocks out of a real slicer
// export, so that file silently decides bed temperature, chamber temperature,
// levelling and purge. Guessing what it contains is how a "PETG" reference
// turned out to have been sliced as PC Blend CF all along -- documented one
// way, printing another, for the whole project's life.
//
// Every export carries a settings footer (`; key = value` lines). Read it
// rather than trusting a filename or a README.

/// What a reference export declares about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefInfo {
    pub path: String,
    /// e.g. "Prusament PC Blend Carbon Fiber @COREONE 0.8"
    pub filament: Option<String>,
    /// PrusaSlicer's coarse type: PLA / PETG / PC / ABS / ...
    pub filament_type: Option<String>,
    pub printer_model: Option<String>,
    pub nozzle: Option<f64>,
    pub nozzle_high_flow: Option<bool>,
    pub abrasive: Option<bool>,
    pub first_layer_temp: Option<i64>,
    pub temp: Option<i64>,
    pub first_layer_bed: Option<i64>,
    pub bed: Option<i64>,
    pub chamber: Option<i64>,
    pub chamber_minimal: Option<i64>,
    /// The vendor's published flow ceiling: the single most useful number for
    /// deciding where a test ladder should sit.
    pub max_volumetric_speed: Option<f64>,
    pub extrusion_multiplier: Option<f64>,
}

impl RefInfo {
    /// One-line summary for a catalogue listing.
    pub fn summary(&self) -> String {
        format!(
            "{} | {} | {} nozzle{} | noz {}C bed {}C chamber {}C | max {} mm3/s",
            self.filament.clone().unwrap_or_else(|| "unknown filament".into()),
            self.filament_type.clone().unwrap_or_else(|| "?".into()),
            self.nozzle.map(|n| format!("{n}")).unwrap_or_else(|| "?".into()),
            if self.nozzle_high_flow == Some(true) { " HF" } else { "" },
            self.temp.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            self.first_layer_bed.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            self.chamber.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            self.max_volumetric_speed.map(|v| format!("{v}")).unwrap_or_else(|| "?".into()),
        )
    }

    pub fn parse(path: &str, body: &str) -> RefInfo {
        let mut i = RefInfo { path: path.to_string(), ..Default::default() };
        for line in body.lines() {
            let Some(rest) = line.strip_prefix("; ") else { continue };
            let Some((k, v)) = rest.split_once(" = ") else { continue };
            let v = v.trim().trim_matches('"');
            if v.is_empty() {
                continue;
            }
            // Values are comma-separated per-extruder; take the first.
            let first = v.split(',').next().unwrap_or(v).trim();
            let f = || first.parse::<f64>().ok();
            let n = || first.parse::<i64>().ok();
            let b = || first.parse::<i64>().ok().map(|x| x != 0);
            match k.trim() {
                "filament_settings_id" => i.filament = Some(first.to_string()),
                "filament_type" => i.filament_type = Some(first.to_string()),
                "printer_model" => i.printer_model = Some(first.to_string()),
                "nozzle_diameter" => i.nozzle = f(),
                "nozzle_high_flow" => i.nozzle_high_flow = b(),
                "filament_abrasive" => i.abrasive = b(),
                "first_layer_temperature" => i.first_layer_temp = n(),
                "temperature" => i.temp = n(),
                "first_layer_bed_temperature" => i.first_layer_bed = n(),
                "bed_temperature" => i.bed = n(),
                "chamber_temperature" => i.chamber = n(),
                "chamber_minimal_temperature" => i.chamber_minimal = n(),
                "filament_max_volumetric_speed" => i.max_volumetric_speed = f(),
                "extrusion_multiplier" => i.extrusion_multiplier = f(),
                _ => {}
            }
        }
        i
    }

    pub fn load(path: &str) -> Result<RefInfo, String> {
        let body = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        Ok(RefInfo::parse(path, &body))
    }
}

/// Every reference export found in a directory, identified from its contents.
pub fn catalog(dir: &str) -> Vec<RefInfo> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("gcode") {
            continue;
        }
        if let Some(s) = p.to_str() {
            if let Ok(info) = RefInfo::load(s) {
                out.push(info);
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Does this reference actually match what the profile claims to be?
///
/// This is the check that was missing: a profile named for one material
/// pointed at an export sliced with another, and nothing noticed. The start
/// block decides bed and chamber temperatures, so a mismatch means printing
/// one material's parameters while believing you're printing another's.
pub fn check_match(profile_name: &str, info: &RefInfo) -> Vec<String> {
    let mut bad = Vec::new();
    let name = profile_name.to_ascii_lowercase();

    // Coarse type is the one that changes bed/chamber the most.
    if let Some(t) = &info.filament_type {
        let t_lc = t.to_ascii_lowercase();
        let claims_pc = name.contains("pc ") || name.contains("pc-") || name.contains("polycarb");
        let claims_petg = name.contains("petg");
        // "pla" alone would also match inside "pla-cf" etc., which is what we
        // want -- but NOT inside words like "plastic" or "plate", hence the
        // boundary check on the character before and after the hit.
        let claims_pla = {
            let b = name.as_bytes();
            name.match_indices("pla").any(|(i, _)| {
                let before_ok = i == 0 || !b[i - 1].is_ascii_alphanumeric();
                let after_ok = i + 3 >= b.len() || !b[i + 3].is_ascii_alphanumeric();
                before_ok && after_ok
            })
        };
        if claims_pc && t_lc != "pc" {
            bad.push(format!(
                "profile names PC but the reference is sliced as {t}: bed and chamber \
                 temperatures in the start block would be wrong"
            ));
        }
        if claims_petg && t_lc != "petg" {
            bad.push(format!(
                "profile names PETG but the reference is sliced as {t}: bed and chamber \
                 temperatures in the start block would be wrong"
            ));
        }
        if claims_pla && t_lc != "pla" {
            bad.push(format!(
                "profile names PLA but the reference is sliced as {t}: the start block \
                 would level and purge at {t}'s bed temperature and could soak the \
                 chamber -- for PLA in this enclosure that is the wrong machine prep"
            ));
        }
    }

    // Nozzle diameter, when the profile name states one (e.g. "@ 0.6 ...").
    // Two profiles can share a diameter and still differ (HF vs standard),
    // so this is necessary but not sufficient -- the flow-geometry check
    // below covers the rest.
    if let Some(ref_noz) = info.nozzle {
        let named: Vec<f64> = ["0.25", "0.3", "0.4", "0.5", "0.6", "0.8"]
            .iter()
            .filter(|d| {
                let b = name.as_bytes();
                name.match_indices(*d).any(|(i, _)| {
                    let end = i + d.len();
                    let before_ok = i == 0 || !b[i - 1].is_ascii_digit();
                    let after_ok = end >= b.len()
                        || (!b[end].is_ascii_digit() && b[end] != b'.');
                    before_ok && after_ok
                })
            })
            .filter_map(|d| d.parse::<f64>().ok())
            .collect();
        if !named.is_empty() && !named.iter().any(|d| (d - ref_noz).abs() < 1e-9) {
            bad.push(format!(
                "profile names a {} mm nozzle but the reference was sliced for {} mm: \
                 every speed and extrusion in the spliced blocks is for the wrong orifice",
                named.iter().map(|d| format!("{d}")).collect::<Vec<_>>().join("/"),
                ref_noz
            ));
        }
    }

    // High-flow vs standard melt geometry. Same diameter, ~2x apart on flow
    // ceiling at 0.6 -- pointing a DiamondBack profile at an HF export (or
    // the reverse) yields a reference whose published max_volumetric_speed,
    // the very anchor a ladder is placed against, is for the other hotend.
    let claims_hf = name.contains("hf") || name.contains("high flow")
        || name.contains("high-flow") || name.contains("obxidian");
    let claims_std = name.contains("diamondback") || name.contains("standard flow")
        || name.contains("standard-flow");
    match info.nozzle_high_flow {
        Some(true) if claims_std && !claims_hf => bad.push(
            "profile names a standard-flow nozzle (DiamondBack) but the reference was \
             sliced with nozzle_high_flow = 1: its published flow ceiling and speeds \
             are for a high-flow melt zone this nozzle does not have"
                .to_string(),
        ),
        Some(false) if claims_hf && !claims_std => bad.push(
            "profile names a high-flow nozzle but the reference was sliced with \
             nozzle_high_flow = 0: the spliced blocks and published flow ceiling \
             are for a standard melt zone"
                .to_string(),
        ),
        _ => {}
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOTER: &str = r#"
G1 X1 Y1
; filament_settings_id = "Prusament PC Blend Carbon Fiber @COREONE 0.8"
; filament_type = PC
; nozzle_diameter = 0.8
; nozzle_high_flow = 0
; filament_abrasive = 1
; first_layer_temperature = 285
; temperature = 285
; first_layer_bed_temperature = 110
; bed_temperature = 115
; chamber_temperature = 55
; chamber_minimal_temperature = 40
; filament_max_volumetric_speed = 18
; extrusion_multiplier = 1.04
; printer_model = COREONEL
"#;

    #[test]
    fn identifies_a_reference_from_its_own_footer() {
        let i = RefInfo::parse("ref.gcode", FOOTER);
        assert_eq!(i.filament_type.as_deref(), Some("PC"));
        assert_eq!(i.filament.as_deref(), Some("Prusament PC Blend Carbon Fiber @COREONE 0.8"));
        assert_eq!(i.nozzle, Some(0.8));
        assert_eq!(i.nozzle_high_flow, Some(false));
        assert_eq!(i.abrasive, Some(true));
        assert_eq!(i.temp, Some(285));
        assert_eq!(i.first_layer_bed, Some(110));
        assert_eq!(i.chamber, Some(55));
        assert_eq!(i.chamber_minimal, Some(40));
        assert_eq!(i.max_volumetric_speed, Some(18.0));
        assert_eq!(i.printer_model.as_deref(), Some("COREONEL"));
    }

    #[test]
    fn catches_the_mistake_that_actually_happened() {
        // a profile named PETG pointing at a PC-sliced export
        let i = RefInfo::parse("ref.gcode", FOOTER);
        let bad = check_match("Prusament PETG CF @ 0.8 DiamondBack", &i);
        assert!(!bad.is_empty(), "a PETG profile on a PC reference must be flagged");
        assert!(bad[0].contains("PC"), "{bad:?}");
        // and the correct pairing passes
        assert!(check_match("Prusament PC Blend CF @ 0.8 DiamondBack", &i).is_empty());
    }

    #[test]
    fn per_extruder_lists_take_the_first_value() {
        let i = RefInfo::parse("x", "; nozzle_diameter = 0.8,0.4\n; temperature = 285,240\n");
        assert_eq!(i.nozzle, Some(0.8));
        assert_eq!(i.temp, Some(285));
    }

    #[test]
    fn missing_footer_yields_unknowns_not_a_panic() {
        let i = RefInfo::parse("x", "G28\nG1 X0\n");
        assert_eq!(i.filament_type, None);
        assert!(i.summary().contains("unknown filament"));
    }

    #[test]
    fn a_pla_profile_on_a_pc_reference_is_flagged() {
        // the PLA analogue of the mistake that actually happened: ref.gcode
        // is PC (bed 110, blocking chamber soak) -- splicing it under a PLA
        // profile preps the machine for the wrong material entirely
        let i = RefInfo::parse("ref.gcode", FOOTER);
        let bad = check_match("Elegoo PLA-CF @ 0.8 DiamondBack", &i);
        assert!(bad.iter().any(|b| b.contains("PLA")), "{bad:?}");
        // and "pla" must not fire inside unrelated words
        assert!(check_match("Plate Fixture PC @ 0.8", &i)
            .iter()
            .all(|b| !b.contains("names PLA")));
    }

    #[test]
    fn nozzle_diameter_mismatch_is_flagged_when_the_name_states_one() {
        let i = RefInfo::parse("ref.gcode", FOOTER); // sliced for 0.8
        let bad = check_match("Prusament PC Blend CF @ 0.6 DiamondBack", &i);
        assert!(bad.iter().any(|b| b.contains("0.6") && b.contains("0.8")), "{bad:?}");
        // matching diameter passes; a name with no diameter stays silent
        assert!(check_match("Prusament PC Blend CF @ 0.8 DiamondBack", &i).is_empty());
        assert!(check_match("Prusament PC Blend CF, big nozzle", &i).is_empty());
    }

    #[test]
    fn hf_and_standard_flow_cannot_be_crossed() {
        const HF06: &str = "\n; filament_settings_id = \"Elegoo PLA-CF HFOBX06 @COREONE\"\n; filament_type = PLA\n; nozzle_diameter = 0.6\n; nozzle_high_flow = 1\n";
        const STD06: &str = "\n; filament_settings_id = \"Elegoo PLA-CF DB06 @COREONE\"\n; filament_type = PLA\n; nozzle_diameter = 0.6\n; nozzle_high_flow = 0\n";
        let hf = RefInfo::parse("hf.gcode", HF06);
        let std_ = RefInfo::parse("std.gcode", STD06);
        // crossed pairings: both directions must be flagged
        let bad = check_match("Elegoo PLA-CF @ 0.6 DiamondBack", &hf);
        assert!(bad.iter().any(|b| b.contains("high-flow melt zone")), "{bad:?}");
        let bad = check_match("Elegoo PLA-CF @ 0.6 HF ObXidian", &std_);
        assert!(bad.iter().any(|b| b.contains("standard melt zone")), "{bad:?}");
        // correct pairings pass -- this is the check the two 0.6 profiles
        // (same diameter, ~2x apart on flow) actually depend on
        assert!(check_match("Elegoo PLA-CF @ 0.6 DiamondBack", &std_).is_empty());
        assert!(check_match("Elegoo PLA-CF @ 0.6 HF ObXidian", &hf).is_empty());
    }

    /// The catalogue must describe the files actually in the repo -- this is
    /// the check that would have caught the mislabelled reference on day one.
    #[test]
    fn catalogues_the_real_reference_directory() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference");
        let found = catalog(dir);
        if found.is_empty() {
            return; // bare checkout
        }
        for i in &found {
            println!("{} -> {}", i.path, i.summary());
            assert!(
                i.filament_type.is_some(),
                "{} has no filament_type: cannot be trusted as a reference",
                i.path
            );
        }
    }
}
