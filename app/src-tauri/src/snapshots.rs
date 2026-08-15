// snapshots.rs -- run-directory naming and JPEG-to-data-URL plumbing for the
// live camera view and per-band stills.
//
// Both helpers are hand-rolled rather than pulled from crates: base64 is ~15
// lines, and the civil-date conversion avoids a chrono dependency for the sole
// purpose of naming a folder. Consistent with the MD5/HTTP already hand-rolled
// in the sibling crates.

use std::time::{SystemTime, UNIX_EPOCH};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding. Used to hand a JPEG to the webview as a
/// `data:` URL -- returning raw Vec<u8> through Tauri's IPC would serialize as
/// a JSON array of numbers, several times larger than the base64 text.
pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18 & 63) as usize] as char);
        out.push(B64[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    out
}

pub fn jpeg_data_url(jpeg: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", base64(jpeg))
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
/// Returns (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Sortable, human-readable run-directory name: `20260811-084233`.
/// UTC -- a local-time conversion would need the tz database.
pub fn run_dir_name() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, mo, d) = civil_from_days(days);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Filename for one band's still. Encodes everything needed to interpret the
/// image without consulting a sidecar: which cylinder, which band, and the
/// commanded flow rate that band was printed at.
pub fn band_image_name(cylinder: usize, band: usize, flow: f64) -> String {
    // band is 0-based internally, 1-based for display -- match the stats table.
    format!("cyl{cylinder}_band{:02}_flow{flow:.1}.jpg", band + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_high_bytes() {
        // JPEG starts with 0xFF 0xD8 -- make sure the sign/shift handling is right.
        assert_eq!(base64(&[0xFF, 0xD8, 0xFF]), "/9j/");
        assert_eq!(base64(&[0x00, 0x00, 0x00]), "AAAA");
    }

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // leap year start
        assert_eq!(civil_from_days(19_784), (2024, 3, 2)); // just past leap day
    }

    #[test]
    fn run_dir_name_is_sortable_and_well_formed() {
        let n = run_dir_name();
        assert_eq!(n.len(), 15, "{n}");
        assert_eq!(&n[8..9], "-");
        assert!(n[..8].chars().all(|c| c.is_ascii_digit()), "{n}");
        assert!(n[9..].chars().all(|c| c.is_ascii_digit()), "{n}");
        // sanity: the year should be plausible, not 1970
        let year: i32 = n[..4].parse().unwrap();
        assert!(year >= 2026, "got year {year} from {n}");
    }

    #[test]
    fn band_image_name_is_1_based_and_carries_the_flow() {
        assert_eq!(band_image_name(0, 0, 8.0), "cyl0_band01_flow8.0.jpg");
        assert_eq!(band_image_name(2, 8, 24.0), "cyl2_band09_flow24.0.jpg");
    }
}
