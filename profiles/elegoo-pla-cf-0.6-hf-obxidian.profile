# Elegoo PLA-CF on a High Flow ObXidian 0.6, CORE One / CORE One L.
#
# The interesting comparison: same diameter as the DiamondBack 0.6, ~2x apart
# on published flow. Anchors from the E3D datasheet and the PrusaSlicer
# vendor bundle, which independently agree.
name = Elegoo PLA-CF @ 0.6 HF ObXidian

# --- the number this test exists to find -------------------------------
note = [sourced] E3D's published max volumetric flow for the HF ObXidian
note = Nextruder 0.6 is 30 mm3/s (PLA @ 220 C) -- e3d-online.com, "Prusa
note = Support: High Flow ObXidian Nextruder Nozzles". Prusa's shipped MVS
note = for Elegoo PLA @COREONE HF0.6 is ALSO 30. Two independent sources.
note = [inferred] Slicer profile for this filament ships 27 (~10% CF derate).
note = Ladder brackets 12-36: bottom rung is a comfortably-printable control,
note = top rung sits past the neat-PLA rating so the knee cannot fall off the
note = top the way PC-CF fell off the bottom.
note = [sourced] For contrast, the standard-flow 0.6 anchor is 15.5. If both
note = benches land where the anchors say, the knees should sit ~2x apart at
note = the same diameter -- that separation is itself a result worth having.
flows = 12, 15, 18, 21, 24, 27, 30, 33, 36

# --- temperatures ------------------------------------------------------
note = [sourced] Envelope 210-240 C. Note Prusa raises its OWN Prusament PLA
note = to 230 on the HF0.6 specifically (220 elsewhere) -- HF geometry moves
note = more material and wants the headroom. The top of this ladder at high
note = flow is where that matters; watch the 210 test object choke first.
temps = 210, 220, 230, 240

bed = 60
fan = 217

# --- geometry / extrusion ---------------------------------------------
note = [sourced] Same bead as the DiamondBack profile on purpose -- identical
note = geometry, identical ladder math, so the two runs differ in exactly one
note = variable: the melt zone. Top-band wall speed 36/0.1956 = 184 mm/s.
nozzle = 0.6
layer_h = 0.32
width = 0.68
first_layer_h = 0.2
first_layer_w = 0.68
first_layer_flow = 8
em = 0.95
pa = 0.014
retract = 0.7
brim = 3
diameter = 50
revs = 4
layout = stagger

# --- wear note ---------------------------------------------------------
note = [sourced] ObXidian is tool steel + E3DLC, not diamond; E3D's own
note = guidance says aggressively filled materials wear even ObXidian. On a
note = steady CF diet treat a drifting em as a wear indicator: re-run the
note = single-wall cube every few kg, and expect this profile's em to need
note = updating before the DiamondBack one ever does.

# --- REQUIRED: a PLA-CF HF-0.6 reference -------------------------------
note = [BLOCKER] Must point at a PrusaSlicer export sliced with the
note = 'Elegoo PLA-CF HFOBX06 @COREONE' filament preset + the
note = 'Prusa CORE One (L) HF0.6 nozzle' printer preset (slicer bundle:
note = docs/verification/). The export footer must carry
note = nozzle_high_flow = 1 -- check_match refuses the standard-flow export
note = under this profile, and that refusal is load-bearing: the two 0.6
note = references differ ONLY in that flag and their published ceilings.
reference = reference/ref-elegoo-pla-cf-hf06.gcode
