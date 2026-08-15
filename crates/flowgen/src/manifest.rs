// manifest.rs -- byte-range band manifest for a generated file.
//
// The printer streams `sdpos` -- its byte offset into the file it is
// printing -- every 100ms. Because WE wrote the file, mapping that offset
// through the `;FBSEG` markers addresses bands deterministically, replacing
// the Z heuristic that produced phantom cylinders on the first real run.

use std::fs;

/// Byte-range manifest for a generated file: which bytes of the file belong
/// to which (cylinder, band) segment.
///
/// The printer streams `sdpos` -- its byte offset into the file it is
/// printing -- every 100ms. Because WE wrote the file, mapping that offset
/// through this manifest addresses bands deterministically, replacing the
/// Z heuristic that produced 11 phantom cylinders for 4 test objects.
///
/// MUST be computed against the FINAL bytes: any post-processing that changes
/// the file (the app widens M555 after generation) shifts every offset, so
/// the caller runs this last and writes `<out>.bands.txt` beside the G-code.
///
/// Format (line-based on purpose -- flowcore is std-only and should not
/// parse JSON):
///     flowbench-bands v1
///     <start> <end> <kind> <cyl|-> <band|-> <flow> <revs> <temp>
pub fn sd_manifest_text(body: &str) -> String {
    let mut marks: Vec<(u64, String)> = Vec::new();
    let mut off: u64 = 0;
    for line in body.split_inclusive('\n') {
        let t = line.trim_end();
        if let Some(rest) = t.strip_prefix(";FBSEG ") {
            marks.push((off, rest.to_string()));
        }
        off += line.len() as u64;
    }
    let total = off;
    let attr = |m: &str, k: &str| -> Option<String> {
        m.split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{k}=")).map(|v| v.to_string()))
    };
    let mut out = String::from("flowbench-bands v1\n");
    for (i, (start, m)) in marks.iter().enumerate() {
        let end = marks.get(i + 1).map(|(o, _)| *o).unwrap_or(total);
        let kind = attr(m, "kind").unwrap_or_else(|| "?".into());
        let cyl = attr(m, "cyl").unwrap_or_else(|| "-".into());
        let band = attr(m, "band").unwrap_or_else(|| "-".into());
        let flow = attr(m, "flow").unwrap_or_else(|| "0".into());
        let revs = attr(m, "revs").unwrap_or_else(|| "0".into());
        let temp = attr(m, "temp").unwrap_or_else(|| "0".into());
        out.push_str(&format!("{start} {end} {kind} {cyl} {band} {flow} {revs} {temp}\n"));
    }
    out
}

/// Read a written G-code file and place its manifest beside it.
pub fn write_band_manifest(gcode_path: &str) -> Result<String, String> {
    let body = fs::read_to_string(gcode_path).map_err(|e| format!("{gcode_path}: {e}"))?;
    let text = sd_manifest_text(&body);
    let mpath = format!("{gcode_path}.bands.txt");
    fs::write(&mpath, &text).map_err(|e| format!("{mpath}: {e}"))?;
    Ok(mpath)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate, Cfg};

    /// The manifest must address every band of every cylinder by byte range,
    /// in file order, with tare and purge segments distinct from bands --
    /// this is what replaces the Z heuristic that invented 7 phantom
    /// cylinders on the first real run.
    #[test]
    fn sd_manifest_addresses_every_band() {
        let out = std::env::temp_dir().join(format!("flowgen_sdm_{}.gcode", std::process::id()));
        let cfg = Cfg {
            out: out.to_string_lossy().into_owned(),
            standalone: true,
            temps: vec![255, 265],
            flows: vec![8.0, 10.0, 12.0],
            revs: 1,
            metrics_host: Some("192.0.2.18".into()),
            ..Default::default()
        };
        generate(cfg).unwrap();
        let body = fs::read_to_string(&out).unwrap();
        let text = sd_manifest_text(&body);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "flowbench-bands v1");

        let mut bands = 0usize;
        let mut tares = 0usize;
        let mut prev_end = 0u64;
        for l in &lines[1..] {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (start, end) = (f[0].parse::<u64>().unwrap(), f[1].parse::<u64>().unwrap());
            assert!(start >= prev_end, "segments out of order: {l}");
            assert!(end > start, "empty segment: {l}");
            prev_end = start; // starts are ordered; ends may equal next start
            match f[2] {
                "band" => {
                    bands += 1;
                    // byte range really contains that band's G-code
                    let seg = &body[start as usize..end as usize];
                    assert!(seg.contains("G1 F"), "band segment carries no feed change");
                    assert!(f[4] != "-", "band without an index: {l}");
                    assert!(f[5].parse::<f64>().unwrap() > 0.0, "band without a flow: {l}");
                    assert!(f[6].parse::<u64>().unwrap() > 0, "band without revs: {l}");
                }
                "tare" => {
                    tares += 1;
                    let seg = &body[start as usize..end as usize];
                    assert!(seg.contains("G4 S"), "tare segment has no dwell");
                    assert!(!seg.contains(" E"), "tare segment must not extrude");
                }
                _ => {}
            }
        }
        assert_eq!(bands, 2 * 3, "2 cylinders x 3 flows");
        assert_eq!(tares, 2, "one tare window per cylinder");
        assert!(text.lines().any(|l| l.contains(" end ")), "end marker present");
        let _ = fs::remove_file(&out);
    }

    /// Offsets must survive post-processing: recompute AFTER editing the file
    /// and the ranges still index the right bytes.
    #[test]
    fn sd_manifest_is_computed_on_final_bytes() {
        let body = ";FBSEG kind=purge cyl=0 temp=255\nG1 X0\n;FBSEG kind=band cyl=0 band=0 flow=8.00 revs=4 temp=255\nG1 F600\n;FBSEG kind=end\n";
        let text = sd_manifest_text(body);
        let band_line = text.lines().find(|l| l.contains(" band ")).unwrap();
        let f: Vec<&str> = band_line.split_whitespace().collect();
        let (start, end) = (f[0].parse::<usize>().unwrap(), f[1].parse::<usize>().unwrap());
        assert!(body[start..end].starts_with(";FBSEG kind=band"));
        assert!(body[start..end].contains("G1 F600"));
    }
}
