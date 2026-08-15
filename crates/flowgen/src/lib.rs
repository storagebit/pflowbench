// flowgen -- flow-rate / temperature cliff test generator for a Prusa CORE One L.
//
// Library port of what used to be the standalone tools/flowcliff.rs CLI. Prints one
// single-wall spiral cylinder per nozzle temperature. Each cylinder is a stack of
// bands; each band is N revolutions extruded at a fixed volumetric flow rate. Wall
// thickness is proportional to delivered volume, so you caliper each band and find
// where the wall goes thin. That Z is the cliff for that temperature.
//
// Generates raw G-code rather than going through a slicer: PrusaSlicer clamps the
// test with filament_max_volumetric_speed, slowdown_below_layer_time, min_print_speed
// and per-role speed overrides, none of which belong inside a measurement.
//
// std-only, no knowledge of Tauri -- app/src-tauri calls `generate()` directly and
// is the only consumer; this crate is tested independently via `cargo test`.
//
// One concern per module; everything a consumer may name is re-exported here,
// so the crate root stays the only public path.

mod calibrate;
mod config;
mod emit;
mod generate;
mod geometry;
mod layout;
mod manifest;
pub mod nozzle;
mod parse;
pub mod profile;
pub mod reference;
mod splice;
mod validate;

pub use calibrate::{generate_calibration, CalReport, Pillar};
pub use config::Cfg;
pub use generate::{generate, GenReport};
pub use geometry::extrusion_xsec;
pub use layout::{
    layout_fits, layout_positions, layout_preview_gcode, test_objects, FlowProgram, Layout,
    TestObject,
};
pub use manifest::{sd_manifest_text, write_band_manifest};
pub use nozzle::{find as find_nozzle, nozzle_catalog, Machine, Nozzle};
pub use parse::{parse_f64_list, parse_i64_list};
pub use profile::Profile;
pub use reference::{catalog, check_match, RefInfo};
pub use validate::validate_output;
