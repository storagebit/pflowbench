// Wire-format parsing. Verified against Prusa-Firmware-Buddy
// src/common/metric_handlers.cpp (textprotocol_append_point / syslog_message_init):
//   <14>1 - <MAC> buddy - - - msg=17,tm=1234567890,v=4 <points>
//   loadcell_value v=-3.421000 812
//   pos_z v=4.213000 1190
// Field key is `v` for ordinary metrics, but CUSTOM ones carry their own
// formatting -- temp_noz emits `temp_noz,n=0,a=1 value=285.00 <us>`, so both
// `v=` and `value=` must be accepted. Integers carry a trailing `i`. The
// trailing number is each point's own microsecond offset from the packet's
// tm= reference (when it was RECORDED, not when the datagram arrived).

/// Strip the RFC5424-ish syslog header, returning the point payload.
pub fn strip_header(pkt: &str) -> &str {
    strip_header_tm(pkt).0
}

/// Body plus the packet's `tm=` reference in MICROSECONDS.
///
/// Buddy writes `msg=<n>,tm=<absolute_timestamp_us>,v=4 ` and then stamps each
/// point with its own offset from that reference (see `parse_point`). Keeping
/// `tm` is what makes those per-point offsets usable as a real sample clock.
/// Verified against Prusa-Firmware-Buddy `src/common/metric_handlers.cpp`
/// (`syslog_message_init` / `metric_handler`).
pub fn strip_header_tm(pkt: &str) -> (&str, Option<i64>) {
    let Some(p) = pkt.find(",v=4 ") else { return (pkt, None) };
    let body = &pkt[p + 5..];
    let tm = pkt[..p].rfind("tm=").and_then(|i| {
        let rest = &pkt[i + 3..];
        let end = rest.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(rest.len());
        rest[..end].parse::<i64>().ok()
    });
    (body, tm)
}

/// One point: metric name, float value, and its microsecond offset from the
/// packet's `tm=` reference.
///
/// Line grammar (Influx line protocol, as Buddy emits it):
///     name[,tag=v[,tag=v]] field=value[,field=value] <offset_us>
///
/// Two things this has to get right:
///
/// * The value key is `v` for the ordinary float/int/string metrics, but
///   CUSTOM metrics carry whatever the firmware formatted -- and `temp_noz`
///   uses `,n=<tool>,a=<active> value=<deg>`. Scanning for a bare `v=`
///   substring finds nothing in `value=`, so every nozzle-temperature point
///   was silently dropped and the live readout sat at a fabricated 0.0 C.
///   Fields are therefore split properly and BOTH keys accepted.
///
/// * The trailing integer is `ticks_diff(point->timestamp, buffer_reference)`
///   -- when the sample was RECORDED, not when the datagram arrived. Buddy
///   batches many points per datagram, so timing them by arrival collapses a
///   whole batch onto one instant.
pub fn parse_point(line: &str) -> Option<(&str, f64, i64)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // `name[,tags]` up to the first space; the rest is `fields [timestamp]`.
    let (head, rest) = line.split_once(' ')?;
    let name = head.split(',').next().filter(|s| !s.is_empty())?;

    // The timestamp is the last space-separated token, when present.
    let (fields, offset) = match rest.rsplit_once(' ') {
        Some((f, ts)) => (f, ts.trim().parse::<i64>().unwrap_or(0)),
        None => (rest, 0),
    };

    for kv in fields.split(',') {
        let Some((k, val)) = kv.split_once('=') else { continue };
        if k != "v" && k != "value" {
            continue;
        }
        // Integers are written with a trailing `i` (`v=420i`).
        let tok = val.strip_suffix('i').unwrap_or(val);
        if let Ok(v) = tok.parse::<f64>() {
            // A non-finite reading is an absent measurement, not a data point:
            // letting NaN through puts the literal string "NaN" in the CSV and
            // poisons any mean computed over the series.
            if v.is_finite() {
                return Some((name, v, offset));
            }
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_float_int_and_header() {
        let pkt = "<14>1 - AABBCC buddy - - - msg=1,tm=1700000000,v=4 loadcell_value v=-3.421000 812\nfan_print v=420i 14\n";
        let (body, tm) = strip_header_tm(pkt);
        assert_eq!(tm, Some(1_700_000_000), "tm is the packet's microsecond reference");
        let pts: Vec<_> = body.split('\n').filter_map(parse_point).collect();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].0, "loadcell_value");
        assert!((pts[0].1 + 3.421).abs() < 1e-9);
        assert_eq!(pts[0].2, 812, "per-point microsecond offset is kept");
        assert_eq!(pts[1], ("fan_print", 420.0, 14));
    }

    /// The exact line Buddy emits for nozzle temperature. `temp_noz` is a
    /// METRIC_VALUE_CUSTOM whose payload is `,n=<tool>,a=<active> value=<deg>`
    /// -- there is no `v=` anywhere in it, so a substring search for one found
    /// nothing and every nozzle sample was dropped on the floor. The live
    /// readout then showed a fabricated 0.0 C for the whole run.
    #[test]
    fn parses_the_custom_nozzle_temperature_point() {
        let (name, v, off) = parse_point("temp_noz,n=0,a=1 value=285.00 1234").expect("must parse");
        assert_eq!(name, "temp_noz");
        assert!((v - 285.0).abs() < 1e-9);
        assert_eq!(off, 1234);

        // the integer-valued custom sibling, `value=%ii`
        let (name, v, _) = parse_point("ttemp_noz,n=0,a=1 value=290i 7").unwrap();
        assert_eq!((name, v), ("ttemp_noz", 290.0));

        // and a tagged point that genuinely has no value field stays rejected
        assert!(parse_point("dock_pos,n=1 x=1,y=2 99").is_none());
    }

    #[test]
    fn rejects_custom_and_garbage() {
        assert!(parse_point("loadcell r=1.0,a=2 99").is_none()); // no v= / value=
        assert!(parse_point("").is_none());
        assert!(parse_point("v=").is_none());
        assert!(parse_point("   ").is_none());
    }

    /// A missing reading must be an ABSENT sample, not a NaN one: NaN reaches
    /// the CSV as the literal "NaN" and poisons any mean taken over the series.
    #[test]
    fn rejects_non_finite_readings() {
        assert!(parse_point("chamber_temp v=nan 5").is_none());
        assert!(parse_point("chamber_temp v=inf 5").is_none());
        assert!(parse_point("chamber_temp v=-inf 5").is_none());
        assert!(parse_point("chamber_temp v=41.6 5").is_some());
    }

    /// Points in one datagram carry DIFFERENT microsecond offsets. Timing them
    /// by arrival collapses them onto a single instant, which is what biased
    /// the derived head speed low on every run.
    #[test]
    fn batched_points_keep_distinct_timestamps() {
        let pkt = "<14>1 - AABBCC buddy - - - msg=9,tm=5000000,v=4 pos_z v=1.0 100\npos_z v=1.5 5100\npos_z v=2.0 10100\n";
        let (body, tm) = strip_header_tm(pkt);
        let tm = tm.unwrap();
        let ts: Vec<i64> = body.split('\n').filter_map(parse_point).map(|(_, _, o)| tm + o).collect();
        assert_eq!(ts, vec![5_000_100, 5_005_100, 5_010_100]);
        // 5ms apart, not simultaneous
        assert_eq!(ts[1] - ts[0], 5_000);
        assert_eq!(ts[2] - ts[1], 5_000);
    }

    #[test]
    fn header_without_tm_still_yields_a_body() {
        let (body, tm) = strip_header_tm("no header here loadcell_value v=1.0 1");
        assert!(tm.is_none());
        assert!(body.contains("loadcell_value"));
    }
}
