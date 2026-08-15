# Prusament PC Blend Carbon Fiber on a DiamondBack 0.8, CORE One L.
#
# Values resolved from the PrusaSlicer vendor bundle by walking the full
# `inherits =` chain for `Prusament PC Blend Carbon Fiber @COREONE 0.8`:
#   github.com/prusa3d/PrusaSlicer  resources/profiles/PrusaResearch.ini
# NOTE: the PrusaSlicer-settings repo does NOT carry CORE One profiles --
# they live in the main PrusaSlicer repo. Cross-checked against the TDS:
#   prusament.com/wp-content/uploads/2023/04/TDS_Prusament-PCCF_2023_EN.pdf
name = Prusament PC Blend CF @ 0.8 DiamondBack

# --- the number this test exists to find -------------------------------
note = [sourced] Prusa publishes filament_max_volumetric_speed = 18 mm3/s for
note = PC Blend CF @COREONE 0.8 (standard nozzle). For UNFILLED PC Blend on a
note = high-flow 0.8 they publish 36. There is NO published high-flow profile
note = for PC-CF at all -- every PC-CF CORE One profile carries
note = `! nozzle_high_flow[0]`.
note = [MEASURED 2026-08-11] That 18 is NOT a floor -- it is unreachable here.
note = The first run laddered 12-32 and every single band failed; the ceiling
note = is between 8 and 12 mm3/s. Treat vendor figures as an upper bound to be
note = disproved, never as a starting point for the bottom of a ladder.
note = [measured] Same direction as PETG CF: published 22, visual knee 16-18.
note = Both materials came in BELOW Prusa's number on this nozzle.

# --- temperatures ------------------------------------------------------
note = [sourced] nozzle 285 first-layer AND other-layer (the 0.8 profile does
note = not step it down). TDS says 285 +/- 10, but the firmware clamps at 290
note = (HEATER_0_MAXTEMP 305 - HEATER_MAXTEMP_SAFETY_MARGIN 15), so 295 is not
note = commandable. The ladder stops at 290 for that reason.
temps = 275, 280, 285, 290

note = [sourced] bed 110 first layer / 115 after. 85 was the PETG value.
bed = 110

# --- cooling -----------------------------------------------------------
note = [sourced] profile is min_fan 15 / max_fan 30, disable_fan_first_layers 4.
note = The TDS is stricter: "Cooling Fan Speed 0 (0-20)", warning that cooling
note = significantly decreases interlayer adhesion. A single-wall spiral has no
note = bridges and PC's weak layer bonding is the main failure risk here, so
note = this sits at the profile floor (15% = M106 S38) rather than PETG's 50%.
fan = 38

# --- geometry / extrusion ---------------------------------------------
note = [sourced] extrusion_multiplier 1.04 for the CF variant (plain PC is 1.0).
em = 1.04
nozzle = 0.8
layer_h = 0.4
width = 0.9
first_layer_h = 0.2
first_layer_w = 1.0

note = [MEASURED 2026-08-11] first layer kept well under the ceiling so adhesion
note = is never the thing that fails -- the test is about the bands, not the
note = base. This turned out to be the run's most useful control: at 8 mm3/s
note = the brim came out solid and continuous while every band above it broke
note = up, which brackets the ceiling to 8 < ceiling < 12. Do NOT raise this
note = without leaving a known-good reference band at the bottom.
first_layer_flow = 8

note = [inferred] PC warps hard. 3 brim rings held for PETG on a 50mm circle;
note = doubled here. If the first run lifts, raise this before anything else.
brim = 6

note = [sourced] PC-CF unsets the filament retract overrides (retract_length =
note = nil), falling through to the printer profile. Kept at the PETG-proven
note = value; the wipe added after the PETG run matters more than the distance.
retract = 0.8

note = [inferred] pressure advance left at the PETG value -- Prusa's PA for
note = this combination was not located, and PA is not what this test measures.
pa = 0.018

# --- the ladder --------------------------------------------------------
note = [MEASURED 2026-08-11] The first real run used 12-32 and EVERY BAND WAS
note = ABOVE THE CLIFF. Photographic evidence (runs/20260811-191503): the brim
note = and first layer, printed at first_layer_flow = 8, come out solid and
note = continuous; band 1 at 12 mm3/s already breaks up, and from there the
note = test objects are a ragged crown of discontinuous spikes rather than a wall.
note = Across cylinder 11's bands 2-9 the object does not grow at all.
note = [MEASURED] The loadcell agrees and identifies the mechanism as a MELT
note = RATE limit, not a geometric one: mean force rises monotonically with
note = commanded flow (2965 g at 17 -> 14202 g at 32 on the 290 C test object),
note = and at any fixed flow it FALLS as temperature rises -- 27 mm3/s gives
note = 12298 / 10772 / 10183 / 9386 g at 275 / 280 / 285 / 290 C. Hotter melts
note = more, so pressure drops. The extruder is being commanded to push more
note = than the hotend can melt; output goes intermittent and the wall fails.
note = [sourced] Consistent with the hardware: DiamondBack 0.8 is a hardened
note = abrasion-resistant nozzle, NOT a high-flow melt zone, and the reference
note = export carries nozzle_high_flow = 0. Prusa's published 18 mm3/s is for
note = the standard 0.8 and is evidently not reachable with this combination.
note = [inferred] New ladder brackets the 8-to-12 transition tightly, and only
note = its top two rungs sit above the point where breakup was first observed.
note = 1.5 steps put the knee resolution at +/-1.5 mm3/s.
flows = 4, 5.5, 7, 8.5, 10, 11.5, 13, 14.5, 16
revs = 4
diameter = 50

# --- bed layout --------------------------------------------------------
note = [measured] Row is the best camera layout -- every test object at one depth,
note = nothing occluding anything -- but it only fits THREE. Pitch is
note = diameter + 2*brim*first_layer_w + 35 = 50 + 12 + 35 = 97mm, so four in a
note = row span 291mm centre-to-centre and the end footprints run from X=-27 to
note = X=327 on a 300x300 bed (CORE One L, confirmed from the reference export's
note = bed_shape = 0x0,300x0,300x300,0x300). That generated silently once, with
note = only a cosmetic warning; generation now refuses outright.
note = [inferred] Stagger keeps all four temperatures and fits: front row at
note = Y=102 (X 102, 198), back row at Y=198 offset half a pitch (X 150, 247),
note = so each back test object sits in the gap between two front ones rather than
note = directly behind one. Back pair is seen at a shallower angle -- if the
note = vision work needs all four dead-on, drop to three and use row instead.
layout = stagger

# --- REQUIRED: a PC-sliced reference -----------------------------------
note = [BLOCKER] This must point at a PrusaSlicer export sliced with PC Blend CF
note = + 0.8 nozzle + CORE One L. The spliced start block carries bed AND
note = chamber temperatures and the whole levelling/purge sequence. The PETG
note = reference would level and purge at 85C bed / 35C chamber.
note = PC needs chamber_temperature 55 with a BLOCKING chamber_minimal 40
note = (M191 S40 waits); CORE One L allows up to 60. PETG CF is 35 and
note = non-blocking, so the PETG start block would not soak the chamber at all.
reference = reference/ref.gcode
