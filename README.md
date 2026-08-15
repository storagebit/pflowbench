# PFlowBench

Flow-rate calibration bench for Prusa Nextruder printers (MK4/S, XL,
CORE One, CORE One L). It measures the real maximum volumetric flow of a
nozzle/filament combination instead of trusting published numbers -- which
this bench has repeatedly measured as too high.

## How it works

The printer's loadcell -- normally used for first-layer sensing -- doubles
as a back-pressure sensor. A generated G-code job prints a stack of
single-wall cylinders; each height band is extruded at a higher volumetric
flow. While printing, the printer streams its metrics (loadcell force, head
position, temperatures) over UDP. The bench bins force by band, and the
point where force spikes while the extrusion thins is the flow ceiling.
Results are cross-checked three ways:

- **Force knee**: mean loadcell force per band, plotted against flow.
- **Vision**: a chamber camera photographs every band from a parked-head
  position; local image analysis detects when the wall stops growing
  (the bed descends a known distance per band, so the test object's top
  edge riding down with the bed means extrusion has collapsed).
- **Calipers**: wall thickness per band, measured after the print.

Everything runs through a desktop app: G-code generation, upload over
PrusaLink, live capture with force/knee charts, per-band camera stills,
timelapse recording, and a unified console.

## Requirements

- A Nextruder-based Prusa printer running Buddy firmware, reachable over
  the network (PrusaLink enabled).
- Metrics enabled on the printer: Settings -> Network -> Metrics & Log.
- An inbound UDP port (default 8514) open on the machine running the app.
- Optional: a network camera with RTSP for the vision features.
- Rust >= 1.75 for the libraries; the app needs the Tauri v2 toolchain.

## Repository layout

    crates/flowcore     metrics parser, band mapping, UDP capture engine
    crates/prusalink    PrusaLink HTTP client (Digest / X-Api-Key auth)
    crates/flowgen      calibration G-code generator, profiles, nozzle database
    crates/flowcam      RTSP camera capture, per-band stills, timelapse
    crates/flowvision   image analysis of per-band stills
    app/src-tauri       desktop app backend (Tauri v2)
    app/ui              frontend, vanilla JS, no build step
    profiles/           material profiles and the nozzle database
    docs/               design and accuracy plans, verification kit

The crates are dependency-free Rust (std only) and know nothing about the
app; all logic lives in them and is unit-tested there. Each crate is split
one concern per file, with the public API on the crate root.

## Build

    cargo test            # full test suite for all five crates

The app:

    cargo install tauri-cli --locked
    cd app/src-tauri && cargo tauri dev      # or: cargo tauri build

On Linux, install `libwebkit2gtk-4.1-dev libgtk-3-dev
libayatana-appindicator3-dev librsvg2-dev` first. On macOS the system
WebKit is used, no extra packages.

## Usage

1. Enable metrics on the printer (see Requirements). Individual metric
   state is RAM-only on Buddy firmware; the generated G-code re-arms the
   needed metrics at the start of every job.
2. In the app, enter the printer address and PrusaLink API key
   (stored in the OS keychain, never in files or logs). Test the
   connection -- a successful test beeps the printer unless it is busy.
3. Pick a material profile. The app fills the form from it; edits override
   the profile but are logged with both values.
4. Point at a PrusaSlicer reference export for the material (see below),
   generate the G-code, and review it.
5. Start capture, upload the job, and print. The live view shows the
   force trace, the per-band table, and the knee plot as bands complete.
6. Confirm the number with calipers before using it: the force knee and
   the wall-thickness knee should land on the same band. Enter the result
   as `filament_max_volumetric_speed` in the slicer's filament profile,
   with a safety margin of one ladder step.

## Profiles and references

A run's parameters come from two files with a strict ownership split:

- `profiles/*.profile` (flat `key = value`) owns the flow ladder,
  temperatures, geometry, and extrusion settings. Every value carries a
  `[sourced]` or `[inferred]` note saying where it came from. Unknown keys
  are a parse error, and `cargo test` validates every shipped profile.
- `reference/*.gcode`, a real PrusaSlicer export, owns the bed and chamber
  temperatures, the levelling pass, and the purge -- its start/end blocks
  are spliced into the generated job.

The generator reads the reference's own settings footer and refuses to
generate when it does not match the profile (wrong material, wrong nozzle
diameter, high-flow vs standard nozzle). References are never trusted by
filename -- only by what their footer says they are.

`profiles/nozzles/` is a database of Nextruder nozzles: one file per
product with published flow numbers kept strictly separate from measured
ones, plus machine motion limits. Published numbers place the test ladder;
only measured numbers are trusted. See `profiles/nozzles/README.md`.

## Status

- All five crates: 89 tests green, including an image-analysis test that
  detects a real failed print from its captured frames.
- The app builds and links; the full pipeline against printer hardware is
  validated run by run -- the caliper cross-check is part of every run,
  not a formality.
- Measured so far (DiamondBack 0.8 nozzle): PETG-CF ceiling 16-18 mm3/s
  vs 22 published; PC Blend CF 8-12 mm3/s vs 18 published.

## License

GPL-2.0-or-later: you can redistribute and/or modify this software under
the terms of the GNU General Public License as published by the Free
Software Foundation, either version 2 of the License, or (at your option)
any later version. See [LICENSE](LICENSE).

Vendored third-party code keeps its own license: `app/ui/vendor/uPlot.*`
is [uPlot](https://github.com/leeoniya/uPlot) (MIT).
