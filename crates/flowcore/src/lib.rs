// flowcore -- parse Buddy metrics, bin loadcell force into flow bands, run capture.
//
// std-only. One concern per module:
//   parse     wire-format decoding (syslog-ish header, metric points)
//   sdmap     sdpos -> segment map from flowgen's band manifest
//   bandmap   Z-window band addressing and the legacy cylinder heuristic
//   stats     running mean/sd accumulator
//   capture   the UDP capture session (hooks, state, snapshot, receive loop)
//
// The re-exports below keep every name reachable at the crate root, exactly
// as before the split -- consumers write `flowcore::Capture`, not module paths.

mod bandmap;
mod capture;
mod parse;
mod sdmap;
mod stats;

pub use bandmap::{BandMap, CylTracker};
pub use capture::{
    BandChange, BandChangeFn, BandStat, Capture, CaptureHooks, CaptureState, LogFn, Snapshot,
    TRAVEL_SPEED_CUTOFF,
};
pub use parse::{parse_point, strip_header, strip_header_tm};
pub use sdmap::{SdMap, SdSeg, SegKind, SD_GUARD_BYTES};
pub use stats::Acc;
