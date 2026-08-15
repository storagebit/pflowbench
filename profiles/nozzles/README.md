# Nextruder nozzle database

One `.nozzle` file per physical nozzle product, one `.machine` file per
printer family, and the verified PrusaSlicer bundle extraction under
`bundle-2.5.5/`. Parsed and linted by `flowgen::nozzle` (unknown keys
rejected; claims without a source note rejected).

## Which numbers to trust

Everything numeric in this database is a **claim** until the bench measures
it. This project has already caught the top tier of the usual advice being
wrong on real hardware, so the order is spelled out:

1. **`measured`** entries -- this bench's loadcell knees and flowstep melt
   ceilings, on this machine, with this spool. The only trusted tier.
2. **Independent measurements** (`[independent]` notes) -- someone else's
   bench with a disclosed method (requested-vs-real-flow charts, click
   ladders). Right method, wrong machine/spool.
3. **Manufacturer tables** (E3D datasheets) -- test method not published, but
   E3D's numbers proved *consistent* with tier 2 where both exist (they
   correctly show standard ObXidian below brass).
4. **Prusa preset ceilings / marketing** (`bundle-2.5.5/`, vendor claims) --
   the tier that produced the failed prints this bench exists to correct.
   Ladder placement only, never a ceiling.

The derate history that fixed this ordering:

| material (DiamondBack 0.8) | published | this bench measured |
|---|---|---|
| Prusament PETG CF | 22 | visual knee 16-18 (undried; provisional) |
| Prusament PC Blend CF | 18 | bracketed 8-12 |

## What the research settled (2026-08)

- **DiamondBack is standard-flow.** Prusa's listing says verbatim to use
  standard flow profiles; E3D concurs; no HF geometry is claimed anywhere.
  (One Prusa KB page gives it *no* classification -- that page over-read as
  "HF class" once; the product pages are unambiguous.)
- **No DiamondBack flow table exists anywhere.** Not on the E3D product
  page, not on the E3D support page, not from Prusa. The two `measured`
  lines in `e3d-diamondback-0.8.nozzle` are the only DiamondBack flow data
  points in existence. This bench is the primary source.
- **The HF slicer presets are nozzle-agnostic.** `nozzle_high_flow = 1`
  covers a *class*: Prusa brass CHT (a genuine Bondtech CHT core -- the
  stock nozzle on every CORE One family machine), E3D HF Brass
  (four-channel), and E3D HF ObXidian. The start-gcode check is just
  `M862.1 ... F1`.
- **HF ObXidian 0.8 has no published table** (E3D's stops at 0.6); its
  `claim_mvs` values are Prusa-preset tier only -- and physics (melt-bound
  ceilings are nearly flat with diameter) says treat 35-36 with suspicion.
- **Standard ObXidian measured *below* plain brass** (independent XL bench:
  clean to 20 vs brass's 22, same filament/temp) -- matching E3D's own
  tables (15 vs 16 PLA). Hardened-and-coated costs flow; abrasive filaments
  are the only reason to fit one.
- **CORE One vs CORE One L are not motion-identical**: L runs X/Y feedrate
  500 vs 350 but travel accel 6000 vs 7000 and retract accel (2500,1200)
  vs (2500,2500); Z 15 vs 12. See the two `.machine` files.

## Platform facts (Nextruder, whole family)

- Heater: 24 V / 40 W cartridge, one part across MK4/S, XL, CORE One/+/L.
- Real temperature ceiling: firmware caps at **290 C** everywhere
  (HEATER_0_MAXTEMP 305 - 15 safety margin) -- the MK4's "300 C" spec is
  marketing. Optional HT hotend (CORE One family): 400 C.
- Melt zone: standard nozzles ~20.9 mm threaded hot section; HF/CHT parts
  trade ~10 mm of cold stem for ~31.5 mm hot section (E3D drawings). That
  +50% heated length is the physical basis of the HF class.
- Loadcell (the bench's sensor): strain gauge bonded to the heatsink, read
  by an HX717 at a fixed **320 samples/s**; firmware scale 0.0192 g/count.
- Drive: 10:1 planetary on a pancake stepper; **no credible published push
  force exists** -- a real gap, since TPU and CF ceilings are often
  push-force-bound, not melt-bound.
- Heatbreak fan is PID-servoed to 36 C heatbreak temperature (not
  on/off) -- steady-state thermals shift with chamber temperature, which
  matters when comparing knees across chamber setpoints (55 C max CORE
  One+, 60 C active on L).

## Rules of thumb for combos nobody has measured

- Melt-bound ceilings scale far below the orifice area ratio: expect
  **+0-60% going 0.4 -> 0.6** (E3D's own tables span +33-55%), nowhere
  near the +125% the area suggests; below ~0.4 the orifice pressure drop
  takes over and the ceiling collapses. Corollary: a 0.8 claim far above
  its 0.6 sibling is suspect -- physics puts 0.8 only modestly above 0.6.
- Within a material's window, ceiling rises ~1%/C (independent click
  ladders measured 9-11%/10C) -- which is why every profile here ladders
  temperature.
- HF geometries measure 1.4-2.7x standard on V6-class hotends, and the
  0.6 Nextruder tables fit (+82% PLA, 17->31) -- but at 0.4 E3D's own
  tables show only **+25-33%**: either the CHT multiplier shrinks at small
  bores or the 0.4 HF figures are sandbagged. Plan 0.4 HF ladders around
  +25-33%, not the CHT-class multiplier.
- **Material ordering is hotend-specific, and the V6 lore inverts here.**
  V6-class data says PETG ~ 0.55-0.65x PLA; on Nextruder hardware E3D's
  tables put PETG *above* PLA on every nozzle (1.20-1.25x), and the
  independent click ladders agree (27 PETG vs 18 PLA). Ladder prior for
  this bench: **PETG >= PLA at their standard temps**; using the V6 ratio
  would start PETG ladders in the wrong half of the range. PC and PA sit
  below-or-near PETG (inferred from viscosity, not measured); TPU lowest
  and push-force-bound.
- Thermal budget: melting PETG at the claimed 36 mm3/s costs ~35 W against
  the shared 40 W heater (~87% duty) -- feasible but with almost no
  headroom, so chamber temperature and part-fan load can move an HF0.6
  knee even when the claim is honest.
- CF fillers raise melt viscosity 2.5-3x at 20-30 wt% and *suppress die
  swell*: CF walls measure thinner at identical true flow, so the caliper
  cross-check must use per-material expected widths, and the knee often
  moves from melt-bound to pressure-bound (watch for the force-fluctuation
  signature in the loadcell trace rather than a clean plateau).
- **Nobody has published systematic CF-vs-neat derates.** The bench's
  PETG-CF (-18..-27%) and PC-CF (-33..-55%) numbers appear to be novel
  data. Expected ordering from rheology: PLA-CF < PETG-CF < PC-CF ~ PA-CF.

## Left out on purpose

- V6-thread nozzles (Bondtech CHT M6, Olsson Ruby, Nozzle X): usable only
  via the Nextruder V6 adapter, which changes melt geometry -- ceilings
  measured through the adapter would not transfer. Catalogue them if one
  ever gets fitted.
- Bondtech native-Nextruder CHT, Micro Swiss, Slice Engineering: confirmed
  to not exist as of 2026-08.
- Per-filament PA and MVS for all 1320 bundle presets: that matrix lives in
  `bundle-2.5.5/filament_matrix.csv` (verified complete against the ini),
  not in the per-nozzle files.

## Where this data came from

Bundle extraction (config_version 2.5.5, local vendor file) verified by
independent re-resolution: 28/28 printer presets, 68/68 print presets,
1320/1320 filament presets, zero mismatches. Web claims cross-checked by
two research waves plus verification passes that attacked every number; where a claim
failed verification it is recorded here corrected (HF table coverage,
DiamondBack classification, CHT stock nozzle, L motion specs). E3D flow
tables were fetched independently by two agents with identical values, then
a third re-fetched all four digit-for-digit. They remain manufacturer-claim
tier: every value is a round integer and no test method is published --
the checks confirm their *ordering* (ObXidian < brass matches the
independent bench), not their exact values.
