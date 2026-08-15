// Loopback tests for the whole capture session: real UDP socket, real receive
// thread, packets shaped exactly as Buddy emits them.

use super::*;
use crate::sdmap::SD_GUARD_BYTES;

/// End-to-end over the sdpos path: with a manifest, samples are binned by
/// the segment the printer's byte offset says we're in -- brim force goes
/// to no band, tare dwell force goes to the cylinder's tare, band force
/// splits into full vs settled windows, and Z excursions CANNOT invent
/// cylinders (the failure that produced 11 of them for 4 test objects).
#[test]
fn sdpos_addressing_bins_by_segment_not_by_z() {
    let manifest = "flowbench-bands v1\n\
        0 1000 travel 0 - 0 0 255\n\
        1000 2000 tare 0 - 0 0 255\n\
        2000 3000 purge 0 - 0 0 255\n\
        3000 4000 first 0 - 0 0 255\n\
        4000 8000 band 0 0 8.00 4 255\n\
        8000 12000 band 0 1 12.00 4 255\n\
        12000 13000 travel 1 - 0 0 265\n\
        13000 14000 tare 1 - 0 0 265\n\
        14000 15000 first 1 - 0 0 265\n\
        15000 19000 band 1 0 8.00 4 265\n\
        19000 20000 end - - 0 0 0\n";
    let sd = SdMap::parse(manifest).unwrap();
    assert_eq!(sd.cylinder_temps(), vec![(0, 255), (1, 265)]);

    let map = BandMap { flows: vec![8., 12.], per_cylinder_flows: Vec::new(),
                        revs: 4, layer_h: 0.4, first_layer_h: 0.2, bead_xsec: None };
    let seen: Arc<Mutex<Vec<BandChange>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let hooks = CaptureHooks {
        logger: None,
        on_band_change: Some(Arc::new(move |b: BandChange| sink.lock().unwrap().push(b))), on_photo_window: None };
    let mut cap = Capture::start("127.0.0.1", 49519, map, Some(sd), hooks).unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut t_us: i64 = 1_000_000;
    let mut send = |sdpos: u64, force: f64, z: f64| {
        // guard is subtracted on lookup, so aim the raw sdpos past it
        let pos = sdpos + SD_GUARD_BYTES;
        tx.send_to(
            format!("<14>1 - AA buddy - - - msg=1,tm={t_us},v=4 \
                     sdpos v={pos}i 0\nloadcell_value v={force:.3} 1\npos_z v={z:.3} 2\n")
                .as_bytes(),
            "127.0.0.1:49519",
        ).unwrap();
        t_us += 50_000;
        std::thread::sleep(Duration::from_millis(3));
    };

    // tare dwell: parked at safe Z (25mm -- would be overtravel to the Z map)
    for _ in 0..5 { send(1500, 2.0, 25.0); }
    // brim / first layer at Z 0.2: near-zero force, NOT band data
    for _ in 0..5 { send(3500, 100.0, 0.2); }
    // band 0 of cylinder 0: first revolution (unsettled) then settled,
    // with a Z DROP mid-band (the exact excursion that used to fake a
    // new cylinder)
    for i in 0..4 { send(4100 + i * 100, 1000.0, 0.5); }
    send(6500, 2000.0, 0.1);           // Z dips -- sdpos says still band 0
    for _ in 0..5 { send(7000, 2000.0, 1.0); }
    // band 1 (cyl 0), settled region only
    for _ in 0..4 { send(11000, 5000.0, 2.5); }
    // cylinder 1, band 0, settled
    for _ in 0..4 { send(18000, 900.0, 0.9); }
    std::thread::sleep(Duration::from_millis(300));

    let snap = cap.snapshot_all();
    cap.stop();

    // tare captured for cylinder 0, at its real level
    let t0 = snap.tares.iter().find(|(c, _, _)| *c == 0).expect("cyl 0 tare");
    assert!((t0.1 - 2.0).abs() < 1e-6 && t0.2 == 5, "tare {t0:?}");

    // exactly the three real (cyl, band) pairs -- no phantoms from Z dips
    let mut keys: Vec<(usize, usize)> =
        snap.bands.iter().map(|b| (b.cylinder, b.band)).collect();
    keys.sort();
    assert_eq!(keys, vec![(0, 1), (0, 2), (1, 1)], "bands: {keys:?}");

    let b00 = snap.bands.iter().find(|b| b.cylinder == 0 && b.band == 1).unwrap();
    assert_eq!(b00.n, 10, "all band-0 samples binned incl. the Z-dip one");
    assert_eq!(b00.settled_n, 6, "first revolution excluded from settled");
    assert!((b00.settled_mean.unwrap() - 2000.0).abs() < 1e-6);
    assert!((b00.flow - 8.0).abs() < 1e-6, "flow comes from the manifest");

    // the hook fired once per band entry, with manifest data
    let fired = seen.lock().unwrap();
    let fired_keys: Vec<(usize, usize, i64)> =
        fired.iter().map(|b| (b.cylinder, b.band, b.flow as i64)).collect();
    assert_eq!(fired_keys, vec![(0, 0, 8), (0, 1, 12), (1, 0, 8)], "{fired_keys:?}");

    // band windows recorded and ordered
    assert_eq!(snap.band_windows.len(), 3);
    assert!(snap.band_windows.windows(2).all(|w| w[0].2 <= w[1].2));
}

#[test]
fn capture_end_to_end_over_loopback() {
    let map = BandMap { flows: vec![8., 10.], per_cylinder_flows: Vec::new(), revs: 4, layer_h: 0.4, first_layer_h: 0.2, bead_xsec: None };
    let mut cap = Capture::start("127.0.0.1", 0, map, None, CaptureHooks::default()).unwrap();
    let port = cap.state.lock().unwrap(); // placeholder to appease borrow order
    drop(port);
    // We bound port 0; recover the actual port from the socket via a fresh bind trick:
    // simpler -- rebind on a fixed high port instead.
    cap.stop();
    let map = BandMap { flows: vec![8., 10.], per_cylinder_flows: Vec::new(), revs: 4, layer_h: 0.4, first_layer_h: 0.2, bead_xsec: None };
    let mut cap = Capture::start("127.0.0.1", 49514, map, None, CaptureHooks::default()).unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let hdr = "<14>1 - AA buddy - - - msg=1,tm=1,v=4 ";
    // first layer: must be excluded from bands
    tx.send_to(format!("{hdr}pos_z v=0.200000 1\nloadcell_value v=99.0 2\n").as_bytes(), "127.0.0.1:49514").unwrap();
    // band 1 (z ~1.0) then band 2 (z ~2.6)
    tx.send_to(format!("{hdr}pos_z v=1.000000 3\nloadcell_value v=1.5 4\n").as_bytes(), "127.0.0.1:49514").unwrap();
    tx.send_to(format!("{hdr}pos_z v=2.600000 5\nloadcell_value v=3.5 6\n").as_bytes(), "127.0.0.1:49514").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let s = cap.state.lock().unwrap();
        if s.acc.len() == 2 {
            assert!((s.acc[&(0, 0)].mean() - 1.5).abs() < 1e-9);
            assert!((s.acc[&(0, 1)].mean() - 3.5).abs() < 1e-9);
            assert_eq!(s.force.len(), 3); // raw stream keeps the 99g point
            break;
        }
        drop(s);
        assert!(Instant::now() < deadline, "capture never binned the samples");
        std::thread::sleep(Duration::from_millis(20));
    }
    cap.stop();
}

/// The photo hook arms at sdpos segment entry and fires when the head
/// demonstrably STOPS (3 consecutive derived speeds < 0.5 mm/s), or on
/// the 5 s firmware-clock fallback -- never at entry itself, which
/// precedes the physical park by the planner queue depth.
#[test]
fn photo_window_fires_on_stationarity_not_entry() {
    let manifest = "flowbench-bands v1\n\
                    0 999 band 0 0 8.00 4 255\n\
                    1000 4999 photo 0 0 8.00 0 255\n\
                    5000 9999 band 0 1 12.00 4 255\n";
    let sd = SdMap::parse(manifest).unwrap();
    assert!(sd.has_photo());
    let map = BandMap {
        flows: vec![8., 12.],
        per_cylinder_flows: Vec::new(),
        revs: 4,
        layer_h: 0.4,
        first_layer_h: 0.2,
        bead_xsec: None,
    };
    let fired = Arc::new(Mutex::new(Vec::new()));
    let f2 = fired.clone();
    let hooks = CaptureHooks {
        logger: None,
        on_band_change: None,
        on_photo_window: Some(Arc::new(move |bc: BandChange| {
            f2.lock().unwrap().push((bc.cylinder, bc.band, bc.flow));
        })),
    };
    let mut cap = Capture::start("127.0.0.1", 49523, map, Some(sd), hooks).unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let send = |t_us: i64, body: String| {
        tx.send_to(
            format!("<14>1 - AA buddy - - - msg=1,tm={t_us},v=4 {body}\n").as_bytes(),
            "127.0.0.1:49523",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(5));
    };
    // enter the photo segment: ARMS, must not fire yet
    send(1_000_000, format!("sdpos v={} 0", 1_500 + SD_GUARD_BYTES));
    // head still moving (travel to the park): 20mm per 100ms
    let mut t_us = 1_100_000i64;
    let mut x = 0.0;
    for _ in 0..3 {
        send(t_us, format!("pos_x v={x:.3} 0\npos_y v=0.0 1\npos_z v=10.0 2"));
        x += 20.0;
        t_us += 100_000;
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(fired.lock().unwrap().is_empty(), "must not fire while moving");
    // head parked: identical positions -> speed 0; 4 samples > 3 needed
    for _ in 0..4 {
        send(t_us, format!("pos_x v={x:.3} 0\npos_y v=0.0 1\npos_z v=10.0 2"));
        t_us += 100_000;
    }
    std::thread::sleep(Duration::from_millis(300));
    cap.stop();
    let got = fired.lock().unwrap().clone();
    assert_eq!(got, vec![(0usize, 0usize, 8.0f64)], "{got:?}");
}

/// No stationarity ever seen (positions keep moving, or the printer
/// stops streaming them cleanly): the firmware-clock fallback fires the
/// window 5 s after the first post-entry sample rather than never.
#[test]
fn photo_window_falls_back_on_firmware_clock() {
    let manifest = "flowbench-bands v1\n\
                    0 999 band 0 0 8.00 4 255\n\
                    1000 4999 photo 0 0 8.00 0 255\n";
    let sd = SdMap::parse(manifest).unwrap();
    let map = BandMap {
        flows: vec![8.],
        per_cylinder_flows: Vec::new(),
        revs: 4,
        layer_h: 0.4,
        first_layer_h: 0.2,
        bead_xsec: None,
    };
    let fired = Arc::new(Mutex::new(Vec::new()));
    let f2 = fired.clone();
    let hooks = CaptureHooks {
        logger: None,
        on_band_change: None,
        on_photo_window: Some(Arc::new(move |bc: BandChange| {
            f2.lock().unwrap().push((bc.cylinder, bc.band));
        })),
    };
    let mut cap = Capture::start("127.0.0.1", 49524, map, Some(sd), hooks).unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let send = |t_us: i64, body: String| {
        tx.send_to(
            format!("<14>1 - AA buddy - - - msg=1,tm={t_us},v=4 {body}\n").as_bytes(),
            "127.0.0.1:49524",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(5));
    };
    send(1_000_000, format!("sdpos v={} 0", 1_500 + SD_GUARD_BYTES));
    // keep moving the whole time; firmware clock advances past 5 s
    let mut t_us = 1_100_000i64;
    let mut x = 0.0;
    for _ in 0..14 {
        send(t_us, format!("pos_x v={x:.3} 0\npos_y v=0.0 1\npos_z v=10.0 2"));
        x += 20.0;
        t_us += 500_000; // 0.5 s steps -> crosses the 5 s fallback
    }
    std::thread::sleep(Duration::from_millis(300));
    cap.stop();
    let got = fired.lock().unwrap().clone();
    assert_eq!(got, vec![(0usize, 0usize)], "{got:?}");
}

/// Buddy publishes no velocity metric, so speed is differentiated from
/// pos_x/pos_y/pos_z. Travel moves must be excluded or they inflate the
/// per-band mean well above the real print speed.
#[test]
fn speed_is_derived_from_position_and_excludes_travel() {
    let map = BandMap {
        flows: vec![8., 10.],
        per_cylinder_flows: Vec::new(),
        revs: 4,
        layer_h: 0.4,
        first_layer_h: 0.2,
        bead_xsec: Some(0.3256), // 0.9 x 0.4 bead
    };
    let mut cap = Capture::start("127.0.0.1", 49517, map, None, CaptureHooks::default()).unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    // Drive the FIRMWARE clock, not the wall clock: every packet carries
    // its own `tm=`, exactly as Buddy emits it. Speed is differentiated
    // against those stamps, so the test no longer depends on how fast the
    // machine running it happens to schedule threads.
    let send = |t_us: i64, x: f64, y: f64, z: f64| {
        tx.send_to(
            format!("<14>1 - AA buddy - - - msg=1,tm={t_us},v=4 \
                     pos_x v={x:.4} 0\npos_y v={y:.4} 1\npos_z v={z:.4} 2\n")
                .as_bytes(),
            "127.0.0.1:49517",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(2)); // let the receiver drain
    };
    // inside band 0 (z=1.0): 2mm per 100ms -> 20mm/s, a print speed
    let mut t_us = 1_000_000i64;
    let mut x = 0.0;
    for _ in 0..12 {
        send(t_us, x, 0.0, 1.0);
        x += 2.0;
        t_us += 100_000;
    }
    // now a travel-speed burst at the same Z: 20mm per 100ms = 200mm/s
    for _ in 0..6 {
        send(t_us, x, 0.0, 1.0);
        x += 20.0;
        t_us += 100_000;
    }
    std::thread::sleep(Duration::from_millis(300));

    let st = cap.state.lock().unwrap();
    let acc = st.speed_acc.get(&(0, 0)).copied().expect("band 0 speed samples");
    drop(st);
    cap.stop();

    assert!(acc.n >= 8, "expected print-speed samples, got {}", acc.n);
    assert!(
        acc.mean() > 10.0 && acc.mean() < 40.0,
        "mean {:.1} mm/s should reflect the ~20mm/s print moves, not the 200mm/s travel",
        acc.mean()
    );
    assert!(acc.max < TRAVEL_SPEED_CUTOFF, "a travel move leaked in: max {:.1}", acc.max);
}

#[test]
fn band_change_hook_fires_once_per_band_with_the_right_flow() {
    let seen: Arc<Mutex<Vec<BandChange>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let hooks = CaptureHooks {
        logger: None,
        on_band_change: Some(Arc::new(move |bc| sink.lock().unwrap().push(bc))), on_photo_window: None };

    let map = BandMap { flows: vec![8., 10., 12.], per_cylinder_flows: Vec::new(), revs: 4, layer_h: 0.4, first_layer_h: 0.2, bead_xsec: None };
    let mut cap = Capture::start("127.0.0.1", 49516, map, None, hooks).unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let hdr = "<14>1 - AA buddy - - - msg=1,tm=1,v=4 ";
    // 0.20 -> first layer (no band), 1.0 -> band 0, repeat band 0 (must not
    // re-fire), 2.6 -> band 1, 4.2 -> band 2
    for z in ["0.200000", "1.000000", "1.200000", "2.600000", "4.200000"] {
        tx.send_to(format!("{hdr}pos_z v={z} 1\n").as_bytes(), "127.0.0.1:49516").unwrap();
        std::thread::sleep(Duration::from_millis(30));
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if seen.lock().unwrap().len() == 3 {
            break;
        }
        assert!(Instant::now() < deadline, "got {:?}", seen.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
    cap.stop();

    let got = seen.lock().unwrap().clone();
    assert_eq!(got[0].band, 0);
    assert_eq!(got[0].flow, 8.0);
    assert_eq!(got[1].band, 1);
    assert_eq!(got[1].flow, 10.0);
    assert_eq!(got[2].band, 2);
    assert_eq!(got[2].flow, 12.0);
    // staying inside a band must not re-fire
    assert_eq!(got.len(), 3);
}

#[test]
fn logger_hook_reports_first_packet_and_band_transitions() {
    let seen: Arc<Mutex<Vec<(&'static str, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let logger: LogFn = Arc::new(move |level, msg| sink.lock().unwrap().push((level, msg)));

    let map = BandMap { flows: vec![8., 10.], per_cylinder_flows: Vec::new(), revs: 4, layer_h: 0.4, first_layer_h: 0.2, bead_xsec: None };
    let mut cap = Capture::start("127.0.0.1", 49515, map, None, CaptureHooks::with_logger(logger)).unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let hdr = "<14>1 - AA buddy - - - msg=1,tm=1,v=4 ";
    tx.send_to(format!("{hdr}pos_z v=1.000000 1\nloadcell_value v=1.5 2\n").as_bytes(), "127.0.0.1:49515").unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let logged = seen.lock().unwrap();
        if logged.iter().any(|(_, m)| m.contains("first UDP packet")) {
            assert!(logged.iter().any(|(lvl, m)| *lvl == "trace" && m.contains("entered band 1")));
            break;
        }
        drop(logged);
        assert!(Instant::now() < deadline, "logger hook never fired");
        std::thread::sleep(Duration::from_millis(20));
    }
    cap.stop();
    // stop() itself logs an "info" summary line.
    assert!(seen.lock().unwrap().iter().any(|(lvl, m)| *lvl == "info" && m.contains("capture stopped")));
}
