# Elegoo PLA-CF on a DiamondBack 0.8, CORE One / CORE One L.
#
# Third material on the nozzle this bench was built around. The two CF runs
# already on it: PETG-CF published 22 -> visual knee 16-18 (undried);
# PC-CF published 18 -> ceiling bracketed 8-12. PLA is the easiest melt of
# the three, so this run calibrates the bench's own derate pattern.
name = Elegoo PLA-CF @ 0.8 DiamondBack

# --- the number this test exists to find -------------------------------
note = [sourced] Prusa ships MVS 19 for neat Elegoo PLA @COREONE 0.8; the
note = slicer profile for this filament runs 18 (~5-10% CF derate). Printed
note = parts at 0.4 layers behaved at 18 -- but behaved-at is not a measured
note = knee, which is what this run is for.
note = [inferred] Same ladder as the PETG-CF profile (8-24): the anchors sit
note = mid-ladder and the PETG comparison comes free -- same nozzle, same
note = rungs, different polymer.
flows = 8, 10, 12, 14, 16, 18, 20, 22, 24

# --- temperatures ------------------------------------------------------
note = [sourced] Envelope 210-240 C, even 10s across it.
temps = 210, 220, 230, 240

bed = 60
fan = 217

# --- geometry / extrusion ---------------------------------------------
note = [sourced] 0.9 width / 0.4 layer: Prusa's default 0.8 bead, and the
note = same bead the PETG-CF and PC-CF runs used -- cross-material force
note = curves stay comparable.
nozzle = 0.8
layer_h = 0.4
width = 0.9
first_layer_h = 0.2
first_layer_w = 1.0
first_layer_flow = 8
em = 0.95

note = [sourced] M572 S0.010 is Prusa's PA for a 0.8 nozzle with this
note = filament family (the 0.6 value is 0.014 -- diameter drives PA, not
note = melt geometry).
pa = 0.010

note = [sourced] The 0.8 printer preset retracts 0.6 @ 25. The PETG runs
note = used 0.8 and did not string only because of the wipe; PLA strings
note = less, so the sourced value stands.
retract = 0.6
brim = 3
diameter = 50
revs = 4
layout = stagger

# --- REQUIRED: a PLA-CF 0.8 reference ----------------------------------
note = [BLOCKER] Must point at a PrusaSlicer export sliced with the
note = 'Elegoo PLA-CF DB08 @COREONE' filament preset + the standard
note = 'Prusa CORE One (L) 0.8 nozzle' printer preset (bundle in
note = docs/verification/). NOT the HF0.8 preset, and NOT the existing PC
note = ref.gcode -- both crossings are now refused by check_match.
reference = reference/ref-elegoo-pla-cf-db08.gcode
