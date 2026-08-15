// Pixel and NAL helpers: 2x RGB downscale, JPEG encoding, Annex-B
// access-unit splitting. Pure functions over byte buffers -- no camera or
// network state.

use std::io;

/// Exact 2x box downscale of an RGB8 buffer. 1920x1080 -> 960x540, which is
/// plenty for the live GUI panel at roughly a quarter the JPEG bytes. Hand
/// written to avoid pulling in an image-processing crate for one operation;
/// odd dimensions are truncated, which is fine for the preview.
pub fn downscale2x_rgb(src: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let (dw, dh) = (w / 2, h / 2);
    let mut out = vec![0u8; dw * dh * 3];
    for y in 0..dh {
        for x in 0..dw {
            let (sx, sy) = (x * 2, y * 2);
            for c in 0..3 {
                let p = |yy: usize, xx: usize| src[(yy * w + xx) * 3 + c] as u32;
                let sum = p(sy, sx) + p(sy, sx + 1) + p(sy + 1, sx) + p(sy + 1, sx + 1);
                out[(y * dw + x) * 3 + c] = (sum / 4) as u8;
            }
        }
    }
    (out, dw, dh)
}

pub(crate) fn encode_jpeg(rgb: &[u8], w: usize, h: usize, quality: u8) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    jpeg_encoder::Encoder::new(&mut buf, quality)
        .encode(rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("jpeg encode: {e}")))?;
    Ok(buf)
}

/// Split an Annex-B access unit into its NAL units, dropping start codes.
/// Handles both 3-byte (00 00 01) and 4-byte (00 00 00 01) start codes.
pub fn split_annex_b(au: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= au.len() {
        if au[i] == 0 && au[i + 1] == 0 {
            if au[i + 2] == 1 {
                starts.push((i, 3));
                i += 3;
                continue;
            } else if i + 4 <= au.len() && au[i + 2] == 0 && au[i + 3] == 1 {
                starts.push((i, 4));
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &(pos, len)) in starts.iter().enumerate() {
        let begin = pos + len;
        let end = starts.get(k + 1).map(|&(p, _)| p).unwrap_or(au.len());
        if end > begin {
            out.push(&au[begin..end]);
        }
    }
    out
}

pub(crate) fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map(|b| b & 0x1F).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_annex_b_handles_both_start_code_lengths() {
        // 4-byte, then 3-byte start code
        let au = [0, 0, 0, 1, 0x67, 0xAA, 0, 0, 1, 0x68, 0xBB, 0xCC];
        let nals = split_annex_b(&au);
        assert_eq!(nals.len(), 2);
        assert_eq!(nals[0], &[0x67, 0xAA]);
        assert_eq!(nals[1], &[0x68, 0xBB, 0xCC]);
        assert_eq!(nal_type(nals[0]), 7, "SPS");
        assert_eq!(nal_type(nals[1]), 8, "PPS");
    }

    #[test]
    fn split_annex_b_on_junk_yields_nothing() {
        assert!(split_annex_b(&[]).is_empty());
        assert!(split_annex_b(&[1, 2, 3, 4]).is_empty());
        // start code with no payload after it
        assert!(split_annex_b(&[0, 0, 0, 1]).is_empty());
    }

    #[test]
    fn downscale_averages_each_2x2_block() {
        // 2x2 image, one block: values 0,10,20,30 per channel -> mean 15.
        let src = vec![
            0, 0, 0, 10, 10, 10, //
            20, 20, 20, 30, 30, 30,
        ];
        let (out, w, h) = downscale2x_rgb(&src, 2, 2);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![15, 15, 15]);
    }

    #[test]
    fn downscale_halves_dimensions_and_truncates_odd() {
        let src = vec![0u8; 5 * 3 * 3]; // 5x3 -> 2x1
        let (out, w, h) = downscale2x_rgb(&src, 5, 3);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out.len(), 2 * 1 * 3);
    }

    #[test]
    fn downscale_preserves_a_flat_colour() {
        let (w, h) = (8, 8);
        let src: Vec<u8> = std::iter::repeat([200u8, 100, 50]).take(w * h).flatten().collect();
        let (out, dw, dh) = downscale2x_rgb(&src, w, h);
        assert_eq!((dw, dh), (4, 4));
        for px in out.chunks_exact(3) {
            assert_eq!(px, [200, 100, 50]);
        }
    }

    #[test]
    fn jpeg_encoder_produces_a_valid_header() {
        let rgb = vec![128u8; 16 * 16 * 3];
        let jpeg = encode_jpeg(&rgb, 16, 16, 80).unwrap();
        // SOI marker, and EOI at the end
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9]);
    }
}
