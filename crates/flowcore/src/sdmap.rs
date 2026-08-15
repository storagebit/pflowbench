// sdpos -> segment map: what each byte range of the printed file IS, parsed
// from the band manifest flowgen writes next to the G-code. Because flowgen
// wrote the file, this addresses cylinders and bands deterministically.

/// What a byte range of the printed file IS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SegKind {
    Travel,
    Tare,
    Purge,
    First,
    Band,
    Photo,
    End,
    Other,
}

/// One segment of the printed file, from flowgen's `;FBSEG` markers.
#[derive(Clone, Debug)]
pub struct SdSeg {
    pub start: u64,
    pub end: u64,
    pub kind: SegKind,
    pub cylinder: usize,
    /// Band index within the cylinder; only meaningful for `Band`.
    pub band: usize,
    pub flow: f64,
    pub revs: u64,
    pub temp: i64,
    /// Byte offset after which the band is considered settled (first
    /// revolution excluded -- melt pressure needs seconds to step).
    pub settle: u64,
}

/// sdpos -> segment map, parsed from the `<out>.bands.txt` manifest flowgen
/// writes next to the G-code.
///
/// `sdpos` is the printer's byte offset into the file it is printing
/// (Buddy metric, enabled by default, 100ms interval). Because flowgen wrote
/// the file, this map addresses cylinders and bands DETERMINISTICALLY --
/// replacing the Z heuristic that counted 11 cylinders for 4 test objects and
/// polluted band 1 with brim samples.
#[derive(Clone, Debug, Default)]
pub struct SdMap {
    pub segs: Vec<SdSeg>,
}

/// sdpos reports the READ position, which leads execution by the firmware's
/// prefetch. Subtracting this before lookup biases attribution backward --
/// conservative, and residual smear is covered by each band's settle window.
pub const SD_GUARD_BYTES: u64 = 2048;

impl SdMap {
    pub fn parse(text: &str) -> Result<SdMap, String> {
        let mut lines = text.lines();
        match lines.next() {
            Some(h) if h.trim() == "flowbench-bands v1" => {}
            other => return Err(format!("not a flowbench-bands v1 manifest: {other:?}")),
        }
        let mut segs = Vec::new();
        for l in lines {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 8 {
                continue;
            }
            let num = |i: usize| f[i].parse::<u64>().map_err(|e| format!("{l}: {e}"));
            let (start, end) = (num(0)?, num(1)?);
            let kind = match f[2] {
                "travel" => SegKind::Travel,
                "tare" => SegKind::Tare,
                "purge" => SegKind::Purge,
                "first" => SegKind::First,
                "band" => SegKind::Band,
                "photo" => SegKind::Photo,
                "end" => SegKind::End,
                _ => SegKind::Other,
            };
            let cylinder = f[3].parse::<usize>().unwrap_or(0);
            let band = f[4].parse::<usize>().unwrap_or(0);
            let flow = f[5].parse::<f64>().unwrap_or(0.0);
            let revs = f[6].parse::<u64>().unwrap_or(0);
            let temp = f[7].parse::<i64>().unwrap_or(0);
            let settle = if kind == SegKind::Band && revs > 0 {
                start + (end - start) / revs
            } else {
                start
            };
            segs.push(SdSeg { start, end, kind, cylinder, band, flow, revs, temp, settle });
        }
        segs.sort_by_key(|s| s.start);
        if segs.iter().all(|s| s.kind != SegKind::Band) {
            return Err("manifest contains no band segments".into());
        }
        Ok(SdMap { segs })
    }

    /// True when the file was generated with photo windows -- the app picks
    /// its snapshot trigger off this.
    pub fn has_photo(&self) -> bool {
        self.segs.iter().any(|s| s.kind == SegKind::Photo)
    }

    /// Segment containing `sdpos`, after the prefetch-guard subtraction.
    pub fn locate(&self, sdpos: u64) -> Option<&SdSeg> {
        let pos = sdpos.saturating_sub(SD_GUARD_BYTES);
        let i = self.segs.partition_point(|s| s.start <= pos);
        if i == 0 {
            return None;
        }
        let seg = &self.segs[i - 1];
        (pos < seg.end).then_some(seg)
    }

    /// Temperatures per cylinder, for the verdict's cross-temperature rules.
    pub fn cylinder_temps(&self) -> Vec<(usize, i64)> {
        let mut out: Vec<(usize, i64)> = Vec::new();
        for s in self.segs.iter().filter(|s| s.kind == SegKind::Band) {
            if !out.iter().any(|(c, _)| *c == s.cylinder) {
                out.push((s.cylinder, s.temp));
            }
        }
        out
    }
}
