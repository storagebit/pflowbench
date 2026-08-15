# Vision & verdict accuracy plan

From the 2026-08-14 deep dive over run 20260811-191503.

## Measured facts

- Scale, front row: 5.7 px/mm horizontal, ~3.9-4.7 vertical (elevation
  32-40 deg). One band (4 revs x 0.4 mm = 1.6 mm) = **6-8 px**.
- Noise (8 frames): height sigma 3.0 px, top-edge position sigma 0.7 px. Scene
  drift +43 px over 8 poses; within-frame differencing cancels it.
- **Scene drift = bed descent** (revs x layer_h per band; chamber-fixed CoreXY
  camera): brim at bed rate, healthy top stationary (nozzle plane), stalled top at
  bed rate (cyl11 5-7 px/band). Base-vs-band regression: px/mm per cylinder (+/-5-8%).
- Cyl11 "height" 207-216 px = ~190 px projected ring diameter + ~20 px wall. Height
  detects collapse, not underextrusion: the vase wall top tracks commanded Z until
  it breaks.
- Raggedness: 0/8 true positives on 8/8 failed bands (max 2.07 vs threshold 2.5;
  reference band itself a broken crown); benign lighting reaches 83% of threshold.
- Bed luma 13-217; fixed 200/30 thresholds sit inside background on ~1/3 of columns;
  works only on dark CF blends (all current profiles).
- H.264 is the only lossy stage (PGM pre-JPEG, same-frame verified), ~1 px after
  aggregation; JPEG quality irrelevant.
- Poses: ~11.7 s each, ~7 min per 4-cylinder run (3.4x estimate); ~105 s/cylinder
  stationary hot melt — viscosity confound for the loadcell knee.
- Hook-to-park modeled 4.3-7.7 s vs shipped 3.5 s wait.

## Priority order

1. **Minimal Phase D verdict engine** (rules 1-4, 8-11 on existing accumulators):
   per-temperature sentence + recommendation; 8/11 unblocked. [next session]
2. **Vote-rule replacement** — DONE: bed-descent self-calibration (base slope =
   px/band ruler); stall detector v_top/s_bed >= 0.75 -> Stall, >= 0.4 -> Marginal;
   relative noise floor; no-reference Stall branch deleted. Kills the 100%
   false-stall (6-8 px/band vs 12 px absolute floor).
3. **Live ROI fix** — DONE: y0 bounded below the projected rim (dark chamber pinned
   the top edge, +140-160 px fake height); neighbor x-clipping; top-pinned
   columns -> NoVote.
4. **Speed-zero capture trigger** — DONE (code): arms at segment entry, fires at
   < 0.5 mm/s x 3 samples in the photo window; 5 s firmware-clock fallback. Open:
   does Buddy stream pos_x/y/z during G4? Check the 5 s capture summary in the next
   run's 8 s tare window; else the fallback stands.
5. **Occlusion guard** — DONE: 1/16-subsampled diff vs first usable frame; moving
   fraction (>40 luma) >0.15 -> NoVote; good 0.00-0.11, head-in-frame 0.14-0.39.
6. **Pose-tax cut** — DONE: pose Z at F900 (machine limit 15 mm/s); clearance =
   stack height + 2 mm, not flat safe_z. Dwell 5 s until the speed trigger is
   hardware-confirmed, then ~2 s.
7. Snapshot-strip audit overlay (ROI + edges + vote per thumbnail): trust gate for
   unreviewed vision votes.
8. Verdict-to-slicer loop: snippet.txt + reviewed `measured =` append into
   profiles/nozzles/*.nozzle.
9. **Raggedness non-voting** — DONE: measured/reported only until a verified-healthy
   reference exists (needs force-family input, Phase D).
10. Median-of-N seq-distinct frames per window: robustness only (correlated-noise
    gains oversold); do simply or not.
11. Phase B accumulators (sag, PWM saturation, plan_slow, fsensor): second evidence
    family for rule 9; parsers exist in the armed G-code, receive loop drops them.
12. Park-position FOV validation — DONE: gcode_generate projects the park point through
    stored calibration, warns if in frame; parked head evades all motion/speed guards.

## Killed

- min_band_s >= 8 revs everywhere: doubles print time for SNR the verdict never
  consults (formula only raises revs on fast bands).
- Printed reference staircase: 2-3% at own bed position, 10-25% across rows; the
  bed-descent ruler beats it at zero cost.
- Homography with guessed intrinsics: 15-30% silent systematic error (barrel
  distortion, unknown focal length); measured anisotropy violates the model.
- Background models: 63-75% (spatial) / 42-55% (temporal) misclassification on real
  frames; per-frame per-column adaptive stats are the route for non-dark filament.
- Frame-difference AND-gate trigger: redundant vs speed trigger + seq gate +
  occlusion guard + FOV check.

## Standing physical limits

- Vision resolves band-level collapse, not percent-level underextrusion: severity is
  a downgrade flag, never a ceiling number. Caliper two orders finer; loadcell the
  primary family.
- Cross-cylinder pixel comparisons: ~27% front/back-row scale bias; stay per-cylinder
  self-normalized.
- Poses add stationary hot-melt time to the measurand; items 4/6 reduce it, only
  removing poses removes it — pose count/duration stays a profile decision.
