// validate.rs -- refuse to ship a file that would ruin a run.
//
// Checked on the assembled body before it is written, and available to the
// app for re-checking after its own post-processing.

/// Inspect a generated file for the mistakes that silently ruin a run.
///
/// These are not style checks -- each corresponds to a real failure that
/// reached the printer: metrics armed before the Z axis was homed (133k force
/// samples, zero Z, no band mapping), and commands prepended ahead of the
/// firmware's compatibility checks (an ATTENTION prompt on upload).
pub fn validate_output(body: &str, expect_metrics: bool) -> Vec<String> {
    let code: Vec<&str> = body
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with(';'))
        .collect();
    let mut bad = Vec::new();
    let pos = |pred: &dyn Fn(&str) -> bool| code.iter().position(|l| pred(l));

    let m334 = pos(&|l: &str| l.starts_with("M334 "));
    if !expect_metrics {
        return bad; // a job generated without telemetry is legitimate
    }
    if m334.is_none() {
        bad.push("no M334: the printer would never stream telemetry anywhere".to_string());
        return bad;
    }
    if let (Some(m862), Some(m334)) = (pos(&|l: &str| l.starts_with("M862")), m334) {
        if m334 < m862 {
            bad.push(
                "M334 precedes the M862 compatibility checks: the file no longer opens like a \
                 stock export, which has caused an ATTENTION prompt on upload"
                    .to_string(),
            );
        }
    }
    // the one that cost a whole run
    match pos(&|l: &str| l.starts_with("G28")) {
        Some(homed) => {
            if !code[homed..].iter().any(|l| *l == "M331 pos_z") {
                bad.push(
                    "pos_z is never armed after G28: enabling it while Z is unhomed yields ZERO \
                     z samples, so nothing can be mapped to a band"
                        .to_string(),
                );
            }
        }
        None => bad.push("no G28: the start block does not home the machine".to_string()),
    }
    if !code.iter().any(|l| *l == "M331 loadcell_value") {
        bad.push("loadcell_value is never armed: no back-pressure would be captured".to_string());
    }
    bad
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{check_match, generate, Profile, RefInfo};

    /// End-to-end against the REAL reference export and the REAL profile --
    /// the combination that actually gets printed. Ignored by default since
    /// it depends on files outside the crate.
    ///   cargo test -p flowgen -- --ignored real_profile
    #[test]
    #[ignore]
    fn real_profile_against_the_real_reference_passes_validation() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let mut checked = 0usize;
        for e in fs::read_dir(format!("{root}/profiles")).expect("profiles dir").flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("profile") {
                continue;
            }
            let p = Profile::load(&path.to_string_lossy()).expect("load profile");
            let Some(rel) = p.reference.as_ref() else { continue };
            let refp = format!("{root}/{rel}");
            if !std::path::Path::new(&refp).exists() {
                // A profile may legitimately be waiting on its export; the
                // catalogue records that rather than failing the suite.
                println!("SKIP {}: reference {rel} not present", p.name);
                continue;
            }

            // The reference decides bed/chamber temperature, so it must be the
            // material the profile claims. This pairing was wrong for months.
            let info = RefInfo::load(&refp).expect("identify reference");
            println!("{} -> {}", p.name, info.summary());
            let mismatch = check_match(&p.name, &info);
            assert!(mismatch.is_empty(), "{}: {mismatch:?}", p.name);

            let out = std::env::temp_dir()
                .join(format!("flowgen_real_{}.gcode", checked));
            let mut cfg = p.cfg.clone();
            cfg.out = out.to_string_lossy().into_owned();
            cfg.reference = Some(refp);
            cfg.metrics_host = Some("192.0.2.18".into());

            let r = generate(cfg).expect("generation must succeed");
            println!("{}", r.summary);

            let body = fs::read_to_string(&out).unwrap();
            let problems = validate_output(&body, true);
            assert!(problems.is_empty(), "validator found: {problems:?}");

            // the two things that broke real runs
            let code: Vec<&str> = body.lines().map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with(';')).collect();
            let g28 = code.iter().position(|l| l.starts_with("G28")).unwrap();
            assert!(code[g28..].iter().any(|l| *l == "M331 pos_z"), "pos_z re-armed after homing");
            let m862 = code.iter().position(|l| l.starts_with("M862")).unwrap();
            let m334 = code.iter().position(|l| l.starts_with("M334 ")).unwrap();
            assert!(m862 < m334, "compat checks come first");
            println!("bands: {:?}", r.bands.iter().map(|b| b.0).collect::<Vec<_>>());
            let _ = fs::remove_file(&out);
            checked += 1;
        }
        assert!(checked > 0, "no profile had a usable reference export");
    }
}
