# PFlowBench: Verdict Engine & Vision — Design and Implementation Plan

Status: ACTIVE (2026-08-11, after the first two real calibration runs). Update when reality disagrees.

Implementation status 2026-08-14: Phase A (sdpos addressing) DONE. Phase B metrics armed. Phase C
item 5 (photo poses) DONE: `kind=photo` windows in flowgen, `on_photo_window` hook in flowcore,
freshness-gated stills in the app (stale frames saved as *_stale, never as silent duplicates).
Phase E Tier 0 + Tier 1 DONE as `crates/flowvision` (§7), validated against run 20260811-191503
(detects cyl11's growth stall; ignored test); app commands `vision_calibrate` and `vision_analyze`.
Open: Phase C items 1-4/6 (tare correction exists; min_band_s, probe cylinder, dryness gate
pending), Phase D verdict engine, Phase F report.

Accuracy addendum 2026-08-14: docs/accuracy-plan.md carries the measured error budget and priority
list; the vision vote engine rides bed-descent physics (top-edge velocity vs base regression), not
height differencing.

## 0. Goal

The tool must print, per temperature, after a run (today: a human over 99 table rows, 98 photos):

> **8 mm³/s SUSTAINED · 10 MARGINAL · 12+ FAILED — recommended filament_max_volumetric_speed = 8**

Everything below feeds a verdict layer that reaches this automatically, states its evidence, and
exports a PrusaSlicer-ready setting. Claim tags: [fw] verified against firmware source (prusa3d/
Prusa-Firmware-Buddy, master, fetched 2026-08-11); [data] validated against runs/20260811-191503;
[adv] survived adversarial review.

## 1. Findings from the first runs

- PC Blend CF on DiamondBack 0.8 fails between 8 and 12 mm³/s — a melt-rate limit (force falls
  with temperature at fixed flow; brim at 8 solid, band 1 at 12 broken). [data]
- The cylinder Z-heuristic over-counts (11 cylinders for 4 test objects); band 1 is contaminated
  (n1/n2 ≈ 2.9 vs 1.21 predicted). [data]
- Loadcell tare drifts (negative band means, −13…−104 g). [data]
- Camera frames go stale (27 unique images of 98 snapshots). [data]
- Speed ~4× understated (arrival-clock bug; per-point µs fix built).
- Force saturates ~12.7–14.2 kg at the ladder top (extruder slip plateau) — NOT evidence of health. [data][adv]

## 2. Architecture

```
flowgen                     printer                  flowcore                verdict
───────                     ───────                  ────────                ───────
G-code with ;FBSEG          sdpos (byte offset) ───► SdMap: sdpos →          rules over settled
markers per segment  ─────► M331-armed metrics ────► (cyl, band, kind)       per-band stats +
+ <out>.bands.txt           UDP @ per-point µs       bins force/speed/       evidence families
manifest (byte ranges)      timestamps               sag/PWM/… per band  ──► SUSTAINED/MARGINAL/
+ tare & photo segments                              + tare windows          FAILED/SATURATED
                                                     + band time windows ──► snippet + report
                                                                     camera ┘  (vision votes)
```

## 3. Phase A — sdpos band addressing (task #15)

`sdpos` [fw: enabled by default, 100 ms interval, plain `v=` int] is the byte offset into the file
being printed; flowgen writes that file, so it knows every segment's byte range. Replaces the
CylTracker Z-heuristic (11 phantom cylinders), removes purge/travel contamination and band-1 pollution.

flowgen:
- Every segment starts with `;FBSEG kind=<pre|purge|first|band|tare|photo|travel|end> cyl=<i> band=<j> flow=<f> revs=<r> temp=<t>`
- `pub fn sd_manifest_text(body: &str) -> String` scans the FINAL file bytes for `;FBSEG` and
  emits `flowbench-bands v1`, then `start end kind cyl band flow revs temp` per segment
  (line-based, not JSON: flowcore is std-only).
- Generate the manifest after all post-processing (the M555 widening rewrite in main.rs changes
  byte offsets): `gcode_generate` writes `<out>.bands.txt` as its last step.

flowcore:
- `SdMap::parse(text)` → sorted ranges; `locate(pos)` by binary search.
- Prefetch guard: sdpos leads execution by the firmware read-ahead; subtract `SD_GUARD_BYTES`
  (default 2048) before locating, and blank each band's first revolution anyway (Phase C).
- Receive loop: `sdpos` updates `cur_seg`; with a map, force/speed bin by `cur_seg` (kind==Band),
  tare windows accumulate per-cylinder (kind==Tare), band entry/exit → `BandWindow { cyl, band, t0, t1 }`.
- Z cross-check stays: warn when sdpos-derived expected Z differs from observed by > one layer height.
- No manifest → legacy Z-heuristic mode, logged as such.

app: `gcode_generate` writes the manifest; `capture_start` loads `<gcode>.bands.txt`, passes the map.

## 4. Phase B — evidence metrics (task #18)

All names/formats verified in firmware source. Arm via M331 (RAM-only, both arming blocks); parse
in flowcore; bin per band (settled window).

| metric | format [fw] | role in the verdict |
|---|---|---|
| `temp_noz` | custom `,n=,a= value=<f>` ~1 s | actual nozzle temp |
| `ttemp_noz` | custom `,n=,a= value=<i>i` ~1 s | target; **sag = target − actual** is the most direct melt-limit evidence |
| `nozzle_pwm` | `v=` int 0–255, 1 s, hardware PWM | pinned ≥250 through a band = every watt spent [fw: hwio_buddy_2209_02.cpp] |
| `heater_current`×`heater_voltage` | `v=` floats, ~1 s, already ENABLED | average watts, corroboration only (point-sampled on a chopped load) |
| `fsensor` | custom `,n= st=<u>i,f=…` ~20 Hz, NO value= key | mid-band `st` flips 2→3 or f= excursions at max force = grinding |
| `temp_hbr` | custom value=, 1 s | heat-creep drift across bands |
| `plan_slow` | `v=` int cumulative | >0 delta = commanded flow NOT executed → corrects the x-axis |
| `stp_stall` | `v=` int cumulative | planner underrun ONLY [fw] — E-skips are firmware-invisible (open-loop). Nonzero = rule out planner starvation; zero = NO information, never a health vote [adv] |

Parser: `parse_point` already accepts `v=`/`value=`; `fsensor` needs its own arm (`st=`/`f=`).
Per-band accumulators: sag, PWM saturation fraction, watts, fsensor flips, plan_slow/stp_stall deltas.

Heater-evidence gate [adv]: votes count only when actual temp held within 2 °C of target for ≥10 s
before the band (else post-M109 heat-up transients read as FAILED). Sustained sag (>3 °C for
seconds during extrusion) is primary; duty saturation alone corroborates, never suffices (chronic
100% duty at 290 °C + fan can be mere heater sizing).

## 5. Phase C — test-program changes (task #20)

All in flowgen's G-code generation; each gets an `;FBSEG` segment.

1. **Per-cylinder tare window.** After each M109 settles: lift to safe_z, `G4 S8`, zero extrusion,
   `kind=tare`. All force features become deltas from the per-cylinder tare; withhold the
   cylinder's verdict when tare drifts beyond bound or tared band means go negative [adv].
2. **Minimum band duration** (`min_band_s`, profile key, default 20): revs_i = max(revs,
   ceil(min_band_s × speed / circumference)); short bands smear the melt-pressure step response [adv].
3. **Settled window:** exclude each band's first revolution from decision statistics (byte-based:
   manifest carries revs; settle boundary = start + range/revs). Judge on the LAST 50% of each
   band (early revolutions lean on the previous band's sound wall) [adv].
4. **Probe cylinder** (`probe_repeat_first`, profile key): repeat the first temperature at the END
   of the run; if its forces don't reproduce the first cylinder's, the run has a drift/chamber
   confound and ALL cross-temperature comparisons are withheld [adv].
5. **Photo pose** (Tier 0, §7): at each band boundary, retract + park at a fixed XY out of the
   camera's sight for ~1.5 s, `kind=photo`; the snapshot fires inside this window (sdpos says when)
   and must be NEWER than the previous frame — fixes task #17's stale frames and toolhead
   occlusion in one mechanism. ~36 poses ≈ +2 min print time.
6. **Dryness gate [adv]:** reference band's normalized spread beyond a dry envelope → stamp the run
   "filament condition suspect — dry and rerun", refuse per-band verdicts (wet PC-CF fakes signals both ways).

## 6. Phase D — verdict engine (task #19)

New std-only module `crates/flowcore/src/verdict.rs`. Input: settled per-band stats, band windows,
tares, per-cylinder temp/program (from the manifest), evidence accumulators. Output:
`Verdict { bands: [(cyl, band, flow, class, fired_rules, confidence)], per_temp_ceiling,
recommendation, run_flags }` → verdict.json, UI card, report.

Classes: SUSTAINED / MARGINAL / FAILED / SATURATED / NO_VOTE. Rules (validated on runs/20260811-191503 unless marked provisional):

1. *Artifact gate* (defense in depth even with sdpos): a cylinder is a real test object iff min
   settled band n ≥ a threshold from revs/flow physics (n ∝ revs/flow — do NOT hard-code 500) AND
   n strictly decreases across equal-rev bands. Classified 11/11 correctly. [data]
2. *Tare correction:* mean_tared = mean − tare[cyl]; negative tared means → withhold the cylinder
   verdict, state why.
3. *FAILED (force family), all normalized, zero absolute grams:*
   - increment ratio R(i) = d(i)/d_ref > 1.5, d_ref = first clean increment above the reference band;
   - saturation branch: max prior R > 2 AND R(i) < 0.75 × that max — increments collapsing after
     strong growth is slip plateau, NOT recovery [data: flagged 19.5–32 on 4/4 cylinders];
   - excess factor E(i) = (meanᵢ/mean_ref)/(Qᵢ/Q_ref) > 1.3 (1.15–1.3 → MARGINAL, provisional
     until sustained-regime data).
4. *Regime cross-check:* force strictly decreasing across ≥3 temperatures at fixed flow →
   melt-limited flag [data: caught 17.0, missed by per-cylinder rules]; force RISING with
   temperature → deep saturation [data: seen at 32].
5. *Heater family:* gated sag/PWM votes per §4.
6. *Mechanical family:* fsensor flips during the settled window.
7. *plan_slow delta > 0:* band marked COMMAND_NOT_MET; correct the x-axis or demote to low confidence.
8. *Vision family* (§7): a band that LOOKS broken is FAILED regardless of force; visual evidence
   only downgrades, never rescues — same philosophy as calipers.
9. *Combination [adv]:* hard FAILED needs the force family strong (E>1.3 or composite R) OR ≥2
   independent families agreeing; a single-signal verdict caps at MARGINAL, disagreement stated.
   Never use sd/mean as primary [data: cyl11 held cv 0.07–0.12 while failing]. Plateau shape never
   votes SUSTAINED alone — its LEVEL must agree with the viscous line from the sustained prefix [adv].
10. *NO_VOTE:* every band above a cylinder's first FAILED (broken foundation) [adv]; ceiling = top of the contiguous SUSTAINED prefix.
11. *Recommendation:* per-temperature ceiling; headline = the profile's primary temperature's ceiling minus a safety margin (default one rung, a setting).

Stated in every output: the first run contains zero sustained bands, so SUSTAINED/MARGINAL
thresholds are provisional until the 4–16 ladder run; verdicts list which evidence families were
available; a run that fails its gates (tare, dryness, probe) says so instead of classifying.

Tests: rule-level unit tests on synthetic accumulators + a golden test on runs/20260811-191503/
bands_recovered.csv (copied to crates/flowcore/testdata/): composite flags 19.5–32 on 4/4 real cylinders, artifact gate 11/11.

## 7. Phase E — vision (three tiers)

**Tier 0 — trustworthy photos (in Phase C, prerequisite):** deterministic photo poses +
freshness-gated capture; otherwise vision reads duplicated stale frames (27 unique of 98).

**Tier 1 — classical CV, offline, in-process (`crates/flowvision`).** layout_positions gives each
test object's bed XY, the camera is fixed; a one-time calibration (4 brim-centre clicks in the UI
per camera setup, stored as runs/vision.calib) yields a bed→image homography → per-object ROIs.
On the lossless PGM lumas already saved:
- *Growth tracking:* outline height in the ROI vs the known expected height at band k; "object
  stopped growing" is a massive, robust signal.
- *Top-edge raggedness:* a healthy spiral wall has a smooth top edge, the failure crown is jagged;
  high-frequency energy along the detected top edge.
- Both self-normalized against the in-run reference band; votes downgrade-only (rule 8).

**Tier 2 — nothing cloud.** Vision is LOCAL ONLY by explicit decision (2026-08-11): no online
models, no per-run cost. If Tier 1 proves insufficient, escalate to a small in-process LOCAL model
(e.g. an ONNX edge-quality classifier), never a network call; until then Tier 1 is the whole vision system.

## 8. Phase F — caliper, snippet, report (task #21)

- Guided caliper entry per band: MINIMUM of several stations (a broken wall calipers FAT over
  blobs/CF fuzz [adv]); "is the wall continuous?" gates everything; thickness only downgrades, never
  rescues; efficiency normalized to the object's reference band (cancels die swell and jaw bias).
- Deliverable: copy-paste PrusaSlicer snippet (`filament_max_volumetric_speed = <ceiling − margin>`).
- Report: self-contained HTML in the run dir — verdict card, knee chart (inline SVG), per-band
  chips, snapshot strip (md5-distinct frames only, staleness flagged), caveats auto-listed. Share
  the report; use the snippet.

## 9. Build order

A (sdpos) → B (signals) → C (test program + photo poses) → D (verdict) → E Tier 1 (local vision)
→ F (caliper/report). A–C change what the NEXT PRINT records → land before the 4–16 run; D ships
provisional thresholds, hardens on that run. Deploy: task #14 fixes ride along; macOS keychain prompt reappears per rebuild (unsigned binary).

## 10. Standing constraints

- Secrets only in the OS keychain; never logged, not even truncated.
- No absolute-gram or absolute-% thresholds; every rule self-normalizes against the run [adv].
- Every capped/sampled path logs what it dropped.
- First git commit deferred until your first successful calibration print (memory: first-commit-deferred).
