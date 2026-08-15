// flowcam -- pull frames from the printer's Buddy3D camera over RTSP.
//
// The camera exposes an unauthenticated H.264 stream at rtsp://<ip>/live
// (1920x1080, Baseline profile, SPS/PPS inline). Note the RTSP server is
// ON DEMAND: it only listens on :554 once local streaming is enabled in the
// Prusa App, and the port is closed (connection refused) otherwise --
// `probe()` exists to give a clear diagnosis for that case rather than a
// confusing connect error.
//
// Design: ONE session is held open for the whole print. Reconnecting per
// snapshot would cost a DESCRIBE/SETUP/PLAY round trip plus a wait for the
// next IDR keyframe -- measured at ~3.4s against this camera, since keyframes
// arrive only every ~3s. Instead the background task decodes every keyframe
// as it arrives, keeping the most recent decoded frame in memory:
//
//   * a small preview JPEG, re-encoded every keyframe, for the live GUI view
//   * the full RGB buffer, so a full-resolution JPEG can be encoded on demand
//     (at a flow-band boundary) with no decode and no waiting.
//
// std-only is not achievable here -- an H.264 decoder is six orders of
// magnitude beyond the hand-rolled MD5/HTTP in the sibling crates -- but
// openh264 builds its C from vendored source, so no system package is needed.
//
// Module map:
//   rtsp       transport choice, :554 probe, DESCRIBE-only test()
//   stream     the background DESCRIBE/SETUP/PLAY + decode loop
//   camera     shared frame state and the public Camera handle
//   decode     downscale, JPEG encoding, Annex-B NAL splitting
//   timelapse  keyframe reel -> MP4 remux

mod camera;
mod decode;
mod rtsp;
mod stream;
mod timelapse;

pub use camera::{Camera, CameraStats, Snapshot};
pub use decode::{downscale2x_rgb, split_annex_b};
pub use rtsp::{probe, test, CameraInfo, Rtsp};
pub use timelapse::{write_timelapse_mp4, TimelapseInfo};

use std::sync::Arc;

/// (level, message) sink, matching `flowcore::LogFn` and `prusalink::LogFn` so
/// the app can wire all three into the same on-screen console.
pub type LogFn = Arc<dyn Fn(&'static str, String) + Send + Sync>;

pub(crate) fn noop_logger() -> LogFn {
    Arc::new(|_level, _msg| {})
}
