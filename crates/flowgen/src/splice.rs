// splice.rs -- where the machine start/end blocks come from.
//
// The generator never writes its own homing/levelling/purge sequence for a
// real run: it lifts the start and end blocks out of a PrusaSlicer export,
// because that file is the only trustworthy source of this machine's chamber
// handling, nozzle-clean routine and loadcell prep. The hand-written
// standalone fallback exists for tests and for machines without an export,
// and says so loudly in the emitted G-code.

use std::fs;

use crate::config::Cfg;

/// Pull the start and end blocks out of a PrusaSlicer-exported G-code file.
pub(crate) fn split_reference(path: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();

    let mut start_idx: Option<usize> = None;
    for (i, ln) in lines.iter().enumerate() {
        let s = ln.trim();
        let hit = s.starts_with(";LAYER_CHANGE")
            || s.starts_with(";AFTER_LAYER_CHANGE")
            || s.starts_with(";TYPE:");
        if hit && !s.starts_with(";TYPE:Custom") {
            start_idx = Some(i);
            break;
        }
    }
    let start_idx = start_idx.ok_or_else(|| {
        format!(
            "{path}: no ;LAYER_CHANGE / ;TYPE: marker, cannot find the end of the start \
             block. Export from PrusaSlicer with default G-code comments, or use \
             standalone mode."
        )
    })?;

    let mut end_idx = lines
        .iter()
        .rposition(|l| l.contains("Filament-specific end gcode"));
    if end_idx.is_none() {
        end_idx = lines.iter().rposition(|l| l.trim().starts_with(";TYPE:Custom"));
    }
    let end_idx = match end_idx {
        Some(e) if e > start_idx => e,
        _ => return Err(format!("{path}: could not locate the end block. Use standalone mode.")),
    };

    Ok((lines[..start_idx].to_vec(), lines[end_idx..].to_vec()))
}

/// Hand-written init. NOT taken from a Prusa profile -- review before running.
pub(crate) fn standalone_blocks(c: &Cfg) -> (Vec<String>, Vec<String>) {
    let start = vec![
        "; ---- HAND-WRITTEN START BLOCK, REVIEW BEFORE RUNNING ----".to_string(),
        "; Not taken from a Prusa profile. It does NOT include the chamber/vent".to_string(),
        "; handling, nozzle-clean routine or loadcell prep that PrusaSlicer emits".to_string(),
        "; for this machine. Replace everything down to END with your own start".to_string(),
        "; block, or regenerate with a PrusaSlicer reference export.".to_string(),
        "M17".to_string(),
        format!("M862.1 P{}", c.nozzle),
        "M107".to_string(),
        "M104 S170".to_string(),
        format!("M140 S{}", c.bed),
        format!("M190 S{}", c.bed),
        "M109 S170".to_string(),
        "G28".to_string(),
        "G29".to_string(),
        "G21".to_string(),
        "G90".to_string(),
        "M83".to_string(),
        "G92 E0".to_string(),
        "; ---- END HAND-WRITTEN START BLOCK ----".to_string(),
    ];
    let end = vec![
        "; ---- HAND-WRITTEN END BLOCK ----".to_string(),
        "M107".to_string(),
        "M104 S0".to_string(),
        "M140 S0".to_string(),
        "G1 E-2 F2400".to_string(),
        format!("G1 Z{:.2} F600", c.safe_z + 30.0),
        format!("G1 X10 Y{:.1} F6000", c.bed_y - 20.0),
        "M84".to_string(),
    ];
    (start, end)
}
