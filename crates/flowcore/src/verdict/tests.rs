use super::*;
use crate::capture::BandStat;

fn set_mean(b: &mut BandStat, v: f64) {
    b.mean = v;
    b.settled_mean = Some(v);
}

fn band(band: usize, flow: f64, n: u64, mean: f64) -> BandStat {
    BandStat {
        cylinder: 0,
        band,
        flow,
        n,
        mean,
        sd: mean * 0.05,
        settled_n: n,
        settled_mean: Some(mean),
        settled_sd: Some(mean * 0.05),
        ..Default::default()
    }
}

/// A healthy ladder: force grows roughly linearly with flow (viscous
/// baseline), sample counts fall as revs/flow says they must.
fn healthy(cylinder: usize, temp: i64) -> CylinderInput {
    let flows = [8.0, 10.0, 12.0, 14.0, 16.0];
    let bands = flows
        .iter()
        .enumerate()
        .map(|(i, &f)| {
            let mut b = band(i + 1, f, (4000.0 * 4.0 / f) as u64, 800.0 + 250.0 * f);
            b.cylinder = cylinder;
            b
        })
        .collect();
    CylinderInput { cylinder, temp: Some(temp), tare: Some((10.0, 50)), bands, vision: Vec::new() }
}

#[test]
fn healthy_ladder_sustains_everything_and_recommends_with_margin() {
    let input = VerdictInput {
        cylinders: vec![healthy(0, 220)],
        revs: 4,
        primary_temp: Some(220),
        margin_rungs: 1,
    };
    let v = judge(&input);
    assert!(v.bands.iter().all(|b| b.class == BandClass::Sustained), "{:#?}", v.bands);
    assert_eq!(v.temps[0].ceiling, Some(16.0));
    // one rung of margin below the 16 ceiling
    assert_eq!(v.recommendation, Some(14.0), "{:?}", v.run_flags);
}

#[test]
fn force_departure_fails_the_band_and_voids_everything_above() {
    let mut c = healthy(0, 220);
    // bands 4 and 5 leave the viscous line hard: melt limit
    set_mean(&mut c.bands[3], 800.0 + 250.0 * 14.0 + 2600.0);
    set_mean(&mut c.bands[4], 800.0 + 250.0 * 16.0 + 6000.0);
    let v = judge(&VerdictInput {
        cylinders: vec![c],
        revs: 4,
        primary_temp: None,
        margin_rungs: 1,
    });
    let classes: Vec<BandClass> = v.bands.iter().map(|b| b.class).collect();
    assert_eq!(classes[..3], [BandClass::Sustained; 3], "{:#?}", v.bands);
    assert_eq!(classes[3], BandClass::Failed, "{:#?}", v.bands);
    // rule 10: a band above the first failure keeps a CONDEMNED class (it
    // also failed -- that is evidence); only healthy classes are voided
    assert_eq!(classes[4], BandClass::Failed, "{:#?}", v.bands);
    assert_eq!(v.temps[0].ceiling, Some(12.0));
    assert_eq!(v.recommendation, Some(10.0));
    assert!(v.temps[0].sentence.contains("SUSTAINED"), "{}", v.temps[0].sentence);
    assert!(v.temps[0].sentence.contains("FAILED"), "{}", v.temps[0].sentence);
}

#[test]
fn slip_plateau_reads_saturated_not_recovered() {
    let mut c = healthy(0, 220);
    // strong departure then a plateau: the extruder slips, force stops
    // growing -- must NOT read as healthy again
    set_mean(&mut c.bands[2], 800.0 + 250.0 * 12.0 + 3000.0); // R >> 2
    let m2 = c.bands[2].mean;
    set_mean(&mut c.bands[3], m2 + 120.0); // increment collapses
    let m3 = c.bands[3].mean;
    set_mean(&mut c.bands[4], m3 + 100.0);
    let v = judge(&VerdictInput {
        cylinders: vec![c],
        revs: 4,
        primary_temp: None,
        margin_rungs: 0,
    });
    let classes: Vec<BandClass> = v.bands.iter().map(|b| b.class).collect();
    assert_eq!(classes[2], BandClass::Failed, "{classes:?}");
    // the plateau after strong growth reads SATURATED -- slip, not recovery
    assert_eq!(classes[3], BandClass::Saturated, "{classes:?}");
    assert_eq!(classes[4], BandClass::Saturated, "{classes:?}");
}

#[test]
fn vision_stall_alone_caps_at_marginal_but_corroborates_to_failed() {
    let mut alone = healthy(0, 220);
    alone.vision = vec![(3, VisionVote::Stall)];
    let v = judge(&VerdictInput {
        cylinders: vec![alone],
        revs: 4,
        primary_temp: None,
        margin_rungs: 0,
    });
    let b3 = v.bands.iter().find(|b| b.band == 3).unwrap();
    assert_eq!(b3.class, BandClass::Marginal, "single family caps at MARGINAL: {b3:?}");

    // now the force family also wobbles into the marginal zone: two
    // families agree, hard FAILED
    let mut both = healthy(0, 220);
    let m = both.bands[2].mean * 1.25; // E lands in 1.15-1.3
    set_mean(&mut both.bands[2], m);
    both.vision = vec![(3, VisionVote::Stall)];
    let v = judge(&VerdictInput {
        cylinders: vec![both],
        revs: 4,
        primary_temp: None,
        margin_rungs: 0,
    });
    let b3 = v.bands.iter().find(|b| b.band == 3).unwrap();
    assert_eq!(b3.class, BandClass::Failed, "{b3:?}");
    assert!((b3.confidence - 1.0).abs() < 1e-9);
}

#[test]
fn negative_tared_means_withhold_the_cylinder() {
    let mut c = healthy(0, 220);
    c.tare = Some((5000.0, 60)); // tare above every band mean
    let v = judge(&VerdictInput {
        cylinders: vec![c],
        revs: 4,
        primary_temp: None,
        margin_rungs: 0,
    });
    assert!(v.bands.iter().all(|b| b.class == BandClass::NoVote));
    assert!(v.run_flags.iter().any(|f| f.contains("tare drifted")), "{:?}", v.run_flags);
}

#[test]
fn cross_temperature_flags_melt_limit() {
    // same flow, force falling as temperature rises across 3 cylinders
    let mut cyls = Vec::new();
    for (i, (temp, scale)) in [(220, 1.2), (230, 1.0), (240, 0.8)].iter().enumerate() {
        let mut c = healthy(i, *temp);
        for b in &mut c.bands {
            let m = b.mean * scale;
            set_mean(b, m);
        }
        cyls.push(c);
    }
    let v = judge(&VerdictInput {
        cylinders: cyls,
        revs: 4,
        primary_temp: None,
        margin_rungs: 0,
    });
    assert!(
        v.run_flags.iter().any(|f| f.contains("melt-limited")),
        "{:?}",
        v.run_flags
    );
}

#[test]
fn reversed_control_is_withheld_not_sustained() {
    // a reversed-control cylinder must never read SUSTAINED or produce a
    // ceiling -- the force rules assume an ascending ladder
    let mut c = healthy(0, 220);
    c.bands.reverse();
    for (i, b) in c.bands.iter_mut().enumerate() {
        b.band = i + 1;
    }
    let v = judge(&VerdictInput {
        cylinders: vec![c],
        revs: 4,
        primary_temp: None,
        margin_rungs: 1,
    });
    assert!(v.bands.iter().all(|b| b.class == BandClass::NoVote), "{:#?}", v.bands);
    assert_eq!(v.temps[0].ceiling, None);
    assert_eq!(v.recommendation, None);
    assert!(v.run_flags.iter().any(|f| f.contains("ascending ladder")), "{:?}", v.run_flags);
}

#[test]
fn flat_force_with_no_clean_increment_is_withheld() {
    // force never rises above the reference band's noise: the family cannot
    // judge and must say so instead of calling everything SUSTAINED
    let mut c = healthy(0, 220);
    for b in &mut c.bands {
        b.mean = 1000.0;
        b.settled_mean = Some(1000.0);
    }
    let v = judge(&VerdictInput {
        cylinders: vec![c],
        revs: 4,
        primary_temp: None,
        margin_rungs: 0,
    });
    assert!(v.bands.iter().all(|b| b.class == BandClass::NoVote), "{:#?}", v.bands);
    assert!(
        v.run_flags.iter().any(|f| f.contains("no clean positive force increment")),
        "{:?}",
        v.run_flags
    );
}

#[test]
fn settled_stats_take_precedence_over_full_band_means() {
    // full-band means carry the previous band's pressure tail; the settled
    // window is the judged input. Make them disagree and check who wins.
    let mut c = healthy(0, 220);
    // full-band means scream failure, settled means stay healthy
    for b in &mut c.bands {
        b.mean = b.mean * 10.0;
    }
    let v = judge(&VerdictInput {
        cylinders: vec![c],
        revs: 4,
        primary_temp: None,
        margin_rungs: 0,
    });
    assert!(
        v.bands.iter().all(|b| b.class == BandClass::Sustained),
        "settled stats must win: {:#?}",
        v.bands
    );
}

/// The golden test: the real failed PC-CF run, recovered band table.
/// Expectations from the design doc: the artifact gate classifies all 11
/// counted cylinders correctly (only 5, 7, 9, 11 printed), and the force
/// family flags the 19.5-32 region on all four real cylinders.
#[test]
fn real_run_20260811_artifact_gate_and_force_flags() {
    let csv = include_str!("../../testdata/run-20260811-bands.csv");
    let mut by_cyl: std::collections::BTreeMap<usize, Vec<BandStat>> = Default::default();
    for line in csv.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 8 {
            continue;
        }
        let mut b = BandStat {
            cylinder: f[0].parse().unwrap(),
            band: f[1].parse().unwrap(),
            flow: f[2].parse().unwrap(),
            n: f[5].parse().unwrap(),
            mean: f[6].parse().unwrap(),
            sd: f[7].parse().unwrap(),
            ..Default::default()
        };
        by_cyl.entry(b.cylinder).or_default().push(b);
    }
    assert_eq!(by_cyl.len(), 11, "the run counted 11 cylinders");
    let temps = [(5usize, 275i64), (7, 280), (9, 285), (11, 290)];
    let cylinders: Vec<CylinderInput> = by_cyl
        .into_iter()
        .map(|(cyl, bands)| CylinderInput {
            cylinder: cyl,
            temp: temps.iter().find(|(c, _)| *c == cyl).map(|(_, t)| *t),
            tare: None, // run 1 predates tare windows
            bands,
            vision: Vec::new(),
        })
        .collect();
    let v = judge(&VerdictInput { cylinders, revs: 4, primary_temp: None, margin_rungs: 1 });

    // artifact gate: exactly the four printed cylinders survive
    let artifacts: Vec<usize> = (1..=11)
        .filter(|c| v.run_flags.iter().any(|f| f.starts_with(&format!("cylinder {c}: artifact"))))
        .collect();
    assert_eq!(artifacts.len(), 7, "7 phantoms: {:?}\nflags: {:#?}", artifacts, v.run_flags);
    for real in [5, 7, 9, 11] {
        assert!(!artifacts.contains(&real), "cylinder {real} is a real test object");
    }

    // force family: on every real cylinder, nothing at or above 19.5 mm3/s
    // reads healthy -- the whole ladder sat above the cliff
    for b in v.bands.iter().filter(|b| [5, 7, 9, 11].contains(&b.cylinder)) {
        if b.flow >= 19.5 {
            assert!(
                matches!(b.class, BandClass::Failed | BandClass::Saturated | BandClass::NoVote),
                "cyl {} flow {} judged {:?}: {:?}",
                b.cylinder,
                b.flow,
                b.class,
                b.fired
            );
        }
    }
    // the slip plateau at the ladder top must be visible as SATURATED on at
    // least one real cylinder -- the branch the plan validated on this run
    assert!(
        v.bands.iter().any(|b| [5, 7, 9, 11].contains(&b.cylinder)
            && b.class == BandClass::Saturated),
        "no saturated band found"
    );
    // and the cross-temperature rule sees the melt limit (force falls with
    // temperature at fixed flow on this run)
    assert!(
        v.run_flags.iter().any(|f| f.contains("melt-limited")),
        "{:#?}",
        v.run_flags
    );
    // run 1 predates settled windows: the engine must say it fell back
    assert!(
        v.run_flags.iter().any(|f| f.contains("full-band means")),
        "{:#?}",
        v.run_flags
    );
}
