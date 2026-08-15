# Elegoo PLA-CF on a DiamondBack 0.6 (standard flow), CORE One / CORE One L.
#
# Anchors resolved from the PrusaSlicer vendor bundle by walking the full
# `inherits =` chain (github.com/prusa3d/PrusaSlicer,
# resources/profiles/PrusaResearch.ini) and from the E3D support pages.
name = Elegoo PLA-CF @ 0.6 DiamondBack

# --- the number this test exists to find -------------------------------
note = [sourced] Prusa ships filament_max_volumetric_speed = 15.5 for NEAT
note = Elegoo PLA on the standard-flow 0.6 (Elegoo PLA @COREONE 0.6). E3D
note = publishes NO flow table for the DiamondBack Nextruder nozzle at all --
note = only for its HF products -- consistent with a standard-flow melt zone
note = whose value is wear resistance, not throughput.
note = [inferred] Slicer profile for this filament ships MVS 14 (~10% CF
note = derate, the ratio that matched on the 0.8). PC-CF's lesson applies:
note = the published number is an anchor for placing the ladder, not a floor.
note = Ladder brackets 5-17 so the whole plausible range is bracketed and the
note = bottom rungs are known-good controls.
note = [sourced] Melt POWER is not the constraint: PLA at 15 mm3/s needs ~7 W
note = of the 40 W cartridge. The constraint is heat transfer into a polymer
note = at ~0.13-0.19 W/(m K); that is what HF geometry changes and a PCD tip
note = insert does not.
flows = 5, 6.5, 8, 9.5, 11, 12.5, 14, 15.5, 17

# --- temperatures ------------------------------------------------------
note = [sourced] Elegoo PLA-CF envelope 210-240 C (vendor listing; dry 55 C).
note = Ladder spans the full envelope in even 10s. Expect the knee to rise
note = with temperature if the limit is melt rate, as it did for PC-CF.
temps = 210, 220, 230, 240

note = [sourced] bed 60 first layer / 55 after in the slicer profile; the
note = generator takes one value, 60. Smooth PEI + Elmer's purple liquid glue.
bed = 60

# --- cooling -----------------------------------------------------------
note = [sourced] Prusa runs Elegoo PLA at min_fan 85 / max_fan 100. 85% of
note = 255 = 217. Full-range cooling is correct for PLA walls; unlike PC,
note = layer bonding is not the failure risk here.
fan = 217

# --- geometry / extrusion ---------------------------------------------
note = [sourced] 0.68 width is Prusa's default bead for every 0.6 CORE One
note = profile; layer 0.32 keeps band wall speeds sane at the ladder top
note = (17 mm3/s / 0.1956 mm2 = 87 mm/s).
nozzle = 0.6
layer_h = 0.32
width = 0.68
first_layer_h = 0.2
first_layer_w = 0.68

note = [inferred] First layer well under any plausible ceiling -- the PC run
note = proved this is the control that brackets the knee from below.
first_layer_flow = 5

note = [inferred] em 0.95: Prusa uses 1.00 for neat Elegoo PLA; CF grades
note = typically land 0.94-0.98. Verify with the single-wall cube in
note = docs/verification/ and update here.
em = 0.95

note = [sourced] M572 S0.014 is Prusa's PA for a 0.6 nozzle with this
note = filament family -- identical in the HF and standard branches, so the
note = same value appears in both 0.6 profiles deliberately.
pa = 0.014

note = [sourced] The 0.6 printer preset retracts 0.7 @ 45 (vs 0.6 @ 25 on
note = the 0.8) -- the wipe still matters more than the distance.
retract = 0.7

note = [inferred] PLA does not warp; 3 rings held PETG on this footprint.
brim = 3
diameter = 50
revs = 4

note = [inferred] 4 temps on the 300x300 bed -- same stagger arithmetic as
note = the PC run: pitch = 50 + 2*3*0.68 + 35 = 89 mm.
layout = stagger

# --- REQUIRED: a PLA-CF standard-0.6 reference -------------------------
note = [BLOCKER] Must point at a PrusaSlicer export sliced with the
note = 'Elegoo PLA-CF DB06 @COREONE' filament preset + the standard
note = 'Prusa CORE One (L) 0.6 nozzle' printer preset (slicer bundle:
note = docs/verification/). The PLA reference carries chamber_minimal 0 ->
note = the start block opens the vent (M870 O). The existing PC ref.gcode
note = would level at bed 110 and soak the chamber at 40+ with PLA loaded;
note = check_match now flags PLA-on-PC, nozzle diameter, and HF/standard
note = crossings, so a wrong pairing refuses to generate.
reference = reference/ref-elegoo-pla-cf-db06.gcode
