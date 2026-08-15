# Elegoo PLA-CF verification kit — three nozzles, three phases

Companion to `profiles/elegoo-pla-cf-*.profile`. The bench measures the flow
ceiling; the two prints here measure what the bench cannot: the extrusion
multiplier (EM) and end-to-end behavior at production profile speeds.

Order matters. Dry the spool first (55 °C, 4–6 h) — wet CF-PLA fails every
phase in ways indistinguishable from a bad profile, and it is what made the
first PETG-CF knee provisional.

Files here:

    slicer/elegoo-pla-cf-0.6-bundle.ini    PrusaSlicer config bundle: DB06 + HFOBX06
                                            filament presets and 0.25mm quality prints
    slicer/elegoo-pla-cf-0.8-bundle.ini    same for the 0.8 DiamondBack
    slicer/em-flowcube-presets.ini         two print presets that slice the EM cube
                                            correctly (1 perimeter, 0% infill, classic)
    models/em_flow_cube.scad               40x40x29.95 solid cube (single-wall EM test)
    models/extrusion_coupon.scad           60x30 coupon: rib, boss, hole, flat top
    models/*.stl                           the same two models, pre-rendered
    gcode/extruder-flowstep-db06.gcode     Phase-0 cross-check, DiamondBack 0.6:
                                            E-axis-only melt-capacity ladder
    gcode/extruder-flowstep-hfobx06.gcode  same ladder shifted up for the HF ObXidian

Pre-rendered STLs sit next to the `.scad` sources (regenerate with
`openscad -o <name>.stl <name>.scad` after any edit). All Z heights sit on the
0.2 + n*0.25 layer grid so nominal = printed.

## Phase 0 — flow ceiling: run the bench

One flowbench run per profile. Each needs its reference export first (the
`[BLOCKER]` note in every profile says exactly which preset pair to slice
with; the bundles are in `slicer/`). The export's own footer must show:

| profile | filament_settings_id | nozzle_high_flow | published anchor |
|---|---|---|---|
| `elegoo-pla-cf-0.6-diamondback` | Elegoo PLA-CF DB06 @COREONE | 0 | 15.5 (neat) / no E3D table |
| `elegoo-pla-cf-0.6-hf-obxidian` | Elegoo PLA-CF HFOBX06 @COREONE | 1 | 30 (E3D + Prusa agree) |
| `elegoo-pla-cf-0.8-diamondback` | Elegoo PLA-CF DB08 @COREONE | 0 | 19 (neat) |

`check_match` now refuses PLA-on-PC, wrong-diameter, and HF/standard
crossings — the last one is load-bearing here because the two 0.6 references
differ only in that flag.

Result handling: knee − 10–15 % → `filament_max_volumetric_speed` in the
slicer filament preset. The slicer presets ship 14 / 27 / 18 as CF-derated
anchors; the bench replaces those inferences with measurements. If a knee
lands far below its anchor (the PC-CF pattern), re-dry and re-run before
believing it.

### Cross-check — motion-free melt capacity (0.6 nozzles)

`gcode/extruder-flowstep-*.gcode` measures the extruder + melt zone alone:
**no homing, no X/Y/Z moves, E-axis only.** Load filament, jog the head up so
the noodle can fall clear, run from the file browser; an `M1` gate waits for
confirmation before extruding. Each file holds 30 s per step at 235 °C, step
shown on the LCD:

| | steps (mm³/s) | shipped MVS | expected noodle mass |
|---|---|---|---|
| DiamondBack 0.6 | 8 → 10 → 12 → 14 → 16 → 18 | 14 | 2.95 g ±4 % |
| HF ObXidian 0.6 | 18 → 21 → 24 → 27 → 30 → 33 | 27 | 5.78 g ±4 % |

Last clean step × 0.85 = the melt-capacity ceiling. Failure signature:
Nextruder click/grind, or the noodle visibly thinning mid-step. Weigh the full
noodle afterwards — mass short of expected with *no* clicking points at a
partial clog or wet filament, not a flow limit.

Read the two ceilings together: the bench knee is the *printing* limit (bead
formation, back-pressure through the loadcell); the flowstep ceiling is the
*melt* limit. A bench knee well below a clean flowstep ceiling says bead
geometry or speed is the constraint, not the hotend — and a flowstep failure
below the bench knee should never happen (re-dry and re-run). Both files carry
the M862.x/M115 compatibility block Buddy firmware demands of hand-written
G-code (the F flag mirrors `nozzle_high_flow`, so the HF file declares F1).

## Phase 1 — extrusion multiplier: single-wall cube

1. Import `slicer/em-flowcube-presets.ini` (config bundle).
2. Slice `em_flow_cube` with `EM FLOW CUBE DB06` or `EM FLOW CUBE HFOBX06`
   to match the installed nozzle; filament preset stays the normal one.
   For the 0.8, use the DB08 quality print preset with manual overrides:
   1 perimeter, 0 top, 0 % infill, 3 bottom, classic generator.
3. Wall is a single external perimeter, commanded **0.64 mm** (0.6 nozzles)
   or **0.85 mm** (0.8). Micrometer all four walls, ≥3 points each, in the
   12–25 mm band, off the corners and off the aligned seam column.
4. Mean within ±0.02 of commanded → keep em 0.95. Otherwise
   `em_new = 0.95 × commanded / mean`; update it in **both** the slicer
   filament preset and the matching `profiles/*.profile`, and reprint once.

Per-nozzle values — do not copy one nozzle's EM to another. On the HF
ObXidian, a drifting EM over kilograms is the wear indicator; on the
DiamondBack it points upstream (drive gear, filament path), not at the tip.

## Phase 2 — end-to-end at profile speeds: coupon

Slice `extrusion_coupon` with the unmodified quality print preset for the
installed nozzle and the Phase-1 EM. This is the print that runs infill at
the MVS-limited regime the cube never reaches.

| feature | nominal | pass band | reads |
|---|---|---|---|
| rib (mic, top half) | 1.36 mm | ±0.05 | flow, two-bead bond |
| length / width | 60.00 / 30.00 | ±0.10 | flow + dimensional |
| plate height | 5.95 | ±0.08 | flow, first-layer squish |
| boss Ø / hole Ø | 10.00 / 8.00 | informational | holes typically read 0.1–0.2 under |

Under-extrusion **only over infill regions** at an otherwise-passing EM means
the slicer MVS is above the real ceiling — that is a Phase-0 number, fix it
there, not in the multiplier. Consistent −0.15…−0.20 on the 60 mm length with
everything else passing is shrinkage: `filament_shrinkage_compensation_xy`
(≈0.2–0.3 %), not EM.

## Where the numbers come from

Anchors: PrusaSlicer vendor bundle (resources/profiles/PrusaResearch.ini,
inherits-chain resolved) for 15.5 / 19 / 30 and PA 0.014 / 0.010 (M572);
E3D "Prusa Support: High Flow ObXidian Nextruder Nozzles" for the 30 mm³/s
table and the absence of any DiamondBack flow table; Elegoo PLA-CF listing
for the 210–240 °C envelope and 55 °C drying. CF derates (14 / 27 / 18) are
inferred at ~10 % pending Phase-0 measurement — which is the point of the
bench.
