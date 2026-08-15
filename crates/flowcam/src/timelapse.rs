// Timelapse muxing: remux retained H.264 keyframe access units into an MP4,
// no decode and no re-encode.

use crate::decode::{nal_type, split_annex_b};

/// What `write_timelapse_mp4` produced.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelapseInfo {
    pub frames: usize,
    pub width: u16,
    pub height: u16,
    pub fps: u32,
    pub bytes: u64,
    /// Wall-clock span compressed into the video, if known.
    pub speedup: Option<f64>,
}

/// Remux retained keyframes into an H.264/MP4 at `fps`.
///
/// No decoding and no re-encoding: every retained access unit is already a
/// self-contained IDR, so the original frames are copied verbatim (lossless)
/// and simply given new timestamps. Playing 3-second-apart frames at 25fps is
/// what produces the timelapse effect -- ~75x here.
///
/// MP4 wants AVCC (4-byte length-prefixed) NALs while the stream is captured
/// as Annex-B, so each unit is re-framed on the way in. SPS/PPS travel inline
/// on every keyframe (FrameFormat::SIMPLE) and are lifted out into the avcC
/// sample-entry, as the container expects.
pub fn write_timelapse_mp4(
    path: &std::path::Path,
    frames: &[Vec<u8>],
    width: u16,
    height: u16,
    fps: u32,
    source_span_secs: Option<f64>,
) -> Result<TimelapseInfo, String> {
    use bytes::Bytes;
    use mp4::{AvcConfig, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType};

    if frames.is_empty() {
        return Err("no frames recorded -- was the camera connected during the run?".into());
    }
    let fps = fps.max(1);

    // Parameter sets from the first keyframe that carries them.
    let (mut sps, mut pps) = (Vec::new(), Vec::new());
    for au in frames {
        for nal in split_annex_b(au) {
            match nal_type(nal) {
                7 if sps.is_empty() => sps = nal.to_vec(),
                8 if pps.is_empty() => pps = nal.to_vec(),
                _ => {}
            }
        }
        if !sps.is_empty() && !pps.is_empty() {
            break;
        }
    }
    if sps.is_empty() || pps.is_empty() {
        return Err("no SPS/PPS found in the recorded stream".into());
    }

    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let writer = std::io::BufWriter::new(f);
    let mut mp4 = Mp4Writer::write_start(
        writer,
        &Mp4Config {
            major_brand: str::parse("isom").unwrap(),
            minor_version: 512,
            compatible_brands: vec![
                str::parse("isom").unwrap(),
                str::parse("iso2").unwrap(),
                str::parse("avc1").unwrap(),
                str::parse("mp41").unwrap(),
            ],
            timescale: 1000,
        },
    )
    .map_err(|e| format!("mp4 start: {e}"))?;

    mp4.add_track(&TrackConfig {
        track_type: TrackType::Video,
        timescale: fps * 1000,
        language: "und".into(),
        media_conf: MediaConfig::AvcConfig(AvcConfig {
            width,
            height,
            seq_param_set: sps,
            pic_param_set: pps,
        }),
    })
    .map_err(|e| format!("mp4 add_track: {e}"))?;

    // One sample per keyframe, each lasting exactly 1/fps.
    let dur = 1000u32;
    let mut written = 0usize;
    for (i, au) in frames.iter().enumerate() {
        let mut avcc = Vec::with_capacity(au.len() + 16);
        for nal in split_annex_b(au) {
            // Parameter sets live in avcC, not in the sample data.
            if matches!(nal_type(nal), 7 | 8) {
                continue;
            }
            avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            avcc.extend_from_slice(nal);
        }
        if avcc.is_empty() {
            continue;
        }
        mp4.write_sample(
            1,
            &Mp4Sample {
                start_time: (i as u64) * dur as u64,
                duration: dur,
                rendering_offset: 0,
                is_sync: true, // every retained frame is an IDR
                bytes: Bytes::from(avcc),
            },
        )
        .map_err(|e| format!("mp4 write_sample: {e}"))?;
        written += 1;
    }
    mp4.write_end().map_err(|e| format!("mp4 write_end: {e}"))?;

    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let speedup = source_span_secs.and_then(|s| {
        let out = written as f64 / fps as f64;
        (out > 0.0).then_some(s / out)
    });
    Ok(TimelapseInfo { frames: written, width, height, fps, bytes, speedup })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Camera, LogFn};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn timelapse_refuses_an_empty_or_parameterless_reel() {
        let p = std::env::temp_dir().join("flowcam_empty.mp4");
        assert!(write_timelapse_mp4(&p, &[], 1920, 1080, 25, None)
            .unwrap_err()
            .contains("no frames"));
        // frames present but carrying no SPS/PPS
        let junk = vec![vec![0, 0, 0, 1, 0x41, 0x9A]]; // non-IDR slice only
        assert!(write_timelapse_mp4(&p, &junk, 1920, 1080, 25, None)
            .unwrap_err()
            .contains("SPS/PPS"));
        let _ = std::fs::remove_file(&p);
    }

    /// Builds a synthetic reel, muxes it, then reads the file back with the
    /// same library to confirm the container is structurally valid.
    #[test]
    fn timelapse_mp4_round_trips() {
        // Real SPS/PPS captured from the Buddy3D (1920x1080 Baseline L4.0),
        // so the avcC sample entry is built from genuine parameter sets.
        let sps = [
            0x67, 0x42, 0xC0, 0x28, 0x8D, 0x8D, 0x50, 0x3C, 0x01, 0x12, 0xF2, 0xCD, 0xC0, 0x40,
            0x40, 0x50, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0x28, 0xF0, 0x88,
            0x46, 0xA0,
        ];
        let pps = [0x68, 0xCF, 0x3C, 0x80];
        let mut au = vec![0, 0, 0, 1];
        au.extend_from_slice(&sps);
        au.extend_from_slice(&[0, 0, 0, 1]);
        au.extend_from_slice(&pps);
        au.extend_from_slice(&[0, 0, 0, 1]);
        au.extend_from_slice(&[0x65, 0x88, 0x84, 0x00]); // IDR slice payload
        let frames = vec![au.clone(), au.clone(), au];

        let p = std::env::temp_dir().join(format!("flowcam_tl_{}.mp4", std::process::id()));
        let info = write_timelapse_mp4(&p, &frames, 1920, 1080, 25, Some(9.0)).unwrap();
        assert_eq!(info.frames, 3);
        assert_eq!((info.width, info.height), (1920, 1080));
        assert!(info.bytes > 0);
        // 3 frames of real time 9s played at 25fps = 0.12s -> 75x
        assert!((info.speedup.unwrap() - 75.0).abs() < 0.01, "{:?}", info.speedup);

        // read it back: a valid MP4 with one 1920x1080 video track of 3 samples
        let f = std::fs::File::open(&p).unwrap();
        let size = f.metadata().unwrap().len();
        let r = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).unwrap();
        let track = r.tracks().values().next().expect("a track");
        assert_eq!(track.width(), 1920);
        assert_eq!(track.height(), 1080);
        assert_eq!(track.sample_count(), 3);
        let _ = std::fs::remove_file(&p);
    }

    /// End-to-end against the real camera: stream, record a few keyframes,
    /// and mux them into a playable timelapse.
    #[test]
    #[ignore]
    fn live_camera_records_a_timelapse() {
        let host = std::env::var("FLOWCAM_HOST").unwrap_or_else(|_| "192.0.2.20".into());
        let logger: LogFn = Arc::new(|lvl, msg| println!("[{lvl}] {msg}"));
        let cam = Camera::start(&host, Some(logger)).expect("start");
        cam.start_recording();

        // keyframes arrive ~every 3s, so allow room for a few
        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        while cam.recorded_frames() < 3 {
            assert!(std::time::Instant::now() < deadline,
                    "only got {} frame(s)", cam.recorded_frames());
            std::thread::sleep(Duration::from_millis(250));
        }
        cam.stop_recording();

        let p = std::env::temp_dir().join("flowcam_live_timelapse.mp4");
        let info = cam.write_timelapse(&p, 25, Some(9.0)).expect("write timelapse");
        println!("wrote {} -> {info:?}", p.display());
        assert!(info.frames >= 3);
        assert_eq!((info.width, info.height), (1920, 1080));
        assert!(info.bytes > 10_000, "suspiciously small: {} bytes", info.bytes);
    }
}
