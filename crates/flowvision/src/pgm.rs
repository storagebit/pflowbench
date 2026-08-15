// Binary (P5) PGM parse/load -- the still format flowcam writes.

use std::fs;
use std::path::Path;

/// A parsed binary (P5) PGM image, as written by flowcam.
pub struct Pgm {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Pgm {
    pub fn parse(bytes: &[u8]) -> Result<Pgm, String> {
        // header: "P5" <ws> width <ws> height <ws> maxval <single ws> data
        if bytes.len() < 2 || &bytes[..2] != b"P5" {
            return Err("not a P5 PGM".into());
        }
        let mut pos = 2;
        let mut fields = [0usize; 3];
        for f in fields.iter_mut() {
            // skip whitespace and '#' comment lines
            loop {
                while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                if pos < bytes.len() && bytes[pos] == b'#' {
                    while pos < bytes.len() && bytes[pos] != b'\n' {
                        pos += 1;
                    }
                } else {
                    break;
                }
            }
            let start = pos;
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
            *f = std::str::from_utf8(&bytes[start..pos])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or("bad PGM header field")?;
        }
        let (w, h, maxval) = (fields[0], fields[1], fields[2]);
        if maxval != 255 {
            return Err(format!("unsupported maxval {maxval}"));
        }
        if w == 0 || h == 0 {
            return Err(format!("degenerate {w}x{h} image"));
        }
        pos += 1; // the single whitespace byte after maxval
        let need = w * h;
        if bytes.len() < pos + need {
            return Err(format!("truncated PGM: need {need} bytes, have {}", bytes.len() - pos));
        }
        Ok(Pgm { w, h, data: bytes[pos..pos + need].to_vec() })
    }

    pub fn load(path: &Path) -> Result<Pgm, String> {
        Self::parse(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
    }

    pub(crate) fn at(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.w + x]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgm_parses_header_comments_and_data() {
        let mut bytes = b"P5\n# a comment\n4 2\n255\n".to_vec();
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let p = Pgm::parse(&bytes).unwrap();
        assert_eq!((p.w, p.h), (4, 2));
        assert_eq!(p.at(3, 1), 8);
        assert!(Pgm::parse(b"P6 no").is_err());
    }
}
